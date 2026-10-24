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
//! Engine-initiated rotation (`POST /auth/engine/rotate`, 2026-10-17): an
//! engine replaces its own key with no admin step by signing
//! `enginekey::rotate_message` (its current key id and the new public key)
//! with the current key. Only a key that is still fully active may rotate
//! itself: a revoked key, or one already on its way out, is refused, so a
//! compromised (revoked) engine can never mint itself a fresh key; it has
//! to be set up again by hand. The old key keeps working for
//! ROTATE_GRACE_SECS, for agents still holding its sessions.
//!
//! Sessions are signed with the engine session secret, never the one that
//! signs people's tokens (AppState::engine_secret), and the sign-in answer
//! carries the TLS certificate fingerprints iter_data serves (tls.rs), so
//! engines learn a coming certificate over the channel they already trust.
//!
//! Join codes (2026-10-23, iter_core::enginekey::JoinCode): an admin makes one
//! (`POST /api/keys/join`, the screen's Add engine) for the URL engines use;
//! iter_data keeps only the secret's id, in a row (sk `join`) that expires.
//! The engine posts the secret with its new public key to
//! `POST /auth/engine/join`; the first such call deletes the row and adds the
//! key, so a code registers one key, once. The same key again (an installer
//! run twice) is answered as done without using the code. A code that names
//! no engine takes the machine's name, or `<name>-2`, … when an engine with a
//! live key has it already, so no code can take over another engine.
//!
//! A TLS front (proxy, load balancer) before iter_data: the engine then sees
//! the front's certificate, not one iter_data serves. `check_seen` accepts it
//! only when a public certificate authority vouches for it under one of
//! iter_data's names: its own certificate's and every join code's URL host
//! (kept in a `server` row); any other certificate in between is refused.

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
/// A join code's row: pk `join-<join id>`, this sk.
const JOIN_SK: &str = "join";
/// The names engines reach iter_data by (a TLS front's public certificate is
/// accepted for these): pk NAMES_PK, sk `server`.
const NAMES_PK: &str = "public-names";
const NAMES_SK: &str = "server";
/// How long a join code works unless the admin says otherwise, and the longest it may.
const JOIN_HOURS: i64 = 24;
const JOIN_MAX_HOURS: i64 = 7 * 24;
const USED_EVERY_SECS: u64 = 600;
/// A session's lifetime. Long enough to outlast an agent session started
/// with it (the engine renews well before), and revocation does not wait
/// for it: every request re-checks the key.
pub const SESSION_SECS: u64 = 24 * 3600;
/// How long a key an engine rotated away from keeps working.
pub const ROTATE_GRACE_SECS: i64 = 3600;

type Ctx = State<Arc<AppState>>;

pub fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/auth/engine", post(engine_login))
        .route("/auth/engine/rotate", post(engine_rotate))
        .route("/auth/engine/join", post(engine_join))
        .route("/api/keys/join", post(join_new))
        .route("/api/keys/join/{id}", delete(join_cancel))
        .route("/api/server/tls", get(tls_get))
        .route("/api/server/tls/next", post(tls_next))
        .route("/api/server/tls/promote", post(tls_promote))
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
    /// the fingerprint of the TLS certificate the engine sees ("" over http)
    #[serde(default)]
    tls_fp: String,
    /// the certificates the engine was shown (base64 DER, the server's first),
    /// so a TLS front's public certificate can be checked (check_seen)
    #[serde(default)]
    tls_chain: Vec<String>,
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

/// Check one signed engine message against key row `row`: the clock skew,
/// the nonce's shape, the Ed25519 signature over `message`, then the nonce's
/// freshness (remembered only once the signature holds).
fn verify_signed(row: &Value, ts: i64, nonce: &str, signature: &str, message: &str) -> Result<(), ApiError> {
    use ed25519_dalek::{Signature, Verifier, VerifyingKey};
    let now = chrono::Utc::now().timestamp();
    if (now - ts).abs() > enginekey::LOGIN_SKEW_SECS {
        return Err(unauthorized("the engine's clock is more than 5 minutes off the server's"));
    }
    if nonce.len() < 16 || nonce.len() > 64 {
        return Err(unauthorized("bad nonce"));
    }
    let key = enginekey::parse_public(&body_str(row, "public_key")).ok_or_else(|| unauthorized("unknown engine key"))?;
    let vk = VerifyingKey::from_bytes(&key).map_err(|_| unauthorized("unknown engine key"))?;
    let sig: [u8; 64] = enginekey::unhex(signature).and_then(|b| b.try_into().ok()).ok_or_else(|| unauthorized("bad signature"))?;
    vk.verify(message.as_bytes(), &Signature::from_bytes(&sig)).map_err(|_| unauthorized("bad signature"))?;
    if nonce_reused(nonce) {
        return Err(unauthorized("this signed message was already used"));
    }
    Ok(())
}

