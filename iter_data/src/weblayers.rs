//! The HTTP layers around every route (MCP/GraphRAG hardening, MG-16, MG-18,
//! MG-20): error ids, the origin guard, CORS and HSTS, applied by `apply` to
//! the whole app in main.rs and to the router the tests drive, so the tests
//! see the layers as deployed.
//!
//! Origins (Stephen, 2026-10-21): open by default, restricted only by one
//! admin setting, never in the way of agents, never tied to a server name.
//! - no `Origin` (every agent that is not a browser): allowed;
//! - `Origin` naming the host the request was addressed to (the webui on its
//!   own server, whatever it is called today): allowed;
//! - `allowed_origins` empty (the default): every other origin allowed too;
//! - otherwise an origin not in the list (the literal `null` included) gets
//!   403: on `/mcp` a JSON-RPC error without an id, elsewhere
//!   `{"error": "origin not allowed"}`.
//!
//! This departs, on purpose, from MCP's "MUST validate Origin": that rule
//! defends servers that trust cookies or a local address, and iter_data
//! trusts only a bearer token a browser never adds by itself. The list lives
//! in the server record (`webui` / `server`, beside the server switch), is
//! edited on the Settings page or with `GET`/`PUT /api/server/origins`
//! (admins), and is re-read within 15 s of a change.
//!
//! HSTS: on HTTPS every response says `Strict-Transport-Security:
//! max-age=31536000` (no includeSubDomains, no preload); a proxy in front
//! passes it through.

use crate::api::{ApiError, AppState, AuthUser, GLOBAL, NOSK, bad};
use axum::extract::{Request, State};
use axum::http::{HeaderMap, HeaderName, HeaderValue, Method, StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use serde_json::{Value, json};
use std::sync::{Arc, RwLock};
use std::time::Duration;
use tower_http::cors::{AllowOrigin, CorsLayer};

/// How often the origins setting is re-read from the store.
pub const REFRESH: Duration = Duration::from_secs(15);
pub const HSTS: &str = "max-age=31536000";
const SERVER_TABLE: &str = "webui";
const SERVER_PK: &str = "server";

/// What the layers read on every request.
pub struct WebCfg {
    origins: RwLock<Vec<String>>,
    pub tls_on: bool,
}

impl WebCfg {
    /// Every origin allowed (the default).
    pub fn open(tls_on: bool) -> Arc<WebCfg> {
        Arc::new(WebCfg { origins: RwLock::new(vec![]), tls_on })
    }
    pub fn origins(&self) -> Vec<String> {
        self.origins.read().map(|o| o.clone()).unwrap_or_default()
    }
    fn set(&self, list: Vec<String>) {
        if let Ok(mut o) = self.origins.write() {
            *o = list;
        }
    }
    /// Re-read the setting from the server record.
    pub async fn reload(&self, store: &dyn crate::storage::Storage) {
        if let Ok(row) = store.get(SERVER_TABLE, SERVER_PK, NOSK).await {
            self.set(stored_list(row.as_ref()));
        }
    }
}

fn stored_list(row: Option<&Value>) -> Vec<String> {
    row.and_then(|r| r.get("allowed_origins")).and_then(|v| v.as_array()).map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect()).unwrap_or_default()
}

/// Keep `cfg` in step with the stored setting (every REFRESH).
pub fn spawn_refresh(cfg: Arc<WebCfg>, store: Arc<dyn crate::storage::Storage>) {
    tokio::spawn(async move {
        loop {
            cfg.reload(store.as_ref()).await;
            tokio::time::sleep(REFRESH).await;
        }
    });
}

/// The MG-18 rule for one request.
pub fn origin_allowed(origin: Option<&str>, host: Option<&str>, list: &[String]) -> bool {
    let Some(origin) = origin else { return true };
    // the origin the request was addressed to: the webui calling its own server
    let authority = origin.split_once("://").map(|(_, a)| a).unwrap_or("");
    if let Some(h) = host {
        if !authority.is_empty() && authority.eq_ignore_ascii_case(h) {
            return true;
        }
    }
    list.is_empty() || list.iter().any(|o| o == origin)
}

fn hstr<'a>(h: &'a HeaderMap, k: HeaderName) -> Option<&'a str> {
    h.get(k).and_then(|v| v.to_str().ok())
}

async fn origin_guard(State(cfg): State<Arc<WebCfg>>, req: Request, next: Next) -> Response {
    let h = req.headers();
    if origin_allowed(hstr(h, header::ORIGIN), hstr(h, header::HOST), &cfg.origins()) {
        return next.run(req).await;
    }
    if req.uri().path() == "/mcp" {
        let body = json!({"jsonrpc": "2.0", "id": null, "error": {"code": -32600, "message": "origin not allowed"}});
        return (StatusCode::FORBIDDEN, Json(body)).into_response();
    }
    (StatusCode::FORBIDDEN, Json(json!({"error": "origin not allowed"}))).into_response()
}

