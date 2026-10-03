//! The settings graph (iter5 spec §7): every setting lives on a node or an
//! edge. Node records stay in their tables (`project`, `engine`, `agent`,
//! `agent_tooling`, `webui_user`) plus `account`, `provider` and
//! `workitem_type`; edges are rows of `sys_edge` (pk "edge", sk = edge id).
//! Node id = `<type>:<name>` (iter_core::settings).
//!
//! Placeholders (`<type>:_deactivated`) and `iter_data:self` are virtual:
//! never stored, always present, never deletable. An edge with an endpoint on
//! a placeholder keeps its settings but is inactive.
//!
//! Startup (`bootstrap`) seeds providers + workitem types and, once, turns
//! iter4-shaped records into edges (`migrate_iter4`).

use crate::api::{ApiError, AppState, AuthUser, GLOBAL, NOSK, bad, forbidden, notfound};
use crate::storage::{Storage, StorageError, body_str};
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::{get, patch, post};
use axum::{Json, Router};
use iter_core::settings::{
    self as cs, Assignments, BilledAccount, HeldAccount, ITER_DATA_SELF, NODE_TYPES, PROVIDERS, ProjectAssignment, SysEdge,
    edge_type_for, is_placeholder, is_protected, node_id, parse_node_id, placeholder_id, table_of, validate_endpoints,
    validate_settings,
};
use iter_core::now_utc;
use serde_json::{Value, json};
use std::collections::{BTreeMap, HashSet};
use std::sync::Arc;

pub const EDGE_TABLE: &str = "sys_edge";
const EDGE_PK: &str = "edge";
const META_PK: &str = "meta";
const MIGRATED_SK: &str = "iter4_migrated";

type Ctx = State<Arc<AppState>>;

// ---------- storage helpers ----------

pub async fn load_edges(store: &dyn Storage) -> Result<Vec<SysEdge>, StorageError> {
    Ok(store
        .query(EDGE_TABLE, EDGE_PK)
        .await?
        .into_iter()
        .filter_map(|v| serde_json::from_value(v).ok())
        .collect())
}

pub async fn get_edge(store: &dyn Storage, id: &str) -> Result<Option<SysEdge>, StorageError> {
    Ok(store.get(EDGE_TABLE, EDGE_PK, id).await?.and_then(|v| serde_json::from_value(v).ok()))
}

pub async fn put_edge(store: &dyn Storage, e: &SysEdge) -> Result<(), StorageError> {
    store.put(EDGE_TABLE, EDGE_PK, &e.id, &serde_json::to_value(e).unwrap()).await?;
    store.bump_seq(GLOBAL, EDGE_TABLE).await?;
    Ok(())
}

async fn delete_edge_row(store: &dyn Storage, id: &str) -> Result<bool, StorageError> {
    let gone = store.delete(EDGE_TABLE, EDGE_PK, id).await?;
    if gone {
        store.bump_seq(GLOBAL, EDGE_TABLE).await?;
    }
    Ok(gone)
}

fn new_edge(edge_type: &str, from: &str, to: &str, tag: &str, settings: Value) -> SysEdge {
    let now = now_utc();
    SysEdge {
        id: uuid::Uuid::new_v4().simple().to_string(),
        edge_type: edge_type.into(),
        from: from.into(),
        to: to.into(),
        tag: tag.into(),
        settings: if settings.is_object() { settings } else { json!({}) },
        active: !is_placeholder(from) && !is_placeholder(to),
        created: now.clone(),
        updated: now,
    }
}

/// Create an edge unless one of the same (type, from, to) exists already
/// (idempotent seeding/migration). Returns true when it wrote one.
pub async fn ensure_edge(store: &dyn Storage, edge_type: &str, from: &str, to: &str, settings: Value) -> Result<bool, StorageError> {
    let edges = load_edges(store).await?;
    if edges.iter().any(|e| e.edge_type == edge_type && e.from == from && e.to == to) {
        return Ok(false);
    }
    put_edge(store, &new_edge(edge_type, from, to, "", settings)).await?;
    Ok(true)
}

/// The stored record of a node (None for virtual nodes and missing ones).
pub async fn node_record(store: &dyn Storage, id: &str) -> Result<Option<Value>, StorageError> {
    let Some((t, name)) = parse_node_id(id) else { return Ok(None) };
    let Some(table) = table_of(t) else { return Ok(None) };
    if name == cs::PLACEHOLDER {
        return Ok(None);
    }
    store.get(table, name, NOSK).await
}

/// Does this node exist (virtual nodes always do)?
pub async fn node_exists(store: &dyn Storage, id: &str) -> Result<bool, StorageError> {
    if is_placeholder(id) || id == ITER_DATA_SELF {
        return Ok(parse_node_id(id).is_some());
    }
    Ok(node_record(store, id).await?.is_some())
}

/// Every edge touching a deleted node moves its endpoint onto the type's
/// placeholder: the edge (and its settings) is kept, inactive.
pub async fn on_node_deleted(store: &dyn Storage, id: &str) -> Result<usize, StorageError> {
    let Some(t) = cs::type_of_id(id) else { return Ok(0) };
    let ph = placeholder_id(t);
    let mut moved = 0;
    for mut e in load_edges(store).await? {
        let mut changed = false;
        if e.from == id {
            e.from = ph.clone();
            changed = true;
        }
        if e.to == id {
            e.to = ph.clone();
            changed = true;
        }
        if changed {
            e.active = false;
            e.updated = now_utc();
            put_edge(store, &e).await?;
            moved += 1;
        }
    }
    Ok(moved)
}