/// A session for key `kid` of `engine`, signed with the engine session
/// secret, plus the TLS fingerprints the engine should trust.
fn session(st: &AppState, engine: &str, kid: &str) -> Result<Value, ApiError> {
    let claims = Claims {
        sub: format!("iter_engine:{engine}"),
        role: "engine".into(),
        tokenver: 0,
        exp: chrono::Utc::now().timestamp() as u64 + SESSION_SECS,
        kid: kid.to_string(),
    };
    let jwt = crate::auth::mint_claims(&st.engine_secret, &claims).map_err(|e| ApiError::Status(StatusCode::INTERNAL_SERVER_ERROR, e))?;
    touch(st.store.clone(), kid);
    Ok(json!({
        "token": format!("{}{jwt}", enginekey::SESSION_PREFIX), "engine": engine, "expires_at": claims.exp,
        "tls": {"mode": st.tls.mode.as_str(), "fingerprints": st.tls.announced()},
    }))
}

async fn engine_login(State(st): Ctx, Json(req): Json<LoginReq>) -> Result<Json<Value>, ApiError> {
    let row = live_key(&st, &req.key_id).await?;
    verify_signed(&row, req.ts, &req.nonce, &req.signature, &enginekey::login_message(&req.key_id, req.ts, &req.nonce, &req.tls_fp))?;
    check_seen(&st, &req.tls_fp, &req.tls_chain).await?;
    Ok(Json(session(&st, &body_str(&row, "engine"), &req.key_id)?))
}

/// When iter_data serves TLS itself, the engine must have seen one of its
/// certificates, or a TLS front's that a public certificate authority vouches
/// for under one of iter_data's names: a sign-in relayed through anything else
/// is refused. (`tls_fp` is signed by the engine; the chain's first
/// certificate must be the one it names.)
async fn check_seen(st: &AppState, tls_fp: &str, chain: &[String]) -> Result<(), ApiError> {
    let announced = st.tls.announced();
    if announced.is_empty() || announced.iter().any(|f| f == tls_fp) {
        return Ok(());
    }
    let names = public_names(st).await?;
    if front_ok(tls_fp, &decode_chain(chain), &names, &[]) {
        return Ok(());
    }
    Err(unauthorized(
        "the engine saw a TLS certificate that is not iter_data's, and no public certificate authority vouches for it under one of iter_data's names: something between them presents its own. (A TLS proxy in front with a public certificate is fine once iter_data knows the name engines use: make a join code for that URL in the Engine keys screen.)",
    ))
}

fn decode_chain(chain: &[String]) -> Vec<Vec<u8>> {
    use base64::Engine as _;
    chain.iter().take(8).filter_map(|c| base64::engine::general_purpose::STANDARD.decode(c.trim()).ok()).collect()
}

/// Is `chain` a TLS front's certificate (the one fingerprint `tls_fp` names)
/// that a public authority (or one of `extra_roots`, tests) vouches for under
/// one of `names`?
pub(crate) fn front_ok(tls_fp: &str, chain: &[Vec<u8>], names: &[String], extra_roots: &[Vec<u8>]) -> bool {
    let Some(leaf) = chain.first() else { return false };
    enginekey::fingerprint_der(leaf) == tls_fp && names.iter().any(|n| enginekey::public_chain_valid(chain, n, extra_roots).is_ok())
}

/// The names engines reach iter_data by: its own certificate's and the URL
/// host of every join code made.
async fn public_names(st: &AppState) -> Result<Vec<String>, ApiError> {
    let mut names = st.tls.names();
    if let Some(row) = st.store.get(TABLE, NAMES_PK, NAMES_SK).await? {
        names.extend(row["names"].as_array().into_iter().flatten().filter_map(|n| n.as_str().map(String::from)));
    }
    Ok(names)
}

/// Remember `name` as one engines reach iter_data by.
async fn add_public_name(st: &AppState, name: &str) -> Result<(), ApiError> {
    let row = st.store.get(TABLE, NAMES_PK, NAMES_SK).await?.unwrap_or_else(|| json!({"names": []}));
    let mut names: Vec<String> = row["names"].as_array().into_iter().flatten().filter_map(|n| n.as_str().map(String::from)).collect();
    if names.iter().any(|n| n == name) {
        return Ok(());
    }
    names.push(name.to_string());
    st.store.put(TABLE, NAMES_PK, NAMES_SK, &json!({"public_names": true, "names": names, "updated": now_utc()})).await?;
    Ok(())
}

/// The host of an http(s) URL, lowercased, without IPv6 brackets.
fn url_host(url: &str) -> String {
    let rest = url.trim().split_once("://").map(|(_, r)| r).unwrap_or(url.trim());
    let hp = rest.split(['/', '?', '#']).next().unwrap_or("");
    let hp = hp.rsplit_once('@').map(|(_, h)| h).unwrap_or(hp);
    if let Some(v6) = hp.strip_prefix('[') {
        return v6.split(']').next().unwrap_or("").to_ascii_lowercase();
    }
    hp.rsplit_once(':').map(|(h, _)| h).unwrap_or(hp).to_ascii_lowercase()
}

fn is_loopback(host: &str) -> bool {
    host == "localhost" || host.parse::<std::net::IpAddr>().is_ok_and(|ip| ip.is_loopback())
}

