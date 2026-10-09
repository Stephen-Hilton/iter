//! REST API. Principle (decided 2026-09-01): the state machine lives HERE —
//! webui, engine, and the future MCP layer all call these same rules.
//! Every write bumps the iter3_versions seq for its (project, table).

use rand::seq::SliceRandom;
use crate::auth::{self, Claims};
use crate::storage::{Storage, StorageError, body_str, body_u64};
use axum::extract::{FromRequestParts, Path, Query, State};
use axum::http::{StatusCode, request::Parts};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post, put};
use axum::{Json, Router};
use base64::Engine as _;
use iter_core::{BLOCKED_TAG_PREFIX, LockRow, dedup, Project, WebuiUser, WorkItem, check_lockshape, lockshape_for, now_utc, widget, pick_unused_priority, usecase_tags, PRIO_BAND_HUMAN, PRIO_BAND_MAINT, PRIO_BAND_USECASE, PRIO_MAX, USECASE_TAG_COLOR, USECASE_TAG_PREFIX};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::sync::Arc;

/// seq bucket for tables that aren't project-scoped (agents, users, engines…)
pub const GLOBAL: &str = "<global>";
/// sort key for single-key tables
pub(crate) const NOSK: &str = "-";

pub struct AppState {
    pub store: Arc<dyn Storage>,
    pub secret: Vec<u8>,
}

type Ctx = State<Arc<AppState>>;

// ---------- error plumbing ----------

#[derive(Debug)]
pub enum ApiError {
    Status(StatusCode, String),
    Conflict(Value),
    /// a refusal whose body the caller reads as data (lease-bound locks:
    /// `{"refused": "not running", "error": …}`), sent as-is
    Refused(StatusCode, Value),
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        match self {
            ApiError::Status(code, msg) => (code, Json(json!({"error": msg}))).into_response(),
            ApiError::Conflict(current) => (
                StatusCode::CONFLICT,
                Json(json!({"error": "conflict", "current": current})),
            )
                .into_response(),
            ApiError::Refused(code, body) => (code, Json(body)).into_response(),
        }
    }
}

impl From<StorageError> for ApiError {
    fn from(e: StorageError) -> Self {
        match e {
            StorageError::Conflict(cur) => ApiError::Conflict(cur.unwrap_or(Value::Null)),
            StorageError::Backend(msg) => ApiError::Status(StatusCode::INTERNAL_SERVER_ERROR, msg),
        }
    }
}

pub(crate) fn bad(msg: impl Into<String>) -> ApiError {
    ApiError::Status(StatusCode::BAD_REQUEST, msg.into())
}
pub(crate) fn notfound() -> ApiError {
    ApiError::Status(StatusCode::NOT_FOUND, "not found".into())
}
pub(crate) fn forbidden() -> ApiError {
    ApiError::Status(StatusCode::FORBIDDEN, "forbidden".into())
}

// ---------- auth extractor ----------

pub struct AuthUser {
    pub sub: String,
    pub role: String,
}

impl AuthUser {
    pub(crate) fn require_admin(&self) -> Result<(), ApiError> {
        if self.role == "admin" { Ok(()) } else { Err(forbidden()) }
    }
    pub(crate) fn require_writer(&self) -> Result<(), ApiError> {
        // engines and admins and users may all mutate queue-level data; a
        // "viewer" (added 2026-09-04) reads everything and changes nothing but
        // their own profile (timezone, password)
        if ["admin", "engine", "user"].contains(&self.role.as_str()) { Ok(()) } else { Err(forbidden()) }
    }
}

impl FromRequestParts<Arc<AppState>> for AuthUser {
    type Rejection = ApiError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &Arc<AppState>,
    ) -> Result<Self, Self::Rejection> {
        let header = parts
            .headers
            .get("authorization")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        let token = header.strip_prefix("Bearer ").unwrap_or("");
        if token.is_empty() {
            return Err(ApiError::Status(StatusCode::UNAUTHORIZED, "missing bearer token".into()));
        }
        let claims: Claims = auth::verify_token(&state.secret, token)
            .map_err(|e| ApiError::Status(StatusCode::UNAUTHORIZED, e))?;
        // tokenver check: the row is the revocation authority
        let row = state
            .store
            .get("webui_user", &claims.sub, NOSK)
            .await
            .map_err(ApiError::from)?
            .ok_or_else(|| ApiError::Status(StatusCode::UNAUTHORIZED, "unknown user".into()))?;
        if body_u64(&row, "tokenver") != claims.tokenver {
            return Err(ApiError::Status(StatusCode::UNAUTHORIZED, "token revoked".into()));
        }
        Ok(AuthUser { sub: claims.sub, role: claims.role })
    }
}

// ---------- router ----------

pub fn router(state: Arc<AppState>) -> Router {
    Router::new()
        .merge(crate::graph::routes())
        .merge(crate::datasync::routes())
        .merge(crate::rag::routes())
        .merge(crate::settings::routes())
        .merge(crate::switches::routes())
        .route("/health", get(health))
        .route("/auth/login", post(login))
        .route("/api/widget/validate", post(widget_validate))
        // users
        .route("/api/users", get(users_list))
        .route("/api/users/{user}", put(user_put).get(user_get))
        .route("/api/users/{user}/token", post(user_token))
        .route("/api/users/{user}/pubkey", post(user_pubkey))
        // agents
        .route("/api/agents", get(agents_list))
        .route("/api/agents/{name}", get(agent_get).put(agent_put).delete(agent_delete))
        .route("/api/tooling", get(tooling_list))
        .route("/api/tooling/{name}", get(tooling_get).put(tooling_put).delete(tooling_delete))
        // projects
        .route("/api/projects", get(projects_list))
        .route("/api/projects/{name}", get(project_get).put(project_put).delete(project_delete))
        .route("/api/projects/{name}/versions", get(versions_get))
        .route("/api/projects/{name}/status", get(project_status))
        .route("/api/projects/{name}/settle", post(project_settle))
        .route("/api/projects/{name}/spend", get(spend_list).post(spend_add))
        .route("/api/projects/{name}/structure", get(structure_get).put(structure_put))
        .route("/api/projects/{name}/prepostwork", get(prepostwork_list))
        .route("/api/prepostwork/{projectname}/{name}", put(prepostwork_put))
        // engines
        .route("/api/engines", get(engines_list))
        .route("/api/engines/{name}", get(engine_get).put(engine_put).delete(engine_delete))
        .route("/api/engines/{name}/heartbeat", post(engine_heartbeat))
        .route("/api/engines/{name}/test", post(engine_test))
        .route("/api/engines/{name}/probe", post(engine_probe))
        // workitems
        .route("/api/projects/{name}/workitems", get(workitems_list).post(workitem_create))
        .route("/api/projects/{name}/migrate_priority", post(project_migrate_priority))
        .route("/api/projects/{name}/realign_priority", post(project_realign_priority))
        .route(
            "/api/projects/{name}/workitems/{id}",
            get(workitem_get).put(workitem_put).delete(workitem_delete),
        )
        .route("/api/projects/{name}/workitems/{id}/details", get(details_list).post(details_append))
        .route("/api/projects/{name}/workitems/{id}/details/{order}", put(detail_put))
        .route("/api/projects/{name}/workitems/{id}/approve", post(workitem_approve))
        .route("/api/projects/{name}/workitems/{id}/reopen", post(workitem_reopen))
        .route("/api/projects/{name}/workitems/{id}/priority", post(workitem_priority))
        .route("/api/projects/{name}/workitems/{id}/state", post(workitem_state))
        .route("/api/projects/{name}/workitems/{id}/explain", post(workitem_explain).delete(workitem_explained))
        .route("/api/projects/{name}/workitems/{id}/explain/claim", post(workitem_explain_claim))
        .route("/api/projects/{name}/workitems/{id}/duplicate_of", post(workitem_duplicate_of))
        // locks
        .route("/api/projects/{name}/locks", get(locks_list))
        .route("/api/projects/{name}/locks/acquire", post(lock_acquire))
        .route("/api/projects/{name}/locks/release", post(lock_release))
        // lease-bound locks (CR 2026-09-25): `extend` is kept as an alias of
        // `renew` so no old caller gets a 404
        .route("/api/projects/{name}/locks/extend", post(lock_renew))
        .route("/api/projects/{name}/locks/renew", post(lock_renew))
        .route("/api/projects/{name}/locks/release_all", post(lock_release_all))
        .route("/api/projects/{name}/locks/sweep", post(lock_sweep))
        .route("/api/projects/{name}/deadlocks", get(deadlocks_get))
        // iter5 server-side get_next (spec §4.3)
        .route("/api/projects/{name}/next", post(crate::next::get_next))
        // iter5 authz (spec §4.4): one layer over every route above, keyed on
        // the path's project segment (graph/rag/datasync included; MCP
        // replays through this router so it inherits it)
        .layer(axum::middleware::from_fn_with_state(state.clone(), crate::authz::project_authz))
        .with_state(state)
}

// ---------- misc ----------

async fn health(State(st): Ctx) -> Json<Value> {
    // on arango, "ok" also means the database answered
    let (ok, db) = match st.store.arango() {
        Some(a) => (a.ping().await, a.db_name().to_string()),
        None => (true, String::new()),
    };
    Json(json!({"ok": ok, "backend": st.store.backend_name(), "db": db, "version": env!("CARGO_PKG_VERSION"), "ts": now_utc()}))
}

async fn widget_validate(_user: AuthUser, Json(body): Json<Value>) -> Json<Value> {
    Json(json!({"errors": widget::validate(&body)}))
}

// ---------- auth ----------

#[derive(serde::Deserialize)]
struct LoginReq {
    user: String,
    password: String,
}

async fn login(State(st): Ctx, Json(req): Json<LoginReq>) -> Result<Json<Value>, ApiError> {
    // sign in by id or by display name; the token always carries the id
    let unknown = || ApiError::Status(StatusCode::UNAUTHORIZED, "bad credentials".into());
    let uid = crate::settings::resolve_key(st.store.as_ref(), "webui_user", req.user.trim()).await?.ok_or_else(unknown)?;
    let row = st.store.get("webui_user", &uid, NOSK).await?.ok_or_else(unknown)?;
    let pwhash = body_str(&row, "pwhash");
    if pwhash.is_empty() || !auth::verify_password(&req.password, &pwhash) {
        return Err(unknown());
    }
    let role = body_str(&row, "role");
    let tokenver = body_u64(&row, "tokenver").max(1);
    let token = auth::mint_token(&st.secret, &uid, &role, tokenver, 24 * 3600)
        .map_err(|e| ApiError::Status(StatusCode::INTERNAL_SERVER_ERROR, e))?;
    Ok(Json(json!({"token": token, "role": role, "user": uid, "name": body_str(&row, "name"), "timezone": body_str(&row, "timezone")})))
}

// ---------- users ----------

fn redact(mut v: Value) -> Value {
    if let Some(o) = v.as_object_mut() {
        o.remove("pwhash");
    }
    v
}

async fn users_list(user: AuthUser, State(st): Ctx) -> Result<Json<Value>, ApiError> {
    user.require_admin()?;
    let rows = st.store.scan("webui_user").await?;
    Ok(Json(Value::Array(rows.into_iter().map(redact).collect())))
}

async fn user_get(user: AuthUser, State(st): Ctx, Path(name): Path<String>) -> Result<Json<Value>, ApiError> {
    if user.role != "admin" && user.sub != name {
        return Err(forbidden());
    }
    let row = st.store.get("webui_user", &name, NOSK).await?.ok_or_else(notfound)?;
    Ok(Json(redact(row)))
}

async fn user_put(
    user: AuthUser,
    State(st): Ctx,
    Path(name): Path<String>,
    Json(mut body): Json<Value>,
) -> Result<Json<Value>, ApiError> {
    // admins edit anyone; a user may edit their OWN row, but role / authz /
    // tokenver / pubkey stay whatever they were (self-service = profile + password)
    let self_edit = user.role != "admin";
    if self_edit && user.sub != name {
        return Err(forbidden());
    }
    let existing = st.store.get("webui_user", &name, NOSK).await?;
    if self_edit {
        let Some(ex) = &existing else { return Err(forbidden()) };
        for k in ["role", "authz", "tokenver", "pubkey"] {
            // a key the stored row never had must stay absent (serde defaults
            // apply to missing keys, not to null): a null here failed every
            // non-admin self-edit of a user created without "authz"
            match ex.get(k) {
                Some(v) if !v.is_null() => body[k] = v.clone(),
                _ => {
                    if let Some(o) = body.as_object_mut() {
                        o.remove(k);
                    }
                }
            }
        }
    }
    crate::settings::settle_name(st.store.as_ref(), "webui_user", &name, &mut body).await?;
    body["user"] = json!(name);
    // password (plaintext, TLS-transported) -> pwhash; else preserve existing
    if let Some(pw) = body.get("password").and_then(|v| v.as_str()).map(String::from) {
        let hash = auth::hash_password(&pw).map_err(|e| ApiError::Status(StatusCode::INTERNAL_SERVER_ERROR, e))?;
        body["pwhash"] = json!(hash);
        body.as_object_mut().unwrap().remove("password");
    } else if body.get("pwhash").map(|v| v.as_str().unwrap_or("").is_empty()).unwrap_or(true) {
        if let Some(ex) = &existing {
            body["pwhash"] = ex.get("pwhash").cloned().unwrap_or(json!(""));
        }
    }
    if body.get("tokenver").and_then(|v| v.as_u64()).unwrap_or(0) == 0 {
        let prior = existing.as_ref().map(|e| body_u64(e, "tokenver")).unwrap_or(0);
        body["tokenver"] = json!(prior.max(1));
    }
    let parsed: WebuiUser =
        serde_json::from_value(body.clone()).map_err(|e| bad(format!("user does not parse: {e}")))?;
    if !["user", "engine", "admin", "viewer"].contains(&parsed.role.as_str()) {
        return Err(bad("role must be user|engine|admin|viewer"));
    }
    st.store.put("webui_user", &name, NOSK, &body).await?;
    st.store.bump_seq(GLOBAL, "webui_user").await?;
    Ok(Json(redact(body)))
}

#[derive(serde::Deserialize)]
struct TokenReq {
    #[serde(default = "default_ttl_days")]
    ttl_days: u64,
}
fn default_ttl_days() -> u64 { 365 }

async fn user_token(
    user: AuthUser,
    State(st): Ctx,
    Path(name): Path<String>,
    Json(req): Json<TokenReq>,
) -> Result<Json<Value>, ApiError> {
    user.require_admin()?;
    let row = st.store.get("webui_user", &name, NOSK).await?.ok_or_else(notfound)?;
    let role = body_str(&row, "role");
    let tokenver = body_u64(&row, "tokenver").max(1);
    let token = auth::mint_token(&st.secret, &name, &role, tokenver, req.ttl_days * 86400)
        .map_err(|e| ApiError::Status(StatusCode::INTERNAL_SERVER_ERROR, e))?;
    Ok(Json(json!({"token": token, "user": name, "role": role, "ttl_days": req.ttl_days})))
}

#[derive(serde::Deserialize)]
struct PubkeyReq {
    pubkey: String,
    #[serde(default)]
    email: String,
}

/// `iter --adduser` registration path: admin or engine may add/refresh a
/// user's pubkey. Add-only for the key: refuses to overwrite a non-empty one
/// (reset flow is manual via the admin users page, per spec).
async fn user_pubkey(
    user: AuthUser,
    State(st): Ctx,
    Path(name): Path<String>,
    Json(req): Json<PubkeyReq>,
) -> Result<Json<Value>, ApiError> {
    if !["admin", "engine"].contains(&user.role.as_str()) {
        return Err(forbidden());
    }
    let mut row = st.store.get("webui_user", &name, NOSK).await?.unwrap_or_else(|| {
        json!({"user": name, "email": req.email, "role": "user", "pwhash": "", "tokenver": 1,
               "css": "", "pubkey": "", "settings": {}, "authz": {}})
    });
    if !body_str(&row, "pubkey").is_empty() && user.role != "admin" {
        return Err(bad("pubkey already set; reset is manual via an admin (see spec)"));
    }
    row["pubkey"] = json!(req.pubkey);
    st.store.put("webui_user", &name, NOSK, &row).await?;
    st.store.bump_seq(GLOBAL, "webui_user").await?;
    Ok(Json(redact(row)))
}

// ---------- agents ----------

async fn agent_delete(user: AuthUser, State(st): Ctx, Path(name): Path<String>) -> Result<Json<Value>, ApiError> {
    user.require_admin()?;
    let deleted = st.store.delete("agent", &name, NOSK).await?;
    st.store.bump_seq(GLOBAL, "agent").await?;
    if deleted {
        crate::settings::on_node_deleted(st.store.as_ref(), &iter_core::settings::node_id("agent", &name)).await?;
    }
    Ok(Json(json!({"deleted": deleted})))
}

// ---------- agent tooling (shared rules, capability docs, source instructions, prose steps, critic) ----------