fn allows_settings(state: &str, project: Option<&Value>) -> Value {
    match state {
        "failed" => {
            let pol = project.and_then(|p| p.get("failure")).filter(|f| f.is_object()).cloned();
            pol.unwrap_or_else(|| serde_json::to_value(iter_core::FailurePolicy {
                maxattempts: 5,
                first_retry_second: 10,
                retry_backoff_exponent: 2,
            }).unwrap())
        }
        "scheduled" => json!({"enabled": true}),
        "question" => json!({"notify": true}),
        _ => json!({}),
    }
}

/// A new project: `allows` to every workitem type, a default `runs` edge for
/// every agent, `hosts` from iter_data, and — for a non-admin creator — a
/// `member` edge (role user).
pub async fn on_project_created(store: &dyn Storage, name: &str, creator: Option<&str>) -> Result<(), StorageError> {
    let pid = node_id("project", name);
    let rec = store.get("project", name, NOSK).await?;
    for s in iter_core::STATES {
        ensure_edge(store, "allows", &pid, &node_id("workitem_type", s), allows_settings(s, rec.as_ref())).await?;
    }
    let overrides = rec.as_ref().and_then(|r| r.get("agents")).cloned().unwrap_or(json!({}));
    for a in store.scan("agent").await? {
        let an = body_str(&a, "name");
        if an.is_empty() {
            continue;
        }
        let ovr = overrides.get(&an).filter(|o| o.is_object()).cloned().unwrap_or(json!({}));
        ensure_edge(store, "runs", &node_id("agent", &an), &pid, ovr).await?;
    }
    ensure_edge(store, "hosts", ITER_DATA_SELF, &pid, json!({})).await?;
    if let Some(u) = creator.filter(|u| !u.is_empty()) {
        ensure_edge(store, "member", &node_id("user", u), &pid, json!({"role": "user"})).await?;
    }
    Ok(())
}

fn is_shared_tooling(row: &Value) -> bool {
    body_str(row, "kind") == "shared" || body_str(row, "name") == "_shared"
}

/// A new agent: enabled on every project (`runs`), `handles queued`, and
/// every shared tooling row `uses` it.
pub async fn on_agent_created(store: &dyn Storage, name: &str) -> Result<(), StorageError> {
    let aid = node_id("agent", name);
    for p in store.scan("project").await? {
        let pn = body_str(&p, "name");
        if pn.is_empty() {
            continue;
        }
        let ovr = p.get("agents").and_then(|a| a.get(name)).filter(|o| o.is_object()).cloned().unwrap_or(json!({}));
        ensure_edge(store, "runs", &aid, &node_id("project", &pn), ovr).await?;
    }
    ensure_edge(store, "handles", &aid, &node_id("workitem_type", "queued"), json!({})).await?;
    for t in store.scan("agent_tooling").await? {
        if is_shared_tooling(&t) {
            ensure_edge(store, "uses", &node_id("agent_tools", &body_str(&t, "name")), &aid, json!({})).await?;
        }
    }
    Ok(())
}

/// A new tooling row: shared tooling `uses` every agent.
pub async fn on_tooling_created(store: &dyn Storage, row: &Value) -> Result<(), StorageError> {
    if !is_shared_tooling(row) {
        return Ok(());
    }
    let tid = node_id("agent_tools", &body_str(row, "name"));
    for a in store.scan("agent").await? {
        let an = body_str(&a, "name");
        if !an.is_empty() {
            ensure_edge(store, "uses", &tid, &node_id("agent", &an), json!({})).await?;
        }
    }
    Ok(())
}

/// A registered engine (or one whose owner changed): `hosts` from iter_data
/// and `owns` from its user — the record's single owner, so an `owns` edge
/// from anyone else is removed.
pub async fn on_engine_registered(store: &dyn Storage, name: &str, user: &str) -> Result<(), StorageError> {
    let eid = node_id("iter_engine", name);
    ensure_edge(store, "hosts", ITER_DATA_SELF, &eid, json!({})).await?;
    if !user.is_empty() {
        let uid = node_id("user", user);
        for e in load_edges(store).await? {
            if e.edge_type == "owns" && e.to == eid && e.from != uid && !is_placeholder(&e.from) {
                delete_edge_row(store, &e.id).await?;
            }
        }
        ensure_edge(store, "owns", &uid, &eid, json!({})).await?;
    }
    Ok(())
}

/// A new account: `of` its provider (settings.provider, default claude).
async fn on_account_created(store: &dyn Storage, name: &str, row: &Value) -> Result<(), StorageError> {
    let prov = Some(body_str(row, "provider")).filter(|p| !p.is_empty()).unwrap_or_else(|| "claude".into());
    if store.get("provider", &prov, NOSK).await?.is_none() {
        store.put("provider", &prov, NOSK, &json!({"name": prov, "desc": ""})).await?;
    }
    ensure_edge(store, "of", &node_id("account", name), &node_id("provider", &prov), json!({})).await?;
    Ok(())
}

// ---------- startup: seed + one-time migration ----------

fn workitem_type_desc(s: &str) -> &'static str {
    match s {
        "queued" => "waiting to be picked by an engine",
        "in-progress" => "claimed and running on an engine",
        "question" => "waiting on a person's answer",
        "parked" => "set aside; not picked until requeued",
        "paused" => "held by a person",
        "failed" => "closed failed (the allows edge holds the retry policy)",
        "complete" => "closed complete",
        "scheduled" => "a schedule template that clones queued runs",
        _ => "",
    }
}