#[derive(serde::Deserialize)]
struct JoinReq {
    /// iter_data's URL as the engine reaches it (the webui sends its own)
    url: String,
    /// the engine's name ("" = the machine's own)
    #[serde(default)]
    engine: String,
    /// hours the code works [JOIN_HOURS], at most JOIN_MAX_HOURS
    #[serde(default)]
    hours: Option<i64>,
}

/// A join code for one new engine: the only thing carried to its machine.
async fn join_new(user: AuthUser, State(st): Ctx, Json(req): Json<JoinReq>) -> Result<Json<Value>, ApiError> {
    user.require_admin()?;
    let url = req.url.trim().trim_end_matches('/').to_string();
    let lower = url.to_ascii_lowercase();
    let host = url_host(&url);
    if host.is_empty() || !(lower.starts_with("https://") || (lower.starts_with("http://") && is_loopback(&host))) {
        return Err(bad("the URL engines use starts with https:// (plain http:// only for an engine on this same machine)"));
    }
    let engine = req.engine.trim().to_string();
    if !engine.is_empty() {
        check_engine(&engine)?;
    }
    let hours = req.hours.unwrap_or(JOIN_HOURS).clamp(1, JOIN_MAX_HOURS);
    let expires = chrono::Utc::now() + chrono::Duration::hours(hours);
    let secret = enginekey::new_join_secret();
    let id = enginekey::join_id(&secret);
    // expired codes go as new ones are made
    let now = chrono::Utc::now().timestamp();
    for r in st.store.scan(TABLE).await? {
        if r.get("join_id").is_some() && r["expires_unix"].as_i64().unwrap_or(0) < now {
            st.store.delete(TABLE, &format!("join-{}", body_str(&r, "join_id")), JOIN_SK).await?;
        }
    }
    let row = json!({
        "join_id": id, "engine": engine, "url": url, "expires": expires.format("%Y-%m-%dT%H:%M:%SZ").to_string(),
        "expires_unix": expires.timestamp(), "created": now_utc(), "created_by": user.sub,
    });
    st.store.put(TABLE, &format!("join-{id}"), JOIN_SK, &row).await?;
    if !is_loopback(&host) {
        add_public_name(&st, &host).await?;
    }
    let code = enginekey::encode_join(&enginekey::JoinCode { url: url.clone(), secret, pins: st.tls.announced(), engine: engine.clone(), expires: expires.timestamp() });
    let mut out = row;
    out["code"] = json!(code);
    Ok(Json(out))
}

async fn join_cancel(user: AuthUser, State(st): Ctx, Path(id): Path<String>) -> Result<Json<Value>, ApiError> {
    user.require_admin()?;
    if !st.store.delete(TABLE, &format!("join-{id}"), JOIN_SK).await? {
        return Err(notfound());
    }
    Ok(Json(json!({"cancelled": id})))
}

#[derive(serde::Deserialize)]
struct JoinUseReq {
    /// the join code's secret
    secret: String,
    /// the engine's own new public key
    public_key: String,
    /// the name the machine would take (used when the code names none)
    #[serde(default)]
    engine: String,
    #[serde(default)]
    tls_fp: String,
    #[serde(default)]
    tls_chain: Vec<String>,
}

/// An engine registers its key with a join code: the code is used up by the
/// first call that adds a key; the same key again is answered as done.
async fn engine_join(State(st): Ctx, Json(req): Json<JoinUseReq>) -> Result<Json<Value>, ApiError> {
    let id = enginekey::join_id(req.secret.trim());
    let pk = format!("join-{id}");
    let used = "this join code is unknown here: it was used already, cancelled, or made by another iter_data. Make a new one: Engine keys → Add engine";
    let row = st.store.get(TABLE, &pk, JOIN_SK).await?.ok_or_else(|| unauthorized(used))?;
    if row["expires_unix"].as_i64().unwrap_or(0) < chrono::Utc::now().timestamp() {
        st.store.delete(TABLE, &pk, JOIN_SK).await?;
        return Err(unauthorized("this join code has expired: make a new one (Engine keys → Add engine)"));
    }
    check_seen(&st, req.tls_fp.trim(), &req.tls_chain).await?;
    let key = enginekey::parse_public(&req.public_key).ok_or_else(|| bad("not an engine public key"))?;
    let kid = enginekey::key_id(&key);
    let fixed = body_str(&row, "engine");
    if let Some(have) = st.store.get(TABLE, &kid, NOSK).await?.filter(|r| r.get("public_key").is_some()) {
        if revoked_at(&have, &now_utc()) {
            return Err(unauthorized("this engine key was revoked: make a new one (iter_engine key new) and join again"));
        }
        let engine = body_str(&have, "engine");
        if !fixed.is_empty() && fixed != engine {
            return Err(ApiError::Status(StatusCode::CONFLICT, format!("this machine's key belongs to engine \"{engine}\", but the code is for \"{fixed}\"")));
        }
        return Ok(Json(json!({"engine": engine, "key_id": kid, "already": true})));
    }
    let wanted = if fixed.is_empty() { req.engine.trim().to_string() } else { fixed.clone() };
    check_engine(&wanted)?;
    let engine = if fixed.is_empty() { free_name(&st, &wanted).await? } else { wanted.clone() };
    // one use: only the call that removes the row adds its key
    if !st.store.delete(TABLE, &pk, JOIN_SK).await? {
        return Err(unauthorized(used));
    }
    let by = format!("join code {} of {}", &id[..8], body_str(&row, "created_by"));
    insert(&st, &by, &enginekey::format_public(&key), &engine, "joined with a code", "").await?;
    Ok(Json(json!({"engine": engine, "key_id": kid, "renamed": engine != wanted})))
}

