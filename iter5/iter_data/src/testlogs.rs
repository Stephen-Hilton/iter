//! Test results (iter5 spec §3.5): a test script prints the standard result
//! JSON as its last stdout line; the engine posts it here. It is stored on the
//! test node (`test`, `front.last_result`, `timestamps.last_tested` → the
//! file is rewritten like any node edit) and appended to the `test_log`
//! collection. A failed (or could-not-run) result files a work item, deduped
//! on `check:tests-failing` + `container:<node>` so a red test is filed once.
//! "Run tests" from the graph is a `test` work item the engine runs.

use crate::api::{ApiError, AppState, AuthUser};
use crate::nodes::{self, Graph, bad, not_found};
use axum::extract::{Path, Query, State};
use axum::routing::{get, post};
use axum::{Json, Router};
use iter_core::nodefile::{self as nf, NodeType};
use iter_core::testresult::{Outcome, TestResult, combine, evaluate};
use serde_json::{Value, json};
use std::sync::Arc;

type Ctx = State<Arc<AppState>>;

pub fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/api/projects/{name}/graph/nodes/{id}/testresult", post(testresult))
        .route("/api/projects/{name}/testlogs", get(testlogs))
        .route("/api/projects/{name}/graph/run_tests", post(run_tests))
}

fn s<'a>(v: &'a Value, k: &str) -> &'a str {
    v.get(k).and_then(|x| x.as_str()).unwrap_or("")
}

/// The result inside a request: `{result: {...}, exit_code?, stdout?, outcome?,
/// engine?, workid?, commit?, file_workitem?}` or the bare standard JSON.
fn read_result(body: &Value, name: &str, id: &str) -> Result<(TestResult, Outcome), ApiError> {
    let exit = body.get("exit_code").and_then(|x| x.as_i64()).map(|x| x as i32);
    if let Some(out) = body.get("stdout").and_then(|x| x.as_str()) {
        return Ok(evaluate(out, exit, name, id));
    }
    let raw = match body.get("result") {
        Some(r) if r.is_object() => r.clone(),
        _ => body.clone(),
    };
    if raw.get("overall_success").is_none() && raw.get("normal").is_none() {
        return Err(bad("a test result needs overall_success / normal (the standard result JSON, spec §3.5)"));
    }
    let r: TestResult = serde_json::from_value(raw).map_err(|e| bad(format!("test result does not parse: {e}")))?;
    let r = r.with_identity(name, id);
    let outcome = match s(body, "outcome") {
        "pass" => Outcome::Pass,
        "fail" => Outcome::Fail,
        "could-not-run" => Outcome::CouldNotRun,
        _ if exit.is_some() => combine(Some(&r), exit),
        _ if r.overall_success => Outcome::Pass,
        _ => Outcome::Fail,
    };
    let mut r = r;
    if outcome != Outcome::Pass {
        r.overall_success = false;
    }
    Ok((r, outcome))
}

fn outcome_str(o: Outcome) -> &'static str {
    match o {
        Outcome::Pass => "pass",
        Outcome::Fail => "fail",
        Outcome::CouldNotRun => "could-not-run",
    }
}

async fn testresult(user: AuthUser, State(st): Ctx, Path((name, id)): Path<(String, String)>, Json(body): Json<Value>) -> Result<Json<Value>, ApiError> {
    user.require_writer()?;
    let guard = nodes::lock(&name).await;
    let mut g = Graph::load(st.store.as_ref(), &name).await?;
    let n = g.live_node(&id)?.clone();
    if n.doc.nodetype != NodeType::Test {
        return Err(bad(format!("{} is a {} node: results are posted on test nodes", n.doc.name, n.doc.nodetype)));
    }
    let (r, outcome) = read_result(&body, &n.doc.name, &id)?;
    let now = iter_core::now_utc();
    let totals = r.totals();
    let summary = json!({
        "result": match outcome { Outcome::Pass => "green", Outcome::Fail => "red", Outcome::CouldNotRun => "error" },
        "outcome": outcome_str(outcome), "overall_success": r.overall_success,
        "totals": totals, "at": now, "by": user.sub,
        "engine": s(&body, "engine"), "workid": s(&body, "workid"), "commit": s(&body, "commit"),
    });
    let mut doc = n.doc.clone();
    doc.front.insert("last_result".into(), serde_json::to_value(&r).unwrap_or(Value::Null));
    doc.timestamps.last_tested = nf::now_ts();
    g.commit_edit(&id, doc, &user.sub, &format!("test result {}: {}", n.doc.name, outcome_str(outcome)), false)?;
    g.get_mut(&id).unwrap().test = summary.clone();
    g.save(st.store.as_ref()).await?;
    drop(guard);

    // a red / unrunnable test is work
    let mut filed = Value::Null;
    if outcome != Outcome::Pass && body.get("file_workitem").and_then(|b| b.as_bool()) != Some(false) {
        let short = &id[id.len().saturating_sub(12)..];
        let failing: Vec<String> = r.details.iter().filter(|d| !d.pass).take(20).map(|d| format!("- {} ({}): {}", d.name, d.bucket, d.msg)).collect();
        let item = json!({
            "name": format!("Fix failing tests: {}", n.doc.name), "agent": "code", "state": "queued", "priority": 50,
            "node": id, "lockdirs": [nf::dir_of(&n.doc.path)], "blockedby": [], "context": [n.doc.path],
            "tags": [{"text": "check:tests-failing", "color": ""}, {"text": format!("container:{short}"), "color": ""}, {"text": "tests", "color": ""}],
            "requestedby": "test", "prework": [], "postwork": [],
            "request": format!(
                "The test node \"{}\" ({}) {} at {now}: normal {}/{} pass, longtail {}/{}, failure {}/{} ({} errors).\n{}\n\nFind the cause and fix the code (or the test, when the test is wrong). Done when the test node's scripts pass.",
                n.doc.name, n.doc.path, if outcome == Outcome::Fail { "failed" } else { "could not run" },
                r.normal.pass, r.normal.total, r.longtail.pass, r.longtail.total, r.failure.pass, r.failure.total, totals.err,
                failing.join("\n")),
        });
        let who = AuthUser { sub: user.sub.clone(), role: user.role.clone() };
        if let Ok(Json(v)) = crate::api::workitem_create(who, State(st.clone()), Path(name.clone()), Json(item)).await {
            filed = json!({"id": v["id"], "already_open": v.get("already_open").cloned().unwrap_or(json!(false))});
        }
    }

    let row = json!({
        "_key": format!("{}-{}", chrono::Utc::now().format("%Y%m%dT%H%M%S%6f"), &uuid::Uuid::new_v4().simple().to_string()[..8]),
        "project": name, "node": id, "name": n.doc.name, "path": n.doc.path, "at": now, "outcome": outcome_str(outcome),
        "overall_success": r.overall_success, "normal": r.normal, "longtail": r.longtail, "failure": r.failure, "details": r.details,
        "exit_code": body.get("exit_code").cloned().unwrap_or(Value::Null), "engine": s(&body, "engine"), "workid": s(&body, "workid"),
        "commit": s(&body, "commit"), "by": user.sub, "workitem": filed,
    });
    nodes::arango(st.store.as_ref())?
        .aql("INSERT @r INTO test_log", json!({"r": row}))
        .await
        .map_err(nodes::backend)?;
    st.store.bump_seq(&name, "test_log").await?;
    let node = Graph::load(st.store.as_ref(), &name).await?.get(&id).map(|n| n.json()).unwrap_or(Value::Null);
    Ok(Json(json!({"node": node, "test": summary, "result": r, "workitem": filed})))
}

