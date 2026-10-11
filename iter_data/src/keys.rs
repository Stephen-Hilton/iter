//! Engine keys (decided 2026-10-11): how an engine machine signs in, and the
//! Engine keys admin screen's API.
//!
//! An engine makes its own Ed25519 key pair (`iter_engine key new`); the
//! private key never leaves the machine. An admin pastes the public key here
//! and names the engine it belongs to; one row per key in `engine_key`, keyed
//! by the key's id (iter_core::enginekey). Nothing secret is stored or
//! copied.
//!
//! Signing in (`POST /auth/engine`, no bearer): the engine signs
//! `enginekey::login_message` — key id, its clock, a one-time nonce. A live
//! key, a timestamp within LOGIN_SKEW_SECS, an unused nonce and a valid
//! signature earn a session token: `iter_` + a JWT whose subject is the
//! engine itself (`iter_engine:<engine>`, role `engine`) and whose `kid`
//! names the key. An engine is a machine, not a person: no user stands
//! behind it and no `owns` edge; the session acts only for its own engine
//! (settings::machine_engine).
//!
//! Every request with a session goes through `check_session`: the key must
//! still be registered, unrevoked and the engine's. So revoking a key stops
//! the engine — and every agent it started — on their next request; the
//! session's own expiry (SESSION_SECS) only bounds a session token copied
//! off the wire while nobody revoked anything.
//!
//! Revoke: the row's `revoked` is the time the key stops working ("" =
//! never). Rotate: the engine's new public key (`iter_engine key new` again
//! keeps it as the next key) is added and the old one revoked, at once or
//! after `grace_minutes` — agents already running hold sessions of the old
//! key. The engine signs in with the next key as soon as it is accepted
//! (client.rs). Delete removes a revoked row for good.
//!
//! When a key was last used lives in a separate row (sk `used`), written at
//! most every USED_EVERY_SECS per key, so a request never rewrites the key's
//! own row (it would race a revoke).
//!
//! The older engine logins — a user with role `engine` signing in with a
//! signed (JWT) token from `POST /api/users/{user}/token` — still work until
//! each engine has its own key; the screen lists those users for removal.

use crate::api::{ApiError, AppState, AuthUser, NOSK, bad, notfound};
use crate::auth::Claims;
use crate::storage::{Storage, body_str};
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use iter_core::{enginekey, now_utc};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Instant;

pub const TABLE: &str = "engine_key";
const USED_SK: &str = "used";
const USED_EVERY_SECS: u64 = 600;
/// A session's lifetime. Long enough to outlast an agent session started
/// with it (the engine renews well before), and revocation does not wait
/// for it: every request re-checks the key.
pub const SESSION_SECS: u64 = 24 * 3600;

type Ctx = State<Arc<AppState>>;

pub fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/auth/engine", post(engine_login))
        .route("/api/keys", get(keys_list).post(key_add))
        .route("/api/keys/{id}/revoke", post(key_revoke))
        .route("/api/keys/{id}/rotate", post(key_rotate))
        .route("/api/keys/{id}", delete(key_delete))
}

fn unauthorized(msg: &str) -> ApiError {
    ApiError::Status(StatusCode::UNAUTHORIZED, msg.into())
}

/// Has the row's revocation taken effect at `now`?
fn revoked_at(row: &Value, now: &str) -> bool {
    let r = body_str(row, "revoked");
    !r.is_empty() && r.as_str() <= now
}

/// The live row of key `kid`, or the 401 that says why not.
async fn live_key(st: &AppState, kid: &str) -> Result<Value, ApiError> {
    let row = st.store.get(TABLE, kid, NOSK).await?.filter(|r| r.get("public_key").is_some()).ok_or_else(|| unauthorized("unknown engine key"))?;
    if revoked_at(&row, &now_utc()) {
        return Err(unauthorized("engine key revoked"));
    }
    Ok(row)
}

#[derive(serde::Deserialize)]
struct LoginReq {
    key_id: String,
    ts: i64,
    nonce: String,
    /// hex of the 64-byte Ed25519 signature over enginekey::login_message
    signature: String,
}