async fn tooling_list(_u: AuthUser, State(st): Ctx) -> Result<Json<Value>, ApiError> {
    let mut rows = st.store.scan("agent_tooling").await?;
    rows.sort_by(|a, b| body_str(a, "kind").cmp(&body_str(b, "kind")).then(body_str(a, "name").cmp(&body_str(b, "name"))));
    Ok(Json(Value::Array(rows)))
}
async fn tooling_get(_u: AuthUser, State(st): Ctx, Path(name): Path<String>) -> Result<Json<Value>, ApiError> {
    Ok(Json(st.store.get("agent_tooling", &name, NOSK).await?.ok_or_else(notfound)?))
}
async fn tooling_put(user: AuthUser, State(st): Ctx, Path(name): Path<String>, Json(mut body): Json<Value>) -> Result<Json<Value>, ApiError> {
    user.require_admin()?;
    crate::settings::settle_name(st.store.as_ref(), "agent_tooling", &name, &mut body).await?;
    let parsed: iter_core::AgentTooling =
        serde_json::from_value(body.clone()).map_err(|e| bad(format!("tooling does not parse: {e}")))?;
    if !iter_core::TOOLING_KINDS.contains(&parsed.kind.as_str()) {
        return Err(bad(format!("kind must be one of {:?}", iter_core::TOOLING_KINDS)));
    }
    st.store.put("agent_tooling", &name, NOSK, &body).await?;
    st.store.bump_seq(GLOBAL, "agent_tooling").await?;
    crate::settings::on_tooling_created(st.store.as_ref(), &body).await?;
    Ok(Json(body))
}
async fn tooling_delete(user: AuthUser, State(st): Ctx, Path(name): Path<String>) -> Result<Json<Value>, ApiError> {
    user.require_admin()?;
    let deleted = st.store.delete("agent_tooling", &name, NOSK).await?;
    st.store.bump_seq(GLOBAL, "agent_tooling").await?;
    if deleted {
        crate::settings::on_node_deleted(st.store.as_ref(), &iter_core::settings::node_id("agent_tools", &name)).await?;
    }
    Ok(Json(json!({"deleted": deleted})))
}

async fn agents_list(_u: AuthUser, State(st): Ctx) -> Result<Json<Value>, ApiError> {
    Ok(Json(Value::Array(st.store.scan("agent").await?)))
}

async fn agent_get(_u: AuthUser, State(st): Ctx, Path(name): Path<String>) -> Result<Json<Value>, ApiError> {
    Ok(Json(st.store.get("agent", &name, NOSK).await?.ok_or_else(notfound)?))
}

async fn agent_put(
    user: AuthUser,
    State(st): Ctx,
    Path(name): Path<String>,
    Json(mut body): Json<Value>,
) -> Result<Json<Value>, ApiError> {
    user.require_admin()?;
    crate::settings::settle_name(st.store.as_ref(), "agent", &name, &mut body).await?;
    let _: iter_core::AgentDef =
        serde_json::from_value(body.clone()).map_err(|e| bad(format!("agent does not parse: {e}")))?;
    let is_new = st.store.get("agent", &name, NOSK).await?.is_none();
    st.store.put("agent", &name, NOSK, &body).await?;
    st.store.bump_seq(GLOBAL, "agent").await?;
    if is_new {
        crate::settings::on_agent_created(st.store.as_ref(), &name).await?;
    }
    Ok(Json(body))
}

// ---------- projects ----------

/// Every project the caller may see (iter5 authz): all for admin, served
/// ones for an engine token, member ones for a user.
async fn projects_list(u: AuthUser, State(st): Ctx) -> Result<Json<Value>, ApiError> {
    let mut rows = st.store.scan("project").await?;
    if let Some(vis) = crate::settings::visible_projects(st.store.as_ref(), &u.sub, &u.role).await? {
        rows.retain(|r| vis.contains(&body_str(r, "id")));
    }
    Ok(Json(Value::Array(rows)))
}

async fn project_get(_u: AuthUser, State(st): Ctx, Path(name): Path<String>) -> Result<Json<Value>, ApiError> {
    Ok(Json(st.store.get("project", &name, NOSK).await?.ok_or_else(notfound)?))
}

async fn project_put(
    user: AuthUser,
    State(st): Ctx,
    Path(name): Path<String>,
    Json(mut body): Json<Value>,
) -> Result<Json<Value>, ApiError> {
    // iter5: an admin edits any project; a user (role user) may CREATE a new
    // one and becomes its member (spec §4.4); edits of an existing project
    // stay admin-only
    let existing = st.store.get("project", &name, NOSK).await?;
    if user.role != "admin" && (existing.is_some() || user.role != "user") {
        return Err(forbidden());
    }
    crate::settings::settle_name(st.store.as_ref(), "project", &name, &mut body).await?;
    let parsed: Project =
        serde_json::from_value(body.clone()).map_err(|e| bad(format!("project does not parse: {e}")))?;
    if !["Running", "Draining", "Stopped"].contains(&parsed.state.as_str()) {
        return Err(bad("project state must be Running|Draining|Stopped"));
    }
    check_pinned_tags(&parsed.pinned_tags)?;
    // an emptied settings box arrives as null (fixed 2026-09-10): drop such
    // keys so the stored row reads as "absent = default" on every reader,
    // engines running an older build included
    let mut body = body;
    if let Some(o) = body.as_object_mut() {
        o.retain(|_, v| !v.is_null());
    }
    st.store.put("project", &name, NOSK, &body).await?;
    st.store.bump_seq(&name, "project").await?;
    if existing.is_none() {
        let creator = if user.role == "admin" { None } else { Some(user.sub.as_str()) };
        crate::settings::on_project_created(st.store.as_ref(), &name, creator).await?;
        crate::sync_hooks::project_created(st.store.as_ref(), &name).await;
    }
    Ok(Json(body))
}

/// `project.pinned_tags` (decided 2026-09-09): plain tag texts the webui
/// offers first.  Blank entries are a typo, and an engine-owned `blocked by: `
/// tag would be deleted by the next tick's `reconcile_waits`, so both are
/// refused here rather than silently ignored.
fn check_pinned_tags(tags: &[String]) -> Result<(), ApiError> {
    for t in tags {
        if t.trim().is_empty() {
            return Err(bad("pinned_tags: blank entry"));
        }
        if t.starts_with(BLOCKED_TAG_PREFIX) {
            return Err(bad(format!("pinned_tags: \"{t}\" is engine-owned (\"{BLOCKED_TAG_PREFIX}…\" tags are derived every tick and would be removed)")));
        }
    }
    Ok(())
}

async fn project_delete(user: AuthUser, State(st): Ctx, Path(name): Path<String>) -> Result<Json<Value>, ApiError> {
    user.require_admin()?;
    let deleted = st.store.delete("project", &name, NOSK).await?;
    st.store.bump_seq(&name, "project").await?;
    if deleted {
        crate::settings::on_node_deleted(st.store.as_ref(), &iter_core::settings::node_id("project", &name)).await?;
    }
    // the project graph goes with the project (nodes, links, conflicts, test logs)
    let purged = crate::nodes::purge_project(st.store.as_ref(), &name).await?;
    Ok(Json(json!({"deleted": deleted, "graph_purged": purged})))
}

async fn versions_get(_u: AuthUser, State(st): Ctx, Path(name): Path<String>) -> Result<Json<Value>, ApiError> {
    let mut rows = st.store.get_versions(&name).await?;
    rows.extend(st.store.get_versions(GLOBAL).await?);
    Ok(Json(serde_json::to_value(rows).unwrap_or(Value::Null)))
}

/// Draining monitoring (spec: "carefully monitor all engines for possible
/// disconnections, to make sure all engines are honoring the command").
/// Computes, centrally: per-engine liveness (last_seen vs 3x ticksec + 5s
/// grace), in-progress counts, whether the drain has completed, and which
/// engines might NOT be honoring it (disconnected while holding work).
async fn project_status(_u: AuthUser, State(st): Ctx, Path(name): Path<String>) -> Result<Json<Value>, ApiError> {
    let project = st.store.get("project", &name, NOSK).await?.ok_or_else(notfound)?;
    let project_state = body_str(&project, "state");
    let items = st.store.query("workitem", &name).await?;
    let mut inprogress_by_engine: HashMap<String, i64> = HashMap::new();
    for i in &items {
        if body_str(i, "state") == "in-progress" {
            *inprogress_by_engine.entry(body_str(i, "engine")).or_insert(0) += 1;
        }
    }
    let now = chrono::Utc::now();
    let mut engines_out = Vec::new();
    let mut not_honoring = Vec::new();
    let edges = crate::settings::load_edges(st.store.as_ref()).await?;
    for e in st.store.scan("engine").await? {
        // only engines that serve this project (an active serves edge, or the
        // deprecated iter4 `projects` map)
        let served = crate::settings::serves_edge(&edges, &body_str(&e, "id"), &name).is_some();
        if !served && e.get("projects").and_then(|p| p.get(&name)).is_none() {
            continue;
        }
        let ename = body_str(&e, "id");
        let ticksec = e.get("ticksec").and_then(|t| t.as_u64()).unwrap_or(5);
        let last_seen = body_str(&e, "last_seen");
        let age_sec = chrono::DateTime::parse_from_rfc3339(&last_seen)
            .map(|t| (now - t.with_timezone(&chrono::Utc)).num_seconds())
            .unwrap_or(i64::MAX);
        let stale = age_sec > (3 * ticksec as i64 + 5);
        let running = inprogress_by_engine.get(&ename).copied().unwrap_or(0);
        if project_state == "Draining" && stale && running > 0 {
            not_honoring.push(ename.clone());
        }
        engines_out.push(json!({
            "name": ename, "display": body_str(&e, "name"),
            "state": body_str(&e, "state"),
            "active": crate::switches::is_on(&e),
            "last_seen": last_seen,
            "age_sec": if age_sec == i64::MAX { Value::Null } else { json!(age_sec) },
            "stale": stale,
            "inprogress": running,
        }));
    }
    let total_inprogress: i64 = inprogress_by_engine.values().sum();
    Ok(Json(json!({
        "project": name,
        "project_state": project_state,
        "server_active": crate::switches::server_on(st.store.as_ref()).await?,
        "engines": engines_out,
        "inprogress": total_inprogress,
        "all_drained": total_inprogress == 0,
        "not_honoring": not_honoring,
    })))
}

/// Draining is transitional (decided 2026-09-04): once nothing is in progress
/// on any live engine, the project settles to Stopped. Engines call this each
/// tick while Draining; the webui calls it on refresh so a drain with nothing
/// running settles at once even with no engine up. Writer role suffices —
/// it can only ever move Draining -> Stopped, and only when drained.
async fn project_settle(user: AuthUser, State(st): Ctx, Path(name): Path<String>) -> Result<Json<Value>, ApiError> {
    user.require_writer()?;
    let mut proj = st.store.get("project", &name, NOSK).await?.ok_or_else(notfound)?;
    if body_str(&proj, "state") != "Draining" {
        return Ok(Json(json!({"state": body_str(&proj, "state"), "settled": false})));
    }
    let items = st.store.query("workitem", &name).await?;
    let inprogress = items.iter().filter(|i| body_str(i, "state") == "in-progress").count();
    if inprogress > 0 {
        return Ok(Json(json!({"state": "Draining", "settled": false, "inprogress": inprogress})));
    }
    proj["state"] = json!("Stopped");
    st.store.put("project", &name, NOSK, &proj).await?;
    st.store.bump_seq(&name, "project").await?;
    Ok(Json(json!({"state": "Stopped", "settled": true})))
}

// ---------- spend (per project per UTC day; engines add after each run) ----------

async fn spend_list(_u: AuthUser, State(st): Ctx, Path(name): Path<String>) -> Result<Json<Value>, ApiError> {
    let mut rows = st.store.query("spend", &name).await?;
    rows.sort_by(|a, b| body_str(b, "date").cmp(&body_str(a, "date")));
    rows.truncate(31);
    Ok(Json(Value::Array(rows)))
}

#[derive(serde::Deserialize)]
struct SpendReq {
    #[serde(default)]
    usd: f64,
    #[serde(default)]
    input_tokens: u64,
    #[serde(default)]
    output_tokens: u64,
    #[serde(default)]
    cache_read_tokens: u64,
    #[serde(default)]
    cache_create_tokens: u64,
    #[serde(default)]
    workid: String,
}

/// Add one run's cost to today's row (read-modify-write; a lost race under-
/// counts by one run, which the daily cap tolerates).
async fn spend_add(user: AuthUser, State(st): Ctx, Path(name): Path<String>, Json(req): Json<SpendReq>) -> Result<Json<Value>, ApiError> {
    user.require_writer()?;
    let date = now_utc()[..10].to_string();
    let mut row = st.store.get("spend", &name, &date).await?.unwrap_or(json!({
        "project": name, "date": date, "usd": 0.0, "input_tokens": 0, "output_tokens": 0, "runs": 0
    }));
    row["usd"] = json!(row.get("usd").and_then(|v| v.as_f64()).unwrap_or(0.0) + req.usd);
    row["input_tokens"] = json!(body_u64(&row, "input_tokens") + req.input_tokens);
    row["output_tokens"] = json!(body_u64(&row, "output_tokens") + req.output_tokens);
    row["cache_read_tokens"] = json!(body_u64(&row, "cache_read_tokens") + req.cache_read_tokens);
    row["cache_create_tokens"] = json!(body_u64(&row, "cache_create_tokens") + req.cache_create_tokens);
    row["runs"] = json!(body_u64(&row, "runs") + 1);
    row["last_workid"] = json!(req.workid);
    row["updated"] = json!(now_utc());
    st.store.put("spend", &name, &date, &row).await?;
    Ok(Json(row))
}

// ---------- structure ----------

async fn structure_get(_u: AuthUser, State(st): Ctx, Path(name): Path<String>) -> Result<Json<Value>, ApiError> {
    Ok(Json(st.store.get("project_structure", &name, NOSK).await?.unwrap_or(Value::Null)))
}

async fn structure_put(
    user: AuthUser,
    State(st): Ctx,
    Path(name): Path<String>,
    Json(mut body): Json<Value>,
) -> Result<Json<Value>, ApiError> {
    user.require_writer()?;
    body["projectname"] = json!(name);
    body["updated"] = json!(now_utc());
    st.store.put("project_structure", &name, NOSK, &body).await?;
    st.store.bump_seq(&name, "project_structure").await?;
    Ok(Json(body))
}

// ---------- prepostwork ----------

async fn prepostwork_list(_u: AuthUser, State(st): Ctx, Path(name): Path<String>) -> Result<Json<Value>, ApiError> {
    // merged view: project-specific rows win over <default> by "name"
    let mut by_name: HashMap<String, Value> = HashMap::new();
    for row in st.store.query("project_prepostwork", "<default>").await? {
        by_name.insert(body_str(&row, "name"), row);
    }
    for row in st.store.query("project_prepostwork", &name).await? {
        by_name.insert(body_str(&row, "name"), row);
    }
    let mut rows: Vec<Value> = by_name.into_values().collect();
    rows.sort_by_key(|r| body_str(r, "name"));
    Ok(Json(Value::Array(rows)))
}

async fn prepostwork_put(
    user: AuthUser,
    State(st): Ctx,
    Path((projectname, name)): Path<(String, String)>,
    Json(mut body): Json<Value>,
) -> Result<Json<Value>, ApiError> {
    user.require_admin()?;
    body["projectname"] = json!(projectname);
    body["name"] = json!(name);
    let _: iter_core::PrePostWork =
        serde_json::from_value(body.clone()).map_err(|e| bad(format!("prepostwork does not parse: {e}")))?;
    st.store.put("project_prepostwork", &projectname, &name, &body).await?;
    st.store.bump_seq(&projectname, "project_prepostwork").await?;
    Ok(Json(body))
}

// ---------- engines ----------

async fn engines_list(_u: AuthUser, State(st): Ctx) -> Result<Json<Value>, ApiError> {
    Ok(Json(Value::Array(st.store.scan("engine").await?)))
}

async fn engine_get(_u: AuthUser, State(st): Ctx, Path(name): Path<String>) -> Result<Json<Value>, ApiError> {
    Ok(Json(st.store.get("engine", &name, NOSK).await?.ok_or_else(notfound)?))
}

async fn engine_put(
    user: AuthUser,
    State(st): Ctx,
    Path(name): Path<String>,
    Json(mut body): Json<Value>,
) -> Result<Json<Value>, ApiError> {
    user.require_writer()?;
    let existing = st.store.get("engine", &name, NOSK).await?;
    let owner = own_engine(&user, &name, existing.as_ref())?;
    crate::settings::settle_name(st.store.as_ref(), "engine", &name, &mut body).await?;
    // the owner is the server's to set (spec §4.4): from the caller's token on
    // register, kept on every later write (an admin may hand it over)
    let asked = body_str(&body, "user");
    body["user"] = json!(if user.role == "admin" && !asked.is_empty() { asked } else { owner });
    let _: iter_core::Engine =
        serde_json::from_value(body.clone()).map_err(|e| bad(format!("engine does not parse: {e}")))?;
    st.store.put("engine", &name, NOSK, &body).await?;
    st.store.bump_seq(GLOBAL, "engine").await?;
    if existing.is_none() || existing.as_ref().map(|e| body_str(e, "user")) != Some(body_str(&body, "user")) {
        crate::settings::on_engine_registered(st.store.as_ref(), &name, &body_str(&body, "user")).await?;
    }
    Ok(Json(body))
}

/// Who may write this engine record, and who owns it afterwards (spec §4.4):
/// a new record belongs to the caller (an admin registering one leaves it
/// unowned); an unowned (iter4) record is claimed by the first engine-token
/// write; an owned record only by its owner or an admin.
fn own_engine(user: &AuthUser, name: &str, existing: Option<&Value>) -> Result<String, ApiError> {
    let owner = existing.map(|e| body_str(e, "user")).unwrap_or_default();
    if user.role == "admin" {
        return Ok(if existing.is_none() { String::new() } else { owner });
    }
    match existing {
        None => Ok(user.sub.clone()),
        Some(_) if owner.is_empty() && user.role == "engine" => Ok(user.sub.clone()),
        Some(_) if owner == user.sub => Ok(owner),
        Some(_) => Err(ApiError::Status(
            StatusCode::FORBIDDEN,
            format!("engine '{name}' belongs to {}: an engine may only write its own record", if owner.is_empty() { "nobody yet (an engine token claims it)".to_string() } else { format!("'{owner}'") }),
        )),
    }
}

