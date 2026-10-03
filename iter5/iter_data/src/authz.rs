//! Project authorization (iter5 spec §4.4), applied in ONE place: a
//! router-level middleware keyed on the project segment of the path, so every
//! `/api/projects/{p}/…` route — work items, locks, graph, rag, datasync,
//! anything added later — and MCP (which replays through the same router)
//! inherits it without the handler knowing.
//!
//! Rules:
//! - admin: every project.
//! - engine token (role `engine`): only projects with an active `serves`
//!   edge from one of the token subject's engines (engine record `user` ==
//!   sub, or an active `owns` edge user:<sub> → iter_engine:<e>).
//! - any other user: an active `member` edge user:<sub> → project:<p>; a
//!   member with role `viewer` may only read (GET/HEAD).
//! - creating a project that does not exist yet (PUT /api/projects/{p}) is
//!   left to the handler (which decides who may create).
//!
//! Authentication itself (missing/bad/revoked token → 401) stays with the
//! `AuthUser` extractor: when the token does not verify here the request is
//! passed on untouched and the handler refuses it.

use crate::api::{AppState, NOSK};
use crate::storage::body_u64;
use axum::extract::{Request, State};
use axum::http::{Method, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::json;
use std::sync::Arc;

/// The project a path is scoped to: `/api/projects/{p}[/…]` or
/// `/api/prepostwork/{p}/…` (percent-decoded). None for every other path.
pub fn project_of_path(path: &str) -> Option<String> {
    let rest = path.strip_prefix("/api/projects/").or_else(|| path.strip_prefix("/api/prepostwork/"))?;
    let seg = rest.split('/').next().unwrap_or("");
    if seg.is_empty() {
        return None;
    }
    Some(percent_decode(seg))
}

fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            if let Ok(v) = u8::from_str_radix(&s[i + 1..i + 3], 16) {
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

fn deny(msg: String) -> Response {
    (StatusCode::FORBIDDEN, Json(json!({"error": msg}))).into_response()
}

/// The decision for one verified caller on one project.
pub async fn check_project(st: &AppState, sub: &str, role: &str, project: &str, method: &Method) -> Result<(), String> {
    if role == "admin" {
        return Ok(());
    }
    let store = st.store.as_ref();
    let edges = crate::settings::load_edges(store).await.map_err(|e| e.to_string())?;
    if role == "engine" {
        let engines = crate::settings::engines_of_user(store, &edges, sub).await.map_err(|e| e.to_string())?;
        if engines.iter().any(|e| crate::settings::serves_edge(&edges, e, project).is_some()) {
            return Ok(());
        }
        return Err(format!("forbidden: no engine of '{sub}' serves project '{project}' (an active serves edge is needed)"));
    }
    match crate::settings::member_role(&edges, sub, project) {
        Some(r) if r == "viewer" && !(method == Method::GET || method == Method::HEAD) => {
            Err(format!("forbidden: '{sub}' is a viewer of project '{project}'"))
        }
        Some(_) => Ok(()),
        None => Err(format!("forbidden: '{sub}' is not a member of project '{project}'")),
    }
}

pub async fn project_authz(State(st): State<Arc<AppState>>, req: Request, next: Next) -> Response {
    let Some(project) = project_of_path(req.uri().path()) else {
        return next.run(req).await;
    };
    let token = req
        .headers()
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|h| h.strip_prefix("Bearer "))
        .unwrap_or("")
        .to_string();
    let Ok(claims) = crate::auth::verify_token(&st.secret, &token) else {
        return next.run(req).await; // the handler's extractor answers 401
    };
    // a revoked token or unknown user: also the extractor's 401
    match st.store.get("webui_user", &claims.sub, NOSK).await {
        Ok(Some(row)) if body_u64(&row, "tokenver") == claims.tokenver => {}
        _ => return next.run(req).await,
    }
    // creating a project that does not exist yet: the handler decides
    let is_project_root = req.uri().path().trim_end_matches('/').matches('/').count() == 3
        && req.uri().path().starts_with("/api/projects/");
    if is_project_root && (req.method() == Method::PUT || req.method() == Method::POST) {
        if let Ok(None) = st.store.get("project", &project, NOSK).await {
            return next.run(req).await;
        }
    }
    match check_project(&st, &claims.sub, &claims.role, &project, req.method()).await {
        Ok(()) => next.run(req).await,
        Err(msg) => deny(msg),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn project_segment() {
        assert_eq!(project_of_path("/api/projects/pdy/workitems"), Some("pdy".into()));
        assert_eq!(project_of_path("/api/projects/pdy"), Some("pdy".into()));
        assert_eq!(project_of_path("/api/projects/my%20proj/graph"), Some("my proj".into()));
        assert_eq!(project_of_path("/api/prepostwork/pdy/x"), Some("pdy".into()));
        assert_eq!(project_of_path("/api/projects"), None);
        assert_eq!(project_of_path("/api/projects/"), None);
        assert_eq!(project_of_path("/api/engines/x"), None);
        assert_eq!(percent_decode("a%2"), "a%2");
    }
}