pub async fn bootstrap(store: &dyn Storage) -> Result<(), StorageError> {
    for p in PROVIDERS {
        if store.get("provider", p, NOSK).await?.is_none() {
            let desc = if *p == "mock" { "deterministic test provider (spec §5)" } else { "Anthropic Claude (claude CLI)" };
            store.put("provider", p, NOSK, &json!({"name": p, "desc": desc})).await?;
        }
    }
    for s in iter_core::STATES {
        if store.get("workitem_type", s, NOSK).await?.is_none() {
            store.put("workitem_type", s, NOSK, &json!({"name": s, "desc": workitem_type_desc(s)})).await?;
        }
    }
    if store.get(EDGE_TABLE, META_PK, MIGRATED_SK).await?.is_none() {
        let report = migrate_iter4(store).await?;
        store.put(EDGE_TABLE, META_PK, MIGRATED_SK, &json!({"at": now_utc(), "report": report})).await?;
        println!("[iter_data] settings graph: iter4 records migrated to edges {report}");
    }
    Ok(())
}

/// One-time: iter4-shaped records → settings-graph nodes and edges.
/// Idempotent (ensure_edge), so a rerun adds nothing.
pub async fn migrate_iter4(store: &dyn Storage) -> Result<Value, StorageError> {
    let mut n: BTreeMap<&str, u64> = BTreeMap::new();
    let mut count = |k: &'static str, wrote: bool| {
        if wrote {
            *n.entry(k).or_insert(0) += 1;
        }
    };
    let projects = store.scan("project").await?;
    let engines = store.scan("engine").await?;
    let agents = store.scan("agent").await?;
    let tooling = store.scan("agent_tooling").await?;
    let project_names: HashSet<String> = projects.iter().map(|p| body_str(p, "name")).filter(|s| !s.is_empty()).collect();

    // Engine.projects -> serves (topdir, read_only); hosts
    let mut serving: BTreeMap<String, Vec<String>> = BTreeMap::new(); // project -> engines
    for e in &engines {
        let en = body_str(e, "name");
        if en.is_empty() {
            continue;
        }
        let eid = node_id("iter_engine", &en);
        count("hosts", ensure_edge(store, "hosts", ITER_DATA_SELF, &eid, json!({})).await?);
        let user = body_str(e, "user");
        if !user.is_empty() {
            count("owns", ensure_edge(store, "owns", &node_id("user", &user), &eid, json!({})).await?);
        }
        if let Some(map) = e.get("projects").and_then(|p| p.as_object()) {
            for (pn, dirs) in map {
                if !project_names.contains(pn) {
                    continue;
                }
                let topdir = dirs.get("dirs").and_then(|d| d.get("topdir")).and_then(|t| t.as_str()).unwrap_or("");
                let ro = dirs.get("read_only").and_then(|r| r.as_bool()).unwrap_or(false);
                let s = json!({"topdir": topdir, "read_only": ro});
                count("serves", ensure_edge(store, "serves", &eid, &node_id("project", pn), s).await?);
                serving.entry(pn.clone()).or_default().push(en.clone());
            }
        }
    }

    for p in &projects {
        let pn = body_str(p, "name");
        if pn.is_empty() {
            continue;
        }
        let pid = node_id("project", &pn);
        // Project.accounts -> account nodes + of + bills; every engine
        // serving the project held the tokens in iter4 -> holds
        for a in p.get("accounts").and_then(|a| a.as_array()).cloned().unwrap_or_default() {
            let an = body_str(&a, "name");
            if an.is_empty() {
                continue;
            }
            if store.get("account", &an, NOSK).await?.is_none() {
                let row = json!({"name": an, "token_envar": body_str(&a, "token_envar"), "provider": "claude", "desc": ""});
                store.put("account", &an, NOSK, &row).await?;
                count("account", true);
            }
            let aid = node_id("account", &an);
            count("of", ensure_edge(store, "of", &aid, &node_id("provider", "claude"), json!({})).await?);
            let s = json!({
                "order": a.get("order").and_then(|v| v.as_i64()).unwrap_or(0),
                "switch": a.get("switch").and_then(|v| v.as_u64()).unwrap_or(0),
                "stop": a.get("stop").and_then(|v| v.as_u64()).unwrap_or(0),
                "model": "",
            });
            count("bills", ensure_edge(store, "bills", &aid, &pid, s).await?);
            for en in serving.get(&pn).cloned().unwrap_or_default() {
                count("holds", ensure_edge(store, "holds", &node_id("iter_engine", &en), &aid, json!({})).await?);
            }
        }
        // allows -> every workitem type (failed carries the failure policy)
        for s in iter_core::STATES {
            count("allows", ensure_edge(store, "allows", &pid, &node_id("workitem_type", s), allows_settings(s, Some(p))).await?);
        }
        // runs: every agent on every project; Project.agents overrides ride on the edge
        let overrides = p.get("agents").cloned().unwrap_or(json!({}));
        for a in &agents {
            let an = body_str(a, "name");
            if an.is_empty() {
                continue;
            }
            let ovr = overrides.get(&an).filter(|o| o.is_object()).cloned().unwrap_or(json!({}));
            count("runs", ensure_edge(store, "runs", &node_id("agent", &an), &pid, ovr).await?);
        }
        count("hosts", ensure_edge(store, "hosts", ITER_DATA_SELF, &pid, json!({})).await?);
    }
    for a in &agents {
        let an = body_str(a, "name");
        if an.is_empty() {
            continue;
        }
        let aid = node_id("agent", &an);
        count("handles", ensure_edge(store, "handles", &aid, &node_id("workitem_type", "queued"), json!({})).await?);
        for t in tooling.iter().filter(|t| is_shared_tooling(t)) {
            count("uses", ensure_edge(store, "uses", &node_id("agent_tools", &body_str(t, "name")), &aid, json!({})).await?);
        }
    }
    Ok(json!(n))
}

