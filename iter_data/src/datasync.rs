//! Datasync (iter4, decided 2026-09-29; iter5: GraphRAG file operations
//! only — node edits go through the project graph store, nodes.rs /
//! filesync.rs): repo file operations that are not node files (storing an
//! uploaded document, removing it, a `.gitignore` line) are accepted at once
//! as *pending* rows here, then applied by whichever engine serving the
//! project gets to them first. They do not queue behind agent
//! work: the heartbeat reply tells each engine how many are waiting
//! (`datasync_waiting`), the engine claims one (a versioned write, so exactly
//! one engine wins; a claim lapses after CLAIM_SEC so a dead engine's claim
//! frees itself), applies it in its checkout once no running item's lock
//! overlaps it, commits just those files, pushes, and reports back here.
//!
//! Row (table `datasync`, pk project, sk id — time-ordered):
//! {id, project, op, lockdirs, summary, state: pending|claimed|applied|failed,
//!  created, by, engine, claim_expires, attempts, applied_at, commit, files, error, note}

use crate::api::{ApiError, AppState, AuthUser};
use crate::storage::{Storage, StorageError};
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use iter_core::now_utc;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::sync::Arc;

type Ctx = State<Arc<AppState>>;
const TABLE: &str = "datasync";
/// A claim older than this is free again (the claiming engine died or lost its network).
pub const CLAIM_SEC: i64 = 600;

pub fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/api/projects/{name}/datasync", get(list))
        .route("/api/projects/{name}/datasync/{id}/claim", post(claim))
        .route("/api/projects/{name}/datasync/{id}/done", post(done))
}

fn s<'a>(v: &'a Value, k: &str) -> &'a str {
    v.get(k).and_then(|x| x.as_str()).unwrap_or("")
}

fn iso_in(sec: i64) -> String {
    (chrono::Utc::now() + chrono::Duration::seconds(sec)).format("%Y-%m-%dT%H:%M:%SZ").to_string()
}

/// Pending, or claimed by an engine whose claim has lapsed.
pub fn claimable(row: &Value, now: &str) -> bool {
    match s(row, "state") {
        "pending" => true,
        "claimed" => !s(row, "claim_expires").is_empty() && s(row, "claim_expires") < now,
        _ => false,
    }
}

/// Store a new pending edit; returns the row.
pub async fn create(store: &dyn Storage, project: &str, op: &Value, by: &str) -> Result<Value, StorageError> {
    if !OPS.contains(&s(op, "op")) {
        return Err(StorageError::Backend(format!("datasync carries only {} (node edits go through the project graph)", OPS.join(" | "))));
    }
    let lockdirs = lock_scope(op);
    let summary = summarize(op);
    let now = now_utc();
    // time-ordered to the microsecond: edits apply in the order they were
    // made, even several in one second (a new actor, then its first edge)
    let id = format!("{}-{}", chrono::Utc::now().format("%Y-%m-%dT%H:%M:%S%.6fZ"), &uuid::Uuid::new_v4().simple().to_string()[..8]);
    let row = json!({"id": id, "project": project, "op": op, "lockdirs": lockdirs, "summary": summary,
        "state": "pending", "created": now, "by": by, "engine": "", "claim_expires": "", "attempts": 0,
        "applied_at": "", "commit": "", "files": [], "error": "", "note": "", "version": 1});
    store.put_versioned(TABLE, project, &id, &row, 0).await?;
    store.bump_seq(project, TABLE).await?;
    Ok(row)
}

/// The ops a row may carry (GraphRAG's repo file operations).
pub const OPS: &[&str] = &["store_doc", "remove_doc", "gitignore_path"];

pub fn summarize(op: &Value) -> String {
    match s(op, "op") {
        "store_doc" => format!("GraphRAG: store {}", s(op, "path")),
        "remove_doc" => format!("GraphRAG: remove {}", s(op, "path")),
        "gitignore_path" => format!("{} {} in .gitignore", if op.get("ignore").and_then(|b| b.as_bool()) == Some(false) { "un-ignore" } else { "ignore" }, s(op, "path")),
        other => other.to_string(),
    }
}

/// The paths a row touches (the engine waits while a running item's lock overlaps them).
pub fn lock_scope(op: &Value) -> Vec<String> {
    let norm = |p: &str| -> String {
        let r = p.trim().trim_start_matches("{topdir}").trim_start_matches('/').trim_end_matches('/');
        if r.is_empty() { "{topdir}".into() } else { format!("{{topdir}}/{r}") }
    };
    match s(op, "op") {
        "store_doc" | "remove_doc" => vec![norm(s(op, "path"))],
        "gitignore_path" => vec!["{topdir}/.gitignore".into()],
        _ => Vec::new(),
    }
}

/// How many edits each project has waiting for an engine (heartbeat reply).
pub async fn waiting(store: &dyn Storage, projects: &[String]) -> Result<HashMap<String, usize>, StorageError> {
    let now = now_utc();
    let mut out = HashMap::new();
    for p in projects {
        let n = store.query(TABLE, p).await?.iter().filter(|r| claimable(r, &now)).count();
        if n > 0 {
            out.insert(p.clone(), n);
        }
    }
    Ok(out)
}

#[derive(serde::Deserialize)]
struct ListQ {
    #[serde(default)]
    state: String,
}