#[derive(serde::Deserialize)]
struct HeartbeatReq {
    #[serde(default)]
    state: String,
    /// how many run threads the engine has (in total / per project): the
    /// webui warns when the store's in-progress count is larger — a record
    /// with no session behind it (2026-09-22); absent = leave alone
    #[serde(default)]
    running: Option<u64>,
    #[serde(default)]
    running_by_project: Option<Value>,
    /// the account the engine picked this tick; "" = none pickable (holding,
    /// or a single-account setup on the ambient CLI login) and is STORED, so
    /// the label always names the account whose `usage` rides with it.
    /// Absent = leave the label alone (2026-09-10: an ignored "" left the last
    /// real name on the record while the default login's usage replaced it).
    #[serde(default)]
    account: Option<String>,
    /// why the engine is dispatching nothing ("" = not holding); absent =
    /// leave alone (older engines).  "all accounts at stop%" -> webui shows
    /// "Suspended, no usage left" with dashed windows.
    #[serde(default)]
    hold: Option<String>,
    /// latest usage snapshot for the active account (engine-owned).  An
    /// explicit null CLEARS it (holding: no account is running, so no numbers
    /// describe this engine); absent = leave alone.
    #[serde(default, deserialize_with = "null_clears")]
    usage: Option<Option<Value>>,
    /// every configured account's windows, reset times and `available_at`
    /// (engine-owned, soonest first); absent = leave alone
    #[serde(default)]
    accounts: Option<Value>,
    /// the account that comes back first: {"account","available_at"}; an
    /// explicit null clears it (nothing estimable); absent = leave alone
    #[serde(default, deserialize_with = "null_clears")]
    next: Option<Option<Value>>,
    /// outcome of a connectivity test the engine just ran
    #[serde(default)]
    test_result: Option<Value>,
    /// the engine consumed test_requested
    #[serde(default)]
    clear_test: bool,
    /// the engine consumed probe_requested (its usage report rides along)
    #[serde(default)]
    clear_probe: bool,
}

/// serde reads a JSON null into `Option<T>` as None, which is the same as an
/// absent field; this keeps the two apart: absent = None (leave the row
/// alone), null = Some(None) (clear the row's value).
fn null_clears<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<Option<Value>>, D::Error> {
    let v = <Value as serde::Deserialize>::deserialize(d)?;
    Ok(Some(if v.is_null() { None } else { Some(v) }))
}

/// webui -> engine: ask for a connectivity nudge (`claude -p "."` on haiku);
/// the engine sees `test_requested` on its next tick and answers via heartbeat.
async fn engine_test(user: AuthUser, State(st): Ctx, Path(name): Path<String>) -> Result<Json<Value>, ApiError> {
    user.require_writer()?;
    let mut row = st.store.get("engine", &name, NOSK).await?.ok_or_else(notfound)?;
    let ts = now_utc();
    row["test_requested"] = json!(ts);
    st.store.put("engine", &name, NOSK, &row).await?;
    st.store.bump_seq(GLOBAL, "engine").await?;
    Ok(Json(json!({"requested": ts})))
}

/// webui -> engine: re-read every account's 5h / 7d usage now (the Accounts
/// pane's refresh, and on open / every 15 min). The engine sees
/// probe_requested on its next tick, asks each account's provider, and
/// heartbeats the fresh report with clear_probe.
async fn engine_probe(user: AuthUser, State(st): Ctx, Path(name): Path<String>) -> Result<Json<Value>, ApiError> {
    user.require_writer()?;
    let mut row = st.store.get("engine", &name, NOSK).await?.ok_or_else(notfound)?;
    let ts = now_utc();
    row["probe_requested"] = json!(ts);
    st.store.put("engine", &name, NOSK, &row).await?;
    st.store.bump_seq(GLOBAL, "engine").await?;
    Ok(Json(json!({"requested": ts})))
}

/// Remove an engine record (admin). A record that heartbeated within three
/// ticks is alive and refused: stop the engine first. Any project listing the
/// engine drops it from its `engines` list.
async fn engine_delete(user: AuthUser, State(st): Ctx, Path(name): Path<String>) -> Result<Json<Value>, ApiError> {
    user.require_admin()?;
    let row = st.store.get("engine", &name, NOSK).await?.ok_or_else(notfound)?;
    let tick = row.get("ticksec").and_then(|t| t.as_i64()).unwrap_or(5).max(1);
    let alive = chrono::DateTime::parse_from_rfc3339(&body_str(&row, "last_seen"))
        .map(|seen| (chrono::Utc::now() - seen.with_timezone(&chrono::Utc)).num_seconds() <= 3 * tick + 5)
        .unwrap_or(false);
    if alive {
        return Err(ApiError::Status(StatusCode::CONFLICT, format!("engine '{name}' is heartbeating — stop it before deleting its record")));
    }
    let deleted = st.store.delete("engine", &name, NOSK).await?;
    st.store.bump_seq(GLOBAL, "engine").await?;
    if deleted {
        crate::settings::on_node_deleted(st.store.as_ref(), &iter_core::settings::node_id("iter_engine", &name)).await?;
    }
    for mut p in st.store.scan("project").await? {
        let list: Vec<Value> = p.get("engines").and_then(|e| e.as_array()).cloned().unwrap_or_default();
        if list.iter().any(|e| e.as_str() == Some(name.as_str())) {
            let pname = body_str(&p, "id");
            p["engines"] = Value::Array(list.into_iter().filter(|e| e.as_str() != Some(name.as_str())).collect());
            st.store.put("project", &pname, NOSK, &p).await?;
            st.store.bump_seq(&pname, "project").await?;
        }
    }
    Ok(Json(json!({"deleted": deleted})))
}

async fn engine_heartbeat(
    user: AuthUser,
    State(st): Ctx,
    Path(name): Path<String>,
    Json(req): Json<HeartbeatReq>,
) -> Result<Json<Value>, ApiError> {
    user.require_writer()?;
    let mut row = st.store.get("engine", &name, NOSK).await?.ok_or_else(notfound)?;
    let owner = own_engine(&user, &name, Some(&row))?;
    let claimed = body_str(&row, "user") != owner;
    row["user"] = json!(owner);
    row["last_seen"] = json!(now_utc());
    if !req.state.is_empty() {
        row["state"] = json!(req.state);
    }
    if let Some(a) = req.account {
        row["account"] = json!(a);
    }
    if let Some(h) = req.hold {
        row["hold"] = json!(h);
    }
    if let Some(n) = req.running {
        row["running"] = json!(n);
    }
    if let Some(m) = req.running_by_project {
        row["running_by_project"] = m;
    }
    if let Some(u) = req.usage {
        row["usage"] = u.unwrap_or(Value::Null);
    }
    if let Some(a) = req.accounts {
        row["accounts"] = a;
    }
    if let Some(n) = req.next {
        row["next"] = n.unwrap_or(Value::Null);
    }
    if let Some(t) = req.test_result {
        row["test_result"] = t;
    }
    if req.clear_test {
        row["test_requested"] = json!("");
    }
    if req.clear_probe {
        row["probe_requested"] = json!("");
    }
    st.store.put("engine", &name, NOSK, &row).await?;
    st.store.bump_seq(GLOBAL, "engine").await?;
    if claimed {
        crate::settings::on_engine_registered(st.store.as_ref(), &name, &owner).await?;
    }
    // the projects this engine serves: active serves edges (iter5), plus the
    // deprecated iter4 `projects` map while engines switch over
    let edges = crate::settings::load_edges(st.store.as_ref()).await?;
    let mut projects: Vec<String> = crate::settings::served_projects(&edges, &name);
    for p in row.get("projects").and_then(|p| p.as_object()).map(|o| o.keys().cloned().collect::<Vec<_>>()).unwrap_or_default() {
        if !projects.contains(&p) {
            projects.push(p);
        }
    }
    projects.sort();
    // datasync (2026-09-29): graph edits waiting for an engine, per project
    // this engine serves — the engine applies them on this tick
    let waiting = crate::datasync::waiting(st.store.as_ref(), &projects).await?;
    let mut reply = row.clone();
    reply["datasync_waiting"] = json!(waiting);
    // iter5 (spec §3.3/§3.4): node edits waiting to be written to files, and
    // designer builds waiting for this engine (sync_hooks, DATA-GRAPH)
    reply["files_waiting"] = json!(crate::sync_hooks::files_waiting(st.store.as_ref(), &projects).await);
    reply["build_waiting"] = json!(crate::sync_hooks::build_waiting(st.store.as_ref(), &projects).await);
    // GraphRAG (2026-09-29): Summary agent work waiting, per project — the
    // engine runs it on its own threads, outside the agent cap and the queue;
    // none is offered while the server or this engine is switched off
    let stopped_by = crate::switches::engine_block(st.store.as_ref(), &name).await?;
    reply["rag_waiting"] = if stopped_by.is_some() { json!({}) } else { json!(crate::rag::pipeline::waiting(st.store.as_ref(), &projects).await) };
    reply["stopped_by"] = json!(stopped_by.unwrap_or_default());
    Ok(Json(reply))
}

// ---------- workitems ----------

async fn workitems_list(
    _u: AuthUser,
    State(st): Ctx,
    Path(name): Path<String>,
    Query(q): Query<HashMap<String, String>>,
) -> Result<Json<Value>, ApiError> {
    let mut rows = st.store.query("workitem", &name).await?;
    if let Some(state) = q.get("state") {
        rows.retain(|r| body_str(r, "state") == *state);
    }
    Ok(Json(Value::Array(rows)))
}

/// Priority + usecase placement of a NEW item (decided 2026-09-08), before the
/// row is parsed:
/// - a `usecase` string field becomes the engine-owned tag `usecase:<name>`;
/// - a child (createdby = an existing item) inherits its creator's priority
///   EXACTLY and every `usecase:` tag; a differing requested priority is
///   ignored and reported in `warnings`;
/// - a root that names no priority takes the lowest number in its band that
///   no open item uses: usecase band when it carries a usecase tag or IS the
///   usecase agent, the human band for a human requester, else maintenance.
async fn place_new_item(st: &Arc<AppState>, project: &str, body: &mut Value) -> Result<Vec<String>, ApiError> {
    let mut warnings = Vec::new();
    // usecase field -> tag
    let uc = body.get("usecase").and_then(|u| u.as_str()).map(|u| u.trim().to_string()).unwrap_or_default();
    if let Some(o) = body.as_object_mut() {
        o.remove("usecase");
    }
    let mut tags: Vec<Value> = body.get("tags").and_then(|t| t.as_array()).cloned().unwrap_or_default();
    let has_tag = |tags: &[Value], text: &str| tags.iter().any(|t| t.get("text").and_then(|x| x.as_str()) == Some(text));
    if !uc.is_empty() {
        let text = if uc.starts_with(USECASE_TAG_PREFIX) { uc.clone() } else { format!("{USECASE_TAG_PREFIX}{uc}") };
        if !has_tag(&tags, &text) {
            tags.push(json!({"text": text, "color": USECASE_TAG_COLOR}));
        }
    }
    let requested = body.get("priority").and_then(|p| p.as_i64());
    let createdby = body_str(body, "createdby");
    let parent = if createdby.len() >= 32 { st.store.get("workitem", project, &createdby).await? } else { None };
    if let Some(parent) = parent {
        // inherit usecase tags
        let ptags: Vec<iter_core::Tag> = parent.get("tags").and_then(|t| serde_json::from_value(t.clone()).ok()).unwrap_or_default();
        for text in usecase_tags(&ptags) {
            if !has_tag(&tags, &text) {
                tags.push(json!({"text": text, "color": USECASE_TAG_COLOR}));
            }
        }
        // inherit priority exactly
        let pprio = parent.get("priority").and_then(|p| p.as_i64()).unwrap_or(PRIO_BAND_HUMAN.0);
        if let Some(r) = requested {
            if r != pprio {
                warnings.push(format!("priority inherited from the creating item (P{pprio}); the requested P{r} was ignored — children carry their lineage's number"));
            }
        }
        body["priority"] = json!(pprio);
    } else if requested.is_none() {
        let is_usecase = body_str(body, "agent") == "usecase" || tags.iter().any(|t| t.get("text").and_then(|x| x.as_str()).map(|x| x.starts_with(USECASE_TAG_PREFIX)).unwrap_or(false));
        let human = !body_str(body, "requestedby").starts_with("agent");
        let band = if is_usecase { PRIO_BAND_USECASE } else if human { PRIO_BAND_HUMAN } else { PRIO_BAND_MAINT };
        let used: Vec<i64> = st
            .store
            .query("workitem", project)
            .await?
            .iter()
            .filter(|r| !is_closed(&body_str(r, "state")) && body_str(r, "state") != "scheduled")
            .filter_map(|r| r.get("priority").and_then(|p| p.as_i64()))
            .collect();
        body["priority"] = json!(pick_unused_priority(band, &used));
    } else if let Some(r) = requested {
        body["priority"] = json!(r.clamp(0, PRIO_MAX));
    }
    body["tags"] = json!(tags);
    Ok(warnings)
}

/// One-time 0–10 -> 0–99 migration (decided 2026-09-08): every workitem's
/// priority ×10 (P0 stays P0, capped at 99), closed items included (they are
/// otherwise immutable), then `priority_scale: 100` on the project so a
/// second call is a no-op.  Admin only.
async fn project_migrate_priority(user: AuthUser, State(st): Ctx, Path(name): Path<String>) -> Result<Json<Value>, ApiError> {
    user.require_admin()?;
    let mut project = st.store.get("project", &name, NOSK).await?.ok_or_else(notfound)?;
    if project.get("priority_scale").and_then(|v| v.as_u64()) == Some(100) {
        return Ok(Json(json!({"migrated": 0, "already": true})));
    }
    let rows = st.store.query("workitem", &name).await?;
    let mut migrated = 0u64;
    for mut row in rows {
        let id = body_str(&row, "id");
        let p = row.get("priority").and_then(|v| v.as_i64()).unwrap_or(5);
        row["priority"] = json!((p * 10).clamp(0, PRIO_MAX));
        row["version"] = json!(body_u64(&row, "version") + 1);
        st.store.put("workitem", &name, &id, &row).await?;
        migrated += 1;
    }
    project["priority_scale"] = json!(100);
    st.store.put("project", &name, NOSK, &project).await?;
    st.store.bump_seq(&name, "workitem").await?;
    st.store.bump_seq(&name, "project").await?;
    Ok(Json(json!({"migrated": migrated, "already": false})))
}

#[derive(serde::Deserialize)]
struct PriorityReq {
    priority: i64,
}

/// Re-assign one item's priority in place (webui Actions -> Set priority…,
/// 2026-09-10) without the pause -> edit -> requeue detour.  A lineage
/// carries ONE number (children inherit exactly), so the new number is
/// written to the item and to everything it created, transitively, open or
/// closed — the same rule realign applies — leaving schedule templates alone.
/// The target itself must be open: closed items stay immutable.  Every
/// version bumps; the state, claims and locks are untouched.
async fn workitem_priority(
    user: AuthUser,
    State(st): Ctx,
    Path((name, id)): Path<(String, String)>,
    Json(req): Json<PriorityReq>,
) -> Result<Json<Value>, ApiError> {
    user.require_writer()?;
    let target = st.store.get("workitem", &name, &id).await?.ok_or_else(notfound)?;
    if is_closed(&body_str(&target, "state")) {
        return Err(closed_err());
    }
    let to = req.priority.clamp(0, PRIO_MAX);
    let rows = st.store.query("workitem", &name).await?;
    let by_id: HashMap<String, Value> = rows.iter().map(|r| (body_str(r, "id"), r.clone())).collect();
    let mut kids: HashMap<String, Vec<String>> = HashMap::new();
    for r in &rows {
        let cb = body_str(r, "createdby");
        if by_id.contains_key(&cb) {
            kids.entry(cb).or_default().push(body_str(r, "id"));
        }
    }
    let mut lineage: Vec<String> = vec![id.clone()];
    let mut stack = vec![id.clone()];
    while let Some(x) = stack.pop() {
        for c in kids.get(&x).cloned().unwrap_or_default() {
            if !lineage.contains(&c) {
                lineage.push(c.clone());
                stack.push(c);
            }
        }
    }
    let mut changed: Vec<String> = Vec::new();
    for lid in &lineage {
        let Some(r) = by_id.get(lid) else { continue };
        if (lid != &id && body_str(r, "state") == "scheduled") || r.get("priority").and_then(|p| p.as_i64()) == Some(to) {
            continue;
        }
        let mut row = r.clone();
        row["priority"] = json!(to);
        row["version"] = json!(body_u64(&row, "version") + 1);
        st.store.put("workitem", &name, lid, &row).await?;
        changed.push(lid.clone());
    }
    if !changed.is_empty() {
        st.store.bump_seq(&name, "workitem").await?;
    }
    Ok(Json(json!({"id": id, "priority": to, "lineage": lineage.len(), "changed": changed})))
}

/// Band of a priority number (see iter_core PRIO_BAND_*).
fn band_of(p: i64) -> (i64, i64) {
    if p <= iter_core::PRIO_BAND_DO_NOW.1 { iter_core::PRIO_BAND_DO_NOW }
    else if p <= PRIO_BAND_USECASE.1 { PRIO_BAND_USECASE }
    else if p <= PRIO_BAND_HUMAN.1 { PRIO_BAND_HUMAN }
    else { PRIO_BAND_MAINT }
}