/// `want`, or `want-2`, `want-3`, … : the first name no engine with a live key has.
async fn free_name(st: &AppState, want: &str) -> Result<String, ApiError> {
    let now = now_utc();
    let taken: std::collections::HashSet<String> = st
        .store
        .scan(TABLE)
        .await?
        .iter()
        .filter(|r| r.get("public_key").is_some() && !revoked_at(r, &now))
        .map(|r| body_str(r, "engine"))
        .collect();
    (1..100)
        .map(|i| if i == 1 { want.to_string() } else { format!("{want}-{i}") })
        .find(|n| !taken.contains(n))
        .ok_or_else(|| bad(format!("engine names {want} … {want}-99 are all taken: choose another name")))
}

#[derive(serde::Deserialize)]
struct RotateSelfReq {
    key_id: String,
    new_public_key: String,
    ts: i64,
    nonce: String,
    /// hex Ed25519 signature, by the CURRENT key, over enginekey::rotate_message
    signature: String,
}

/// An engine replaces its own key: signed by the current key, which must be
/// fully active (not revoked, not already rotating out). Answers with a
/// session of the new key.
async fn engine_rotate(State(st): Ctx, Json(req): Json<RotateSelfReq>) -> Result<Json<Value>, ApiError> {
    let mut old = live_key(&st, &req.key_id).await?;
    if !body_str(&old, "revoked").is_empty() {
        return Err(unauthorized("this key is already being replaced or revoked: it cannot rotate itself (an admin sets the engine up again)"));
    }
    let new_public = req.new_public_key.trim().to_string();
    verify_signed(&old, req.ts, &req.nonce, &req.signature, &enginekey::rotate_message(&req.key_id, &new_public, req.ts, &req.nonce))?;
    let engine = body_str(&old, "engine");
    let by = format!("iter_engine:{engine}");
    let new = insert(&st, &by, &new_public, &engine, &body_str(&old, "label"), &req.key_id).await?;
    let at = chrono::Utc::now() + chrono::Duration::seconds(ROTATE_GRACE_SECS);
    old["revoked"] = json!(at.format("%Y-%m-%dT%H:%M:%SZ").to_string());
    old["revoked_by"] = json!(by);
    old["replaced_by"] = new["id"].clone();
    st.store.put(TABLE, &req.key_id, NOSK, &old).await?;
    let mut out = session(&st, &engine, &body_str(&new, "id"))?;
    out["key_id"] = new["id"].clone();
    out["old_key_stops"] = old["revoked"].clone();
    Ok(Json(out))
}

/// The certificate iter_data serves (admins: the Engine keys screen; an
/// engine session: the fingerprints only).
async fn tls_get(user: AuthUser, State(st): Ctx) -> Result<Json<Value>, ApiError> {
    if user.role == "engine" {
        return Ok(Json(json!({"mode": st.tls.mode.as_str(), "fingerprints": st.tls.announced()})));
    }
    user.require_admin()?;
    Ok(Json(st.tls.describe()))
}

/// Make the next self-signed certificate; engines pick its fingerprint up at
/// their next sign-in (within 12 hours, or at once on a restart).
async fn tls_next(user: AuthUser, State(st): Ctx) -> Result<Json<Value>, ApiError> {
    user.require_admin()?;
    st.tls.make_next().map_err(bad)?;
    Ok(Json(st.tls.describe()))
}

