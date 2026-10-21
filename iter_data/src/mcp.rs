//! Stateless MCP (2026-09-29): every agent-facing call of the iter_data
//! API as an MCP tool, served at `POST /mcp` (Streamable HTTP, JSON responses,
//! no sessions — each request stands alone, so any iter_data replica can
//! answer it and nothing is held between calls).
//!
//! Auth is the API's own: the MCP request carries `Authorization: Bearer
//! <token>` (an agent session's ITER_ENGINE_TOKEN, or a user token), and
//! every tool call is replayed through the real API router with that header —
//! the same role checks, validation and seq bumps as the HTTP routes, never a
//! second copy of the rules. Optional headers `X-Iter-Project` /
//! `X-Iter-Workid` supply the default project and calling work item (what
//! ITER_PROJECT / ITER_WORKID are to the `iter` CLI).
//!
//! The verbs of the agent CLI whose logic spans several calls (`add`, `ask`,
//! `reject`, `wait`, `doc`, `block`, `status`, `capability`) are the same
//! call sequences as iter_engine/src/cli.rs. Not here: the checkout verbs
//! (runtests, validate, markers, teststate, usecase, ids, sync, sweep,
//! rag sync) — they read the repo, which iter_data never has — and
//! `critreview`, which runs an LLM session on the engine's machine.

use crate::api::{AppState, AuthUser};
use axum::body::Body;
use axum::extract::State;
use axum::http::{HeaderMap, Request, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::{Json, Router};
use serde_json::{Value, json};
use std::sync::Arc;
use tower::ServiceExt;

pub const PROTOCOL_VERSIONS: &[&str] = &["2025-06-18", "2025-03-26", "2024-11-05"];

#[derive(Clone)]
struct Mcp {
    api: Router,
    state: Arc<AppState>,
}

pub fn routes(state: Arc<AppState>) -> Router {
    let mcp = Mcp { api: crate::api::router(state.clone()), state };
    Router::new()
        .route("/mcp", post(handle).get(no_stream).delete(no_session))
        .layer(axum::extract::DefaultBodyLimit::max(64 * 1024 * 1024))
        .with_state(mcp)
}

/// Stateless: no server-initiated stream to open.
async fn no_stream() -> Response {
    (StatusCode::METHOD_NOT_ALLOWED, "this MCP server is stateless: POST only").into_response()
}
async fn no_session() -> Response {
    (StatusCode::METHOD_NOT_ALLOWED, "this MCP server keeps no sessions").into_response()
}

/// The caller's defaults for one request.
struct Caller {
    auth: String,
    project: String,
    workid: String,
    sub: String,
}

async fn handle(State(m): State<Mcp>, headers: HeaderMap, Json(msg): Json<Value>) -> Response {
    // the bearer token is checked before anything (the same extractor as every route)
    let auth = headers.get("authorization").and_then(|v| v.to_str().ok()).unwrap_or("").to_string();
    let user = {
        let (mut parts, _) = Request::builder().uri("/mcp").header("authorization", &auth).body(()).unwrap().into_parts();
        match <AuthUser as axum::extract::FromRequestParts<Arc<AppState>>>::from_request_parts(&mut parts, &m.state).await {
            Ok(u) => u,
            Err(e) => return e.into_response(),
        }
    };
    let hdr = |k: &str| headers.get(k).and_then(|v| v.to_str().ok()).unwrap_or("").trim().to_string();
    // the client's protocol version (MG-19): none = 2025-03-26, as the
    // specification says; one we do not speak is a 400
    let version = hdr("mcp-protocol-version");
    if !version.is_empty() && !PROTOCOL_VERSIONS.contains(&version.as_str()) {
        let msg = format!("unsupported MCP-Protocol-Version {version} (supported: {})", PROTOCOL_VERSIONS.join(", "));
        return (StatusCode::BAD_REQUEST, Json(err(&Value::Null, -32600, &msg))).into_response();
    }
    let version = if version.is_empty() { "2025-03-26".to_string() } else { version };
    let caller = Caller { auth, project: hdr("x-iter-project"), workid: hdr("x-iter-workid"), sub: user.sub };
    match msg {
        // batching was removed in 2025-06-18 (dates compare as strings)
        Value::Array(_) if version.as_str() >= "2025-06-18" => {
            let msg = format!("batching is not supported in protocol version {version}");
            (StatusCode::BAD_REQUEST, Json(err(&Value::Null, -32600, &msg))).into_response()
        }
        Value::Array(batch) => {
            let mut out = Vec::new();
            for one in batch {
                if let Some(r) = rpc(&m, &caller, one).await {
                    out.push(r);
                }
            }
            if out.is_empty() { StatusCode::ACCEPTED.into_response() } else { Json(Value::Array(out)).into_response() }
        }
        one => match rpc(&m, &caller, one).await {
            Some(r) => Json(r).into_response(),
            None => StatusCode::ACCEPTED.into_response(), // a notification
        },
    }
}

fn ok(id: &Value, result: Value) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "result": result})
}
fn err(id: &Value, code: i64, message: &str) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}})
}