// ---------- queries used by authz, assignments, get_next ----------

/// Engines a token subject owns: engine records with `user == sub`, plus
/// active `owns` edges from `user:<sub>`.
pub async fn engines_of_user(store: &dyn Storage, edges: &[SysEdge], sub: &str) -> Result<Vec<String>, StorageError> {
    let mut out: Vec<String> = store
        .scan("engine")
        .await?
        .iter()
        .filter(|e| body_str(e, "user") == sub)
        .map(|e| body_str(e, "name"))
        .collect();
    let uid = node_id("user", sub);
    for e in edges.iter().filter(|e| e.edge_type == "owns" && e.from == uid && e.is_active()) {
        let n = e.to_name().to_string();
        if !out.contains(&n) {
            out.push(n);
        }
    }
    Ok(out)
}

/// The active serves edge engine → project, if any.
pub fn serves_edge<'a>(edges: &'a [SysEdge], engine: &str, project: &str) -> Option<&'a SysEdge> {
    let (eid, pid) = (node_id("iter_engine", engine), node_id("project", project));
    edges.iter().find(|e| e.edge_type == "serves" && e.from == eid && e.to == pid && e.is_active())
}

/// The member role (`user` | `viewer`) of an active member edge, if any.
pub fn member_role(edges: &[SysEdge], user: &str, project: &str) -> Option<String> {
    let (uid, pid) = (node_id("user", user), node_id("project", project));
    edges
        .iter()
        .find(|e| e.edge_type == "member" && e.from == uid && e.to == pid && e.is_active())
        .map(|e| Some(e.setting_str("role")).filter(|r| !r.is_empty()).unwrap_or_else(|| "user".into()))
}

/// Projects with an active serves edge from this engine.
pub fn served_projects(edges: &[SysEdge], engine: &str) -> Vec<String> {
    let eid = node_id("iter_engine", engine);
    edges.iter().filter(|e| e.edge_type == "serves" && e.from == eid && e.is_active()).map(|e| e.to_name().to_string()).collect()
}

/// Agents enabled on the project (active `runs` edges) → their override settings.
pub fn agents_enabled(edges: &[SysEdge], project: &str) -> BTreeMap<String, Value> {
    let pid = node_id("project", project);
    edges
        .iter()
        .filter(|e| e.edge_type == "runs" && e.to == pid && e.is_active())
        .map(|e| (e.from_name().to_string(), e.settings.clone()))
        .collect()
}

/// Projects a caller may see: all for admin; served by its engines for an
/// engine token; active member edges for a user.
pub async fn visible_projects(store: &dyn Storage, sub: &str, role: &str) -> Result<Option<HashSet<String>>, StorageError> {
    if role == "admin" {
        return Ok(None);
    }
    let edges = load_edges(store).await?;
    let mut out = HashSet::new();
    if role == "engine" {
        for en in engines_of_user(store, &edges, sub).await? {
            out.extend(served_projects(&edges, &en));
        }
    } else {
        let uid = node_id("user", sub);
        for e in edges.iter().filter(|e| e.edge_type == "member" && e.from == uid && e.is_active()) {
            out.insert(e.to_name().to_string());
        }
    }
    Ok(Some(out))
}

/// `GET /api/engines/{name}/assignments` (spec §4.2), from active edges only.
pub async fn assignments(store: &dyn Storage, engine: &str) -> Result<Assignments, StorageError> {
    let edges = load_edges(store).await?;
    let eid = node_id("iter_engine", engine);
    let provider_of = |acct: &str| -> String {
        let aid = node_id("account", acct);
        edges
            .iter()
            .find(|e| e.edge_type == "of" && e.from == aid && e.is_active())
            .map(|e| e.to_name().to_string())
            .unwrap_or_else(|| "claude".into())
    };
    // held accounts: active holds edges whose account record exists
    let mut held: Vec<HeldAccount> = Vec::new();
    for h in edges.iter().filter(|e| e.edge_type == "holds" && e.from == eid && e.is_active()) {
        let an = h.to_name().to_string();
        let Some(rec) = store.get("account", &an, NOSK).await? else { continue };
        let ovr = h.setting_str("token_envar");
        held.push(HeldAccount {
            name: an.clone(),
            provider: provider_of(&an),
            token_envar: if ovr.is_empty() { body_str(&rec, "token_envar") } else { ovr },
        });
    }
    held.sort_by(|a, b| a.name.cmp(&b.name));
    let mut projects = Vec::new();
    for s in edges.iter().filter(|e| e.edge_type == "serves" && e.from == eid && e.is_active()) {
        let pn = s.to_name().to_string();
        let Some(prec) = store.get("project", &pn, NOSK).await? else { continue };
        let pid = node_id("project", &pn);
        let mut accounts: Vec<BilledAccount> = edges
            .iter()
            .filter(|e| e.edge_type == "bills" && e.to == pid && e.is_active())
            .filter_map(|b| {
                let h = held.iter().find(|h| h.name == b.from_name())?;
                let pct = |k: &str| b.setting(k).and_then(|v| v.as_u64()).unwrap_or(0).min(100) as u8;
                Some(BilledAccount {
                    name: h.name.clone(),
                    provider: h.provider.clone(),
                    token_envar: h.token_envar.clone(),
                    order: b.setting("order").and_then(|v| v.as_i64()).unwrap_or(0),
                    switch: pct("switch"),
                    stop: pct("stop"),
                    model: b.setting_str("model"),
                })
            })
            .collect();
        accounts.sort_by(|a, b| (a.order, &a.name).cmp(&(b.order, &b.name)));
        projects.push(ProjectAssignment {
            project: pn.clone(),
            topdir: s.setting_str("topdir"),
            read_only: s.setting("read_only").and_then(|v| v.as_bool()).unwrap_or(false),
            state: Some(body_str(&prec, "state")).filter(|x| !x.is_empty()).unwrap_or_else(|| "Running".into()),
            accounts,
            edge_tag: s.tag.clone(),
            agents: agents_enabled(&edges, &pn),
        });
    }
    projects.sort_by(|a, b| a.project.cmp(&b.project));
    Ok(Assignments { engine: engine.into(), projects, accounts: held })
}