#[derive(serde::Deserialize)]
struct LogsQ {
    #[serde(default)]
    node: String,
    #[serde(default)]
    limit: Option<u64>,
}

/// Test log rows, newest first (one node with `node=`).
async fn testlogs(_u: AuthUser, State(st): Ctx, Path(name): Path<String>, Query(q): Query<LogsQ>) -> Result<Json<Value>, ApiError> {
    let limit = q.limit.unwrap_or(50).clamp(1, 1000);
    let rows = nodes::arango(st.store.as_ref())?
        .aql(
            "FOR r IN test_log FILTER r.project == @p AND (@n == '' OR r.node == @n) SORT r.at DESC, r._key DESC LIMIT @l
             RETURN MERGE(UNSET(r, '_id', '_rev', '_key'), {log_id: r._key})",
            json!({"p": name, "n": q.node, "l": limit}),
        )
        .await
        .map_err(nodes::backend)?;
    Ok(Json(json!({"logs": rows})))
}

/// "Run tests" on a node: a deterministic `test` work item. The engine runs
/// the node's test scripts (a test node's own, or every test node under any
/// other node) and posts each result back.
async fn run_tests(user: AuthUser, State(st): Ctx, Path(name): Path<String>, Json(req): Json<Value>) -> Result<Json<Value>, ApiError> {
    user.require_writer()?;
    let id = s(&req, "node").trim().to_string();
    if id.is_empty() {
        return Err(bad("run_tests needs node (a node id)"));
    }
    let g = Graph::load(st.store.as_ref(), &name).await?;
    let n = g.get(&id).filter(|n| n.live()).ok_or_else(|| not_found(format!("no node {id} in {name}")))?;
    let item = json!({"name": format!("Run tests: {}", n.doc.name), "agent": "test", "state": "queued", "priority": 5,
           "node": id, "exec_shell": format!("iter runtests --project \"$ITER_PROJECT\" --node \"{id}\""),
           "lockdirs": [], "blockedby": [], "context": [n.doc.path], "tags": [{"text": "tests", "color": ""}],
           "requestedby": user.sub, "prework": [], "postwork": [],
           "request": format!("Run the tests of \"{}\" ({}) once (queued from the Project graph).", n.doc.name, n.doc.path)});
    crate::api::workitem_create(user, State(st), Path(name), Json(item)).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn results_read_bare_wrapped_and_from_stdout() {
        let bare = json!({"overall_success": true, "normal": {"total": 2, "pass": 2, "err": 0}});
        let (r, o) = read_result(&bare, "t", "id1").unwrap();
        assert_eq!((o, r.name.as_str(), r.id.as_str()), (Outcome::Pass, "t", "id1"));
        let wrapped = json!({"result": {"overall_success": true, "normal": {"total": 2, "pass": 2, "err": 0}}, "exit_code": 1});
        let (r, o) = read_result(&wrapped, "t", "x").unwrap();
        assert_eq!(o, Outcome::Fail);
        assert!(!r.overall_success, "exit 1 overrides");
        let (_, o) = read_result(&json!({"stdout": "ITER_RESULT pass=1 fail=0 total=1", "exit_code": 0}), "t", "x").unwrap();
        assert_eq!(o, Outcome::Pass);
        let (_, o) = read_result(&json!({"stdout": "boom", "exit_code": 2}), "t", "x").unwrap();
        assert_eq!(o, Outcome::CouldNotRun);
        assert!(read_result(&json!({"x": 1}), "t", "x").is_err());
    }
}