async fn rpc(m: &Mcp, c: &Caller, msg: Value) -> Option<Value> {
    let id = msg.get("id").cloned();
    let method = msg.get("method").and_then(|x| x.as_str()).unwrap_or("");
    let Some(id) = id else {
        return None; // notifications (initialized, cancelled…) need no answer
    };
    let params = msg.get("params").cloned().unwrap_or(json!({}));
    Some(match method {
        "initialize" => {
            let asked = params.get("protocolVersion").and_then(|v| v.as_str()).unwrap_or("");
            let v = if PROTOCOL_VERSIONS.contains(&asked) { asked } else { PROTOCOL_VERSIONS[0] };
            ok(&id, json!({
                "protocolVersion": v,
                "capabilities": {"tools": {"listChanged": false}},
                "serverInfo": {"name": "iter_data", "title": "iter data API", "version": iter_core::VERSION},
                "instructions": INSTRUCTIONS,
            }))
        }
        "ping" => ok(&id, json!({})),
        "tools/list" => ok(&id, json!({"tools": tools()})),
        "tools/call" => {
            let name = params.get("name").and_then(|x| x.as_str()).unwrap_or("");
            let args = params.get("arguments").cloned().unwrap_or(json!({}));
            if !tools().iter().any(|t| t["name"] == name) {
                return Some(err(&id, -32602, &format!("unknown tool {name:?}")));
            }
            let started = std::time::Instant::now();
            let (is_error, body) = match call_tool(m, c, name, &args).await {
                Ok(v) => (false, v),
                Err(e) => (true, json!({"error": e})),
            };
            // one line per tool call: who used which tool for which item (ops + proof agents use it)
            println!(
                "[iter_data] mcp {} {name} project={} workid={} {}ms{}",
                c.sub,
                if a(&args, "project").is_empty() { c.project.as_str() } else { a(&args, "project") },
                if a(&args, "workid").is_empty() { &c.workid[c.workid.len().saturating_sub(12)..] } else { a(&args, "workid") },
                started.elapsed().as_millis(),
                if is_error { " ERROR" } else { "" }
            );
            // every result is an object, so structuredContent is always sent (MG-1)
            debug_assert!(body.is_object(), "{name} returned a non-object result");
            let body = if body.is_object() { body } else { json!({"result": body}) };
            let text = serde_json::to_string_pretty(&body).unwrap_or_default();
            ok(&id, json!({"content": [{"type": "text", "text": text}], "isError": is_error, "structuredContent": body}))
        }
        "resources/list" => ok(&id, json!({"resources": []})),
        "prompts/list" => ok(&id, json!({"prompts": []})),
        other => err(&id, -32601, &format!("method not found: {other}")),
    })
}

pub const INSTRUCTIONS: &str = "iter's data API as tools. Each tool's description has six parts: Use, Arguments, Example, Returns, Errors, Limits — read them instead of guessing. \
Work items: status (the open queue), workitem_list (every item, any filter and sort, paged: pass next_cursor back as `cursor` until it is null), get, details, create (iter add), ask, reject, wait, doc, block. \
Who filed or changed an item (requestedby, createdby) is set by the server from your token and calling work item, never from the arguments. \
The project map (nodes = the *.iter.md node files): graph_* tools look a node up by path or name, walk its neighbours, find the code node that owns a path, \
and edit the map — graph_node_create / graph_node_update / graph_edge_add / graph_edge_remove change the nodes at once and an engine writes the files. \
Requirements are the `## ` sections of bizreq/techreq files: req_create / req_update / req_move / req_delete edit one section. \
testlogs: the test results of test nodes. settings_graph: engines, projects, accounts, agents and their edges. \
GraphRAG: rag_search finds the chunks of the project's documents and node files closest in meaning to a question and returns the FULL chunk text with its summaries and map neighbours — search before reading files blind. \
Do not upload *.iter.md node files: the iter engine indexes them from the checkout after each map sync. A node edited with graph_node_update reaches search after the engine writes the file and its next sync (about a minute). \
An uploaded document (rag_add_document) is not searchable at once: an engine must ingest it first (about 5–30 s with a live engine; it stays queued while none is live). Pass wait_sec, or poll rag_doc until searchable is true; summaries follow minutes later. \
Most tools take `project` (default: the X-Iter-Project header); ask/reject/wait/block act on the calling work item (`workid`, default: the X-Iter-Workid header).";

pub fn tools() -> &'static [Value] {
    crate::mcp_docs::tools()
}

// ---------- dispatch ----------

fn a<'a>(args: &'a Value, k: &str) -> &'a str {
    args.get(k).and_then(|x| x.as_str()).unwrap_or("").trim()
}
fn arr(args: &Value, k: &str) -> Vec<String> {
    args.get(k).and_then(|x| x.as_array()).map(|v| v.iter().filter_map(|x| x.as_str().map(|s| s.trim().to_string())).filter(|s| !s.is_empty()).collect()).unwrap_or_default()
}

/// The longest id or path segment a tool passes on.
pub const MAX_SEGMENT: usize = 200;

/// One id checked and percent-encoded as a path segment (MG-11): refused
/// when empty, holding a control character, or longer than MAX_SEGMENT;
/// everything but `A-Z a-z 0-9 - _ . ~` encoded, and `.` / `..` too, so no
/// caller text can add a segment, a query or a fragment.
pub fn seg(s: &str) -> Result<String, String> {
    check_id(s)?;
    if s == "." || s == ".." {
        return Ok(s.replace('.', "%2E"));
    }
    Ok(pct(s))
}

/// The id rules of `seg`, without the encoding.
pub fn check_id(s: &str) -> Result<(), String> {
    if s.is_empty() {
        return Err("an id is empty".into());
    }
    if s.chars().any(|c| c.is_control()) {
        return Err("an id holds a control character".into());
    }
    if s.chars().count() > MAX_SEGMENT {
        return Err(format!("an id is longer than {MAX_SEGMENT} characters"));
    }
    Ok(())
}