/// Lineage realignment (Stephen, 2026-09-08): every LIVE lineage (a root plus
/// everything it created, transitively, with at least one open item) gets ONE
/// number — the root's own when no earlier lineage took it, else the next
/// unused number in the root's band — and every item in the lineage, open or
/// closed, is set to it.  Lineages that are entirely closed, and schedule
/// templates, are left alone.  Admin only; idempotent (a second run finds
/// every live lineage already distinct).
async fn project_realign_priority(user: AuthUser, State(st): Ctx, Path(name): Path<String>) -> Result<Json<Value>, ApiError> {
    user.require_admin()?;
    let rows = st.store.query("workitem", &name).await?;
    let by_id: HashMap<String, Value> = rows.iter().map(|r| (body_str(r, "id"), r.clone())).collect();
    let mut kids: HashMap<String, Vec<String>> = HashMap::new();
    for r in &rows {
        let cb = body_str(r, "createdby");
        if by_id.contains_key(&cb) {
            kids.entry(cb).or_default().push(body_str(r, "id"));
        }
    }
    let is_open = |r: &Value| { let s = body_str(r, "state"); !is_closed(&s) && s != "scheduled" };
    let mut roots: Vec<&Value> = rows
        .iter()
        .filter(|r| !by_id.contains_key(&body_str(r, "createdby")) && body_str(r, "state") != "scheduled")
        .collect();
    roots.sort_by_key(|r| (r.get("priority").and_then(|p| p.as_i64()).unwrap_or(0), r.get("ts").and_then(|t| t.get("receive")).and_then(|x| x.as_str()).unwrap_or("").to_string()));
    let mut used: Vec<i64> = Vec::new();
    let mut report: Vec<Value> = Vec::new();
    let mut written = 0u64;
    for root in roots {
        // the lineage
        let root_id = body_str(root, "id");
        let mut lineage: Vec<String> = vec![root_id.clone()];
        let mut stack = vec![root_id.clone()];
        while let Some(x) = stack.pop() {
            for c in kids.get(&x).cloned().unwrap_or_default() {
                if !lineage.contains(&c) {
                    lineage.push(c.clone());
                    stack.push(c);
                }
            }
        }
        if !lineage.iter().any(|id| by_id.get(id).map(is_open).unwrap_or(false)) {
            continue; // history: leave as is
        }
        let from = root.get("priority").and_then(|p| p.as_i64()).unwrap_or(PRIO_BAND_HUMAN.0);
        let to = if used.contains(&from) { pick_unused_priority(band_of(from), &used) } else { from };
        used.push(to);
        let mut changed = 0u64;
        for id in &lineage {
            let Some(r) = by_id.get(id) else { continue };
            if body_str(r, "state") == "scheduled" || r.get("priority").and_then(|p| p.as_i64()) == Some(to) {
                continue;
            }
            let mut row = r.clone();
            row["priority"] = json!(to);
            row["version"] = json!(body_u64(&row, "version") + 1);
            st.store.put("workitem", &name, id, &row).await?;
            changed += 1;
            written += 1;
        }
        report.push(json!({"root": root_id, "name": body_str(root, "name"), "from": from, "to": to, "items": lineage.len(), "changed": changed}));
    }
    if written > 0 {
        st.store.bump_seq(&name, "workitem").await?;
    }
    Ok(Json(json!({"lineages": report.len(), "written": written, "report": report})))
}

fn normalize_new_item(project: &str, mut body: Value) -> Result<(String, Value), ApiError> {
    body["project"] = json!(project);
    if body_str(&body, "id").is_empty() {
        body["id"] = json!(uuid::Uuid::new_v4().to_string());
    }
    body["version"] = json!(1);
    if body_str(&body, "state").is_empty() {
        body["state"] = json!("queued"); // queued is the default create-state (spec)
    }
    if body.get("ts").map(|t| body_str(t, "receive").is_empty()).unwrap_or(true) {
        body["ts"] = json!({"receive": now_utc(), "start": "", "complete": ""});
    }
    let parsed: WorkItem =
        serde_json::from_value(body.clone()).map_err(|e| bad(format!("workitem does not parse: {e}")))?;
    if !iter_core::STATES.contains(&parsed.state.as_str()) {
        return Err(bad(format!("unknown state '{}'", parsed.state)));
    }
    Ok((parsed.id, body))
}

/// The test dir name the engine hands agents (ITER_TEST_DIR); `{test_dir}`
/// in a lock-shape pattern resolves to it.
const TEST_DIR: &str = "tests";

/// Lock shape (decided 2026-09-07): an item's lockdirs must fit its agent's
/// declared shape (agent record `lockshape`, project override merged key by
/// key).  Refusals are a 400 naming the rule and the overlap count; warnings
/// come back to the writer in the response's `warnings` array.  Checked on
/// create and on any PUT that changes `lockdirs`, never on other edits.
async fn lockshape_findings(
    st: &Arc<AppState>,
    project: &str,
    body: &Value,
) -> Result<Vec<String>, ApiError> {
    let agent = body_str(body, "agent");
    // shell runs (exec, a deterministic `test` item) hold no agent lock shape
    if agent.is_empty() || agent == "exec" || (iter_core::is_test_agent(&agent) && !body_str(body, "exec_shell").trim().is_empty()) {
        return Ok(vec![]);
    }
    let Some(def) = st.store.get("agent", &agent, NOSK).await? else { return Ok(vec![]) };
    let ovr = st
        .store
        .get("project", project, NOSK)
        .await?
        .and_then(|p| p.get("agents").and_then(|a| a.get(&agent)).cloned())
        .unwrap_or(Value::Null);
    let Some(shape) = lockshape_for(&def, &ovr) else { return Ok(vec![]) };
    let lockdirs: Vec<String> = body
        .get("lockdirs")
        .and_then(|l| l.as_array())
        .map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect())
        .unwrap_or_default();
    let me = body_str(body, "id");
    let others: Vec<(String, Vec<String>)> = st
        .store
        .query("workitem", project)
        .await?
        .iter()
        .filter(|r| body_str(r, "id") != me && !is_closed(&body_str(r, "state")) && body_str(r, "state") != "scheduled")
        .map(|r| {
            (
                body_str(r, "id"),
                r.get("lockdirs").and_then(|l| l.as_array()).map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect()).unwrap_or_default(),
            )
        })
        .collect();
    let findings = check_lockshape(&agent, &shape, &lockdirs, &others, TEST_DIR);
    let refused: Vec<String> = findings.iter().filter(|f| f.refuse).map(|f| f.msg.clone()).collect();
    if !refused.is_empty() {
        return Err(bad(refused.join("; ")));
    }
    Ok(findings.into_iter().map(|f| f.msg).collect())
}

/// workitem_dependency.md: a `blockedby` that closes a loop is refused with
/// the path named (last-12 ids, the way the webui shows them).  Shared by
/// create and PUT, so `iter add --depends-on`, `iter wait --on` and the
/// webui all inherit it (2026-09-11: pdy-dev item 3822952ce442 waited on
/// 6832b938a004, which the plan had blocked on 3822952ce442 — both sat
/// queued forever, and 40 more items nested under the pair).
async fn refuse_dependency_cycle(st: &Arc<AppState>, project: &str, id: &str, body: &Value) -> Result<(), ApiError> {
    let blockers: Vec<String> = body
        .get("blockedby")
        .and_then(|b| b.as_array())
        .map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect())
        .unwrap_or_default();
    if blockers.is_empty() {
        return Ok(());
    }
    let rows = st.store.query("workitem", project).await?;
    let items: Vec<WorkItem> = rows.iter().filter_map(|r| serde_json::from_value(r.clone()).ok()).collect();
    let by_id: HashMap<String, &WorkItem> = items.iter().map(|i| (i.id.clone(), i)).collect();
    if let Some(path) = iter_core::blockedby_cycle(id, &blockers, &by_id) {
        let short = |x: &String| x[x.len().saturating_sub(12)..].to_string();
        let shown: Vec<String> = path.iter().map(short).collect();
        return Err(bad(format!(
            "refused: dependency cycle — {} would wait on itself ({}); an item cannot wait on something that waits on it",
            short(&id.to_string()),
            shown.join(" -> ")
        )));
    }
    // deep edges too (CR 2026-09-25 option (c)): an item also waits on the
    // open follow-ups of its complete blockers, and a loop through one of
    // those is just as permanent as a declared one
    let mut me: WorkItem = serde_json::from_value(body.clone()).unwrap_or_default();
    me.id = id.to_string();
    me.blockedby = blockers;
    if let Some(cycle) = iter_core::waitgraph::dependency_cycle_on_write(&me, &items) {
        let short = |x: &str| x[x.len().saturating_sub(12)..].to_string();
        let mut shown: Vec<String> = vec![short(id)];
        for e in &cycle {
            match &e.kind {
                iter_core::WaitKind::Deep { via } => shown.push(format!("{} (deep: created by {})", short(&e.to), short(via))),
                _ => shown.push(short(&e.to)),
            }
        }
        return Err(bad(format!(
            "refused: dependency cycle — {} would wait on itself ({}); an item cannot wait on something that waits on it",
            short(id),
            shown.join(" -> ")
        )));
    }
    Ok(())
}

fn has_sched(row: &Value) -> bool {
    row.get("sched").map(|s| !s.is_null()).unwrap_or(false)
}

/// The engine-owned test sweep's template (iter_core::TEST_SWEEP): the one
/// item per project that may be paused but never deleted.
fn is_test_sweep_template(row: &Value) -> bool {
    body_str(row, "system") == iter_core::TEST_SWEEP && has_sched(row)
}

/// Create-time rules for `system` (2026-09-30). Only the test sweep exists.
/// Its template: `scheduled` or `paused`, and one per project (a second is
/// refused, naming the first — so the engine's create and a person's
/// `--install-schedule` can both run without making two). A run cloned from
/// it (the engine's fire, the webui's Run now) is stamped `system` here, so
/// every path that clones it gets the timer-run treatment. Returns true when
/// the body is the template.
async fn test_sweep_on_create(st: &Arc<AppState>, project: &str, body: &mut Value) -> Result<bool, ApiError> {
    let system = body_str(body, "system");
    if !system.is_empty() && system != iter_core::TEST_SWEEP {
        return Err(bad(format!("unknown system '{system}' (the only one is '{}')", iter_core::TEST_SWEEP)));
    }
    if !has_sched(body) {
        let src = body_str(body, "source_schedule");
        if system.is_empty() && !src.is_empty() {
            if let Some(tpl) = st.store.get("workitem", project, &src).await? {
                if is_test_sweep_template(&tpl) {
                    body["system"] = json!(iter_core::TEST_SWEEP);
                }
            }
        }
        return Ok(false);
    }
    if system.is_empty() {
        return Ok(false);
    }
    let state = body_str(body, "state");
    if state != "scheduled" && state != "paused" {
        return Err(bad(format!("the test sweep is created scheduled or paused, not '{state}'")));
    }
    let rows = st.store.query("workitem", project).await?;
    if let Some(existing) = rows.iter().find(|r| is_test_sweep_template(r)) {
        return Err(ApiError::Status(
            StatusCode::CONFLICT,
            format!("this project already has its test sweep ({}); change that one instead", body_str(existing, "id")),
        ));
    }
    Ok(true)
}

/// Update rules for the test sweep template (2026-09-30): `system` never
/// changes (an omitted key keeps the stored value, like `lease`), and the
/// template stays a schedule, `scheduled` or `paused` — pausing is how it is
/// turned off. Everything else (interval, name, command flags) is editable.
fn test_sweep_on_update(current: &Value, body: &mut Value) -> Result<(), ApiError> {
    let stored = body_str(current, "system");
    if body.get("system").is_none() && !stored.is_empty() {
        body["system"] = json!(stored);
    }
    if body_str(body, "system") != stored {
        return Err(bad("`system` is set when an item is created and never changes"));
    }
    if is_test_sweep_template(current) {
        let state = body_str(body, "state");
        if !has_sched(body) || (state != "scheduled" && state != "paused") {
            return Err(ApiError::Status(
                StatusCode::CONFLICT,
                format!("the test sweep stays a schedule: pause it to turn it off (asked for '{state}')"),
            ));
        }
    }
    Ok(())
}

fn with_warnings(mut body: Value, warnings: Vec<String>) -> Value {
    if !warnings.is_empty() {
        body["warnings"] = json!(warnings);
    }
    body
}