/// Serve the next certificate from now on. An engine that never signed in
/// while it was announced no longer trusts the server and needs
/// `iter_engine key trust` again.
async fn tls_promote(user: AuthUser, State(st): Ctx) -> Result<Json<Value>, ApiError> {
    user.require_admin()?;
    st.tls.promote().await.map_err(bad)?;
    Ok(Json(st.tls.describe()))
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
    Ok(AuthUser { sub: claims.sub.clone(), role: claims.role.clone(), exp: claims.exp })
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
    let unix = chrono::Utc::now().timestamp();
    let joins: Vec<&Value> = rows.iter().filter(|r| r.get("join_id").is_some() && r["expires_unix"].as_i64().unwrap_or(0) >= unix).collect();
    let names: Vec<Value> = rows.iter().find(|r| r.get("public_names").is_some()).and_then(|r| r["names"].as_array().cloned()).unwrap_or_default();
    Ok(Json(json!({"keys": keys, "joins": joins, "public_names": names, "tls": st.tls.describe()})))
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

/// `iter_data engine-key add` (main.rs): registers the key of an engine on this
/// same machine without the webui. The engine installer runs it as the `iter`
/// user with iter_data's env file, so it reaches the database exactly as the
/// service does and no one's password is involved. The same key for the same
/// engine again is a no-op, so installers may rerun it.
pub async fn add_local(store: &dyn Storage, public_key: &str, engine: &str) -> Result<String, String> {
    let key = enginekey::parse_public(public_key)
        .ok_or_else(|| format!("that is not an engine public key: `iter_engine key show` prints the {}… line", enginekey::PREFIX))?;
    let id = enginekey::key_id(&key);
    if let Some(row) = store.get(TABLE, &id, NOSK).await.map_err(|e| e.to_string())? {
        if body_str(&row, "engine") != engine {
            return Err(format!("key {id} is registered already, for engine \"{}\"", body_str(&row, "engine")));
        }
        if revoked_at(&row, &now_utc()) {
            return Err(format!("key {id} of engine \"{engine}\" was revoked: make a new one (iter_engine key new)"));
        }
        return Ok(format!("key {id} of engine \"{engine}\" is registered already"));
    }
    check_engine(engine).map_err(|e| match e {
        ApiError::Status(_, m) => m,
        _ => "bad engine name".into(),
    })?;
    let row = json!({
        "id": id, "engine": engine, "label": "added on this machine by its installer", "public_key": enginekey::format_public(&key),
        "created": now_utc(), "created_by": "iter_data engine-key add", "revoked": "", "revoked_by": "",
        "replaces": "", "replaced_by": "",
    });
    store.put(TABLE, &id, NOSK, &row).await.map_err(|e| e.to_string())?;
    Ok(format!("registered key {id} of engine \"{engine}\""))
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

/// Tests: a live key row for `engine` and a session of it, the way an
/// engine that signed in holds one.
#[cfg(test)]
pub(crate) async fn test_session(st: &AppState, engine: &str) -> String {
    let id = format!("test-{engine}");
    if st.store.get(TABLE, &id, NOSK).await.unwrap().is_none() {
        let row = json!({"id": id, "engine": engine, "public_key": enginekey::format_public(&[7u8; 32]), "revoked": "", "created": now_utc()});
        st.store.put(TABLE, &id, NOSK, &row).await.unwrap();
    }
    session(st, engine, &id).unwrap()["token"].as_str().unwrap().to_string()
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

    #[test]
    fn url_hosts() {
        assert_eq!(url_host("https://Iter.Example.com:8400/x"), "iter.example.com");
        assert_eq!(url_host("https://iter.example.com"), "iter.example.com");
        assert_eq!(url_host("https://[::1]:8400"), "::1");
        assert_eq!(url_host("https://u@10.0.0.5/"), "10.0.0.5");
        assert!(is_loopback("127.0.0.1") && is_loopback("::1") && is_loopback("localhost") && !is_loopback("10.0.0.5"));
    }

    /// A TLS front's certificate is accepted only when an authority the
    /// check trusts vouches for it under one of iter_data's names, and only
    /// as the very certificate the engine signed the fingerprint of.
    #[test]
    fn a_front_needs_a_trusted_authority_and_one_of_our_names() {
        use base64::Engine as _;
        let dir = std::env::temp_dir().join(format!("iter-front-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        // a stand-in for a public authority: the local CA, trusted as an extra root
        let front = crate::tls::TlsState::setup("auto", "", "", &dir, &["iter.example.com".into()]).unwrap();
        let ders = |pem: &str| -> Vec<Vec<u8>> {
            pem.split("-----BEGIN CERTIFICATE-----")
                .skip(1)
                .map(|b| base64::engine::general_purpose::STANDARD.decode(b.split("-----END CERTIFICATE-----").next().unwrap().split_whitespace().collect::<String>()).unwrap())
                .collect()
        };
        let chain = ders(&std::fs::read_to_string(dir.join(crate::tls::CERT)).unwrap());
        let ca = ders(&front.ca_pem().unwrap());
        let fp = front.fingerprint();
        let names = vec!["iter.example.com".to_string()];
        assert!(front_ok(&fp, &chain, &names, &ca));
        assert!(front_ok(&fp, &chain[..1], &names, &ca), "the intermediates are optional when the root signed it");
        assert!(!front_ok(&fp, &chain, &["other.example.com".into()], &ca), "not one of our names");
        assert!(!front_ok(&"ab".repeat(32), &chain, &names, &ca), "not the certificate the engine signed");
        assert!(!front_ok(&fp, &chain, &names, &[]), "no public authority vouches for a local CA");
        assert!(!front_ok(&fp, &[], &names, &ca));
        assert_eq!(decode_chain(&[base64::engine::general_purpose::STANDARD.encode(&chain[0]), "!!".into()]), vec![chain[0].clone()]);
        let _ = std::fs::remove_dir_all(&dir);
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
        let msg = enginekey::login_message(&kid(k), ts, nonce, "");
        let sig: String = k.sign(msg.as_bytes()).to_bytes().iter().map(|b| format!("{b:02x}")).collect();
        json!({"key_id": kid(k), "ts": ts, "nonce": nonce, "signature": sig})
    }
    fn nonce() -> String {
        uuid::Uuid::new_v4().simple().to_string()
    }

    impl T {
        async fn new() -> T {
            let (a, db) = crate::test_db::store().await;
            let st = Arc::new(AppState::for_tests(Arc::new(a), b"keys-test"));
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

    fn rotate_body(old: &SigningKey, new_pub: &str, ts: i64) -> Value {
        let n = nonce();
        let msg = enginekey::rotate_message(&kid(old), new_pub, ts, &n);
        let sig: String = old.sign(msg.as_bytes()).to_bytes().iter().map(|b| format!("{b:02x}")).collect();
        json!({"key_id": kid(old), "new_public_key": new_pub, "ts": ts, "nonce": n, "signature": sig})
    }

    #[tokio::test]
    async fn an_engine_rotates_its_own_key_but_never_a_revoked_one_and_secrets_stay_apart() {
        let t = T::new().await;
        let k1 = new_key();
        t.admin("POST", "/api/keys", Some(json!({"public_key": pubkey(&k1), "engine": "lap1"}))).await;
        let s1 = t.login(&k1).await.unwrap();
        let (_, v) = t.call("", "POST", "/auth/engine", Some(login_body(&k1, chrono::Utc::now().timestamp(), &nonce()))).await;
        assert_eq!(v["tls"]["mode"], "off", "the sign-in answer says what TLS the server serves: {v}");

        // signed by the current key: accepted with no admin step; the new key signs in at once
        let now = chrono::Utc::now().timestamp();
        let k2 = new_key();
        // a rotation signed by some other key, or naming a different new key than it signed, is refused
        let mut forged = rotate_body(&new_key(), &pubkey(&k2), now);
        forged["key_id"] = json!(kid(&k1));
        assert_eq!(t.call("", "POST", "/auth/engine/rotate", Some(forged)).await.0, 401);
        let mut swapped = rotate_body(&k1, &pubkey(&k2), now);
        swapped["new_public_key"] = json!(pubkey(&new_key()));
        assert_eq!(t.call("", "POST", "/auth/engine/rotate", Some(swapped)).await.0, 401);
        let (c, r) = t.call("", "POST", "/auth/engine/rotate", Some(rotate_body(&k1, &pubkey(&k2), now))).await;
        assert_eq!((c, r["key_id"].as_str()), (200, Some(kid(&k2).as_str())), "{r}");
        let s_new = r["token"].as_str().unwrap().to_string();
        assert_eq!(t.call(&s_new, "GET", "/api/engines", None).await.0, 200);
        // the old key keeps working for its grace (agents still hold its sessions) …
        assert_eq!(t.call(&s1, "GET", "/api/engines", None).await.0, 200);
        let (_, list) = t.admin("GET", "/api/keys", None).await;
        let old = list["keys"].as_array().unwrap().iter().find(|k| k["id"] == kid(&k1)).unwrap().clone();
        assert_eq!((old["status"].as_str(), old["replaced_by"].as_str()), (Some("expiring"), Some(kid(&k2).as_str())), "{old}");
        // … but a key on its way out cannot rotate again
        assert_eq!(t.call("", "POST", "/auth/engine/rotate", Some(rotate_body(&k1, &pubkey(&new_key()), now))).await.0, 401);
        // a revoked key never rotates itself: it is assumed compromised
        assert_eq!(t.admin("POST", &format!("/api/keys/{}/revoke", kid(&k2)), None).await.0, 200);
        assert_eq!(t.call("", "POST", "/auth/engine/rotate", Some(rotate_body(&k2, &pubkey(&new_key()), now))).await.0, 401);
        assert_eq!(t.call(&s_new, "GET", "/api/engines", None).await.0, 401);

        // engines and people never share a signing secret: an engine-shaped
        // session signed with the user secret, and a user token with role engine,
        // are both refused
        let k3 = new_key();
        t.admin("POST", "/api/keys", Some(json!({"public_key": pubkey(&k3), "engine": "lap2"}))).await;
        let claims = crate::auth::Claims { sub: "iter_engine:lap2".into(), role: "engine".into(), tokenver: 0, exp: now as u64 + 600, kid: kid(&k3) };
        let wrong = format!("{}{}", enginekey::SESSION_PREFIX, crate::auth::mint_claims(&t.st.secret, &claims).unwrap());
        assert_eq!(t.call(&wrong, "GET", "/api/engines", None).await.0, 401);
        let right = format!("{}{}", enginekey::SESSION_PREFIX, crate::auth::mint_claims(&t.st.engine_secret, &claims).unwrap());
        assert_eq!(t.call(&right, "GET", "/api/engines", None).await.0, 200);
        assert_eq!(t.call(&t.signed("alice", "engine"), "GET", "/api/engines", None).await.0, 401, "a user token claiming role engine");
        assert_eq!(t.admin("PUT", "/api/users/bot", Some(json!({"role": "engine", "password": "x"}))).await.0, 400, "no engine users");
        // the certificate endpoints: admins see the details, engines the fingerprints, others nothing
        assert_eq!(t.admin("GET", "/api/server/tls", None).await.1["mode"], "off");
        assert!(t.call(&right, "GET", "/api/server/tls", None).await.1["fingerprints"].is_array());
        assert_eq!(t.call(&t.signed("alice", "user"), "GET", "/api/server/tls", None).await.0, 403);
        assert_eq!(t.admin("POST", "/api/server/tls/next", None).await.0, 400, "nothing to rotate without --tls auto");
        crate::test_db::drop(&t.db).await;
    }

    fn secret_of(code: &str) -> String {
        enginekey::decode_join(code).unwrap().secret
    }

    #[tokio::test]
    async fn a_join_code_registers_one_engine_key_once() {
        let t = T::new().await;
        let alice = t.signed("alice", "user");
        // only an admin makes one, for an https URL (plain http only to this machine)
        let new_code = |engine: &str| json!({"url": "https://iter.example.com/", "engine": engine});
        assert_eq!(t.call(&alice, "POST", "/api/keys/join", Some(new_code(""))).await.0, 403);
        assert_eq!(t.admin("POST", "/api/keys/join", Some(json!({"url": "http://10.0.0.5:8400"}))).await.0, 400);
        assert_eq!(t.admin("POST", "/api/keys/join", Some(json!({"url": "iter.example.com"}))).await.0, 400);
        assert_eq!(t.admin("POST", "/api/keys/join", Some(new_code("a b"))).await.0, 400);
        assert_eq!(t.admin("POST", "/api/keys/join", Some(json!({"url": "http://127.0.0.1:8400"}))).await.0, 200);
        let (c, made) = t.admin("POST", "/api/keys/join", Some(new_code(""))).await;
        assert_eq!(c, 200, "{made}");
        let code = enginekey::decode_join(made["code"].as_str().unwrap()).unwrap();
        assert_eq!((code.url.as_str(), code.engine.as_str(), code.pins.len()), ("https://iter.example.com", "", 0), "no TLS of its own here: no pins");
        assert!(code.expires > chrono::Utc::now().timestamp() + 23 * 3600);
        // iter_data keeps an id of the secret, never the secret; the URL's host is now one of its names
        let (_, list) = t.admin("GET", "/api/keys", None).await;
        assert!(!list.to_string().contains(&code.secret), "{list}");
        assert_eq!(list["joins"].as_array().unwrap().len(), 2, "{list}");
        assert_eq!(list["public_names"], json!(["iter.example.com"]), "{list}");

        // a wrong secret registers nothing
        let k1 = new_key();
        let use_code = |secret: &str, k: &SigningKey, engine: &str| json!({"secret": secret, "public_key": pubkey(k), "engine": engine});
        assert_eq!(t.call("", "POST", "/auth/engine/join", Some(use_code(&enginekey::new_join_secret(), &k1, "lap1"))).await.0, 401);
        assert_eq!(t.login(&k1).await, Err(401));
        // the code registers the key under the machine's name; the key signs in
        let (c, v) = t.call("", "POST", "/auth/engine/join", Some(use_code(&code.secret, &k1, "lap1"))).await;
        assert_eq!((c, v["engine"].as_str(), v["already"].as_bool()), (200, Some("lap1"), None), "{v}");
        assert!(t.login(&k1).await.is_ok());
        // once: a used code is gone, for any key
        assert_eq!(t.call("", "POST", "/auth/engine/join", Some(use_code(&code.secret, &new_key(), "lap9"))).await.0, 401);
        assert_eq!(t.call("", "POST", "/auth/engine/join", Some(use_code(&code.secret, &k1, "lap1"))).await.0, 401);
        // a key registered already (an installer run twice) is answered as done, and the code stays unused
        let (_, again) = t.admin("POST", "/api/keys/join", Some(new_code(""))).await;
        let s2 = secret_of(again["code"].as_str().unwrap());
        let (c, v) = t.call("", "POST", "/auth/engine/join", Some(use_code(&s2, &k1, "lap1"))).await;
        assert_eq!((c, v["already"].as_bool(), v["engine"].as_str()), (200, Some(true), Some("lap1")), "{v}");
        // a name with a live key is not taken over: the next machine called lap1 becomes lap1-2
        let k2 = new_key();
        let (c, v) = t.call("", "POST", "/auth/engine/join", Some(use_code(&s2, &k2, "lap1"))).await;
        assert_eq!((c, v["engine"].as_str(), v["renamed"].as_bool()), (200, Some("lap1-2"), Some(true)), "{v}");
        // a code that names its engine registers that name, whatever the machine asks for
        let (_, named) = t.admin("POST", "/api/keys/join", Some(new_code("attic"))).await;
        let k3 = new_key();
        let (c, v) = t.call("", "POST", "/auth/engine/join", Some(use_code(&secret_of(named["code"].as_str().unwrap()), &k3, "whatever"))).await;
        assert_eq!((c, v["engine"].as_str()), (200, Some("attic")), "{v}");
        // a machine whose key is another engine's cannot use a code for a named engine
        let (_, named) = t.admin("POST", "/api/keys/join", Some(new_code("cellar"))).await;
        assert_eq!(t.call("", "POST", "/auth/engine/join", Some(use_code(&secret_of(named["code"].as_str().unwrap()), &k3, "x"))).await.0, 409);
        // a cancelled code and an expired one register nothing
        let (_, gone) = t.admin("POST", "/api/keys/join", Some(new_code(""))).await;
        assert_eq!(t.admin("DELETE", &format!("/api/keys/join/{}", gone["join_id"].as_str().unwrap()), None).await.0, 200);
        assert_eq!(t.call("", "POST", "/auth/engine/join", Some(use_code(&secret_of(gone["code"].as_str().unwrap()), &new_key(), "z"))).await.0, 401);
        let (_, old) = t.admin("POST", "/api/keys/join", Some(new_code(""))).await;
        let pk = format!("join-{}", old["join_id"].as_str().unwrap());
        let mut row = t.st.store.get(super::TABLE, &pk, super::JOIN_SK).await.unwrap().unwrap();
        row["expires_unix"] = json!(chrono::Utc::now().timestamp() - 1);
        t.st.store.put(super::TABLE, &pk, super::JOIN_SK, &row).await.unwrap();
        let (c, v) = t.call("", "POST", "/auth/engine/join", Some(use_code(&secret_of(old["code"].as_str().unwrap()), &new_key(), "z"))).await;
        assert_eq!(c, 401);
        assert!(v["error"].as_str().unwrap().contains("expired"), "{v}");
        // the keys a code added are listed like any other, saying where they came from
        let (_, list) = t.admin("GET", "/api/keys", None).await;
        let row = list["keys"].as_array().unwrap().iter().find(|k| k["engine"] == "attic").unwrap().clone();
        assert!(row["created_by"].as_str().unwrap().starts_with("join code "), "{row}");
        crate::test_db::drop(&t.db).await;
    }

    /// With TLS of its own, iter_data refuses a sign-in or a join from an
    /// engine that saw some other certificate (no front it can vouch for).
    #[tokio::test]
    async fn a_certificate_in_between_is_refused() {
        let (a, db) = crate::test_db::store().await;
        let dir = std::env::temp_dir().join(format!("iter-seen-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut st = AppState::for_tests(Arc::new(a), b"keys-test");
        st.tls = Arc::new(crate::tls::TlsState::setup("auto", "", "", &dir, &[]).unwrap());
        let st = Arc::new(st);
        st.store.put("webui_user", "adm", NOSK, &json!({"user": "adm", "role": "admin", "tokenver": 1, "pwhash": ""})).await.unwrap();
        let t = T { app: crate::api::router(st.clone()), st, db };
        let k = new_key();
        t.admin("POST", "/api/keys", Some(json!({"public_key": pubkey(&k), "engine": "lap1"}))).await;
        let signed = |fp: &str| {
            let n = nonce();
            let ts = chrono::Utc::now().timestamp();
            let sig: String = k.sign(enginekey::login_message(&kid(&k), ts, &n, fp).as_bytes()).to_bytes().iter().map(|b| format!("{b:02x}")).collect();
            json!({"key_id": kid(&k), "ts": ts, "nonce": n, "tls_fp": fp, "signature": sig})
        };
        let (c, v) = t.call("", "POST", "/auth/engine", Some(signed(&"ab".repeat(32)))).await;
        assert_eq!(c, 401);
        assert!(v["error"].as_str().unwrap().contains("not iter_data's"), "{v}");
        assert_eq!(t.call("", "POST", "/auth/engine", Some(signed(&t.st.tls.fingerprint()))).await.0, 200);
        // a join made over such a connection is refused before the code is used
        let (_, made) = t.admin("POST", "/api/keys/join", Some(json!({"url": "https://iter.example.com"}))).await;
        let code = enginekey::decode_join(made["code"].as_str().unwrap()).unwrap();
        assert_eq!(code.pins, t.st.tls.announced(), "the code names iter_data's own certificate");
        let body = |fp: &str| json!({"secret": code.secret, "public_key": pubkey(&new_key()), "engine": "lap2", "tls_fp": fp});
        assert_eq!(t.call("", "POST", "/auth/engine/join", Some(body(&"ab".repeat(32)))).await.0, 401);
        assert_eq!(t.call("", "POST", "/auth/engine/join", Some(body(&t.st.tls.fingerprint()))).await.0, 200, "the code was still there");
        let _ = std::fs::remove_dir_all(&dir);
        crate::test_db::drop(&t.db).await;
    }

    #[tokio::test]
    async fn a_local_engine_is_registered_without_the_webui_and_reruns_are_no_ops() {
        let t = T::new().await;
        let k = new_key();
        let store = t.st.store.as_ref();
        assert!(super::add_local(store, "iter_nope", "lap9").await.is_err());
        assert!(super::add_local(store, &pubkey(&k), "a b").await.is_err());
        assert!(super::add_local(store, &pubkey(&k), "lap9").await.unwrap().starts_with("registered key"));
        // it signs in like a key an admin added
        assert!(t.login(&k).await.is_ok());
        assert!(super::add_local(store, &pubkey(&k), "lap9").await.unwrap().contains("registered already"), "a rerun");
        assert!(super::add_local(store, &pubkey(&k), "other").await.unwrap_err().contains("for engine \"lap9\""));
        assert_eq!(t.admin("POST", &format!("/api/keys/{}/revoke", kid(&k)), None).await.0, 200);
        assert!(super::add_local(store, &pubkey(&k), "lap9").await.unwrap_err().contains("revoked"), "a revoked key stays revoked");
        crate::test_db::drop(&t.db).await;
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
        assert_eq!(list["tls"]["mode"], "off");

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