fn pct(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// `/api/<parts…>?<query>`: every part through `seg`, every query value
/// encoded the same way. The only way a tool builds a path.
pub fn api_path(parts: &[&str], query: &[(&str, &str)]) -> Result<String, String> {
    let mut p = String::from("/api");
    for part in parts {
        p.push('/');
        p.push_str(&seg(part)?);
    }
    if !query.is_empty() {
        let q: Vec<String> = query.iter().map(|(k, v)| format!("{}={}", pct(k), pct(v))).collect();
        p.push('?');
        p.push_str(&q.join("&"));
    }
    Ok(p)
}

/// A required argument: present and non-empty, or "<arg> is required".
fn need<'a>(args: &'a Value, k: &str) -> Result<&'a str, String> {
    let v = a(args, k);
    if v.is_empty() { Err(format!("{k} is required")) } else { Ok(v) }
}

impl Mcp {
    /// One API call through the real router, as the caller.
    async fn call(&self, c: &Caller, method: &str, path: &str, body: Option<&Value>) -> Result<Value, String> {
        // a project (or other record) named by its display name: the router
        // below sees ids only (names.rs; the HTTP server does this before routing)
        let resolved = crate::names::resolve_path(self.state.store.as_ref(), path.split('?').next().unwrap_or(path)).await;
        let owned;
        let path = match resolved {
            Some(p) => {
                owned = match path.split_once('?') { Some((_, q)) => format!("{p}?{q}"), None => p };
                owned.as_str()
            }
            None => path,
        };
        let mut b = Request::builder().method(method).uri(path).header("authorization", &c.auth);
        // the calling work item reaches the API, which decides who acted (MG-12)
        if !c.workid.is_empty() {
            b = b.header("x-iter-workid", &c.workid);
        }
        let req = match body {
            Some(v) => {
                b = b.header("content-type", "application/json");
                b.body(Body::from(serde_json::to_vec(v).unwrap_or_default()))
            }
            None => b.body(Body::empty()),
        }
        .map_err(|e| e.to_string())?;
        let resp = self.api.clone().oneshot(req).await.map_err(|e| e.to_string())?;
        let status = resp.status();
        let bytes = axum::body::to_bytes(resp.into_body(), 256 * 1024 * 1024).await.map_err(|e| e.to_string())?;
        let v: Value = serde_json::from_slice(&bytes).unwrap_or_else(|_| json!(String::from_utf8_lossy(&bytes)));
        if status.is_success() {
            Ok(v)
        } else {
            // never the internal route (MG-16): the status, the API's message, the error id
            let msg = v.get("error").and_then(|e| e.as_str()).map(String::from).unwrap_or_else(|| v.to_string());
            let id = v.get("error_id").and_then(|e| e.as_str()).map(|i| format!(" (error_id {i})")).unwrap_or_default();
            Err(format!("{}: {msg}{id}", status.as_u16()))
        }
    }

    async fn items(&self, c: &Caller, project: &str) -> Result<Vec<Value>, String> {
        Ok(self.call(c, "GET", &api_path(&["projects", project, "workitems"], &[])?, None).await?.as_array().cloned().unwrap_or_default())
    }

    /// Full id from a full id or unique prefix/suffix (like `iter`'s resolve_id).
    async fn resolve(&self, c: &Caller, project: &str, needle: &str, all: Option<&[Value]>) -> Result<String, String> {
        let n = needle.trim();
        check_id(n).map_err(|e| format!("work item id: {e}"))?;
        let owned;
        let all = match all {
            Some(a) => a,
            None => {
                owned = self.items(c, project).await?;
                &owned
            }
        };
        let hits: Vec<&str> = all
            .iter()
            .filter_map(|i| i.get("id").and_then(|x| x.as_str()))
            .filter(|id| *id == n || id.ends_with(n) || id.starts_with(n) || id.replace('-', "").ends_with(&n.replace('-', "")))
            .collect();
        match hits.len() {
            1 => Ok(hits[0].to_string()),
            0 => Err(format!("no work item in '{project}' matches '{n}'")),
            k => Err(format!("'{n}' is ambiguous ({k} matches) — use more of the id")),
        }
    }

    /// The calling work item (`workid`, else the X-Iter-Workid header),
    /// resolved against the project's real items: only a full id that exists
    /// is ever put in a path.
    async fn workid(&self, c: &Caller, args: &Value, project: &str, all: Option<&[Value]>) -> Result<String, String> {
        let w = if a(args, "workid").is_empty() { c.workid.as_str() } else { a(args, "workid") };
        if w.is_empty() {
            return Err("no calling work item: pass workid or send X-Iter-Workid".into());
        }
        self.resolve(c, project, w, all).await
    }

    /// Who this request acts as on `project` (the server's principal).
    async fn actor(&self, c: &Caller, project: &str) -> String {
        match api_path(&["projects", project, "whoami"], &[]) {
            Ok(p) => self.call(c, "GET", &p, None).await.ok().and_then(|v| v["requestedby"].as_str().map(String::from)).unwrap_or_else(|| c.sub.clone()),
            Err(_) => c.sub.clone(),
        }
    }
}

fn project_of(c: &Caller, args: &Value) -> Result<String, String> {
    let p = if a(args, "project").is_empty() { c.project.clone() } else { a(args, "project").to_string() };
    if p.is_empty() { Err("no project: pass `project` or send the X-Iter-Project header".into()) } else { Ok(p) }
}

fn question_widget(question: &str) -> Value {
    let title: String = question.lines().find(|l| !l.trim().is_empty()).unwrap_or("Question").chars().take(150).collect();
    json!({"title": title, "summary": "", "detail": question, "fields": [{"key": "answer", "label": "Answer", "type": "text", "value": ""}]})
}

/// absolute / {topdir}-relative / repo-relative → "{topdir}/…" (no checkout here: absolute paths stay as given)
fn lockdir(p: &str) -> String {
    let p = p.trim();
    if p.starts_with("{topdir}") || p.starts_with('/') || p.starts_with('~') {
        p.to_string()
    } else {
        format!("{{topdir}}/{}", p.trim_start_matches("./"))
    }
}