/// Has this nonce been seen in the last 2 × LOGIN_SKEW_SECS? (Remembers it.)
/// A login replayed within the skew window is refused; one replayed later is
/// refused by its timestamp.
fn nonce_reused(nonce: &str) -> bool {
    static SEEN: Mutex<Option<HashMap<String, Instant>>> = Mutex::new(None);
    let mut g = SEEN.lock().unwrap_or_else(|p| p.into_inner());
    let m = g.get_or_insert_with(HashMap::new);
    let keep = (2 * enginekey::LOGIN_SKEW_SECS) as u64;
    m.retain(|_, t| t.elapsed().as_secs() < keep);
    m.insert(nonce.to_string(), Instant::now()).is_some()
}

async fn engine_login(State(st): Ctx, Json(req): Json<LoginReq>) -> Result<Json<Value>, ApiError> {
    use ed25519_dalek::{Signature, Verifier, VerifyingKey};
    let row = live_key(&st, &req.key_id).await?;
    let now = chrono::Utc::now().timestamp();
    if (now - req.ts).abs() > enginekey::LOGIN_SKEW_SECS {
        return Err(unauthorized("the engine's clock is more than 5 minutes off the server's"));
    }
    if req.nonce.len() < 16 || req.nonce.len() > 64 {
        return Err(unauthorized("bad nonce"));
    }
    let key = enginekey::parse_public(&body_str(&row, "public_key")).ok_or_else(|| unauthorized("unknown engine key"))?;
    let vk = VerifyingKey::from_bytes(&key).map_err(|_| unauthorized("unknown engine key"))?;
    let sig: [u8; 64] = enginekey::unhex(&req.signature).and_then(|b| b.try_into().ok()).ok_or_else(|| unauthorized("bad signature"))?;
    vk.verify(enginekey::login_message(&req.key_id, req.ts, &req.nonce).as_bytes(), &Signature::from_bytes(&sig))
        .map_err(|_| unauthorized("bad signature"))?;
    if nonce_reused(&req.nonce) {
        return Err(unauthorized("this login was already used"));
    }
    let engine = body_str(&row, "engine");
    let claims = Claims {
        sub: format!("iter_engine:{engine}"),
        role: "engine".into(),
        tokenver: 0,
        exp: now as u64 + SESSION_SECS,
        kid: req.key_id.clone(),
    };
    let jwt = crate::auth::mint_claims(&st.secret, &claims).map_err(|e| ApiError::Status(StatusCode::INTERNAL_SERVER_ERROR, e))?;
    touch(st.store.clone(), &req.key_id);
    Ok(Json(json!({"token": format!("{}{jwt}", enginekey::SESSION_PREFIX), "engine": engine, "expires_at": claims.exp})))
}

/// A verified session's claims to the engine it acts for; 401 once its key
/// is revoked or gone.
pub(crate) async fn check_session(st: &AppState, claims: &Claims) -> Result<AuthUser, ApiError> {
    let row = live_key(st, &claims.kid).await?;
    let engine = crate::settings::machine_engine(&claims.sub).ok_or_else(|| unauthorized("bad engine session"))?;
    if engine != body_str(&row, "engine") || claims.role != "engine" {
        return Err(unauthorized("bad engine session"));
    }
    touch(st.store.clone(), &claims.kid);
    Ok(AuthUser { sub: claims.sub.clone(), role: claims.role.clone() })
}

/// Record that a key was used, at most every USED_EVERY_SECS (this process
/// remembers when it last wrote), without delaying the request.
fn touch(store: Arc<dyn Storage>, id: &str) {
    static LAST: Mutex<Option<HashMap<String, Instant>>> = Mutex::new(None);
    {
        let mut g = LAST.lock().unwrap_or_else(|p| p.into_inner());
        let m = g.get_or_insert_with(HashMap::new);
        if m.get(id).is_some_and(|t| t.elapsed().as_secs() < USED_EVERY_SECS) {
            return;
        }
        m.insert(id.to_string(), Instant::now());
    }
    let id = id.to_string();
    tokio::spawn(async move {
        let _ = store.put(TABLE, &id, USED_SK, &json!({"usage_of": id, "last_used": now_utc()})).await;
    });
}

/// The row as the screen shows it, with its status and when it was last used.
fn public(row: &Value, used: Option<&Value>, now: &str) -> Value {
    let mut r = row.clone();
    if let Some(o) = r.as_object_mut() {
        o.insert("last_used".into(), json!(used.map(|u| body_str(u, "last_used")).unwrap_or_default()));
        let status = if revoked_at(row, now) {
            "revoked"
        } else if body_str(row, "revoked").is_empty() {
            "active"
        } else {
            "expiring"
        };
        o.insert("status".into(), json!(status));
    }
    r
}