async fn hsts(req: Request, next: Next) -> Response {
    let mut resp = next.run(req).await;
    resp.headers_mut().entry(header::STRICT_TRANSPORT_SECURITY).or_insert(HeaderValue::from_static(HSTS));
    resp
}

fn cors(cfg: Arc<WebCfg>) -> CorsLayer {
    let headers = ["authorization", "content-type", "accept", "mcp-protocol-version", "mcp-session-id", "last-event-id", "x-iter-project", "x-iter-workid"];
    CorsLayer::new()
        // the guard has refused every other origin already; echo the one that came
        .allow_origin(AllowOrigin::predicate(move |origin: &HeaderValue, parts: &axum::http::request::Parts| {
            origin_allowed(origin.to_str().ok(), hstr(&parts.headers, header::HOST), &cfg.origins())
        }))
        .allow_methods([Method::GET, Method::POST, Method::PUT, Method::PATCH, Method::DELETE, Method::OPTIONS])
        .allow_headers(headers.map(HeaderName::from_static))
        .expose_headers([HeaderName::from_static("mcp-session-id"), HeaderName::from_static("mcp-protocol-version")])
        .max_age(Duration::from_secs(600))
}

/// Every layer, around the whole app (outermost last).
pub fn apply(app: Router, cfg: Arc<WebCfg>) -> Router {
    let mut app = app
        .layer(axum::middleware::from_fn(crate::errid::layer))
        .layer(cors(cfg.clone()))
        .layer(axum::middleware::from_fn_with_state(cfg.clone(), origin_guard));
    if cfg.tls_on {
        app = app.layer(axum::middleware::from_fn(hsts));
    }
    app
}

// ---------- the setting ----------

pub fn routes() -> Router<Arc<AppState>> {
    Router::new().route("/api/server/origins", get(origins_get).put(origins_put))
}

async fn origins_get(u: AuthUser, State(st): State<Arc<AppState>>) -> Result<Json<Value>, ApiError> {
    u.require_admin()?;
    let row = st.store.get(SERVER_TABLE, SERVER_PK, NOSK).await?;
    Ok(Json(json!({"allowed_origins": stored_list(row.as_ref())})))
}

/// `scheme://host[:port]`, nothing more (no path, query or trailing slash).
pub fn check_origin(o: &str) -> Result<String, String> {
    let o = o.trim();
    let Some((scheme, rest)) = o.split_once("://") else { return Err(format!("{o:?} is not scheme://host[:port]")) };
    let host_ok = !rest.is_empty() && !rest.contains(['/', '?', '#', ' ', '@']);
    let port_ok = rest.rsplit_once(':').map(|(h, p)| !h.is_empty() && !p.is_empty() && p.chars().all(|c| c.is_ascii_digit())).unwrap_or(true);
    if !["http", "https"].contains(&scheme) || !host_ok || !port_ok {
        return Err(format!("{o:?} is not scheme://host[:port]"));
    }
    Ok(o.to_string())
}

#[derive(serde::Deserialize)]
struct OriginsReq {
    allowed_origins: Vec<String>,
}

async fn origins_put(u: AuthUser, State(st): State<Arc<AppState>>, Json(req): Json<OriginsReq>) -> Result<Json<Value>, ApiError> {
    u.require_admin()?;
    let mut list: Vec<String> = Vec::new();
    for o in req.allowed_origins.iter().filter(|o| !o.trim().is_empty()) {
        let o = check_origin(o).map_err(|e| bad(format!("allowed_origins: {e}")))?;
        if !list.contains(&o) {
            list.push(o);
        }
    }
    let mut row = crate::switches::server_row(st.store.as_ref()).await?;
    row["allowed_origins"] = json!(list);
    row["allowed_origins_by"] = json!(u.sub);
    st.store.put(SERVER_TABLE, SERVER_PK, NOSK, &row).await?;
    st.store.bump_seq(GLOBAL, SERVER_TABLE).await?;
    st.web.set(list.clone());
    Ok(Json(json!({"allowed_origins": list})))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_origin_rule() {
        let list = vec!["https://ok.example".to_string()];
        assert!(origin_allowed(None, Some("iter.local"), &list), "no Origin: an agent");
        assert!(origin_allowed(Some("https://iter.local:8400"), Some("iter.local:8400"), &list), "its own webui");
        assert!(origin_allowed(Some("https://evil.example"), Some("iter.local"), &[]), "empty list: open");
        assert!(origin_allowed(Some("https://ok.example"), Some("iter.local"), &list));
        assert!(!origin_allowed(Some("https://evil.example"), Some("iter.local"), &list));
        assert!(!origin_allowed(Some("null"), Some("iter.local"), &list));
    }

    #[test]
    fn origins_are_scheme_host_port() {
        assert!(check_origin("https://a.example").is_ok() && check_origin("http://10.0.0.5:8400").is_ok());
        for bad in ["a.example", "https://a.example/", "https://a.example/x", "ftp://a", "https://", "https://a:x", "null"] {
            assert!(check_origin(bad).is_err(), "{bad}");
        }
    }
}