/// `args` without the keys the tool consumed itself (the rest is the body).
fn body_without(args: &Value, keys: &[&str]) -> Value {
    let mut body = args.clone();
    if let Some(o) = body.as_object_mut() {
        for k in keys {
            o.remove(*k);
        }
    }
    body
}

async fn call_tool(m: &Mcp, c: &Caller, name: &str, args: &Value) -> Result<Value, String> {
    match name {
        "capability" => {
            let rows = m.call(c, "GET", &api_path(&["tooling"], &[])?, None).await?.as_array().cloned().unwrap_or_default();
            let caps: Vec<&Value> = rows.iter().filter(|r| r["kind"] == "capability").collect();
            let want = a(args, "name");
            if want.is_empty() {
                return Ok(json!({"capabilities": caps.iter().map(|c| json!({"name": c["id"], "title": c["name"], "desc": c["desc"]})).collect::<Vec<_>>()}));
            }
            let w = want.trim_start_matches('_').trim_end_matches(".md");
            caps.iter()
                .find(|x| ["id", "name"].iter().any(|k| x[*k].as_str().map(|n| n == want || n.trim_start_matches('_') == w).unwrap_or(false)))
                .map(|x| json!({"name": x["id"], "title": x["name"], "body": x["body"]}))
                .ok_or_else(|| format!("no capability named '{want}'"))
        }
        "settings_graph" => m.call(c, "GET", &api_path(&["settings", "graph"], &[])?, None).await,
        _ => {
            let project = project_of(c, args)?;
            check_id(&project).map_err(|e| format!("project: {e}"))?;
            project_tool(m, c, name, args, &project).await
        }
    }
}