async fn keys_list(user: AuthUser, State(st): Ctx) -> Result<Json<Value>, ApiError> {
    user.require_admin()?;
    let rows = st.store.scan(TABLE).await?;
    let used: HashMap<String, &Value> = rows.iter().filter(|r| r.get("usage_of").is_some()).map(|r| (body_str(r, "usage_of"), r)).collect();
    let now = now_utc();
    let mut keys: Vec<Value> = rows
        .iter()
        .filter(|r| r.get("public_key").is_some())
        .map(|r| public(r, used.get(&body_str(r, "id")).copied(), &now))
        .collect();
    keys.sort_by(|a, b| (body_str(a, "engine"), body_str(b, "created")).cmp(&(body_str(b, "engine"), body_str(a, "created"))));
    // the older engine logins: users with role engine, listed for removal
    let older: Vec<Value> = st
        .store
        .scan("webui_user")
        .await?
        .iter()
        .filter(|u| body_str(u, "role") == "engine")
        .map(|u| json!({"user": body_str(u, "user"), "name": body_str(u, "name")}))
        .collect();
    Ok(Json(json!({"keys": keys, "older": older})))
}

#[derive(serde::Deserialize)]
struct AddReq {
    public_key: String,
    /// the engine (its id) the key signs in as
    #[serde(default)]
    engine: String,
    #[serde(default)]
    label: String,
}

/// An engine id a key may be for: the name the engine runs as (`--name`,
/// else its hostname). It need not be registered yet: the engine registers
/// itself on its first start.
fn check_engine(engine: &str) -> Result<(), ApiError> {
    if engine.is_empty() || !engine.chars().all(|c| c.is_ascii_alphanumeric() || "-_.".contains(c)) {
        return Err(bad("an engine name is required: letters, digits, - _ . only"));
    }
    Ok(())
}

/// Store a new key's row; refuses a malformed or already-registered one.
async fn insert(st: &AppState, admin: &str, public_key: &str, engine: &str, label: &str, replaces: &str) -> Result<Value, ApiError> {
    let key = enginekey::parse_public(public_key).ok_or_else(|| {
        bad(format!("that is not an engine public key: run `iter_engine key new` on the engine machine and paste the {}… line it prints", enginekey::PREFIX))
    })?;
    let id = enginekey::key_id(&key);
    if st.store.get(TABLE, &id, NOSK).await?.is_some() {
        return Err(ApiError::Status(StatusCode::CONFLICT, format!("key {id} is already registered")));
    }
    check_engine(engine)?;
    let row = json!({
        "id": id, "engine": engine, "label": label.trim(), "public_key": enginekey::format_public(&key),
        "created": now_utc(), "created_by": admin, "revoked": "", "revoked_by": "",
        "replaces": replaces, "replaced_by": "",
    });
    st.store.put(TABLE, &id, NOSK, &row).await?;
    Ok(row)
}

async fn key_add(user: AuthUser, State(st): Ctx, Json(req): Json<AddReq>) -> Result<Json<Value>, ApiError> {
    user.require_admin()?;
    let row = insert(&st, &user.sub, &req.public_key, req.engine.trim(), &req.label, "").await?;
    Ok(Json(public(&row, None, &now_utc())))
}

/// An admin's view of a key row that is not yet revoked.
async fn unrevoked_row(st: &AppState, id: &str) -> Result<Value, ApiError> {
    let row = st.store.get(TABLE, id, NOSK).await?.filter(|r| r.get("public_key").is_some()).ok_or_else(notfound)?;
    if revoked_at(&row, &now_utc()) {
        return Err(bad(format!("key {id} is already revoked")));
    }
    Ok(row)
}

async fn key_revoke(user: AuthUser, State(st): Ctx, Path(id): Path<String>) -> Result<Json<Value>, ApiError> {
    user.require_admin()?;
    let mut row = unrevoked_row(&st, &id).await?;
    row["revoked"] = json!(now_utc());
    row["revoked_by"] = json!(user.sub);
    st.store.put(TABLE, &id, NOSK, &row).await?;
    Ok(Json(public(&row, None, &now_utc())))
}

#[derive(serde::Deserialize)]
struct RotateReq {
    public_key: String,
    /// minutes the old key keeps working (0 = revoked at once)
    #[serde(default)]
    grace_minutes: u64,
}