// ---------- API ----------

pub fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/api/settings/graph", get(graph_get))
        .route("/api/settings/nodes", post(node_create))
        .route("/api/settings/nodes/{id}", patch(node_patch).delete(node_delete).get(node_get))
        .route("/api/settings/edges", post(edge_create))
        .route("/api/settings/edges/{id}", patch(edge_patch).delete(edge_delete).get(edge_get))
        .route("/api/settings/edges/{id}/copy", post(edge_copy))
        .route("/api/engines/{name}/assignments", get(assignments_get))
}

fn conflict(msg: impl Into<String>) -> ApiError {
    ApiError::Status(StatusCode::CONFLICT, msg.into())
}

/// A node's settings as shown (secrets removed; the name lives in the id).
fn shown_settings(t: &str, rec: &Value) -> Value {
    let mut s = rec.clone();
    if let Some(o) = s.as_object_mut() {
        o.remove("name");
        if t == "user" {
            o.remove("user");
            o.remove("pwhash");
            o.remove("tokenver");
        }
    }
    s
}

fn summary_of(t: &str, rec: &Value) -> String {
    let s = |k: &str| body_str(rec, k);
    match t {
        "project" => s("state"),
        "iter_engine" => format!("{} {}", s("state"), s("last_seen")).trim().to_string(),
        "agent" => s("model"),
        "agent_tools" => s("kind"),
        "user" => s("role"),
        "account" => format!("{} {}", s("provider"), s("token_envar")).trim().to_string(),
        _ => s("desc"),
    }
}

fn node_json(id: &str, settings: Value, summary: String) -> Value {
    let (t, name) = parse_node_id(id).unwrap_or(("", ""));
    let ph = is_placeholder(id);
    json!({"id": id, "type": t, "name": name, "deactivated": ph, "placeholder": ph, "settings": settings, "summary": summary})
}

fn iter_data_self(st: &AppState) -> Value {
    let db = st.store.arango().map(|a| a.db_name().to_string()).unwrap_or_default();
    node_json(
        ITER_DATA_SELF,
        json!({"version": env!("CARGO_PKG_VERSION"), "backend": st.store.backend_name(), "db": db}),
        format!("iter_data {}", env!("CARGO_PKG_VERSION")),
    )
}

fn edge_json(e: &SysEdge) -> Value {
    let mut v = serde_json::to_value(e).unwrap();
    // `active` as stored is the admin's switch; the shown value is effective
    v["enabled"] = json!(e.active);
    v["active"] = json!(e.is_active());
    v
}

/// Every node (records + virtual) as `node_json`.
async fn all_nodes(st: &AppState) -> Result<Vec<Value>, StorageError> {
    let mut out = vec![iter_data_self(st)];
    for t in NODE_TYPES {
        if *t != "iter_data" {
            out.push(node_json(&placeholder_id(t), json!({}), "placeholder: edges here are inactive".into()));
        }
        let Some(table) = table_of(t) else { continue };
        for rec in st.store.scan(table).await? {
            let name = if *t == "user" { body_str(&rec, "user") } else { body_str(&rec, "name") };
            if name.is_empty() {
                continue;
            }
            out.push(node_json(&node_id(t, &name), shown_settings(t, &rec), summary_of(t, &rec)));
        }
    }
    Ok(out)
}

async fn graph_get(user: AuthUser, State(st): Ctx) -> Result<Json<Value>, ApiError> {
    let nodes = all_nodes(&st).await?;
    let edges = load_edges(st.store.as_ref()).await?;
    if user.role == "admin" {
        return Ok(Json(json!({"nodes": nodes, "edges": edges.iter().map(edge_json).collect::<Vec<_>>()})));
    }
    // non-admin: the parts touching the caller's projects, own user node and engines
    let visible = visible_projects(st.store.as_ref(), &user.sub, &user.role).await?.unwrap_or_default();
    let mut anchors: HashSet<String> = visible.iter().map(|p| node_id("project", p)).collect();
    anchors.insert(node_id("user", &user.sub));
    for en in engines_of_user(st.store.as_ref(), &edges, &user.sub).await? {
        anchors.insert(node_id("iter_engine", &en));
    }
    let shown: Vec<&SysEdge> = edges.iter().filter(|e| anchors.contains(&e.from) || anchors.contains(&e.to)).collect();
    let mut keep: HashSet<String> = anchors.clone();
    keep.insert(ITER_DATA_SELF.into());
    for e in &shown {
        keep.insert(e.from.clone());
        keep.insert(e.to.clone());
    }
    let nodes: Vec<Value> = nodes.into_iter().filter(|n| n["id"].as_str().map(|i| keep.contains(i)).unwrap_or(false)).collect();
    Ok(Json(json!({"nodes": nodes, "edges": shown.into_iter().map(edge_json).collect::<Vec<_>>()})))
}