async fn project_tool(m: &Mcp, c: &Caller, name: &str, args: &Value, project: &str) -> Result<Value, String> {
    let p = project;
    // /api/projects/<p>/<rest…>
    let pp = |rest: &[&str]| -> Result<String, String> {
        let mut parts = vec!["projects", p];
        parts.extend_from_slice(rest);
        api_path(&parts, &[])
    };
    let ppq = |rest: &[&str], q: &[(&str, &str)]| -> Result<String, String> {
        let mut parts = vec!["projects", p];
        parts.extend_from_slice(rest);
        api_path(&parts, q)
    };
    match name {
        "status" => {
            let mut all = m.items(c, project).await?;
            let order = |s: &str| match s {
                "in-progress" => 0, "queued" => 1, "question" => 2, "paused" => 3, "parked" => 4, "scheduled" => 5, _ => 7,
            };
            all.retain(|i| !matches!(i["state"].as_str(), Some("complete") | Some("failed")));
            all.sort_by_key(|i| (order(i["state"].as_str().unwrap_or("")), i["priority"].as_i64().unwrap_or(5)));
            let deadlocks = m.call(c, "GET", &pp(&["deadlocks"])?, None).await.unwrap_or(Value::Null);
            Ok(json!({"project": project, "open": all.iter().map(brief).collect::<Vec<_>>(), "open_count": all.len(),
                      "deadlocks": deadlocks.get("cycles").cloned().unwrap_or(json!([])),
                      "stale_lock_rows": deadlocks.get("stale_lock_rows").and_then(|s| s.as_array()).map(|a| a.len()).unwrap_or(0)}))
        }
        "workitem_list" => m.call(c, "POST", &pp(&["workitems", "query"])?, Some(&body_without(args, &["project"]))).await,
        "workitem_get" => {
            let id = m.resolve(c, project, need(args, "id")?, None).await?;
            m.call(c, "GET", &pp(&["workitems", &id])?, None).await
        }
        "workitem_details" => {
            let id = m.resolve(c, project, need(args, "id")?, None).await?;
            Ok(json!({"id": id, "details": m.call(c, "GET", &pp(&["workitems", &id, "details"])?, None).await?}))
        }
        "workitem_create" => create(m, c, args, project).await,
        "workitem_ask" => {
            let q = a(args, "question");
            if q.is_empty() {
                return Err("the question is empty".into());
            }
            let wid = m.workid(c, args, project, None).await?;
            let item = m.call(c, "GET", &pp(&["workitems", &wid])?, None).await?;
            m.call(c, "POST", &pp(&["workitems", &wid, "details"])?, Some(&json!({"key": "question", "valuetype": "json", "value": question_widget(q)}))).await?;
            set_state(m, c, project, &wid, item, "question", None).await?;
            Ok(json!({"ok": true, "message": "question recorded — this work item parks in `question` when this turn ends and queues again once a human answers. Finish your turn now."}))
        }
        "workitem_reject" => {
            let reason = a(args, "reason");
            if reason.is_empty() {
                return Err("a reason is required".into());
            }
            let wid = m.workid(c, args, project, None).await?;
            m.call(c, "POST", &pp(&["workitems", &wid, "reject"])?, Some(&json!({"reason": reason}))).await?;
            Ok(json!({"ok": true, "message": "rejected — this work item parks for human review when this turn ends. Finish your turn now."}))
        }
        "workitem_wait" => {
            let all = m.items(c, project).await?;
            let wid = m.workid(c, args, project, Some(&all)).await?;
            let mut ids = Vec::new();
            for n in arr(args, "on") {
                ids.push(m.resolve(c, project, &n, Some(&all)).await?);
            }
            if ids.is_empty() {
                return Err("`on` names no work item".into());
            }
            let mut item = m.call(c, "GET", &pp(&["workitems", &wid])?, None).await?;
            let version = item["version"].as_u64().unwrap_or(1);
            let existing: Vec<String> = item["blockedby"].as_array().map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect()).unwrap_or_default();
            let mut merged = existing.clone();
            for id in &ids {
                if *id == wid {
                    return Err("an item cannot wait on itself".into());
                }
                if !merged.contains(id) {
                    merged.push(id.clone());
                }
            }
            let added: Vec<String> = merged.iter().filter(|x| !existing.contains(x)).cloned().collect();
            if added.is_empty() {
                return Ok(json!({"ok": true, "added": [], "message": "already waiting on all of them — nothing to add"}));
            }
            item["blockedby"] = json!(merged);
            let ver = version.to_string();
            m.call(c, "PUT", &ppq(&["workitems", &wid], &[("expect_version", &ver)])?, Some(&item)).await?;
            let reason: String = a(args, "reason").chars().take(400).collect();
            let actor = m.actor(c, project).await;
            let _ = m
                .call(c, "POST", &pp(&["workitems", &wid, "details"])?, Some(&json!({"key": "doc", "valuetype": "text", "value": format!(
                    "waiting on {} ({actor}, attempt {}){}", added.join(", "),
                    item["attempt"].as_u64().unwrap_or(0), if reason.is_empty() { String::new() } else { format!(": {reason}") })})))
                .await;
            Ok(json!({"ok": true, "added": added, "message": "linked as dependencies. Finish what you can, then end your turn listing what still waits on them as NOT DONE lines: the close gate queues this item behind them and the engine re-runs it once they close complete."}))
        }
        "workitem_doc" => {
            let text = args.get("text").and_then(|t| t.as_str()).unwrap_or("").trim_end();
            if text.trim().is_empty() {
                return Err("doc text is empty".into());
            }
            let target = if !a(args, "id").is_empty() { m.resolve(c, project, a(args, "id"), None).await? } else { m.workid(c, args, project, None).await? };
            let row = m.call(c, "POST", &pp(&["workitems", &target, "details"])?, Some(&json!({"key": "doc", "valuetype": "text", "value": text}))).await?;
            Ok(json!({"ok": true, "id": target, "order": row["order"]}))
        }
        "workitem_block" => block(m, c, args, project).await,
        "locks_list" => Ok(json!({"locks": m.call(c, "GET", &pp(&["locks"])?, None).await?})),
        "graph_stats" => m.call(c, "GET", &pp(&["graph", "stats"])?, None).await,
        "graph_lookup" => {
            let q = [("path", a(args, "path")), ("name", a(args, "name")), ("nodetype", a(args, "nodetype"))];
            m.call(c, "GET", &ppq(&["graph", "lookup"], &q)?, None).await
        }
        "graph_node" => m.call(c, "GET", &pp(&["graph", "nodes", need(args, "id")?])?, None).await,
        "graph_neighbors" => {
            let depth = args.get("depth").and_then(|d| d.as_u64()).unwrap_or(1).to_string();
            let dir = if a(args, "direction").is_empty() { "any" } else { a(args, "direction") };
            m.call(c, "GET", &ppq(&["graph", "nodes", need(args, "id")?, "neighbors"], &[("depth", &depth), ("direction", dir)])?, None).await
        }
        "graph_owner" => m.call(c, "GET", &ppq(&["graph", "owner"], &[("path", need(args, "path")?)])?, None).await,
        "graph_usecase" => m.call(c, "GET", &pp(&["graph", "usecases", need(args, "ucid")?])?, None).await,
        "graph_node_create" => m.call(c, "POST", &pp(&["graph", "nodes"])?, Some(&body_without(args, &["project"]))).await,
        "graph_node_update" => {
            let id = need(args, "id")?;
            m.call(c, "PATCH", &pp(&["graph", "nodes", id])?, Some(&body_without(args, &["project", "id"]))).await
        }
        "graph_edge_add" => {
            let body = json!({"from": a(args, "from"), "to": a(args, "to"), "kind": a(args, "kind")});
            m.call(c, "POST", &pp(&["graph", "edges"])?, Some(&body)).await
        }
        "graph_edge_remove" => {
            let body = json!({"from": a(args, "from"), "to": a(args, "to"), "kind": a(args, "kind"), "reason": a(args, "reason")});
            m.call(c, "DELETE", &pp(&["graph", "edges"])?, Some(&body)).await
        }
        "req_create" => m.call(c, "POST", &pp(&["graph", "reqs"])?, Some(&body_without(args, &["project"]))).await,
        "req_update" => {
            let id = need(args, "id")?;
            m.call(c, "PATCH", &pp(&["graph", "reqs", id])?, Some(&body_without(args, &["project", "id"]))).await
        }
        "req_move" => {
            let id = need(args, "id")?;
            let mut body = json!({"to_file": a(args, "to_file")});
            if !a(args, "after").is_empty() {
                body["after"] = json!(a(args, "after"));
            }
            m.call(c, "POST", &pp(&["graph", "reqs", id, "move"])?, Some(&body)).await
        }
        "req_delete" => {
            let id = need(args, "id")?;
            m.call(c, "DELETE", &pp(&["graph", "reqs", id])?, Some(&json!({"reason": a(args, "reason")}))).await
        }
        "testlogs" => {
            let limit = args.get("limit").and_then(|l| l.as_u64()).unwrap_or(50).to_string();
            m.call(c, "GET", &ppq(&["testlogs"], &[("node", a(args, "node")), ("limit", &limit)])?, None).await
        }
        "rag_search" => m.call(c, "POST", &pp(&["rag", "search"])?, Some(&body_without(args, &["project"]))).await,
        "rag_status" => m.call(c, "GET", &pp(&["rag"])?, None).await,
        "rag_docs" => {
            let q = [("kind", a(args, "kind")), ("state", a(args, "state"))];
            Ok(json!({"documents": m.call(c, "GET", &ppq(&["rag", "docs"], &q)?, None).await?}))
        }
        "rag_doc" => m.call(c, "GET", &pp(&["rag", "docs", need(args, "id")?])?, None).await,
        "rag_link_document" => {
            let (doc, node) = (need(args, "doc")?, need(args, "node")?);
            let body = json!({"node": node, "unlink": args.get("unlink").and_then(|u| u.as_bool()).unwrap_or(false)});
            m.call(c, "POST", &pp(&["rag", "docs", doc, "links"])?, Some(&body)).await
        }
        "rag_add_document" => add_document(m, c, args, project).await,
        other => Err(format!("unknown tool {other}")),
    }
}