async fn key_rotate(user: AuthUser, State(st): Ctx, Path(id): Path<String>, Json(req): Json<RotateReq>) -> Result<Json<Value>, ApiError> {
    user.require_admin()?;
    let mut old = unrevoked_row(&st, &id).await?;
    let new = insert(&st, &user.sub, &req.public_key, &body_str(&old, "engine"), &body_str(&old, "label"), &id).await?;
    let at = chrono::Utc::now() + chrono::Duration::minutes(req.grace_minutes.min(7 * 24 * 60) as i64);
    old["revoked"] = json!(at.format("%Y-%m-%dT%H:%M:%SZ").to_string());
    old["revoked_by"] = json!(user.sub);
    old["replaced_by"] = new["id"].clone();
    st.store.put(TABLE, &id, NOSK, &old).await?;
    let now = now_utc();
    Ok(Json(json!({"new": public(&new, None, &now), "old": public(&old, None, &now)})))
}

async fn key_delete(user: AuthUser, State(st): Ctx, Path(id): Path<String>) -> Result<Json<Value>, ApiError> {
    user.require_admin()?;
    let row = st.store.get(TABLE, &id, NOSK).await?.ok_or_else(notfound)?;
    if !revoked_at(&row, &now_utc()) {
        return Err(bad("revoke the key first; only a revoked key can be deleted"));
    }
    st.store.delete(TABLE, &id, NOSK).await?;
    st.store.delete(TABLE, &id, USED_SK).await?;
    Ok(Json(json!({"deleted": id})))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn revocation_takes_effect_at_its_time() {
        let now = "2026-10-11T12:00:00Z";
        assert!(!revoked_at(&json!({"revoked": ""}), now));
        assert!(!revoked_at(&json!({}), now));
        assert!(revoked_at(&json!({"revoked": "2026-10-11T12:00:00Z"}), now));
        assert!(revoked_at(&json!({"revoked": "2026-10-11T11:59:59Z"}), now));
        assert!(!revoked_at(&json!({"revoked": "2026-10-11T12:10:00Z"}), now));
        let p = public(&json!({"id": "x", "public_key": "iter_00", "revoked": "2026-10-11T12:10:00Z"}), Some(&json!({"last_used": "t"})), now);
        assert_eq!((p["status"].as_str(), p["last_used"].as_str()), (Some("expiring"), Some("t")));
        assert!(!nonce_reused("nonce-test-aaaaaaaaaaaaaaa"));
        assert!(nonce_reused("nonce-test-aaaaaaaaaaaaaaa"));
    }
}

/// The Keys API and engine sign-in through the real router (and its authz
/// layer), on a throwaway ArangoDB database (test_db.rs).
#[cfg(test)]
mod api_tests {
    use crate::api::{AppState, NOSK};
    use axum::body::Body;
    use axum::http::Request;
    use axum::Router;
    use ed25519_dalek::{Signer, SigningKey};
    use iter_core::enginekey;
    use serde_json::{Value, json};
    use std::sync::Arc;
    use tower::ServiceExt;

    struct T {
        st: Arc<AppState>,
        db: String,
        app: Router,
    }

    fn new_key() -> SigningKey {
        SigningKey::from_bytes(&rand::random::<[u8; 32]>())
    }
    fn pubkey(k: &SigningKey) -> String {
        enginekey::format_public(&k.verifying_key().to_bytes())
    }
    fn kid(k: &SigningKey) -> String {
        enginekey::key_id(&k.verifying_key().to_bytes())
    }
    fn login_body(k: &SigningKey, ts: i64, nonce: &str) -> Value {
        let msg = enginekey::login_message(&kid(k), ts, nonce);
        let sig: String = k.sign(msg.as_bytes()).to_bytes().iter().map(|b| format!("{b:02x}")).collect();
        json!({"key_id": kid(k), "ts": ts, "nonce": nonce, "signature": sig})
    }
    fn nonce() -> String {
        uuid::Uuid::new_v4().simple().to_string()
    }

