//! Display names in URL paths. Every settings record is keyed by its stable
//! id (iter_core::settings); its `name` can be renamed. A path segment that
//! names a record by its display name (`/api/projects/My App/…`,
//! `/api/engines/Beast/heartbeat`, `/api/settings/nodes/user:Ann`) is
//! rewritten to the record's id before routing, so every handler, the authz
//! layer and every row keyed by the id see the id. A segment that is already
//! an id, or that names nothing, passes through unchanged.
//!
//! `rewrite` wraps the whole app in main.rs (a URI rewrite has to run before
//! routing); MCP replays its calls through the bare router, so it resolves
//! its paths with `resolve_path` itself.

use crate::api::AppState;
use crate::storage::Storage;
use axum::extract::{Request, State};
use axum::http::Uri;
use axum::middleware::Next;
use axum::response::Response;
use std::collections::HashMap;
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, Instant};

/// `/api/<collection>/{key}` → the table the key belongs to.
const COLLECTIONS: &[(&str, &str)] = &[
    ("projects", "project"),
    ("prepostwork", "project"),
    ("engines", "engine"),
    ("users", "webui_user"),
    ("agents", "agent"),
    ("tooling", "agent_tooling"),
];

/// Ids seen to exist recently: most requests name a record by its id, and
/// this spares them a lookup. Only hits are kept, for a short while, so a
/// deleted record is forgotten soon (and a stale hit only skips a rewrite).
static KNOWN: LazyLock<Mutex<HashMap<(String, String), Instant>>> = LazyLock::new(|| Mutex::new(HashMap::new()));
const KNOWN_TTL: Duration = Duration::from_secs(30);

fn known(table: &str, key: &str) -> bool {
    let mut m = KNOWN.lock().unwrap();
    match m.get(&(table.to_string(), key.to_string())) {
        Some(t) if t.elapsed() < KNOWN_TTL => true,
        Some(_) => {
            m.remove(&(table.to_string(), key.to_string()));
            false
        }
        None => false,
    }
}

fn remember(table: &str, key: &str) {
    let mut m = KNOWN.lock().unwrap();
    if m.len() > 10_000 {
        m.clear();
    }
    m.insert((table.to_string(), key.to_string()), Instant::now());
}

fn decode(seg: &str) -> String {
    let b = seg.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            if let Some(v) = std::str::from_utf8(&b[i + 1..i + 3]).ok().and_then(|h| u8::from_str_radix(h, 16).ok()) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn encode(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || b"-_.~:".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// The id behind a display name in `table`, or None when `key` is an id
/// already or names nothing.
async fn id_for(store: &dyn Storage, table: &str, key: &str) -> Option<String> {
    if key.is_empty() || known(table, key) {
        return None;
    }
    let id = crate::settings::resolve_key(store, table, key).await.ok()??;
    remember(table, &id);
    if id == key { None } else { Some(id) }
}

/// `path` with a display-name segment replaced by the record's id; None
/// when nothing needs replacing.
pub async fn resolve_path(store: &dyn Storage, path: &str) -> Option<String> {
    let mut segs: Vec<String> = path.split('/').map(String::from).collect();
    // ["", "api", <collection>, <key>, …]
    if segs.len() < 4 || segs[1] != "api" {
        return None;
    }
    if segs[2] == "settings" {
        if segs.len() < 5 || segs[3] != "nodes" {
            return None;
        }
        let r = decode(&segs[4]);
        let (t, key) = iter_core::settings::parse_node_id(&r)?;
        let table = iter_core::settings::table_of(t)?;
        let id = id_for(store, table, key).await?;
        segs[4] = encode(&iter_core::settings::node_id(t, &id));
        return Some(segs.join("/"));
    }
    let table = COLLECTIONS.iter().find(|(c, _)| *c == segs[2]).map(|(_, t)| *t)?;
    let id = id_for(store, table, &decode(&segs[3])).await?;
    segs[3] = encode(&id);
    Some(segs.join("/"))
}

/// Middleware (outermost, before routing): rewrite a display name in the
/// path to its id.
pub async fn rewrite(State(st): State<Arc<AppState>>, mut req: Request, next: Next) -> Response {
    if let Some(p) = resolve_path(st.store.as_ref(), req.uri().path()).await {
        let pq = match req.uri().query() {
            Some(q) => format!("{p}?{q}"),
            None => p,
        };
        if let Ok(u) = pq.parse::<Uri>() {
            *req.uri_mut() = u;
        }
    }
    next.run(req).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn percent_coding_round_trips() {
        assert_eq!(decode("My%20App"), "My App");
        assert_eq!(decode("a%2Fb"), "a/b");
        assert_eq!(decode("100%"), "100%");
        assert_eq!(decode("%zz"), "%zz");
        assert_eq!(encode("My App"), "My%20App");
        assert_eq!(encode("project:p1"), "project:p1");
    }
}
