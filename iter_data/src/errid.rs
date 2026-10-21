//! Error ids (MCP/GraphRAG hardening, MG-16): a response never shows
//! internal detail (database errors, library error names, server paths).
//! The detail is logged once, on a line carrying an `error_id`, and the
//! caller gets `{"error": "<generic message>", "error_id": "e-<12 hex>"}` to
//! quote when asking about it.
//!
//! `ApiError::Internal` carries the detail out of a handler; `layer`
//! (outermost in the router, where the method and path are known) logs it
//! and writes the generic body.

use axum::extract::Request;
use axum::http::StatusCode;
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::json;

/// The generic text of every internal error.
pub const INTERNAL: &str = "internal error";

/// What an internal-error response carries to `layer`: the generic message
/// the caller sees and the detail only the log gets.
#[derive(Clone)]
pub struct Detail {
    pub public: String,
    pub detail: String,
}

/// Log `detail` under a new error id; returns the id.
pub fn record(detail: &str, method: &str, path: &str) -> String {
    let id = format!("e-{}", &uuid::Uuid::new_v4().simple().to_string()[..12]);
    eprintln!("[iter_data] error {id} {method} {path}: {detail}");
    #[cfg(test)]
    if let Ok(mut r) = ring().lock() {
        r.push(format!("{id} {method} {path}: {detail}"));
        if r.len() > 1000 {
            r.remove(0);
        }
    }
    id
}

/// The generic body for an internal error with this id.
pub fn body(message: &str, id: &str) -> serde_json::Value {
    json!({"error": message, "error_id": id})
}

/// The logged lines (tests read them).
#[cfg(test)]
pub fn ring() -> &'static std::sync::Mutex<Vec<String>> {
    static R: std::sync::OnceLock<std::sync::Mutex<Vec<String>>> = std::sync::OnceLock::new();
    R.get_or_init(Default::default)
}

/// An error leaving a handler: the body is `public`, the detail rides in an
/// extension until `layer` logs it and adds the id.
pub fn response(status: StatusCode, public: &str, detail: String) -> Response {
    let mut r = (status, Json(json!({"error": public}))).into_response();
    r.extensions_mut().insert(Detail { public: public.to_string(), detail });
    r
}

/// Logs each internal error's detail with the request's method and path and
/// gives the response its `error_id`.
pub async fn layer(req: Request, next: Next) -> Response {
    let (method, path) = (req.method().to_string(), req.uri().path().to_string());
    let resp = next.run(req).await;
    let Some(d) = resp.extensions().get::<Detail>().cloned() else { return resp };
    let id = record(&d.detail, &method, &path);
    (resp.status(), Json(body(&d.public, &id))).into_response()
}