    impl T {
        async fn new() -> T {
            let (a, db) = crate::test_db::store().await;
            let st = Arc::new(AppState { store: Arc::new(a), secret: b"keys-test".to_vec() });
            for (u, role) in [("adm", "admin"), ("alice", "user")] {
                st.store.put("webui_user", u, NOSK, &json!({"user": u, "role": role, "tokenver": 1, "pwhash": ""})).await.unwrap();
            }
            let app = crate::api::router(st.clone());
            T { st, db, app }
        }
        fn signed(&self, user: &str, role: &str) -> String {
            crate::auth::mint_token(&self.st.secret, user, role, 1, 3600).unwrap()
        }
        async fn call(&self, bearer: &str, method: &str, path: &str, body: Option<Value>) -> (u16, Value) {
            let mut b = Request::builder().method(method).uri(path);
            if !bearer.is_empty() {
                b = b.header("authorization", format!("Bearer {bearer}"));
            }
            let req = match body {
                Some(v) => b.header("content-type", "application/json").body(Body::from(serde_json::to_vec(&v).unwrap())).unwrap(),
                None => b.body(Body::empty()).unwrap(),
            };
            let resp = self.app.clone().oneshot(req).await.unwrap();
            let code = resp.status().as_u16();
            let bytes = axum::body::to_bytes(resp.into_body(), 1 << 20).await.unwrap();
            (code, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
        }
        async fn admin(&self, method: &str, path: &str, body: Option<Value>) -> (u16, Value) {
            self.call(&self.signed("adm", "admin"), method, path, body).await
        }
        /// sign in with `k`: the session token, or the refusal's status
        async fn login(&self, k: &SigningKey) -> Result<String, u16> {
            let (c, v) = self.call("", "POST", "/auth/engine", Some(login_body(k, chrono::Utc::now().timestamp(), &nonce()))).await;
            if c == 200 { Ok(v["token"].as_str().unwrap().to_string()) } else { Err(c) }
        }
    }