/// Newest first. `state=claimable` = what an engine may take now.
async fn list(_u: AuthUser, State(st): Ctx, Path(name): Path<String>, Query(q): Query<ListQ>) -> Result<Json<Value>, ApiError> {
    let now = now_utc();
    let mut rows = st.store.query(TABLE, &name).await?;
    match q.state.as_str() {
        "" => {}
        "claimable" => rows.retain(|r| claimable(r, &now)),
        st_ => rows.retain(|r| s(r, "state") == st_),
    }
    if q.state == "claimable" {
        rows.sort_by(|a, b| s(a, "id").cmp(s(b, "id"))); // oldest first: edits apply in the order they were made
    } else {
        rows.sort_by(|a, b| s(b, "id").cmp(s(a, "id")));
    }
    Ok(Json(Value::Array(rows)))
}

#[derive(serde::Deserialize)]
struct ClaimReq {
    engine: String,
}

/// One engine wins: a versioned write from claimable to claimed.
async fn claim(user: AuthUser, State(st): Ctx, Path((name, id)): Path<(String, String)>, Json(req): Json<ClaimReq>) -> Result<Json<Value>, ApiError> {
    user.require_writer()?;
    let Some(mut row) = st.store.get(TABLE, &name, &id).await? else {
        return Err(ApiError::Status(StatusCode::NOT_FOUND, format!("no datasync row {id}")));
    };
    if !claimable(&row, &now_utc()) {
        return Err(ApiError::Conflict(row));
    }
    let v = row.get("version").and_then(|x| x.as_u64()).unwrap_or(1);
    row["state"] = json!("claimed");
    row["engine"] = json!(req.engine);
    row["claim_expires"] = json!(iso_in(CLAIM_SEC));
    row["attempts"] = json!(row.get("attempts").and_then(|x| x.as_u64()).unwrap_or(0) + 1);
    row["version"] = json!(v + 1);
    st.store.put_versioned(TABLE, &name, &id, &row, v).await?;
    st.store.bump_seq(&name, TABLE).await?;
    Ok(Json(row))
}

#[derive(serde::Deserialize)]
struct DoneReq {
    engine: String,
    /// applied | failed | retry (back to pending, e.g. a lock or a git conflict in the way)
    outcome: String,
    #[serde(default)]
    commit: String,
    #[serde(default)]
    files: Vec<String>,
    #[serde(default)]
    error: String,
    #[serde(default)]
    note: String,
}

async fn done(user: AuthUser, State(st): Ctx, Path((name, id)): Path<(String, String)>, Json(req): Json<DoneReq>) -> Result<Json<Value>, ApiError> {
    user.require_writer()?;
    let Some(mut row) = st.store.get(TABLE, &name, &id).await? else {
        return Err(ApiError::Status(StatusCode::NOT_FOUND, format!("no datasync row {id}")));
    };
    if s(&row, "state") != "claimed" || s(&row, "engine") != req.engine {
        return Err(ApiError::Status(StatusCode::CONFLICT, format!("row {id} is {} by {:?}, not claimed by {}", s(&row, "state"), s(&row, "engine"), req.engine)));
    }
    let v = row.get("version").and_then(|x| x.as_u64()).unwrap_or(1);
    match req.outcome.as_str() {
        "applied" => {
            row["state"] = json!("applied");
            row["applied_at"] = json!(now_utc());
            row["commit"] = json!(req.commit);
            row["files"] = json!(req.files);
        }
        "failed" => row["state"] = json!("failed"),
        "retry" => {
            row["state"] = json!("pending");
            row["engine"] = json!("");
        }
        other => return Err(ApiError::Status(StatusCode::BAD_REQUEST, format!("outcome must be applied | failed | retry (got {other:?})"))),
    }
    row["claim_expires"] = json!("");
    row["error"] = json!(req.error);
    row["note"] = json!(req.note);
    row["version"] = json!(v + 1);
    st.store.put_versioned(TABLE, &name, &id, &row, v).await?;
    st.store.bump_seq(&name, TABLE).await?;
    Ok(Json(row))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn pending_claim_done_and_lapsed_claims() {
        let (st, db) = crate::test_db::store().await;
        let op = json!({"op": "store_doc", "path": "{topdir}/docs/a.pdf", "content_b64": ""});
        let row = create(&st, "p", &op, "u").await.unwrap();
        assert_eq!((row["state"].as_str(), row["summary"].as_str()), (Some("pending"), Some("GraphRAG: store {topdir}/docs/a.pdf")));
        assert_eq!(row["lockdirs"], json!(["{topdir}/docs/a.pdf"]));
        assert!(create(&st, "p", &json!({"op": "new_node", "name": "x"}), "u").await.is_err(), "node edits are not datasync rows");
        assert_eq!(waiting(&st, &["p".into(), "q".into()]).await.unwrap().get("p"), Some(&1));
        // a lapsed claim is claimable again; a live one is not
        let mut claimed = row.clone();
        claimed["state"] = json!("claimed");
        claimed["claim_expires"] = json!("2000-01-01T00:00:00Z");
        assert!(claimable(&claimed, &now_utc()));
        claimed["claim_expires"] = json!("2999-01-01T00:00:00Z");
        assert!(!claimable(&claimed, &now_utc()));
        assert!(!claimable(&json!({"state": "applied"}), &now_utc()));
        // several edits in one second still sort in the order they were made
        let mut ids = Vec::new();
        for i in 0..5 {
            ids.push(create(&st, "p", &json!({"op": "gitignore_path", "path": format!("a{i}")}), "u").await.unwrap()["id"].as_str().unwrap().to_string());
        }
        let mut sorted = ids.clone();
        sorted.sort();
        assert_eq!(ids, sorted);
        crate::test_db::drop(&db).await;
    }
}