async fn node_get(user: AuthUser, State(st): Ctx, Path(id): Path<String>) -> Result<Json<Value>, ApiError> {
    if id == ITER_DATA_SELF {
        return Ok(Json(iter_data_self(&st)));
    }
    let (t, _) = parse_node_id(&id).ok_or_else(|| bad(format!("'{id}' is not a node id (<type>:<name>)")))?;
    if user.role != "admin" && t != "user" && t != "iter_engine" && t != "project" {
        return Err(forbidden());
    }
    if is_placeholder(&id) {
        return Ok(Json(node_json(&id, json!({}), "placeholder".into())));
    }
    let rec = node_record(st.store.as_ref(), &id).await?.ok_or_else(notfound)?;
    Ok(Json(node_json(&id, shown_settings(t, &rec), summary_of(t, &rec))))
}

#[derive(serde::Deserialize)]
struct NodeCreateReq {
    #[serde(rename = "type")]
    node_type: String,
    name: String,
    #[serde(default)]
    settings: Value,
}

/// Merge `patch` into `base` key by key; a null removes the key.
fn merge(base: &mut Value, patch: &Value) {
    if !base.is_object() {
        *base = json!({});
    }
    if let (Some(b), Some(p)) = (base.as_object_mut(), patch.as_object()) {
        for (k, v) in p {
            if v.is_null() {
                b.remove(k);
            } else {
                b.insert(k.clone(), v.clone());
            }
        }
    }
}

/// Validate a node record for its type and fill what the type needs.
fn check_record(t: &str, name: &str, rec: &mut Value) -> Result<(), ApiError> {
    if !rec.is_object() {
        return Err(bad("settings must be an object"));
    }
    if t == "user" {
        rec["user"] = json!(name);
        if let Some(pw) = rec.get("password").and_then(|p| p.as_str()).map(String::from) {
            let h = crate::auth::hash_password(&pw).map_err(|e| ApiError::Status(StatusCode::INTERNAL_SERVER_ERROR, e))?;
            rec["pwhash"] = json!(h);
        }
        if let Some(o) = rec.as_object_mut() {
            o.remove("password");
            o.remove("name");
        }
    } else {
        rec["name"] = json!(name);
    }
    let parse = |r: Result<(), serde_json::Error>, what: &str| r.map_err(|e| bad(format!("{what} does not parse: {e}")));
    match t {
        "project" => {
            let p: iter_core::Project = serde_json::from_value(rec.clone()).map_err(|e| bad(format!("project does not parse: {e}")))?;
            if !["Running", "Draining", "Stopped"].contains(&p.state.as_str()) {
                return Err(bad("project state must be Running|Draining|Stopped"));
            }
        }
        "iter_engine" => parse(serde_json::from_value::<iter_core::Engine>(rec.clone()).map(|_| ()), "engine")?,
        "agent" => parse(serde_json::from_value::<iter_core::AgentDef>(rec.clone()).map(|_| ()), "agent")?,
        "agent_tools" => parse(serde_json::from_value::<iter_core::AgentTooling>(rec.clone()).map(|_| ()), "tooling")?,
        "user" => {
            let u: iter_core::WebuiUser = serde_json::from_value(rec.clone()).map_err(|e| bad(format!("user does not parse: {e}")))?;
            if !["admin", "user", "engine", "viewer"].contains(&u.role.as_str()) {
                return Err(bad("user role must be admin|user|engine|viewer"));
            }
            if rec.get("tokenver").is_none() {
                rec["tokenver"] = json!(1);
            }
            if rec.get("role").is_none() {
                rec["role"] = json!("user");
            }
        }
        "workitem_type" => {
            if !iter_core::STATES.contains(&name) {
                return Err(bad(format!("workitem_type nodes are the work-item states ({})", iter_core::STATES.join(", "))));
            }
        }
        "account" => {
            if let Some(v) = rec.get("token_envar") {
                if !v.is_string() {
                    return Err(bad("account.token_envar must be a string"));
                }
            }
        }
        _ => {}
    }
    Ok(())
}

fn seq_bucket(t: &str, name: &str) -> (String, &'static str) {
    let table = table_of(t).unwrap_or("");
    if t == "project" { (name.to_string(), table) } else { (GLOBAL.to_string(), table) }
}