/// `rag_add_document`: the upload, then (with `wait_sec`) a poll once a second
/// until the document is searchable, fails, or the wait runs out (MG-5).
async fn add_document(m: &Mcp, c: &Caller, args: &Value, project: &str) -> Result<Value, String> {
    use base64::Engine as _;
    let filename = need(args, "filename")?;
    if filename.ends_with(".iter.md") {
        return Err(crate::rag::NODE_FILE_REFUSAL.into());
    }
    let b64 = if !a(args, "content_b64").is_empty() {
        a(args, "content_b64").to_string()
    } else {
        let t = args.get("text").and_then(|t| t.as_str()).unwrap_or("");
        if t.trim().is_empty() {
            return Err("give `text` or `content_b64`".into());
        }
        base64::engine::general_purpose::STANDARD.encode(t.as_bytes())
    };
    let body = json!({"filename": filename, "content_b64": b64, "title": a(args, "title"),
                      "store": args.get("store").and_then(|s| s.as_bool()).unwrap_or(true)});
    let up = m.call(c, "POST", &api_path(&["projects", project, "rag", "docs"], &[])?, Some(&body)).await?;
    let id = up["id"].as_str().unwrap_or("").to_string();
    let live = up["live_engines"].as_u64().unwrap_or(0);
    let wait = args.get("wait_sec").and_then(|w| w.as_u64()).unwrap_or(0).min(MAX_WAIT_SEC);
    let started = std::time::Instant::now();
    let mut doc = up.clone();
    if wait > 0 {
        let path = api_path(&["projects", project, "rag", "docs", &id], &[])?;
        let deadline = started + std::time::Duration::from_secs(wait);
        loop {
            if doc["searchable"] == true || doc["state"] == "failed" {
                break;
            }
            let now = std::time::Instant::now();
            if now >= deadline {
                break;
            }
            tokio::time::sleep((deadline - now).min(std::time::Duration::from_secs(1))).await;
            doc = m.call(c, "GET", &path, None).await?;
        }
    }
    let state = doc["state"].as_str().unwrap_or("queued").to_string();
    let searchable = doc["searchable"] == true;
    let message = if searchable {
        "searchable now".to_string()
    } else if state == "failed" {
        format!("failed: {}", doc["error"].as_str().unwrap_or("indexing failed"))
    } else if live == 0 {
        crate::rag::NO_LIVE_ENGINE.to_string()
    } else {
        format!("still {state}: poll rag_doc until searchable is true")
    };
    Ok(json!({"id": id, "state": state, "searchable": searchable, "live_engines": live,
              "waited_sec": started.elapsed().as_secs(), "message": message}))
}

/// The longest `wait_sec` of rag_add_document.
pub const MAX_WAIT_SEC: u64 = 60;

/// The default shape of a work item in a list (wiquery's, the same for `status`).
fn brief(i: &Value) -> Value {
    crate::wiquery::project_item(i, &[])
}

async fn set_state(m: &Mcp, c: &Caller, project: &str, wid: &str, mut item: Value, state: &str, note: Option<&str>) -> Result<(), String> {
    let version = item["version"].as_u64().unwrap_or(1).to_string();
    item["state"] = json!(state);
    if let Some(n) = note {
        item["lasterror"] = json!(n);
    }
    m.call(c, "PUT", &api_path(&["projects", project, "workitems", wid], &[("expect_version", &version)])?, Some(&item)).await.map(|_| ())
}