    #[tokio::test]
    async fn engine_keys_register_sign_in_rotate_and_revoke() {
        let t = T::new().await;
        let k1 = new_key();
        let id = kid(&k1);

        // an unregistered key signs in as nobody
        assert_eq!(t.login(&k1).await, Err(401));
        // only an admin registers one, and only a well-formed one, once
        let alice = t.signed("alice", "user");
        assert_eq!(t.call(&alice, "POST", "/api/keys", Some(json!({"public_key": pubkey(&k1), "engine": "lap1"}))).await.0, 403);
        assert_eq!(t.admin("POST", "/api/keys", Some(json!({"public_key": "iter_nope", "engine": "lap1"}))).await.0, 400);
        assert_eq!(t.admin("POST", "/api/keys", Some(json!({"public_key": pubkey(&new_key()), "engine": "a b"}))).await.0, 400);
        let (c, row) = t.admin("POST", "/api/keys", Some(json!({"public_key": format!(" {}\n", pubkey(&k1)), "engine": "lap1", "label": "attic laptop"}))).await;
        assert_eq!((c, row["status"].as_str(), row["id"].as_str()), (200, Some("active"), Some(id.as_str())), "{row}");
        assert_eq!(t.admin("POST", "/api/keys", Some(json!({"public_key": pubkey(&k1), "engine": "lap1"}))).await.0, 409);
        // no user is made for the engine
        assert!(t.st.store.get("webui_user", "lap1", NOSK).await.unwrap().is_none());

        // the signed login: a stale clock, a forged signature and a replay are refused
        let now = chrono::Utc::now().timestamp();
        assert_eq!(t.call("", "POST", "/auth/engine", Some(login_body(&k1, now - 900, &nonce()))).await.0, 401);
        let mut forged = login_body(&new_key(), now, &nonce());
        forged["key_id"] = json!(id);
        assert_eq!(t.call("", "POST", "/auth/engine", Some(forged)).await.0, 401);
        let once = login_body(&k1, now, &nonce());
        let (c, v) = t.call("", "POST", "/auth/engine", Some(once.clone())).await;
        assert_eq!((c, v["engine"].as_str()), (200, Some("lap1")), "{v}");
        assert_eq!(t.call("", "POST", "/auth/engine", Some(once)).await.0, 401, "a replayed login");
        let s1 = v["token"].as_str().unwrap().to_string();
        assert!(s1.starts_with("iter_eyJ"), "{s1}");

        // the session is the engine itself: its own record (owned by no user,
        // no owns edge), never another engine's
        assert_eq!(t.call(&s1, "GET", "/api/engines", None).await.0, 200);
        assert_eq!(t.call(&s1, "PUT", "/api/engines/lap1", Some(json!({"state": "Running"}))).await.0, 200);
        let rec = t.st.store.get("engine", "lap1", NOSK).await.unwrap().unwrap();
        assert_eq!(rec["user"].as_str().unwrap_or(""), "", "{rec}");
        let edges = crate::settings::load_edges(t.st.store.as_ref()).await.unwrap();
        assert!(!edges.iter().any(|e| e.edge_type == "owns" && e.to == "iter_engine:lap1"));
        assert_eq!(t.call(&s1, "PUT", "/api/engines/other", Some(json!({"state": "Running"}))).await.0, 403);
        assert_eq!(t.call(&s1, "GET", "/api/engines/lap1/assignments", None).await.0, 200);
        // the project authz layer sees it as that engine
        t.admin("PUT", "/api/projects/p", Some(json!({"state": "Running"}))).await;
        let (c, v) = t.call(&s1, "GET", "/api/projects/p/workitems", None).await;
        assert_eq!(c, 403, "{v}");
        assert!(v["error"].as_str().unwrap_or("").contains("engine 'lap1' does not serve"), "{v}");
        let (c, v) = t.admin("POST", "/api/settings/edges", Some(json!({"from": "iter_engine:lap1", "to": "project:p", "settings": {"topdir": "~/dev/p"}}))).await;
        assert_eq!(c, 200, "{v}");
        assert_eq!(t.call(&s1, "GET", "/api/projects/p/workitems", None).await.0, 200);
        // a session without its prefix, or tampered, is nobody; a person's id never looks like an engine
        assert_eq!(t.call(&s1["iter_".len()..], "GET", "/api/engines", None).await.0, 401);
        assert_eq!(t.call(&format!("{s1}x"), "GET", "/api/engines", None).await.0, 401);
        assert_eq!(t.admin("PUT", "/api/users/iter_engine:lap1", Some(json!({"role": "admin", "password": "x"}))).await.0, 400);
        // admin-only listing
        assert_eq!(t.call(&s1, "GET", "/api/keys", None).await.0, 403);
        let (_, list) = t.admin("GET", "/api/keys", None).await;
        assert_eq!(list["keys"].as_array().unwrap().len(), 1);
        assert!(list["older"].as_array().unwrap().is_empty());

        // rotate with a grace period: sessions of both keys work until the old one's time comes
        let k2 = new_key();
        let (c, r) = t.admin("POST", &format!("/api/keys/{id}/rotate"), Some(json!({"public_key": pubkey(&k2), "grace_minutes": 30}))).await;
        assert_eq!((c, r["old"]["status"].as_str(), r["new"]["replaces"].as_str()), (200, Some("expiring"), Some(id.as_str())), "{r}");
        let s2 = t.login(&k2).await.unwrap();
        assert_eq!(t.call(&s1, "GET", "/api/engines", None).await.0, 200);
        assert_eq!(t.call(&s2, "GET", "/api/engines", None).await.0, 200);
        // revoking a key stops its sessions at once, not when they expire
        assert_eq!(t.admin("DELETE", &format!("/api/keys/{id}"), None).await.0, 400);
        assert_eq!(t.admin("POST", &format!("/api/keys/{id}/revoke"), None).await.0, 200);
        let (c, v) = t.call(&s1, "GET", "/api/engines", None).await;
        assert_eq!((c, v["error"].as_str()), (401, Some("engine key revoked")), "{v}");
        assert_eq!(t.login(&k1).await, Err(401));
        assert_eq!(t.call(&s2, "GET", "/api/engines", None).await.0, 200);

        // rotate with no grace: the old key's sessions stop at once
        let k3 = new_key();
        assert_eq!(t.admin("POST", &format!("/api/keys/{}/rotate", kid(&k2)), Some(json!({"public_key": pubkey(&k3)}))).await.0, 200);
        assert_eq!(t.call(&s2, "GET", "/api/engines", None).await.0, 401);
        let s3 = t.login(&k3).await.unwrap();
        assert_eq!(t.call(&s3, "GET", "/api/engines", None).await.0, 200);

        // a revoked row deletes for good
        assert_eq!(t.admin("DELETE", &format!("/api/keys/{id}"), None).await.0, 200);
        assert!(t.st.store.get(super::TABLE, &id, NOSK).await.unwrap().is_none());
        let (_, list) = t.admin("GET", "/api/keys", None).await;
        assert_eq!(list["keys"].as_array().unwrap().len(), 2);
        crate::test_db::drop(&t.db).await;
    }
}