pub(crate) async fn workitem_create(
    user: AuthUser,
    State(st): Ctx,
    Path(name): Path<String>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, ApiError> {
    user.require_writer()?;
    let mut body = body;
    let sweep_template = test_sweep_on_create(&st, &name, &mut body).await?;
    // users-only rule (itersched.md): schedules come from humans via the
    // webui/API — the engine role (the agents' path) may not create them.
    // The one exception is the engine-owned test sweep (2026-09-30), which
    // the engine creates paused, once per project.
    if user.role == "engine"
        && !sweep_template
        && (body_str(&body, "state") == "scheduled" || body.get("sched").map(|s| !s.is_null()).unwrap_or(false))
    {
        return Err(ApiError::Status(
            StatusCode::FORBIDDEN,
            "schedules are users-only: the engine/agent path may not create scheduled items".into(),
        ));
    }
    // the request text may ride in the create body (decided 2026-09-10): it
    // becomes detail row 0 ("request"), and a create refused as a repeat can
    // still leave what it observed on the survivor
    let request = body.get("request").and_then(|r| r.as_str()).map(String::from).unwrap_or_default();
    // a placeholder is not a request (2026-09-21: an item was stored and
    // dispatched with "PLACEHOLDER - replaced immediately by the filing agent")
    if request.trim().to_ascii_uppercase().starts_with("PLACEHOLDER") {
        return Err(bad("refused: the request is a placeholder — write the real instructions"));
    }
    if let Some(o) = body.as_object_mut() {
        o.remove("request");
    }
    let mut warnings = place_new_item(&st, &name, &mut body).await?;
    // stage 1 repeat detection (iter_core::dedup, built 2026-09-10): a
    // request carrying both `check:` and `container:` tags is a repeat of any
    // OPEN item with the same two — no second row; the twin is told (doc row,
    // repeats, priority) and returned with `already_open`.  A CLOSED twin
    // does not block: the fault has recurred, and the new item says so.
    let mut recurrence: Option<(String, String)> = None;
    if let Some(key) = dedup::key_of_row(&body) {
        let rows = st.store.query("workitem", &name).await?;
        let twins: Vec<&Value> = rows.iter().filter(|r| dedup::key_of_row(r).as_ref() == Some(&key)).collect();
        let received = |r: &Value| r.get("ts").map(|t| body_str(t, "receive")).unwrap_or_default();
        if let Some(open) = twins.iter().filter(|r| dedup::is_open_row(r)).min_by_key(|r| received(r)) {
            let source = repeat_source(&body);
            let mut survivor = record_repeat(&st, &user, &name, open, &source, "a create request refused as a repeat", &request).await?;
            survivor["already_open"] = json!(true);
            return Ok(Json(survivor));
        }
        let completed = |r: &Value| r.get("ts").map(|t| body_str(t, "complete")).unwrap_or_default();
        if let Some(closed) = twins.iter().filter(|r| is_closed(&body_str(r, "state"))).max_by_key(|r| completed(r)) {
            recurrence = Some((body_str(closed, "id"), completed(closed)));
        }
    }
    let (id, body) = normalize_new_item(&name, body)?;
    warnings.extend(lockshape_findings(&st, &name, &body).await?);
    refuse_dependency_cycle(&st, &name, &id, &body).await?;
    st.store.put_versioned("workitem", &name, &id, &body, 0).await?;
    st.store.bump_seq(&name, "workitem").await?;
    if !request.is_empty() {
        let row = prepare_detail(&user, &id, 0, json!({"key": "request", "valuetype": "text", "value": request}))?;
        st.store.put("workitem_detail", &id, &detail_sk(0), &row).await?;
        st.store.bump_seq(&name, "workitem_detail").await?;
    }
    if let Some((closed_id, when)) = recurrence {
        let note = format!("recurrence of {closed_id}, which closed {}", if when.is_empty() { "earlier".to_string() } else { when });
        let _ = append_detail(&st, &user, &name, &id, json!({"key": DOC_KEY, "valuetype": "text", "value": note})).await?;
    }
    Ok(Json(with_warnings(body, warnings)))
}

/// Who a repeat came from, for the survivor's "seen again" row.
fn repeat_source(row: &Value) -> String {
    let r = body_str(row, "requestedby");
    if !r.is_empty() {
        return r;
    }
    body_str(row, "createdby")
}

/// The survivor's bookkeeping for one observed repeat (iter_core::dedup,
/// 2026-09-10) — every time, in both stages: a "doc" row "seen again <UTC>
/// by <source>" carrying the repeat's request text (a repeat is evidence),
/// `repeats` + 1, priority halved (floor 1), and the `repeated` tag once
/// `repeats` reaches the project's `dedup.repeated_threshold`.  A versioned
/// write, re-read and retried on a lost race.  Returns the written row.
async fn record_repeat(
    st: &Arc<AppState>,
    user: &AuthUser,
    project: &str,
    survivor: &Value,
    source: &str,
    via: &str,
    request: &str,
) -> Result<Value, ApiError> {
    let threshold = st
        .store
        .get("project", project, NOSK)
        .await?
        .and_then(|p| serde_json::from_value::<Project>(p).ok())
        .map(|p| p.dedup.repeated_threshold)
        .unwrap_or_else(|| dedup::DedupConfig::default().repeated_threshold);
    let id = body_str(survivor, "id");
    let mut row = survivor.clone();
    for attempt in 0..6 {
        if attempt > 0 {
            row = st.store.get("workitem", project, &id).await?.ok_or_else(notfound)?;
        }
        let expect = body_u64(&row, "version");
        let out = dedup::apply_repeat(&mut row, threshold);
        row["version"] = json!(expect + 1);
        match st.store.put_versioned("workitem", project, &id, &row, expect).await {
            Ok(()) => {
                st.store.bump_seq(project, "workitem").await?;
                let now = now_utc();
                let note = format!(
                    "{}\n\n(repeat #{}; priority P{} -> P{}{})",
                    dedup::seen_again_note(&now, source, via, request),
                    out.repeats, out.priority_from, out.priority_to,
                    if out.newly_repeated { format!("; tagged `{}`", dedup::REPEATED_TAG) } else { String::new() }
                );
                let _ = append_detail(st, user, project, &id, json!({"key": DOC_KEY, "valuetype": "text", "value": note})).await?;
                return Ok(row);
            }
            Err(StorageError::Conflict(_)) => continue,
            Err(e) => return Err(e.into()),
        }
    }
    Err(ApiError::Status(StatusCode::CONFLICT, "could not record the repeat on the survivor (contention)".into()))
}

#[derive(serde::Deserialize, Default)]
struct DuplicateReq {
    survivor: String,
    /// one sentence from the judge, or "same check and container"
    #[serde(default)]
    reason: String,
    /// other open items the judge also called the same fault (noted on the survivor)
    #[serde(default)]
    others: Vec<String>,
}

/// Merge (iter_core::dedup, 2026-09-10): close THIS item as a duplicate of
/// `survivor` — state `complete` (never a new state), the tag
/// `dup of: <last 12 of the survivor>`, a "doc" row naming the survivor and
/// the reason — and run the survivor's repeat bookkeeping with this item's
/// request text.  Refused when either side is closed or in progress: a
/// running item is never closed under its agent, and a running or finished
/// item never absorbs new evidence unseen.
async fn workitem_duplicate_of(
    user: AuthUser,
    State(st): Ctx,
    Path((name, id)): Path<(String, String)>,
    Json(req): Json<DuplicateReq>,
) -> Result<Json<Value>, ApiError> {
    user.require_writer()?;
    let dup = st.store.get("workitem", &name, &id).await?.ok_or_else(notfound)?;
    let refuse = |what: &str, why: String| ApiError::Status(StatusCode::CONFLICT, format!("{what} {why}: nothing merged"));
    let dstate = body_str(&dup, "state");
    if !dedup::is_open_row(&dup) {
        return Err(refuse("the duplicate", format!("is {dstate}")));
    }
    if dstate == "in-progress" {
        return Err(refuse("the duplicate", "is in progress".into()));
    }
    let survivor_id = req.survivor.trim().to_string();
    if survivor_id.is_empty() || survivor_id == id {
        return Err(bad("survivor must name another workitem"));
    }
    let survivor = st
        .store
        .get("workitem", &name, &survivor_id)
        .await?
        .ok_or_else(|| ApiError::Status(StatusCode::NOT_FOUND, "survivor not found".into()))?;
    let sstate = body_str(&survivor, "state");
    if !dedup::is_open_row(&survivor) {
        return Err(refuse("the survivor", format!("is {sstate}")));
    }
    if sstate == "in-progress" {
        return Err(refuse("the survivor", "is in progress".into()));
    }
    let reason = if req.reason.trim().is_empty() { "same check and container".to_string() } else { req.reason.trim().to_string() };
    // close the duplicate
    let now = now_utc();
    let expect = body_u64(&dup, "version");
    let mut closed = dup.clone();
    closed["state"] = json!("complete");
    closed["ts"]["complete"] = json!(now);
    closed["dedup_checked"] = json!(now);
    closed["version"] = json!(expect + 1);
    let dup_tag = dedup::dup_of_tag(&survivor_id);
    let mut tags: Vec<Value> = closed
        .get("tags")
        .and_then(|t| t.as_array())
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .filter(|t| !t.get("text").and_then(|x| x.as_str()).map(|x| x.starts_with(BLOCKED_TAG_PREFIX)).unwrap_or(false))
        .collect();
    if !tags.iter().any(|t| t.get("text").and_then(|x| x.as_str()) == Some(dup_tag.text.as_str())) {
        tags.push(json!({"text": dup_tag.text, "color": dup_tag.color}));
    }
    closed["tags"] = json!(tags);
    match st.store.put_versioned("workitem", &name, &id, &closed, expect).await {
        Ok(()) => {}
        Err(StorageError::Conflict(_)) => {
            return Err(ApiError::Status(StatusCode::CONFLICT, "the duplicate changed underneath the merge: re-read and retry".into()));
        }
        Err(e) => return Err(e.into()),
    }
    st.store.bump_seq(&name, "workitem").await?;
    let _ = append_detail(&st, &user, &name, &id, json!({"key": DOC_KEY, "valuetype": "text",
        "value": format!("Duplicate of {survivor_id}: {reason}")})).await?;
    // the survivor: seen again, with everything the duplicate observed
    let request = st
        .store
        .query("workitem_detail", &id)
        .await?
        .iter()
        .find(|d| body_str(d, "key") == "request")
        .map(|d| body_str(d, "value"))
        .unwrap_or_default();
    let via = if req.others.is_empty() {
        format!("duplicate {id} merged here")
    } else {
        format!("duplicate {id} merged here; also judged the same fault: {}", req.others.join(", "))
    };
    let survivor = record_repeat(&st, &user, &name, &survivor, &repeat_source(&dup), &via, &request).await?;
    Ok(Json(json!({"duplicate": closed, "survivor": survivor})))
}

async fn workitem_get(
    _u: AuthUser,
    State(st): Ctx,
    Path((name, id)): Path<(String, String)>,
) -> Result<Json<Value>, ApiError> {
    Ok(Json(st.store.get("workitem", &name, &id).await?.ok_or_else(notfound)?))
}

async fn workitem_put(
    user: AuthUser,
    State(st): Ctx,
    Path((name, id)): Path<(String, String)>,
    Query(q): Query<HashMap<String, String>>,
    Json(mut body): Json<Value>,
) -> Result<Json<Value>, ApiError> {
    user.require_writer()?;
    let expect: u64 = q
        .get("expect_version")
        .and_then(|v| v.parse().ok())
        .or_else(|| body.get("expect_version").and_then(|v| v.as_u64()))
        .ok_or_else(|| bad("expect_version required (query param or body field)"))?;
    if let Some(o) = body.as_object_mut() {
        o.remove("expect_version");
    }
    // closed items are immutable (decided 2026-09-03): append a "doc" detail
    // row, or POST .../reopen — never edit the record in place.  The one
    // exception is "tags", so finished work can still be organized.
    let current = st.store.get("workitem", &name, &id).await?;
    if let Some(current) = &current {
        test_sweep_on_update(current, &mut body)?;
        if is_closed(&body_str(current, "state")) && !tags_only_change(current, &body) {
            return Err(closed_err());
        }
    }
    body["project"] = json!(name);
    body["id"] = json!(id);
    body["version"] = json!(expect + 1);
    // the run's lease (CR 2026-09-25): an omitted key keeps the stored one —
    // an agent's GET-modify-PUT script or an older webui bundle that drops
    // unknown fields must never end a live run's locks by accident; only an
    // explicit "" or a new value changes it
    let old_lease = current.as_ref().map(|c| body_str(c, "lease")).unwrap_or_default();
    if body.get("lease").is_none() && !old_lease.is_empty() {
        body["lease"] = json!(old_lease);
    }
    let parsed: WorkItem =
        serde_json::from_value(body.clone()).map_err(|e| bad(format!("workitem does not parse: {e}")))?;
    if !iter_core::STATES.contains(&parsed.state.as_str()) {
        return Err(bad(format!("unknown state '{}'", parsed.state)));
    }
    // lock shape: only when the lockdirs (or the agent) actually change, so a
    // rule added later never refuses an unrelated edit of an existing item
    let lockdirs_changed = current
        .as_ref()
        .map(|c| c.get("lockdirs") != body.get("lockdirs") || body_str(c, "agent") != body_str(&body, "agent"))
        .unwrap_or(true);
    let warnings = if lockdirs_changed { lockshape_findings(&st, &name, &body).await? } else { vec![] };
    // dependency cycles: only when the links change, so an item already in a
    // loop (before this refusal existed) can still be edited out of it
    let blockedby_changed = current.as_ref().map(|c| c.get("blockedby") != body.get("blockedby")).unwrap_or(true);
    if blockedby_changed {
        refuse_dependency_cycle(&st, &name, &id, &body).await?;
    }
    st.store.put_versioned("workitem", &name, &id, &body, expect).await?;
    st.store.bump_seq(&name, "workitem").await?;
    // the server half of "every exit from a run removes every lock the run
    // held": a changed lease deletes the old lease's rows, including rows
    // the agent took outside its lockdirs — no engine path can forget it
    if !old_lease.is_empty() && parsed.lease != old_lease {
        release_rows(&st, &name, |r| r.workid == id && r.lease == old_lease).await?;
    }
    Ok(Json(with_warnings(body, warnings)))
}

async fn workitem_delete(
    user: AuthUser,
    State(st): Ctx,
    Path((name, id)): Path<(String, String)>,
) -> Result<Json<Value>, ApiError> {
    user.require_writer()?;
    if let Some(current) = st.store.get("workitem", &name, &id).await? {
        if is_test_sweep_template(&current) {
            return Err(ApiError::Status(
                StatusCode::CONFLICT,
                "the test sweep cannot be deleted: pause it to turn it off".into(),
            ));
        }
    }
    let deleted = st.store.delete("workitem", &name, &id).await?;
    st.store.bump_seq(&name, "workitem").await?;
    Ok(Json(json!({"deleted": deleted})))
}

// ---------- workitem details ----------

async fn details_list(
    _u: AuthUser,
    State(st): Ctx,
    Path((_name, id)): Path<(String, String)>,
) -> Result<Json<Value>, ApiError> {
    let mut rows = st.store.query("workitem_detail", &id).await?;
    // sk is a string; sort numerically by order
    rows.sort_by_key(|r| r.get("order").and_then(|v| v.as_i64()).unwrap_or(0));
    Ok(Json(Value::Array(rows)))
}

/// Closed states (decided 2026-09-03): the record is frozen; only "doc"
/// detail rows may be appended, and only `reopen` moves it back to queued.
pub const CLOSED_STATES: &[&str] = &["complete", "failed"];
/// The one detail key that may be appended to a closed item.
pub const DOC_KEY: &str = "doc";
/// the ELI5 row the engine appends (spec: Explain / ELI5); like "doc", it may
/// land on a closed item
pub const EXPLAINED_KEY: &str = "explained";

pub fn is_closed(state: &str) -> bool {
    CLOSED_STATES.contains(&state)
}

/// True when `proposed` differs from `current` in nothing but "tags"
/// (version is the write's own bump and is ignored).
pub fn tags_only_change(current: &Value, proposed: &Value) -> bool {
    let strip = |v: &Value| -> Value {
        let mut c = v.clone();
        if let Some(o) = c.as_object_mut() {
            o.remove("tags");
            o.remove("version");
            o.remove("expect_version");
        }
        c
    };
    strip(current) == strip(proposed)
}

fn closed_err() -> ApiError {
    ApiError::Status(
        StatusCode::FORBIDDEN,
        "closed workitem is immutable: append a \"doc\" detail row (POST .../details) or POST .../reopen".into(),
    )
}

/// Validate + provenance-stamp a detail body.  Every detail write records
/// who (JWT principal) and when, so a closeout note is a real record.
fn prepare_detail(user: &AuthUser, id: &str, order: i64, mut body: Value) -> Result<Value, ApiError> {
    body["id"] = json!(id);
    body["order"] = json!(order);
    body["by"] = json!(user.sub);
    body["ts"] = json!(now_utc());
    let parsed: iter_core::WorkItemDetail =
        serde_json::from_value(body.clone()).map_err(|e| bad(format!("detail does not parse: {e}")))?;
    if parsed.key.trim().is_empty() {
        return Err(bad("detail key is required"));
    }
    // question widgets are validated at write time so malformed ones bounce here
    if parsed.valuetype == "json" && parsed.value.get("fields").is_some() {
        let errs = widget::validate(&parsed.value);
        if !errs.is_empty() {
            return Err(bad(format!("widget invalid: {}", errs.join("; "))));
        }
    }
    Ok(body)
}

/// zero-pad sk so lexical order == numeric order in backends
fn detail_sk(order: i64) -> String {
    format!("{order:010}")
}

async fn detail_put(
    user: AuthUser,
    State(st): Ctx,
    Path((name, id, order)): Path<(String, String, i64)>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, ApiError> {
    user.require_writer()?;
    // in-place detail writes (question answers overwrite value) are for OPEN items only
    if let Some(item) = st.store.get("workitem", &name, &id).await? {
        if is_closed(&body_str(&item, "state")) {
            return Err(closed_err());
        }
    }
    let body = prepare_detail(&user, &id, order, body)?;
    st.store.put("workitem_detail", &id, &detail_sk(order), &body).await?;
    st.store.bump_seq(&name, "workitem_detail").await?;
    Ok(Json(body))
}

/// Append a detail row: iter_data allocates the next order atomically
/// (create-if-absent on the zero-padded sk, retried on a lost race), so two
/// appenders can never overwrite each other.  On a closed item only "doc"
/// rows are accepted.
async fn details_append(
    user: AuthUser,
    State(st): Ctx,
    Path((name, id)): Path<(String, String)>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, ApiError> {
    user.require_writer()?;
    let item = st.store.get("workitem", &name, &id).await?.ok_or_else(notfound)?;
    // closed items take only appended notes: "doc" (humans), "explained" (the
    // ELI5 run) and the "spend" row that run costs — never a new response
    let key = body_str(&body, "key");
    if is_closed(&body_str(&item, "state")) && key != DOC_KEY && key != EXPLAINED_KEY && key != "spend" {
        return Err(ApiError::Status(
            StatusCode::FORBIDDEN,
            format!("closed workitem: only \"{DOC_KEY}\", \"{EXPLAINED_KEY}\" and \"spend\" detail rows may be appended"),
        ));
    }
    append_detail(&st, &user, &name, &id, body).await
}

async fn append_detail(st: &Arc<AppState>, user: &AuthUser, name: &str, id: &str, body: Value) -> Result<Json<Value>, ApiError> {
    for _ in 0..8 {
        let next = st
            .store
            .query("workitem_detail", id)
            .await?
            .iter()
            .map(|r| r.get("order").and_then(|o| o.as_i64()).unwrap_or(0))
            .max()
            .map(|m| m + 1)
            .unwrap_or(0);
        let row = prepare_detail(user, id, next, body.clone())?;
        match st.store.put_versioned("workitem_detail", id, &detail_sk(next), &row, 0).await {
            Ok(()) => {
                st.store.bump_seq(name, "workitem_detail").await?;
                return Ok(Json(row));
            }
            Err(StorageError::Conflict(_)) => continue, // lost the race for this order; re-read
            Err(e) => return Err(e.into()),
        }
    }
    Err(ApiError::Status(StatusCode::CONFLICT, "could not allocate a detail order (contention)".into()))
}

// ---------- explain (ELI5) ----------

/// webui -> engine: ask for a plain-language explanation of this item (spec:
/// Explain / ELI5). Stamps `explain_requested`; the engine serving the project
/// sees it on its next tick and runs the read-only `explain` agent at once,
/// outside the agent cap. Works on closed items too (no state change).
async fn workitem_explain(
    user: AuthUser,
    State(st): Ctx,
    Path((name, id)): Path<(String, String)>,
) -> Result<Json<Value>, ApiError> {
    user.require_writer()?;
    let mut item = st.store.get("workitem", &name, &id).await?.ok_or_else(notfound)?;
    let pending = body_str(&item, "explain_requested");
    if !pending.is_empty() {
        return Ok(Json(json!({"requested": pending, "already": true})));
    }
    let ts = now_utc();
    let expect = body_u64(&item, "version");
    // one engine only (decided 2026-09-04): pick at random among the LIVE
    // engines serving this project so a second engine never duplicates the
    // run; none live = leave it open for the first engine to claim
    let engine = live_engines_for(&st, &name).await?.choose(&mut rand::thread_rng()).cloned().unwrap_or_default();
    item["explain_requested"] = json!(ts);
    item["explain_engine"] = json!(engine);
    item["version"] = json!(expect + 1);
    st.store.put_versioned("workitem", &name, &id, &item, expect).await?;
    st.store.bump_seq(&name, "workitem").await?;
    Ok(Json(json!({"requested": ts, "engine": engine})))
}

/// Engines whose record names this project and that have heartbeated within
/// three ticks (the webui's own liveness rule).
async fn live_engines_for(st: &Arc<AppState>, project: &str) -> Result<Vec<String>, ApiError> {
    let now = chrono::Utc::now();
    let edges = crate::settings::load_edges(st.store.as_ref()).await?;
    Ok(st
        .store
        .scan("engine")
        .await?
        .iter()
        .filter(|e| {
            crate::settings::serves_edge(&edges, &body_str(e, "id"), project).is_some()
                || e.get("projects").and_then(|p| p.get(project)).is_some()
        })
        .filter(|e| body_str(e, "state") == "Running")
        .filter(|e| {
            let tick = e.get("ticksec").and_then(|t| t.as_i64()).unwrap_or(5).max(1);
            chrono::DateTime::parse_from_rfc3339(&body_str(e, "last_seen"))
                .map(|seen| (now - seen.with_timezone(&chrono::Utc)).num_seconds() <= 3 * tick + 5)
                .unwrap_or(false)
        })
        .map(|e| body_str(e, "id"))
        .filter(|n| !n.is_empty())
        .collect())
}

#[derive(serde::Deserialize, Default)]
struct ExplainClaimReq {
    #[serde(default)]
    engine: String,
}

/// engine -> iter_data: "I will run this ELI5". Succeeds when the item is
/// assigned to this engine or to nobody (then it becomes this engine's);
/// 409 when another engine holds it, so at most one engine ever runs it.
async fn workitem_explain_claim(
    user: AuthUser,
    State(st): Ctx,
    Path((name, id)): Path<(String, String)>,
    Json(req): Json<ExplainClaimReq>,
) -> Result<Json<Value>, ApiError> {
    user.require_writer()?;
    if req.engine.trim().is_empty() {
        return Err(bad("engine is required"));
    }
    let mut item = st.store.get("workitem", &name, &id).await?.ok_or_else(notfound)?;
    if body_str(&item, "explain_requested").is_empty() {
        return Err(bad("no ELI5 is pending on this workitem"));
    }
    let holder = body_str(&item, "explain_engine");
    if !holder.is_empty() && holder != req.engine {
        return Err(ApiError::Status(StatusCode::CONFLICT, format!("ELI5 on this workitem is assigned to engine '{holder}'")));
    }
    if holder.is_empty() {
        let expect = body_u64(&item, "version");
        item["explain_engine"] = json!(req.engine);
        item["version"] = json!(expect + 1);
        st.store.put_versioned("workitem", &name, &id, &item, expect).await?;
        st.store.bump_seq(&name, "workitem").await?;
    }
    Ok(Json(json!({"engine": req.engine})))
}

/// engine -> iter_data: the explanation landed (or could not be produced);
/// clear the flag so the button re-arms.
async fn workitem_explained(
    user: AuthUser,
    State(st): Ctx,
    Path((name, id)): Path<(String, String)>,
) -> Result<Json<Value>, ApiError> {
    user.require_writer()?;
    let mut item = st.store.get("workitem", &name, &id).await?.ok_or_else(notfound)?;
    if body_str(&item, "explain_requested").is_empty() {
        return Ok(Json(json!({"cleared": false})));
    }
    let expect = body_u64(&item, "version");
    item["explain_requested"] = json!("");
    item["explain_engine"] = json!("");
    item["version"] = json!(expect + 1);
    st.store.put_versioned("workitem", &name, &id, &item, expect).await?;
    st.store.bump_seq(&name, "workitem").await?;
    Ok(Json(json!({"cleared": true})))
}

// ---------- reopen ----------

#[derive(serde::Deserialize, Default)]
struct ReopenReq {
    #[serde(default)]
    reason: String,
}

/// Reopen a closed item (users-only, like schedules): back to queued with the
/// bounce counter reset, and a "doc" row recording who reopened it and why.
/// Consequence (spec): downstream items still queued become blocked again.
async fn workitem_reopen(
    user: AuthUser,
    State(st): Ctx,
    Path((name, id)): Path<(String, String)>,
    body: Option<Json<ReopenReq>>,
) -> Result<Json<Value>, ApiError> {
    user.require_writer()?;
    if user.role == "engine" {
        return Err(ApiError::Status(StatusCode::FORBIDDEN, "reopen is users-only: the engine/agent path may not reopen closed items".into()));
    }
    let req = body.map(|b| b.0).unwrap_or_default();
    let mut item = st.store.get("workitem", &name, &id).await?.ok_or_else(notfound)?;
    if !is_closed(&body_str(&item, "state")) {
        return Err(bad("workitem is not closed"));
    }
    let expect = body_u64(&item, "version");
    let was = body_str(&item, "state");
    item["state"] = json!("queued");
    item["gate_bounces"] = json!(0);
    item["lasterror"] = json!("");
    item["ts"]["complete"] = json!("");
    item["version"] = json!(expect + 1);
    st.store.put_versioned("workitem", &name, &id, &item, expect).await?;
    st.store.bump_seq(&name, "workitem").await?;
    let note = if req.reason.trim().is_empty() {
        format!("reopened by {} (was {was})", user.sub)
    } else {
        format!("reopened by {} (was {was}): {}", user.sub, req.reason.trim())
    };
    let _ = append_detail(&st, &user, &name, &id, json!({"key": DOC_KEY, "valuetype": "text", "value": note})).await?;
    Ok(Json(item))
}

#[derive(serde::Deserialize, Default)]
struct StateReq {
    #[serde(default)]
    state: String,
    #[serde(default)]
    reason: String,
}

/// Move an item to ANY state by hand (webui Actions -> Change to…, decided
/// 2026-09-10) — the escape hatch beside the shortcuts (Queue, Park, Reopen…),
/// including closed -> closed (failed -> complete).  Users-only like reopen:
/// the engine/agent path keeps its own transitions.  Bookkeeping mirrors the
/// shortcuts: into a closed state stamps ts.complete when empty; out of one
/// clears ts.complete, the bounce counter and lasterror; every move appends a
/// "doc" row naming who, from, to and why.  Nothing is stopped or started:
/// a session an agent is running on the item is untouched.
async fn workitem_state(
    user: AuthUser,
    State(st): Ctx,
    Path((name, id)): Path<(String, String)>,
    body: Option<Json<StateReq>>,
) -> Result<Json<Value>, ApiError> {
    user.require_writer()?;
    if user.role == "engine" {
        return Err(ApiError::Status(StatusCode::FORBIDDEN, "state change is users-only: the engine/agent path keeps its own transitions".into()));
    }
    let req = body.map(|b| b.0).unwrap_or_default();
    let to = req.state.trim().to_string();
    if !iter_core::STATES.contains(&to.as_str()) {
        return Err(bad(format!("unknown state '{to}' (one of {})", iter_core::STATES.join(", "))));
    }
    let mut item = st.store.get("workitem", &name, &id).await?.ok_or_else(notfound)?;
    let was = body_str(&item, "state");
    if was == to {
        return Err(bad(format!("workitem is already {to}")));
    }
    let expect = body_u64(&item, "version");
    item["state"] = json!(to);
    if is_closed(&to) {
        if body_str(&item, "ts").is_empty() && item.get("ts").and_then(|t| t.get("complete")).and_then(|c| c.as_str()).unwrap_or("").is_empty() {
            item["ts"]["complete"] = json!(now_utc());
        }
    } else if is_closed(&was) {
        item["gate_bounces"] = json!(0);
        item["lasterror"] = json!("");
        item["ts"]["complete"] = json!("");
    }
    item["version"] = json!(expect + 1);
    st.store.put_versioned("workitem", &name, &id, &item, expect).await?;
    st.store.bump_seq(&name, "workitem").await?;
    let note = if req.reason.trim().is_empty() {
        format!("state changed by {}: {was} -> {to}", user.sub)
    } else {
        format!("state changed by {}: {was} -> {to}: {}", user.sub, req.reason.trim())
    };
    let _ = append_detail(&st, &user, &name, &id, json!({"key": DOC_KEY, "valuetype": "text", "value": note})).await?;
    Ok(Json(item))
}

// ---------- approval ----------

#[derive(serde::Deserialize)]
struct ApproveReq {
    user: String,
    /// base64 of the 64-byte ed25519 signature over the workitem id (utf-8)
    signature: String,
}

async fn workitem_approve(
    caller: AuthUser,
    State(st): Ctx,
    Path((name, id)): Path<(String, String)>,
    Json(req): Json<ApproveReq>,
) -> Result<Json<Value>, ApiError> {
    caller.require_writer()?;
    let mut item = st.store.get("workitem", &name, &id).await?.ok_or_else(notfound)?;
    let user_row = st
        .store
        .get("webui_user", &req.user, NOSK)
        .await?
        .ok_or_else(|| bad("unknown approving user"))?;
    let pubkey_b64 = body_str(&user_row, "pubkey");
    let verified = verify_ed25519(&pubkey_b64, &id, &req.signature);
    let expect = body_u64(&item, "version");
    if verified {
        item["approval_code"] = json!(req.signature);
        item["needs_approval"] = json!(false);
        item["state"] = json!("queued");
        item["version"] = json!(expect + 1);
        st.store.put_versioned("workitem", &name, &id, &item, expect).await?;
        st.store.bump_seq(&name, "workitem").await?;
        Ok(Json(json!({"approved": true, "item": item})))
    } else {
        // spec: clear the approval code, log the failure, leave it needing approval
        item["approval_code"] = json!("");
        item["version"] = json!(expect + 1);
        st.store.put_versioned("workitem", &name, &id, &item, expect).await?;
        st.store.bump_seq(&name, "workitem").await?;
        eprintln!("[iter_data] approval FAILED for workitem {id} by {}", req.user);
        Err(bad("signature did not verify against the user's pubkey"))
    }
}

fn verify_ed25519(pubkey_b64: &str, message: &str, sig_b64: &str) -> bool {
    use ed25519_dalek::{Signature, Verifier, VerifyingKey};
    let Ok(pk_bytes) = base64::engine::general_purpose::STANDARD.decode(pubkey_b64.trim()) else {
        return false;
    };
    let Ok(pk_arr): Result<[u8; 32], _> = pk_bytes.as_slice().try_into() else {
        return false;
    };
    let Ok(vk) = VerifyingKey::from_bytes(&pk_arr) else {
        return false;
    };
    let Ok(sig_bytes) = base64::engine::general_purpose::STANDARD.decode(sig_b64.trim()) else {
        return false;
    };
    let Ok(sig_arr): Result<[u8; 64], _> = sig_bytes.as_slice().try_into() else {
        return false;
    };
    vk.verify(message.as_bytes(), &Signature::from_bytes(&sig_arr)).is_ok()
}

// ---------- locks ----------

/// Live rows only: an expired lock or reservation is free to `acquire`, so
/// it must not be shown (webui) or honored (engine scope gate) either.  Each
/// row also says whose it is (CR 2026-09-25): `holder_state` and
/// `holder_lease_live` (the row counts: its holder is running under the
/// row's lease), so a row held by an item that is not running is visible.
async fn locks_list(_u: AuthUser, State(st): Ctx, Path(name): Path<String>) -> Result<Json<Value>, ApiError> {
    let now = now_utc();
    let items = project_items(&st, &name).await?;
    let by_id: HashMap<String, &WorkItem> = items.iter().map(|i| (i.id.clone(), i)).collect();
    let rows = st.store.query("lock", &name).await?;
    Ok(Json(Value::Array(
        rows.into_iter()
            .filter(|r| body_str(r, "expires").is_empty() || body_str(r, "expires") >= now)
            .map(|mut r| {
                let lr: LockRow = serde_json::from_value(r.clone()).unwrap_or_default();
                let holder = by_id.get(&lr.workid).copied();
                r["holder_state"] = json!(holder.map(|h| h.state.clone()).unwrap_or_default());
                r["holder_lease_live"] = json!(iter_core::waitgraph::lock_row_is_live(&lr, holder, &now));
                r
            })
            .collect(),
    )))
}

pub(crate) async fn project_items(st: &Arc<AppState>, project: &str) -> Result<Vec<WorkItem>, ApiError> {
    Ok(st.store.query("workitem", project).await?.into_iter().filter_map(|r| serde_json::from_value(r).ok()).collect())
}

/// Delete every lock-table row `pick` selects (the storage layer has no
/// secondary index; a project holds tens of rows); returns their paths.
pub(crate) async fn release_rows(st: &Arc<AppState>, project: &str, pick: impl Fn(&LockRow) -> bool) -> Result<Vec<LockRow>, ApiError> {
    let mut gone: Vec<LockRow> = Vec::new();
    for r in st.store.query("lock", project).await? {
        let lr: LockRow = serde_json::from_value(r).unwrap_or_default();
        if pick(&lr) && st.store.delete("lock", project, &iter_core::lock_sk(&lr.kind, &lr.path)).await? {
            gone.push(lr);
        }
    }
    if !gone.is_empty() {
        st.store.bump_seq(project, "lock").await?;
    }
    Ok(gone)
}

pub(crate) fn iso_in(sec: i64) -> String {
    (chrono::Utc::now() + chrono::Duration::seconds(sec)).format("%Y-%m-%dT%H:%M:%SZ").to_string()
}

fn refused(refused: &str, msg: String, extra: Value) -> ApiError {
    let mut body = json!({"error": msg, "refused": refused});
    if let (Some(o), Some(x)) = (body.as_object_mut(), extra.as_object()) {
        o.extend(x.clone());
    }
    ApiError::Refused(StatusCode::CONFLICT, body)
}

#[derive(serde::Deserialize)]
struct LockAcquireReq {
    path: String,
    #[serde(default = "default_kind")]
    kind: String,
    #[serde(default)]
    engine: String,
    workid: String,
    /// 0 = the default: the lease lifetime (LOCK_LEASE_TTL_SEC); locks are capped at it
    #[serde(default)]
    ttl_sec: i64,
    /// the run's lease; "" = whatever lease the holder carries now (an agent's own acquire)
    #[serde(default)]
    lease: String,
}
fn default_kind() -> String { "lock".into() }

/// One lock or reservation (CR 2026-09-25 6.2 item 3): a lock goes only to
/// an item with a live lease and carries that lease, so it dies with the run
/// (`iter_core::waitgraph::lock_grant`); a reservation only to a queued item.
/// A reservation lives under its own key (`reserve:<path>`), so it never
/// makes another item's lock on the same path fail.
async fn lock_acquire(
    user: AuthUser,
    State(st): Ctx,
    Path(name): Path<String>,
    Json(req): Json<LockAcquireReq>,
) -> Result<Json<Value>, ApiError> {
    user.require_writer()?;
    let holder: WorkItem = st
        .store
        .get("workitem", &name, &req.workid)
        .await?
        .and_then(|v| serde_json::from_value(v).ok())
        .ok_or_else(|| ApiError::Status(StatusCode::NOT_FOUND, format!("no such work item {}", req.workid)))?;
    // lease-bound locks are always enforced (decided 2026-09-28): a lock goes
    // only to a running item and carries its run's lease
    let lease = match iter_core::waitgraph::lock_grant(&req.kind, &holder, &req.lease) {
        iter_core::waitgraph::LockGrant::Grant { lease } => lease,
        iter_core::waitgraph::LockGrant::Refuse { refused: word, msg } => {
            return Err(refused(word, msg, json!({"workid": req.workid, "state": holder.state})));
        }
    };
    let mut ttl = if req.ttl_sec > 0 { req.ttl_sec } else { iter_core::LOCK_LEASE_TTL_SEC };
    if req.kind == "lock" {
        ttl = ttl.min(iter_core::LOCK_LEASE_TTL_SEC);
    }
    let now = now_utc();
    let row = LockRow {
        project: name.clone(),
        path: req.path.clone(),
        kind: req.kind.clone(),
        // rows name the engine whose run holds them, never ""
        engine: if req.engine.is_empty() { holder.engine.clone() } else { req.engine },
        workid: req.workid.clone(),
        acquired: now.clone(),
        expires: iso_in(ttl),
        lease,
    };
    let body = serde_json::to_value(&row).unwrap();
    st.store.acquire_lock("lock", &name, &iter_core::lock_sk(&req.kind, &req.path), &body, &now, &req.workid).await?;
    st.store.bump_seq(&name, "lock").await?;
    Ok(Json(body))
}

#[derive(serde::Deserialize)]
struct LockReleaseReq {
    path: String,
    workid: String,
    #[serde(default = "default_kind")]
    kind: String,
}

async fn lock_release(
    user: AuthUser,
    State(st): Ctx,
    Path(name): Path<String>,
    Json(req): Json<LockReleaseReq>,
) -> Result<Json<Value>, ApiError> {
    user.require_writer()?;
    let sk = iter_core::lock_sk(&req.kind, &req.path);
    if let Some(row) = st.store.get("lock", &name, &sk).await? {
        if body_str(&row, "workid") != req.workid {
            return Err(bad("lock held by a different workid"));
        }
        st.store.delete("lock", &name, &sk).await?;
        st.store.bump_seq(&name, "lock").await?;
    }
    Ok(Json(json!({"released": true})))
}

#[derive(serde::Deserialize)]
struct LockRenewReq {
    workid: String,
    #[serde(default)]
    lease: String,
    #[serde(default)]
    ttl_sec: i64,
}

/// The only way a lock row's life is extended (CR 2026-09-25 option (e)):
/// the engine running the item renews every row carrying the item's live
/// lease.  A lease that is not the live one is a run that has ended — or a
/// stray process that never knew the lease — and is refused.
async fn lock_renew(
    user: AuthUser,
    State(st): Ctx,
    Path(name): Path<String>,
    Json(req): Json<LockRenewReq>,
) -> Result<Json<Value>, ApiError> {
    user.require_writer()?;
    let holder: WorkItem = st
        .store
        .get("workitem", &name, &req.workid)
        .await?
        .and_then(|v| serde_json::from_value(v).ok())
        .ok_or_else(|| ApiError::Status(StatusCode::NOT_FOUND, format!("no such work item {}", req.workid)))?;
    let id12 = &req.workid[req.workid.len().saturating_sub(12)..];
    if holder.lease.is_empty() || holder.lease != req.lease {
        return Err(refused(
            "stale lease",
            format!("refused: lease {} is not the live lease of {id12}", &req.lease[..8.min(req.lease.len())]),
            json!({"workid": req.workid, "state": holder.state}),
        ));
    }
    let ttl = if req.ttl_sec > 0 { req.ttl_sec.min(iter_core::LOCK_LEASE_TTL_SEC) } else { iter_core::LOCK_LEASE_TTL_SEC };
    let now = now_utc();
    let expires = iso_in(ttl);
    let mut renewed = 0u64;
    for r in st.store.query("lock", &name).await? {
        let lr: LockRow = serde_json::from_value(r.clone()).unwrap_or_default();
        if lr.workid != req.workid || lr.lease != req.lease {
            continue;
        }
        let mut row = r;
        row["expires"] = json!(expires);
        // the same holder may always rewrite its own row
        st.store.acquire_lock("lock", &name, &iter_core::lock_sk(&lr.kind, &lr.path), &row, &now, &req.workid).await?;
        renewed += 1;
    }
    if renewed > 0 {
        st.store.bump_seq(&name, "lock").await?;
    }
    Ok(Json(json!({"renewed": renewed, "expires": expires})))
}

#[derive(serde::Deserialize)]
struct LockReleaseAllReq {
    workid: String,
    #[serde(default)]
    lease: String,
}

/// Every row (lock or reservation) of one item — and of one lease, when
/// given.  The engine calls it at every exit from a run, with a workid that
/// may hold nothing, so zero rows is success.
async fn lock_release_all(
    user: AuthUser,
    State(st): Ctx,
    Path(name): Path<String>,
    Json(req): Json<LockReleaseAllReq>,
) -> Result<Json<Value>, ApiError> {
    user.require_writer()?;
    let gone = release_rows(&st, &name, |r| r.workid == req.workid && (req.lease.is_empty() || r.lease == req.lease)).await?;
    Ok(Json(json!({"released": gone.iter().map(|r| r.path.clone()).collect::<Vec<_>>()})))
}

#[derive(serde::Deserialize)]
struct LockSweepReq {
    #[serde(default)]
    dry_run: bool,
}

/// Remove every lock row that does not count (its holder is not running
/// under the row's lease, or it expired) and every reservation whose holder
/// is gone or no longer queued.  `dry_run` only lists them (for a person
/// inspecting the table; the engine always deletes).  Engine or admin only.
async fn lock_sweep(
    user: AuthUser,
    State(st): Ctx,
    Path(name): Path<String>,
    Json(req): Json<LockSweepReq>,
) -> Result<Json<Value>, ApiError> {
    if user.role != "engine" && user.role != "admin" {
        return Err(forbidden());
    }
    let items = project_items(&st, &name).await?;
    let by_id: HashMap<String, &WorkItem> = items.iter().map(|i| (i.id.clone(), i)).collect();
    let now = now_utc();
    let mut removed: Vec<Value> = Vec::new();
    for r in st.store.query("lock", &name).await? {
        let lr: LockRow = serde_json::from_value(r).unwrap_or_default();
        let Some(why) = iter_core::waitgraph::sweep_reason(&lr, by_id.get(&lr.workid).copied(), &now) else { continue };
        if !req.dry_run {
            st.store.delete("lock", &name, &iter_core::lock_sk(&lr.kind, &lr.path)).await?;
        }
        removed.push(json!({"path": lr.path, "kind": lr.kind, "workid": lr.workid, "why": why}));
    }
    if !req.dry_run && !removed.is_empty() {
        st.store.bump_seq(&name, "lock").await?;
    }
    Ok(Json(json!({"dry_run": req.dry_run, "removed": removed})))
}

/// Every deadlock in the project's wait-for graph right now (CR 6.2 item 8):
/// declared, deep and lock edges out of queued items, one canonical cycle
/// per component; plus the lock rows that are unexpired but do not count.
async fn deadlocks_get(_u: AuthUser, State(st): Ctx, Path(name): Path<String>) -> Result<Json<Value>, ApiError> {
    let items = project_items(&st, &name).await?;
    let by_id: HashMap<String, &WorkItem> = items.iter().map(|i| (i.id.clone(), i)).collect();
    let rows: Vec<LockRow> = st.store.query("lock", &name).await?.into_iter().filter_map(|r| serde_json::from_value(r).ok()).collect();
    let now = now_utc();
    let live = iter_core::live_lock_rows(&rows, &by_id, &now);
    let cycles: Vec<Value> = iter_core::find_wait_cycles(&iter_core::wait_edges(&items, &live))
        .iter()
        .map(|c| {
            let mut members: Vec<&str> = c.iter().map(|e| e.from.as_str()).collect();
            members.sort();
            json!({"members": members, "edges": c, "text": iter_core::waitgraph::describe_cycle(c),
                   "auto_resolvable": iter_core::waitgraph::auto_resolvable(c, &by_id)})
        })
        .collect();
    let stale: Vec<Value> = rows
        .iter()
        .filter(|r| r.kind == "lock" && (r.expires.is_empty() || r.expires >= now))
        .filter(|r| !iter_core::waitgraph::lock_row_is_live(r, by_id.get(&r.workid).copied(), &now))
        .map(|r| json!({"path": r.path, "workid": r.workid, "holder_state": by_id.get(&r.workid).map(|h| h.state.clone()).unwrap_or_default()}))
        .collect();
    Ok(Json(json!({"computed": now, "cycles": cycles, "stale_lock_rows": stale})))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn closed_items_accept_only_tag_changes() {
        let cur = json!({"id":"a","state":"complete","priority":5,"tags":[],"version":3});
        let tagged = json!({"id":"a","state":"complete","priority":5,"tags":[{"text":"regressed","color":"#f00"}],"version":3});
        assert!(tags_only_change(&cur, &tagged));
        let reprioritized = json!({"id":"a","state":"complete","priority":1,"tags":[{"text":"x","color":""}],"version":3});
        assert!(!tags_only_change(&cur, &reprioritized));
        let reopened = json!({"id":"a","state":"queued","priority":5,"tags":[],"version":3});
        assert!(!tags_only_change(&cur, &reopened));
        assert!(is_closed("complete") && is_closed("failed") && !is_closed("question"));
    }

    /// An emptied settings box (null) is accepted and never stored.
    #[tokio::test]
    async fn project_put_accepts_and_strips_null_settings() {
        let st = mem().await;
        let body = json!({"name": "p", "state": "Running", "gitrepo": "", "dedup": null, "cluster_restart": null, "pinned_tags": null, "maxdailycost": 5});
        let out = match project_put(admin(), State(st.clone()), Path("p".into()), Json(body)).await {
            Ok(Json(v)) => v,
            Err(ApiError::Status(c, m)) => panic!("refused: {c} {m}"),
            Err(_) => panic!("conflict"),
        };
        assert!(out.get("dedup").is_none() && out.get("cluster_restart").is_none() && out.get("pinned_tags").is_none());
        let stored = st.store.get("project", "p", NOSK).await.unwrap().unwrap();
        assert!(!stored.as_object().unwrap().values().any(|v| v.is_null()), "no null is ever stored: {stored}");
        assert_eq!(stored["maxdailycost"], json!(5));
    }

    #[test]
    fn pinned_tags_round_trip_and_refuse_engine_owned() {
        // absent = empty, present = kept verbatim, in order
        let p: Project = serde_json::from_value(json!({"name":"x","state":"Running"})).unwrap();
        assert!(p.pinned_tags.is_empty());
        let p: Project = serde_json::from_value(json!({"name":"x","state":"Running",
            "pinned_tags":["blocked-by-cluster-restart","blocked-until-cluster-restart"]})).unwrap();
        assert_eq!(p.pinned_tags, vec!["blocked-by-cluster-restart", "blocked-until-cluster-restart"]);
        assert!(check_pinned_tags(&p.pinned_tags).is_ok());
        // refusals: blank, and the engine-owned prefix
        assert!(check_pinned_tags(&["ok".into(), "  ".into()]).is_err());
        assert!(check_pinned_tags(&["blocked by: cluster restart".into()]).is_err());
    }

    // ---------- dedup (iter3/plans/!iter_dedup_spec.md, 2026-09-10) ----------
    // Handlers called directly on a throwaway ArangoDB database: no server, no model.

    async fn mem() -> Arc<AppState> {
        Arc::new(AppState { store: Arc::new(crate::test_db::store().await.0), secret: b"test".to_vec() })
    }
    fn admin() -> AuthUser {
        AuthUser { sub: "tester".into(), role: "admin".into() }
    }
    async fn create(st: &Arc<AppState>, body: Value) -> Value {
        match workitem_create(admin(), State(st.clone()), Path("p".into()), Json(body)).await {
            Ok(Json(v)) => v,
            Err(ApiError::Status(code, msg)) => panic!("create failed: {code} {msg}"),
            Err(ApiError::Conflict(v)) => panic!("create conflict: {v}"),
            Err(ApiError::Refused(c, v)) => panic!("create refused: {c} {v}"),
        }
    }

    /// workitem_dependency.md cycle refusal (built 2026-09-11): a PUT or
    /// create whose `blockedby` closes a loop is refused with the path
    /// named; unrelated edits of an item already in a loop still go through.
    #[tokio::test]
    async fn dependency_cycles_are_refused_with_the_path_named() {
        let st = mem().await;
        let a = create(&st, json!({"name": "a", "agent": "code", "state": "queued", "priority": 5})).await;
        let aid = a["id"].as_str().unwrap().to_string();
        let b = create(&st, json!({"name": "b", "agent": "code", "state": "queued", "priority": 5, "blockedby": [aid]})).await;
        let bid = b["id"].as_str().unwrap().to_string();
        let c = create(&st, json!({"name": "c", "agent": "code", "state": "queued", "priority": 5, "blockedby": [bid]})).await;
        let cid = c["id"].as_str().unwrap().to_string();
        let put = |st: Arc<AppState>, id: String, body: Value| async move {
            let q: HashMap<String, String> = [("expect_version".to_string(), body["version"].as_u64().unwrap().to_string())].into();
            workitem_put(admin(), State(st), Path(("p".into(), id)), Query(q), Json(body)).await
        };
        // a -> c would close a -> c -> b -> a
        let mut bad_a = a.clone();
        bad_a["blockedby"] = json!([cid.clone()]);
        match put(st.clone(), aid.clone(), bad_a).await {
            Err(ApiError::Status(code, msg)) => {
                assert_eq!(code, StatusCode::BAD_REQUEST);
                assert!(msg.contains("dependency cycle"), "{msg}");
                for id in [&aid, &cid, &bid] {
                    assert!(msg.contains(&id[id.len() - 12..]), "path names {id}: {msg}");
                }
            }
            Ok(_) => panic!("cycle accepted"),
            Err(ApiError::Conflict(v)) | Err(ApiError::Refused(_, v)) => panic!("conflict: {v}"),
        }
        // a create that would wait on itself is refused too
        let dup = json!({"name": "self", "agent": "code", "state": "queued", "priority": 5, "id": "self-id", "blockedby": ["self-id"]});
        assert!(matches!(workitem_create(admin(), State(st.clone()), Path("p".into()), Json(dup)).await, Err(ApiError::Status(_, _))));
        // an acyclic link is fine, and a tag edit on a looped item (data
        // from before the refusal) is not blocked by the loop it is in
        let mut fine = a.clone();
        fine["tags"] = json!([{"text": "x", "color": ""}]);
        let a2 = match put(st.clone(), aid.clone(), fine).await {
            Ok(Json(v)) => v,
            Err(ApiError::Status(c, m)) => panic!("acyclic edit refused: {c} {m}"),
            Err(ApiError::Conflict(v)) | Err(ApiError::Refused(_, v)) => panic!("conflict: {v}"),
        };
        assert_eq!(a2["version"], json!(2));
    }
    fn keyed(name: &str, prio: i64, check: &str, container: &str, request: &str) -> Value {
        json!({"name": name, "agent": "code", "priority": prio, "requestedby": "user", "request": request,
               "tags": [{"text": format!("check:{check}"), "color": ""}, {"text": format!("container:{container}"), "color": ""}]})
    }
    async fn rows(st: &Arc<AppState>) -> Vec<Value> {
        st.store.query("workitem", "p").await.unwrap()
    }
    async fn details(st: &Arc<AppState>, id: &str) -> Vec<Value> {
        let mut d = st.store.query("workitem_detail", id).await.unwrap();
        d.sort_by_key(|r| r.get("order").and_then(|o| o.as_i64()).unwrap_or(0));
        d
    }
    fn tag_texts(row: &Value) -> Vec<String> {
        row["tags"].as_array().unwrap().iter().map(|t| body_str(t, "text")).collect()
    }
    /// a fresh row carries no `repeats` field at all (the create body is stored as sent)
    fn repeats(row: &Value) -> u64 {
        row.get("repeats").and_then(|r| r.as_u64()).unwrap_or(0)
    }

    /// Case 1: same check + container, open twin → the twin comes back with
    /// `already_open`, no new row, repeats 1, priority halved, doc row with
    /// the repeat's request text.
    #[tokio::test]
    async fn stage1_open_twin_is_returned_not_duplicated() {
        let st = mem().await;
        let a = create(&st, keyed("clearing cannot be measured", 77, "rollingupdate-guarded-analysis", "pdy_core_clearing", "first report")).await;
        assert!(a.get("already_open").is_none());
        let b = create(&st, keyed("clearing cannot be measured (again)", 77, "rollingupdate-guarded-analysis", "pdy_core_clearing", "second report, run 2")).await;
        assert_eq!(b["already_open"], json!(true));
        assert_eq!(b["id"], a["id"]);
        assert_eq!(b["repeats"], json!(1));
        assert_eq!(b["priority"], json!(38));
        assert_eq!(rows(&st).await.len(), 1, "no second row");
        let d = details(&st, a["id"].as_str().unwrap()).await;
        assert_eq!(body_str(&d[0], "key"), "request");
        assert_eq!(body_str(&d[0], "value"), "first report");
        assert_eq!(body_str(&d[1], "key"), "doc");
        let note = body_str(&d[1], "value");
        assert!(note.starts_with("seen again ") && note.contains("by user") && note.contains("second report, run 2"), "{note}");
    }

    /// Case 2: same check, different container → a new row.
    #[tokio::test]
    async fn stage1_same_check_different_container_is_new() {
        let st = mem().await;
        let a = create(&st, keyed("clearing", 77, "rollingupdate-guarded-analysis", "pdy_core_clearing", "")).await;
        let b = create(&st, keyed("authority", 77, "rollingupdate-guarded-analysis", "pdy_core_authority", "")).await;
        assert!(b.get("already_open").is_none());
        assert_ne!(a["id"], b["id"]);
        assert_eq!(rows(&st).await.len(), 2);
        assert_eq!(repeats(&a), 0);
    }

    /// Case 3: same key, twin closed complete → a new row carrying the
    /// recurrence doc row; the closed twin is untouched.
    #[tokio::test]
    async fn stage1_closed_twin_recurs_as_new_item() {
        let st = mem().await;
        let a = create(&st, keyed("clearing", 77, "rule", "pdy_core_clearing", "")).await;
        let aid = a["id"].as_str().unwrap().to_string();
        let mut closed = a.clone();
        closed["state"] = json!("complete");
        closed["ts"]["complete"] = json!("2026-09-10T07:00:00Z");
        st.store.put("workitem", "p", &aid, &closed).await.unwrap();
        let b = create(&st, keyed("clearing again", 77, "rule", "pdy_core_clearing", "it is back")).await;
        assert!(b.get("already_open").is_none());
        assert_ne!(b["id"], a["id"]);
        assert_eq!(rows(&st).await.len(), 2);
        let d = details(&st, b["id"].as_str().unwrap()).await;
        let doc = d.iter().find(|r| body_str(r, "key") == "doc").expect("recurrence doc row");
        assert_eq!(body_str(doc, "value"), format!("recurrence of {aid}, which closed 2026-09-10T07:00:00Z"));
        let still = st.store.get("workitem", "p", &aid).await.unwrap().unwrap();
        assert_eq!(repeats(&still), 0);
        assert_eq!(still["state"], json!("complete"));
    }

    /// Case 4: no key tags → a new row every time, no lookup, response shape unchanged.
    #[tokio::test]
    async fn no_key_tags_creates_as_today() {
        let st = mem().await;
        let body = json!({"name": "same words", "agent": "code", "priority": 50, "requestedby": "user",
                          "tags": [{"text": "container:pdy_core_clearing", "color": ""}]});
        let a = create(&st, body.clone()).await;
        let b = create(&st, body).await;
        assert_ne!(a["id"], b["id"]);
        assert_eq!(rows(&st).await.len(), 2);
        assert!(b.get("already_open").is_none() && b.get("warnings").is_none());
        assert_eq!(repeats(&b), 0);
    }

    /// Case 5 (through the service): the halving chain on a real row, and
    /// priority 1 stays 1.
    #[tokio::test]
    async fn stage1_repeats_halve_priority_down_to_one() {
        let st = mem().await;
        let a = create(&st, keyed("x", 77, "r", "c", "")).await;
        let mut want = vec![38, 19, 9, 4, 2, 1, 1];
        want.reverse();
        while let Some(p) = want.pop() {
            let r = create(&st, keyed("x", 77, "r", "c", "")).await;
            assert_eq!(r["priority"], json!(p));
            assert_eq!(r["id"], a["id"]);
        }
        assert_eq!(rows(&st).await.len(), 1);
    }

    /// Case 6 (through the service): `repeated` appears at the project's
    /// `dedup.repeated_threshold` and not before.
    #[tokio::test]
    async fn repeated_tag_follows_project_threshold() {
        let st = mem().await;
        st.store.put("project", "p", NOSK, &json!({"name": "p", "state": "Running", "dedup": {"repeated_threshold": 2}})).await.unwrap();
        create(&st, keyed("x", 60, "r", "c", "")).await;
        let r1 = create(&st, keyed("x", 60, "r", "c", "")).await;
        assert!(!tag_texts(&r1).contains(&"repeated".to_string()), "not before the threshold");
        let r2 = create(&st, keyed("x", 60, "r", "c", "")).await;
        assert!(tag_texts(&r2).contains(&"repeated".to_string()), "at the threshold");
        assert_eq!(r2["repeats"], json!(2));
    }

    /// Case 8 (service half): the merge closes the newer item complete with
    /// `dup of:` and a doc row, and the survivor is seen again with the
    /// duplicate's request text.
    #[tokio::test]
    async fn duplicate_of_merges_newer_into_survivor() {
        let st = mem().await;
        let old = create(&st, json!({"name": "clearing red", "agent": "code", "priority": 40, "requestedby": "user", "request": "older report"})).await;
        let new = create(&st, json!({"name": "clearing cannot be measured", "agent": "code", "priority": 40, "requestedby": "agent:code", "request": "newer report with a stack trace"})).await;
        let (oid, nid) = (old["id"].as_str().unwrap().to_string(), new["id"].as_str().unwrap().to_string());
        let out = workitem_duplicate_of(admin(), State(st.clone()), Path(("p".into(), nid.clone())),
            Json(DuplicateReq { survivor: oid.clone(), reason: "both describe the unmeasurable clearing release".into(), others: vec![] })).await
            .map(|j| j.0).unwrap_or_else(|_| panic!("merge refused"));
        assert_eq!(out["duplicate"]["state"], json!("complete"));
        assert_eq!(out["duplicate"]["id"], json!(nid));
        assert!(tag_texts(&out["duplicate"]).contains(&format!("dup of: {}", &oid[oid.len() - 12..])));
        assert_eq!(out["survivor"]["repeats"], json!(1));
        assert_eq!(out["survivor"]["priority"], json!(20));
        let dd = details(&st, &nid).await;
        assert!(dd.iter().any(|r| body_str(r, "key") == "doc" && body_str(r, "value") == format!("Duplicate of {oid}: both describe the unmeasurable clearing release")));
        let sd = details(&st, &oid).await;
        let seen = sd.iter().find(|r| body_str(r, "key") == "doc").expect("seen-again row");
        let v = body_str(seen, "value");
        assert!(v.contains("by agent:code") && v.contains(&format!("duplicate {nid} merged here")) && v.contains("newer report with a stack trace"), "{v}");
        assert_eq!(rows(&st).await.len(), 2, "a merge closes, never deletes");
    }

    /// Case 9 (service half): never into an in-progress or closed survivor;
    /// never closes an in-progress duplicate.
    #[tokio::test]
    async fn duplicate_of_refuses_in_progress_and_closed() {
        let st = mem().await;
        let a = create(&st, json!({"name": "a", "agent": "code", "priority": 40})).await;
        let b = create(&st, json!({"name": "b", "agent": "code", "priority": 40})).await;
        let (aid, bid) = (a["id"].as_str().unwrap().to_string(), b["id"].as_str().unwrap().to_string());
        let set_state = |st: Arc<AppState>, id: String, state: &'static str| async move {
            let mut r = st.store.get("workitem", "p", &id).await.unwrap().unwrap();
            r["state"] = json!(state);
            st.store.put("workitem", "p", &id, &r).await.unwrap();
        };
        let merge = |st: Arc<AppState>, dup: String, surv: String| async move {
            workitem_duplicate_of(admin(), State(st), Path(("p".into(), dup)), Json(DuplicateReq { survivor: surv, reason: String::new(), others: vec![] })).await.map(|j| j.0)
        };
        // survivor in progress
        set_state(st.clone(), aid.clone(), "in-progress").await;
        assert!(merge(st.clone(), bid.clone(), aid.clone()).await.is_err());
        // survivor closed
        set_state(st.clone(), aid.clone(), "complete").await;
        assert!(merge(st.clone(), bid.clone(), aid.clone()).await.is_err());
        // duplicate in progress
        set_state(st.clone(), aid.clone(), "queued").await;
        set_state(st.clone(), bid.clone(), "in-progress").await;
        assert!(merge(st.clone(), bid.clone(), aid.clone()).await.is_err());
        // nothing changed
        let b_now = st.store.get("workitem", "p", &bid).await.unwrap().unwrap();
        assert_eq!(b_now["state"], json!("in-progress"));
        assert!(tag_texts(&b_now).is_empty());
        let a_now = st.store.get("workitem", "p", &aid).await.unwrap().unwrap();
        assert_eq!(repeats(&a_now), 0);
        // and the happy path still works once both are open
        set_state(st.clone(), bid.clone(), "queued").await;
        assert!(merge(st.clone(), bid.clone(), aid.clone()).await.is_ok());
    }

    // ---------- lease-bound locks (CR 2026-09-25) ----------

    fn engine_user() -> AuthUser {
        AuthUser { sub: "e".into(), role: "engine".into() }
    }
    async fn put_item(st: &Arc<AppState>, body: Value) -> Value {
        let id = body["id"].as_str().unwrap().to_string();
        let q: HashMap<String, String> = [("expect_version".to_string(), body["version"].as_u64().unwrap().to_string())].into();
        match workitem_put(admin(), State(st.clone()), Path(("p".into(), id)), Query(q), Json(body)).await {
            Ok(Json(v)) => v,
            Err(ApiError::Status(c, m)) => panic!("put: {c} {m}"),
            Err(_) => panic!("put refused"),
        }
    }
    async fn acquire(st: &Arc<AppState>, body: Value) -> Result<Value, (StatusCode, Value)> {
        let req: LockAcquireReq = serde_json::from_value(body).unwrap();
        match lock_acquire(engine_user(), State(st.clone()), Path("p".into()), Json(req)).await {
            Ok(Json(v)) => Ok(v),
            Err(ApiError::Refused(c, v)) => Err((c, v)),
            Err(ApiError::Status(c, m)) => Err((c, json!({"error": m}))),
            Err(ApiError::Conflict(v)) => Err((StatusCode::CONFLICT, v)),
        }
    }
    async fn lock_rows(st: &Arc<AppState>) -> Vec<Value> {
        st.store.query("lock", "p").await.unwrap()
    }
    async fn project(st: &Arc<AppState>) {
        st.store.put("project", "p", NOSK, &json!({"name": "p", "state": "Running"})).await.unwrap();
    }

    /// F1 (ii), CR 8.1: a lock taken outside lockdirs dies with the run's
    /// lease; the stray loop's exact request is refused once the item is not
    /// running; renewal with the ended lease is refused; an omitted lease key
    /// keeps the live lease.
    #[tokio::test]
    async fn lock_for_a_queued_item_is_refused_and_released() {
        let st = mem().await;
        project(&st).await;
        let h = create(&st, json!({"name": "h", "agent": "code", "state": "queued", "lockdirs": ["{topdir}/a/src"]})).await;
        let hid = h["id"].as_str().unwrap().to_string();
        let mut running = h.clone();
        running["state"] = json!("in-progress");
        running["lease"] = json!("L1");
        running["engine"] = json!("E1");
        let running = put_item(&st, running).await;
        for p in ["{topdir}/a/src", "{topdir}/b/extra.rs"] {
            let row = acquire(&st, json!({"path": p, "workid": hid})).await.expect("granted while running");
            assert_eq!(row["lease"], "L1");
            assert_eq!(row["engine"], "E1", "engine filled from the holder");
        }
        // the run ends: the close PUT clears the lease -> both rows are gone
        let mut back = running.clone();
        back["state"] = json!("queued");
        back["lease"] = json!("");
        let back = put_item(&st, back).await;
        assert!(lock_rows(&st).await.is_empty(), "rows left: {:?}", lock_rows(&st).await);
        // byte for byte what the orphaned renewer sends
        let (code, body) = acquire(&st, json!({"path": "{topdir}/b/extra.rs", "workid": hid})).await.expect_err("refused");
        assert_eq!(code, StatusCode::CONFLICT);
        assert_eq!(body["refused"], "not running");
        assert!(lock_rows(&st).await.is_empty());
        // renewal with the ended lease
        let req = LockRenewReq { workid: hid.clone(), lease: "L1".into(), ttl_sec: 600 };
        assert!(matches!(lock_renew(engine_user(), State(st.clone()), Path("p".into()), Json(req)).await, Err(ApiError::Refused(StatusCode::CONFLICT, _))));
        // a PUT that omits the key keeps a live lease
        let mut again = back.clone();
        again["state"] = json!("in-progress");
        again["lease"] = json!("L2");
        let mut again = put_item(&st, again).await;
        again.as_object_mut().unwrap().remove("lease");
        again["priority"] = json!(3);
        let kept = put_item(&st, again).await;
        assert_eq!(kept["lease"], "L2");
        // an unknown holder is a 404
        let (code, _) = acquire(&st, json!({"path": "{topdir}/z", "workid": "w-test"})).await.expect_err("404");
        assert_eq!(code, StatusCode::NOT_FOUND);
    }

    /// F4: renew extends the live lease's rows and refuses a stale lease
    /// without touching `expires`; the ttl is capped at the lease lifetime.
    #[tokio::test]
    async fn renew_with_a_stale_lease_is_refused_and_leaves_expires() {
        let st = mem().await;
        project(&st).await;
        let h = create(&st, json!({"name": "h", "agent": "code", "state": "queued"})).await;
        let hid = h["id"].as_str().unwrap().to_string();
        let mut r = h.clone();
        r["state"] = json!("in-progress");
        r["lease"] = json!("L1");
        put_item(&st, r).await;
        let row = acquire(&st, json!({"path": "{topdir}/a", "workid": hid, "ttl_sec": 3900})).await.unwrap();
        let cap = (chrono::Utc::now() + chrono::Duration::seconds(iter_core::LOCK_LEASE_TTL_SEC + 5)).format("%Y-%m-%dT%H:%M:%SZ").to_string();
        assert!(row["expires"].as_str().unwrap() <= cap.as_str(), "ttl capped: {row}");
        let before = lock_rows(&st).await[0]["expires"].clone();
        let stale = LockRenewReq { workid: hid.clone(), lease: "L0".into(), ttl_sec: 600 };
        assert!(matches!(lock_renew(engine_user(), State(st.clone()), Path("p".into()), Json(stale)).await, Err(ApiError::Refused(StatusCode::CONFLICT, _))));
        assert_eq!(lock_rows(&st).await[0]["expires"], before);
        let live = LockRenewReq { workid: hid.clone(), lease: "L1".into(), ttl_sec: 600 };
        let Ok(Json(v)) = lock_renew(engine_user(), State(st.clone()), Path("p".into()), Json(live)).await else { panic!("renew") };
        assert_eq!(v["renewed"], 1);
    }

    /// F17: a reservation lives under its own key, so a strictly-better
    /// item's lock on exactly the reserved path succeeds; a reservation is a
    /// queued item's only; release_all clears both kinds.
    #[tokio::test]
    async fn reserve_and_lock_on_one_path_do_not_collide() {
        let st = mem().await;
        project(&st).await;
        let r = create(&st, json!({"name": "r", "agent": "code", "state": "queued", "priority": 5})).await;
        let rid = r["id"].as_str().unwrap().to_string();
        let b = create(&st, json!({"name": "b", "agent": "code", "state": "queued", "priority": 1})).await;
        let bid = b["id"].as_str().unwrap().to_string();
        acquire(&st, json!({"path": "{topdir}/a", "kind": "reserve", "workid": rid, "ttl_sec": 600})).await.expect("reserve");
        let mut run = b.clone();
        run["state"] = json!("in-progress");
        run["lease"] = json!("LB");
        put_item(&st, run).await;
        acquire(&st, json!({"path": "{topdir}/a", "workid": bid, "lease": "LB"})).await.expect("lock beside the reservation");
        assert_eq!(lock_rows(&st).await.len(), 2);
        // a running item may not reserve
        let (code, body) = acquire(&st, json!({"path": "{topdir}/q", "kind": "reserve", "workid": bid})).await.expect_err("not queued");
        assert_eq!((code, body["refused"].as_str()), (StatusCode::CONFLICT, Some("not queued")));
        let req = LockReleaseAllReq { workid: rid.clone(), lease: String::new() };
        let Ok(Json(v)) = lock_release_all(engine_user(), State(st.clone()), Path("p".into()), Json(req)).await else { panic!() };
        assert_eq!(v["released"], json!(["{topdir}/a"]));
        assert_eq!(lock_rows(&st).await.len(), 1, "only B's lock is left");
    }

    /// Lease-bound locks are always enforced (2026-09-28, no switch): a lock
    /// for an item that is not running is refused even with no lease anywhere;
    /// a leftover lease-less row (an iter3 row carried over by the migration)
    /// blocks nobody, is listed as stale, and the sweep deletes it.
    #[tokio::test]
    async fn leases_always_enforced_and_leftover_rows_swept() {
        let st = mem().await;
        project(&st).await;
        let h = create(&st, json!({"name": "h", "agent": "code", "state": "queued"})).await;
        let hid = h["id"].as_str().unwrap().to_string();
        let err = acquire(&st, json!({"path": "{topdir}/a", "workid": hid})).await.expect_err("refused: not running");
        assert_eq!(err.1["refused"], "not running", "{:?}", err.1);
        // a leftover row, as the iter3 migration copies them: no lease
        let leftover = json!({"project": "p", "path": "{topdir}/a", "kind": "lock", "engine": "Engine01", "workid": hid,
            "acquired": "2026-09-20T00:00:00Z", "expires": "2099-01-01T00:00:00Z", "lease": ""});
        st.store.put("lock", "p", "{topdir}/a", &leftover).await.unwrap();
        let Ok(Json(v)) = lock_sweep(engine_user(), State(st.clone()), Path("p".into()), Json(LockSweepReq { dry_run: true })).await else { panic!() };
        assert_eq!(v["removed"].as_array().unwrap().len(), 1);
        assert_eq!(lock_rows(&st).await.len(), 1, "dry run deletes nothing");
        let Ok(Json(list)) = locks_list(admin(), State(st.clone()), Path("p".into())).await else { panic!() };
        assert_eq!((list[0]["holder_state"].as_str(), list[0]["holder_lease_live"].as_bool()), (Some("queued"), Some(false)));
        let Ok(Json(d)) = deadlocks_get(admin(), State(st.clone()), Path("p".into())).await else { panic!() };
        assert_eq!(d["cycles"], json!([]));
        assert_eq!(d["stale_lock_rows"][0]["holder_state"], "queued");
        let Ok(Json(v)) = lock_sweep(engine_user(), State(st.clone()), Path("p".into()), Json(LockSweepReq { dry_run: false })).await else { panic!() };
        assert_eq!(v["removed"].as_array().unwrap().len(), 1);
        assert!(lock_rows(&st).await.is_empty());
        assert!(matches!(lock_sweep(AuthUser { sub: "u".into(), role: "user".into() }, State(st.clone()), Path("p".into()), Json(LockSweepReq { dry_run: true })).await, Err(ApiError::Status(StatusCode::FORBIDDEN, _))));
    }

    /// F5: a blockedby write that closes a loop through deep edges only
    /// (CR 5.4) is refused, naming the deep hop.
    #[tokio::test]
    async fn deep_dependency_cycle_is_refused_on_write() {
        let st = mem().await;
        let p = create(&st, json!({"name": "p", "agent": "code", "state": "complete"})).await;
        let pid = p["id"].as_str().unwrap().to_string();
        let q = create(&st, json!({"name": "q", "agent": "code", "state": "complete"})).await;
        let qid = q["id"].as_str().unwrap().to_string();
        let x = create(&st, json!({"name": "x", "agent": "code", "state": "queued", "blockedby": [pid.clone()]})).await;
        let xid = x["id"].as_str().unwrap().to_string();
        create(&st, json!({"name": "c", "agent": "code", "state": "queued", "createdby": pid.clone(), "blockedby": [qid.clone()]})).await;
        let d = create(&st, json!({"name": "d", "agent": "code", "state": "queued", "createdby": qid.clone()})).await;
        let did = d["id"].as_str().unwrap().to_string();
        let mut closing = d.clone();
        closing["blockedby"] = json!([xid]);
        let q: HashMap<String, String> = [("expect_version".to_string(), closing["version"].as_u64().unwrap().to_string())].into();
        match workitem_put(admin(), State(st.clone()), Path(("p".into(), did)), Query(q), Json(closing)).await {
            Err(ApiError::Status(code, msg)) => {
                assert_eq!(code, StatusCode::BAD_REQUEST);
                assert!(msg.contains("dependency cycle") && msg.contains("(deep: created by"), "{msg}");
            }
            _ => panic!("deep cycle accepted"),
        }
    }

    /// F15: a create whose request is a placeholder is refused for every client.
    #[tokio::test]
    async fn create_with_a_placeholder_request_is_refused() {
        let st = mem().await;
        let body = json!({"name": "t", "agent": "code", "request": "PLACEHOLDER - replaced immediately by the filing agent"});
        match workitem_create(admin(), State(st.clone()), Path("p".into()), Json(body)).await {
            Err(ApiError::Status(code, msg)) => assert!(code == StatusCode::BAD_REQUEST && msg.contains("placeholder"), "{msg}"),
            _ => panic!("placeholder accepted"),
        }
        assert!(st.store.query("workitem", "p").await.unwrap().is_empty());
    }

    /// The engine-owned test sweep (2026-09-30): the engine may create the
    /// template (paused) although schedules are users-only; there is one per
    /// project; it can be paused and resumed but not deleted, closed, or
    /// stripped of `system`; a run cloned from it is stamped `system`.
    #[tokio::test]
    async fn the_test_sweep_is_one_undeletable_schedule() {
        let st = mem().await;
        let tpl = match workitem_create(engine_user(), State(st.clone()), Path("p".into()), Json(iter_core::test_sweep_template_body("paused", 240))).await {
            Ok(Json(v)) => v,
            _ => panic!("the engine could not create the test sweep"),
        };
        let id = tpl["id"].as_str().unwrap().to_string();
        assert_eq!(tpl["state"], "paused");
        // an ordinary schedule from the engine is still refused
        let other = json!({"name": "s", "agent": "exec", "exec_shell": "true", "state": "scheduled", "sched": {"kind": "every", "every_min": 5}});
        assert!(matches!(workitem_create(engine_user(), State(st.clone()), Path("p".into()), Json(other)).await, Err(ApiError::Status(StatusCode::FORBIDDEN, _))));
        // a second template is refused, naming the first
        match workitem_create(admin(), State(st.clone()), Path("p".into()), Json(iter_core::test_sweep_template_body("scheduled", 60))).await {
            Err(ApiError::Status(code, msg)) => assert!(code == StatusCode::CONFLICT && msg.contains(&id), "{msg}"),
            _ => panic!("a second test sweep was accepted"),
        }
        let put = |st: Arc<AppState>, body: Value| {
            let id = body["id"].as_str().unwrap().to_string();
            let q: HashMap<String, String> = [("expect_version".to_string(), body["version"].as_u64().unwrap().to_string())].into();
            async move { workitem_put(admin(), State(st), Path(("p".into(), id)), Query(q), Json(body)).await }
        };
        // resume, with the `system` key omitted (kept from the stored row)
        let mut on = tpl.clone();
        on["state"] = json!("scheduled");
        on.as_object_mut().unwrap().remove("system");
        let Json(on) = put(st.clone(), on).await.map_err(|_| "resume refused").unwrap();
        assert_eq!(on["system"], iter_core::TEST_SWEEP);
        // not closed, not stripped
        let mut done = on.clone();
        done["state"] = json!("complete");
        assert!(matches!(put(st.clone(), done).await, Err(ApiError::Status(StatusCode::CONFLICT, _))));
        let mut plain = on.clone();
        plain["system"] = json!("");
        assert!(matches!(put(st.clone(), plain).await, Err(ApiError::Status(StatusCode::BAD_REQUEST, _))));
        // not deleted
        assert!(matches!(workitem_delete(admin(), State(st.clone()), Path(("p".into(), id.clone()))).await, Err(ApiError::Status(StatusCode::CONFLICT, _))));
        // a Run-now clone (webui: no `system` in the body) is stamped
        let run = create(&st, json!({"name": "run", "agent": "test", "exec_shell": "iter sweep", "state": "queued", "source_schedule": id})).await;
        assert_eq!(run["system"], iter_core::TEST_SWEEP);
        let rid = run["id"].as_str().unwrap().to_string();
        assert!(workitem_delete(admin(), State(st.clone()), Path(("p".into(), rid))).await.is_ok(), "a run is an ordinary item");
    }
}