/// `iter add`, the same body the CLI sends. Who filed it (`requestedby`) and
/// its parent (`createdby`) are the server's to set from the token (MG-12):
/// the calling item is offered as the parent and kept only when it checks out.
async fn create(m: &Mcp, c: &Caller, args: &Value, project: &str) -> Result<Value, String> {
    // the `iter add --file` names are accepted too (type, name, mainwork, codepath, codepaths/lockdirs, blockedby)
    let pick = |keys: &[&str]| keys.iter().map(|k| a(args, k)).find(|v| !v.is_empty()).unwrap_or("");
    let title = pick(&["title", "name"]);
    if title.is_empty() {
        return Err("a title is required".into());
    }
    let agent = { let t = pick(&["agent", "type"]); if t.is_empty() { "code" } else { t } };
    let question = a(args, "question");
    let request = ["request", "mainwork"].iter().filter_map(|k| args.get(*k).and_then(|r| r.as_str())).map(str::trim).find(|r| !r.is_empty()).unwrap_or("");
    if request.is_empty() && question.is_empty() {
        return Err("an item needs a request (what to do and how to know it is done), or a question".into());
    }
    let all = m.items(c, project).await?;
    // inside an agent: the calling item is the parent (resolved when it can be;
    // otherwise sent as given, and the server replaces it with a warning)
    let given = if a(args, "workid").is_empty() { c.workid.clone() } else { a(args, "workid").to_string() };
    let workid = if given.is_empty() { String::new() } else { m.resolve(c, project, &given, Some(&all)).await.unwrap_or(given) };
    let mut blockedby = Vec::new();
    let mut deps = arr(args, "depends_on");
    deps.extend(arr(args, "blockedby"));
    for d in deps {
        blockedby.push(m.resolve(c, project, &d, Some(&all)).await?);
    }
    if !workid.is_empty() && blockedby.contains(&workid) {
        return Err("an item cannot depend on the item that creates it".into());
    }
    let calling_agent = all.iter().find(|i| i["id"] == workid.as_str()).and_then(|i| i["agent"].as_str()).unwrap_or("").to_string();
    let state = if !question.is_empty() {
        "question".to_string()
    } else {
        let proj = m.call(c, "GET", &api_path(&["projects", project], &[])?, None).await.unwrap_or(json!({}));
        let over = proj.pointer(&format!("/agents/{calling_agent}/childstate")).and_then(|x| x.as_str()).map(String::from);
        let def = if calling_agent.is_empty() {
            None
        } else {
            m.call(c, "GET", &api_path(&["agents", &calling_agent], &[])?, None).await.ok().and_then(|a| a["childstate"].as_str().map(String::from))
        };
        over.or(def).filter(|x| !x.is_empty()).unwrap_or_else(|| "queued".into())
    };
    let tags: Vec<Value> = arr(args, "tags")
        .iter()
        .map(|t| match t.rsplit_once(':') {
            Some((text, color)) if color.starts_with('#') => json!({"text": text.trim(), "color": color}),
            _ => json!({"text": t, "color": ""}),
        })
        .collect();
    let usecase = a(args, "usecase");
    let mut paths = arr(args, "codepaths");
    paths.extend(arr(args, "lockdirs"));
    if !a(args, "codepath").is_empty() {
        paths.insert(0, a(args, "codepath").to_string());
    }
    let lockdirs: Vec<String> = paths.iter().map(|p| lockdir(p)).collect();
    let mut body = json!({
        "name": title, "agent": agent, "state": state, "priority": args.get("priority").and_then(|p| p.as_i64()),
        "lockdirs": lockdirs,
        "blockedby": blockedby, "blockedby_shallow": args.get("depends_on_shallow").and_then(|b| b.as_bool()).unwrap_or(false),
        "context": arr(args, "context"), "model": a(args, "model"), "tags": tags,
        "usecase": if usecase.is_empty() { Value::Null } else { json!(usecase) },
        "prework": [], "postwork": [], "request": request,
    });
    if !workid.is_empty() {
        body["createdby"] = json!(workid);
    }
    let created = m.call(c, "POST", &api_path(&["projects", project, "workitems"], &[])?, Some(&body)).await?;
    let id = created["id"].as_str().unwrap_or("").to_string();
    let already = created.get("already_open").and_then(|b| b.as_bool()).unwrap_or(false);
    if !already && !question.is_empty() {
        let _ = m.call(c, "POST", &api_path(&["projects", project, "workitems", &id, "details"], &[])?, Some(&json!({"key": "question", "valuetype": "json", "value": question_widget(question)}))).await;
    }
    Ok(json!({"id": id, "state": created["state"], "agent": created["agent"], "priority": created["priority"],
              "requestedby": created["requestedby"], "createdby": created["createdby"],
              "already_open": already, "warnings": created.get("warnings").cloned().unwrap_or(json!([])),
              "message": if already { "already open: the same check and container is filed; your request text was recorded on it. Do not file it again." } else { "added" }}))
}