async fn node_create(user: AuthUser, State(st): Ctx, Json(req): Json<NodeCreateReq>) -> Result<Json<Value>, ApiError> {
    user.require_admin()?;
    let t = req.node_type.as_str();
    if !cs::is_node_type(t) {
        return Err(bad(format!("unknown node type '{t}' ({})", NODE_TYPES.join(", "))));
    }
    if t == "iter_data" {
        return Err(bad("there is one iter_data node (iter_data:self)"));
    }
    let name = req.name.trim().to_string();
    if name.is_empty() || name == cs::PLACEHOLDER || name.contains('/') {
        return Err(bad("name must be non-empty, contain no '/', and not be the placeholder name"));
    }
    let id = node_id(t, &name);
    let store = st.store.as_ref();
    if node_record(store, &id).await?.is_some() {
        return Err(conflict(format!("{id} exists already")));
    }
    let mut rec = if req.settings.is_null() { json!({}) } else { req.settings.clone() };
    if t == "project" && rec.get("state").is_none() {
        rec["state"] = json!("Running");
    }
    check_record(t, &name, &mut rec)?;
    let table = table_of(t).unwrap();
    store.put(table, &name, NOSK, &rec).await?;
    let (bucket, tname) = seq_bucket(t, &name);
    store.bump_seq(&bucket, tname).await?;
    match t {
        "project" => {
            on_project_created(store, &name, None).await?;
            crate::sync_hooks::project_created(store, &name).await;
        }
        "agent" => on_agent_created(store, &name).await?,
        "agent_tools" => on_tooling_created(store, &rec).await?,
        "iter_engine" => on_engine_registered(store, &name, &body_str(&rec, "user")).await?,
        "account" => on_account_created(store, &name, &rec).await?,
        _ => {}
    }
    Ok(Json(node_json(&id, shown_settings(t, &rec), summary_of(t, &rec))))
}

#[derive(serde::Deserialize)]
struct NodePatchReq {
    #[serde(default)]
    settings: Value,
}

async fn node_patch(user: AuthUser, State(st): Ctx, Path(id): Path<String>, Json(req): Json<NodePatchReq>) -> Result<Json<Value>, ApiError> {
    user.require_admin()?;
    if is_protected(&id) {
        return Err(bad(format!("{id} is not editable (placeholders and iter_data:self)")));
    }
    let (t, name) = parse_node_id(&id).ok_or_else(|| bad(format!("'{id}' is not a node id")))?;
    let store = st.store.as_ref();
    let mut rec = node_record(store, &id).await?.ok_or_else(notfound)?;
    let mut patch = req.settings.clone();
    if let Some(o) = patch.as_object_mut() {
        // the name is the id; secrets go through their own keys
        o.remove("name");
        o.remove("user");
        o.remove("pwhash");
    }
    merge(&mut rec, &patch);
    check_record(t, name, &mut rec)?;
    store.put(table_of(t).unwrap(), name, NOSK, &rec).await?;
    let (bucket, tname) = seq_bucket(t, name);
    store.bump_seq(&bucket, tname).await?;
    Ok(Json(node_json(&id, shown_settings(t, &rec), summary_of(t, &rec))))
}

/// Delete a node record; its edges move to the placeholder (inactive).
/// Refused for placeholders and iter_data:self.
pub async fn delete_node(store: &dyn Storage, id: &str) -> Result<Value, ApiError> {
    if is_protected(id) {
        return Err(bad(format!("{id} cannot be deleted (placeholders and iter_data:self are permanent)")));
    }
    let (t, name) = parse_node_id(id).ok_or_else(|| bad(format!("'{id}' is not a node id")))?;
    let table = table_of(t).ok_or_else(|| bad("not a stored node type"))?;
    let deleted = store.delete(table, name, NOSK).await?;
    if !deleted {
        return Err(notfound());
    }
    let (bucket, tname) = seq_bucket(t, name);
    store.bump_seq(&bucket, tname).await?;
    let moved = on_node_deleted(store, id).await?;
    if t == "project" {
        // the project graph goes with the project (same as DELETE /api/projects/{p})
        crate::nodes::purge_project(store, name).await?;
    }
    Ok(json!({"deleted": true, "edges_moved_to_placeholder": moved}))
}

async fn node_delete(user: AuthUser, State(st): Ctx, Path(id): Path<String>) -> Result<Json<Value>, ApiError> {
    user.require_admin()?;
    Ok(Json(delete_node(st.store.as_ref(), &id).await?))
}

#[derive(serde::Deserialize)]
struct EdgeCreateReq {
    #[serde(default, rename = "type")]
    edge_type: Option<String>,
    from: String,
    to: String,
    #[serde(default)]
    tag: String,
    #[serde(default)]
    settings: Value,
}

/// Validate a would-be edge: endpoints exist, the type fits them (inferred
/// when absent), settings check; no duplicate (type, from, to) between real
/// nodes. Returns the edge type.
async fn check_edge(store: &dyn Storage, edge_type: Option<&str>, from: &str, to: &str, settings: &Value, except: Option<&str>) -> Result<String, ApiError> {
    let ft = cs::type_of_id(from).ok_or_else(|| bad(format!("'{from}' is not a node id (<type>:<name>)")))?;
    let tt = cs::type_of_id(to).ok_or_else(|| bad(format!("'{to}' is not a node id (<type>:<name>)")))?;
    let et = match edge_type.filter(|t| !t.is_empty()) {
        Some(t) => t.to_string(),
        None => edge_type_for(ft, tt).ok_or_else(|| bad(format!("no edge type joins {ft} → {tt}")))?.to_string(),
    };
    validate_endpoints(&et, from, to).map_err(bad)?;
    validate_settings(&et, settings).map_err(bad)?;
    for id in [from, to] {
        if !node_exists(store, id).await? {
            return Err(ApiError::Status(StatusCode::NOT_FOUND, format!("no node {id}")));
        }
    }
    if !is_placeholder(from) && !is_placeholder(to) {
        let dup = load_edges(store)
            .await?
            .into_iter()
            .any(|e| e.edge_type == et && e.from == from && e.to == to && Some(e.id.as_str()) != except);
        if dup {
            return Err(conflict(format!("a {et} edge {from} → {to} exists already")));
        }
    }
    Ok(et)
}