/// `iter block --cluster-restart`.
async fn block(m: &Mcp, c: &Caller, args: &Value, project: &str) -> Result<Value, String> {
    let all = m.items(c, project).await?;
    let wid = m.workid(c, args, project, Some(&all)).await?;
    let reason: String = {
        let r = a(args, "reason");
        if r.is_empty() { "the cluster is required for this work".into() } else { r.chars().take(400).collect() }
    };
    let proj: Option<iter_core::Project> = m.call(c, "GET", &api_path(&["projects", project], &[])?, None).await.ok().and_then(|v| serde_json::from_value(v).ok());
    if let Some(proj) = proj {
        let items: Vec<iter_core::WorkItem> = all.iter().filter_map(|v| serde_json::from_value(v.clone()).ok()).collect();
        if let Some(clone) = iter_core::cluster::newest_clone(&proj.cluster_restart, &items) {
            let details: Vec<Value> = m.call(c, "GET", &api_path(&["projects", project, "workitems", &clone.id, "details"], &[])?, None).await.ok().and_then(|v| v.as_array().cloned()).unwrap_or_default();
            if iter_core::cluster::rebuild_skipped(Some(clone), &details, chrono::Utc::now()) {
                return Err(format!(
                    "refused: tonight's cluster rebuild was skipped (restart window {} posted \"skipped\"), so there is no restart to wait for — the cluster stays up. Do the work now; if the cluster is really unavailable, say so with workitem_ask.",
                    &clone.id[clone.id.len().saturating_sub(12)..]
                ));
            }
        }
    }
    let item_path = api_path(&["projects", project, "workitems", &wid], &[])?;
    let mut item = m.call(c, "GET", &item_path, None).await?;
    let version = item["version"].as_u64().unwrap_or(1).to_string();
    let before = item["attempt"].as_u64().unwrap_or(0);
    iter_core::cluster::apply_block(&mut item, &reason);
    let after = item["attempt"].as_u64().unwrap_or(0);
    let actor = m.actor(c, project).await;
    let _ = m
        .call(c, "POST", &api_path(&["projects", project, "workitems", &wid, "details"], &[])?, Some(&json!({"key": "doc", "valuetype": "text", "value": format!(
            "blocked by cluster restart ({actor}): {reason} — attempt put back from {before} to {after}; parked until the cluster is back up and healthy")})))
        .await;
    m.call(c, "PUT", &api_path(&["projects", project, "workitems", &wid], &[("expect_version", &version)])?, Some(&item)).await?;
    Ok(json!({"ok": true, "attempt_before": before, "attempt_after": after, "message": format!(
        "blocked on the cluster restart — this item parks when this turn ends, tagged `{}`; the engine requeues it once the cluster is healthy. Finish your turn now.",
        iter_core::cluster::CLUSTER_RESTART_TAG)}))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The tools of the hardening plan's §2.1, in order.
    const TOOL_NAMES: [&str; 34] = [
        "status", "workitem_list", "workitem_get", "workitem_details", "workitem_create", "workitem_ask", "workitem_reject",
        "workitem_wait", "workitem_doc", "workitem_block", "capability", "locks_list",
        "graph_stats", "graph_lookup", "graph_node", "graph_neighbors", "graph_owner", "graph_usecase",
        "graph_node_create", "graph_node_update", "graph_edge_add", "graph_edge_remove",
        "req_create", "req_update", "req_move", "req_delete", "testlogs", "settings_graph",
        "rag_search", "rag_status", "rag_docs", "rag_doc", "rag_link_document", "rag_add_document",
    ];

    #[test]
    fn every_tool_is_documented() {
        let t = tools();
        assert_eq!(t.iter().map(|x| x["name"].as_str().unwrap()).collect::<Vec<_>>(), TOOL_NAMES);
        for tool in t {
            let name = tool["name"].as_str().unwrap();
            let d = tool["description"].as_str().unwrap();
            // the six labels, in order, each part non-empty
            let mut at = Vec::new();
            for l in crate::mcp_docs::LABELS {
                let i = d.find(&format!("{l} ")).unwrap_or_else(|| panic!("{name}: no {l}"));
                at.push(i);
            }
            assert!(at.windows(2).all(|w| w[0] < w[1]), "{name}: labels out of order");
            for (k, l) in crate::mcp_docs::LABELS.iter().enumerate() {
                let end = at.get(k + 1).copied().unwrap_or(d.len());
                assert!(!d[at[k] + l.len()..end].trim().is_empty(), "{name}: {l} is empty");
            }
            let props = tool["inputSchema"]["properties"].as_object().unwrap();
            let required: Vec<&str> = tool["inputSchema"]["required"].as_array().unwrap().iter().map(|r| r.as_str().unwrap()).collect();
            for (k, p) in props {
                let desc = p["description"].as_str().unwrap_or("");
                assert!(!desc.is_empty(), "{name}.{k}: no description");
                if !required.contains(&k.as_str()) {
                    assert!(p.get("default").is_some() || desc.contains("default"), "{name}.{k}: neither a default nor \"no default\"");
                }
            }
            // the example is one complete arguments object
            let ex_text = d[at[2] + "Example:".len()..at[3]].trim();
            let ex: Value = serde_json::from_str(ex_text).unwrap_or_else(|e| panic!("{name}: Example is not JSON ({e}): {ex_text}"));
            let ex = ex.as_object().unwrap_or_else(|| panic!("{name}: Example is not an object"));
            for k in ex.keys() {
                assert!(props.contains_key(k), "{name}: Example key {k} is not an argument");
            }
            for r in &required {
                assert!(ex.contains_key(*r), "{name}: Example lacks required {r}");
            }
            let out = &tool["outputSchema"];
            assert!(out["type"] == "object" && out["properties"].is_object() && out["required"].is_array(), "{name}: outputSchema");
        }
        let limit = &tools()[1]["inputSchema"]["properties"]["limit"];
        assert_eq!((limit["default"].as_u64(), limit["maximum"].as_u64()), (Some(50), Some(500)));
    }

    #[test]
    fn instructions_state_the_rules() {
        assert!(INSTRUCTIONS.contains("Do not upload *.iter.md node files: the iter engine indexes them from the checkout after each map sync."));
        for want in ["searchable", "next_cursor", "set by the server"] {
            assert!(INSTRUCTIONS.contains(want), "{want}");
        }
    }

    #[test]
    fn every_tool_has_a_schema_and_unique_name() {
        let t = tools();
        let mut names: Vec<&str> = t.iter().map(|x| x["name"].as_str().unwrap()).collect();
        let n = names.len();
        names.sort();
        names.dedup();
        assert_eq!(names.len(), n);
        assert!(t.iter().all(|x| x["inputSchema"]["type"] == "object" && !x["description"].as_str().unwrap().is_empty()));
    }

    #[test]
    fn lockdirs_and_encoding() {
        assert_eq!(lockdir("src/x"), "{topdir}/src/x");
        assert_eq!(lockdir("{topdir}/a"), "{topdir}/a");
        assert_eq!(seg("{topdir}/a b").unwrap(), "%7Btopdir%7D%2Fa%20b");
    }

    #[test]
    fn api_path_encodes_every_segment() {
        assert_eq!(seg("a/b?c#d%e f").unwrap(), "a%2Fb%3Fc%23d%25e%20f");
        assert_eq!((seg("..").unwrap().as_str(), seg(".").unwrap().as_str()), ("%2E%2E", "%2E"));
        assert_eq!(seg("x.code.iter.md").unwrap(), "x.code.iter.md");
        assert!(seg("").is_err());
        assert!(seg("a\nb").is_err() && seg("a\u{7f}").is_err());
        assert!(seg(&"a".repeat(201)).is_err());
        assert!(seg(&"a".repeat(200)).is_ok());
        assert_eq!(api_path(&["projects", "p q", "workitems", "id/explain?"], &[("x", "1&y=2")]).unwrap(),
                   "/api/projects/p%20q/workitems/id%2Fexplain%3F?x=1%26y%3D2");
    }
}