async fn edge_create(user: AuthUser, State(st): Ctx, Json(req): Json<EdgeCreateReq>) -> Result<Json<Value>, ApiError> {
    user.require_admin()?;
    let settings = if req.settings.is_null() { json!({}) } else { req.settings.clone() };
    let et = check_edge(st.store.as_ref(), req.edge_type.as_deref(), &req.from, &req.to, &settings, None).await?;
    let e = new_edge(&et, &req.from, &req.to, &req.tag, settings);
    put_edge(st.store.as_ref(), &e).await?;
    Ok(Json(edge_json(&e)))
}

async fn edge_get(user: AuthUser, State(st): Ctx, Path(id): Path<String>) -> Result<Json<Value>, ApiError> {
    let e = get_edge(st.store.as_ref(), &id).await?.ok_or_else(notfound)?;
    if user.role != "admin" {
        let vis = visible_projects(st.store.as_ref(), &user.sub, &user.role).await?.unwrap_or_default();
        let touches = |n: &str| parse_node_id(n).map(|(t, name)| t == "project" && vis.contains(name)).unwrap_or(false)
            || n == node_id("user", &user.sub);
        if !touches(&e.from) && !touches(&e.to) {
            return Err(forbidden());
        }
    }
    Ok(Json(edge_json(&e)))
}

#[derive(serde::Deserialize)]
struct EdgePatchReq {
    #[serde(default)]
    from: Option<String>,
    #[serde(default)]
    to: Option<String>,
    #[serde(default)]
    tag: Option<String>,
    #[serde(default)]
    settings: Option<Value>,
    #[serde(default)]
    active: Option<bool>,
}

/// Edit an edge; a changed `from`/`to` moves that endpoint (drag in the
/// webui). The new endpoint must fit the edge's type; moving onto the
/// placeholder deactivates the edge, moving off it reactivates it unless
/// `active` says otherwise.
async fn edge_patch(user: AuthUser, State(st): Ctx, Path(id): Path<String>, Json(req): Json<EdgePatchReq>) -> Result<Json<Value>, ApiError> {
    user.require_admin()?;
    let store = st.store.as_ref();
    let mut e = get_edge(store, &id).await?.ok_or_else(notfound)?;
    let from = req.from.clone().unwrap_or_else(|| e.from.clone());
    let to = req.to.clone().unwrap_or_else(|| e.to.clone());
    let mut settings = e.settings.clone();
    if let Some(s) = &req.settings {
        if !s.is_object() {
            return Err(bad("settings must be an object"));
        }
        merge(&mut settings, s);
    }
    check_edge(store, Some(&e.edge_type), &from, &to, &settings, Some(&e.id)).await?;
    let moved = from != e.from || to != e.to;
    let was_placeholder = is_placeholder(&e.from) || is_placeholder(&e.to);
    e.from = from;
    e.to = to;
    e.settings = settings;
    if let Some(t) = req.tag {
        e.tag = t;
    }
    let now_placeholder = is_placeholder(&e.from) || is_placeholder(&e.to);
    if let Some(a) = req.active {
        e.active = a;
    } else if moved && now_placeholder {
        e.active = false;
    } else if moved && was_placeholder && !now_placeholder {
        e.active = true;
    }
    e.updated = now_utc();
    put_edge(store, &e).await?;
    Ok(Json(edge_json(&e)))
}

async fn edge_delete(user: AuthUser, State(st): Ctx, Path(id): Path<String>) -> Result<Json<Value>, ApiError> {
    user.require_admin()?;
    let gone = delete_edge_row(st.store.as_ref(), &id).await?;
    if !gone {
        return Err(notfound());
    }
    Ok(Json(json!({"deleted": true})))
}

#[derive(serde::Deserialize, Default)]
struct EdgeCopyReq {
    #[serde(default)]
    from: Option<String>,
    #[serde(default)]
    to: Option<String>,
}

/// Copy/paste: a new edge with the same type, tag and settings, with one
/// (or both) endpoints replaced.
async fn edge_copy(user: AuthUser, State(st): Ctx, Path(id): Path<String>, body: Option<Json<EdgeCopyReq>>) -> Result<Json<Value>, ApiError> {
    user.require_admin()?;
    let req = body.map(|b| b.0).unwrap_or_default();
    let store = st.store.as_ref();
    let src = get_edge(store, &id).await?.ok_or_else(notfound)?;
    let from = req.from.unwrap_or_else(|| src.from.clone());
    let to = req.to.unwrap_or_else(|| src.to.clone());
    check_edge(store, Some(&src.edge_type), &from, &to, &src.settings, None).await?;
    let mut e = new_edge(&src.edge_type, &from, &to, &src.tag, src.settings.clone());
    e.active = src.active || is_placeholder(&src.from) || is_placeholder(&src.to);
    put_edge(store, &e).await?;
    Ok(Json(edge_json(&e)))
}

async fn assignments_get(user: AuthUser, State(st): Ctx, Path(name): Path<String>) -> Result<Json<Value>, ApiError> {
    let store = st.store.as_ref();
    let rec = store.get("engine", &name, NOSK).await?.ok_or_else(notfound)?;
    if user.role != "admin" {
        let owner = body_str(&rec, "user");
        let edges = load_edges(store).await?;
        let owned = owner == user.sub || engines_of_user(store, &edges, &user.sub).await?.contains(&name);
        // an unowned (iter4) record may be read by any engine token until
        // its first heartbeat claims it
        if !owned && !(owner.is_empty() && user.role == "engine") {
            return Err(forbidden());
        }
    }
    Ok(Json(serde_json::to_value(assignments(store, &name).await?).unwrap()))
}

#[cfg(test)]
mod tests;
