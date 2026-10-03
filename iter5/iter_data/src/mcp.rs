//! Stateless MCP (iter4 2026-09-29, iter5): every agent-facing call of the iter_data
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
    role: String,
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
    let caller = Caller { auth, project: hdr("x-iter-project"), workid: hdr("x-iter-workid"), sub: user.sub, role: user.role };
    match msg {
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
                "serverInfo": {"name": "iter_data", "title": "iter5 data API", "version": env!("CARGO_PKG_VERSION")},
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
            let text = serde_json::to_string_pretty(&body).unwrap_or_default();
            let mut result = json!({"content": [{"type": "text", "text": text}], "isError": is_error});
            if body.is_object() {
                result["structuredContent"] = body;
            }
            ok(&id, result)
        }
        "resources/list" => ok(&id, json!({"resources": []})),
        "prompts/list" => ok(&id, json!({"prompts": []})),
        other => err(&id, -32601, &format!("method not found: {other}")),
    })
}

const INSTRUCTIONS: &str = "iter5's data API as tools. Work items: status, list, get, details, create (iter add), ask, reject, wait, doc, block. \
The project graph (nodes = the *.iter.md node files): graph_* tools look a node up by path or name, walk its neighbours, find the code node that owns a path, \
and edit the graph — graph_node_create / graph_node_update / graph_edge_add / graph_edge_remove change the nodes at once and an engine writes the files. \
Requirements are the `## ` sections of bizreq/techreq files (graph nodes of type req): req_create / req_update / req_move / req_delete edit one section. \
testlogs: the test results of test nodes. settings_graph: the settings graph (engines, projects, accounts, agents and their edges). \
GraphRAG: rag_search finds the chunks of the project's documents and *.iter.md node files closest in meaning to a question and returns the FULL chunk text, \
with its chapter and document summaries and, for node files, the node's neighbours in the map — search before reading files blind. \
Most tools take `project` (default: the X-Iter-Project header); ask/reject/wait/block act on the calling work item (`workid`, default X-Iter-Workid).";

// ---------- the tool catalogue ----------

fn p_project() -> Value {
    json!({"type": "string", "description": "project name (default: the X-Iter-Project header)"})
}
fn p_workid() -> Value {
    json!({"type": "string", "description": "the calling work item id (default: the X-Iter-Workid header)"})
}
fn tool(name: &str, title: &str, desc: &str, props: Value, required: &[&str], read_only: bool) -> Value {
    json!({
        "name": name, "title": title, "description": desc,
        "inputSchema": {"type": "object", "properties": props, "required": required},
        "annotations": {"readOnlyHint": read_only, "destructiveHint": false, "idempotentHint": read_only, "openWorldHint": false},
    })
}

fn tools() -> Vec<Value> {
    let strs = json!({"type": "array", "items": {"type": "string"}});
    vec![
        // ----- work items -----
        tool("status", "Project status", "Open work items (in progress, queued, question, paused, parked, scheduled) ordered like `iter status`, plus wait-for deadlocks and stale lock rows.",
            json!({"project": p_project()}), &[], true),
        tool("workitem_list", "List work items", "Work items of a project, newest first, filtered. Returns id, name, state, agent, priority, tags, blockedby.",
            json!({"project": p_project(), "state": {"type": "string", "description": "only this state (queued, in-progress, question, complete, …)"},
                   "text": {"type": "string", "description": "case-insensitive substring of the name"}, "tag": {"type": "string"},
                   "limit": {"type": "integer", "description": "default 50, max 500"}}), &[], true),
        tool("workitem_get", "Get a work item", "One work item's full record. `id` may be the full id or a unique suffix (the webui shows the last 12 characters).",
            json!({"project": p_project(), "id": {"type": "string"}}), &["id"], true),
        tool("workitem_details", "Work item history", "A work item's detail rows (request, runs, doc notes, questions and answers, reviews), in order.",
            json!({"project": p_project(), "id": {"type": "string"}}), &["id"], true),
        tool("workitem_create", "Create a work item (iter add)",
            "File new work. Inside a work item it becomes that item's child (inherits priority and use-case tags). A `question` files a question for the human instead of runnable work.",
            json!({"project": p_project(), "workid": p_workid(),
                   "title": {"type": "string"}, "agent": {"type": "string", "description": "code | plan | test | … (default code)"},
                   "request": {"type": "string", "description": "the instructions: what to do and how to know it is done"},
                   "codepaths": {"type": "array", "items": {"type": "string"}, "description": "lock scope: {topdir}/… or repo-relative paths"},
                   "priority": {"type": "integer", "description": "0–99, lower = sooner (ignored inside an agent: children inherit)"},
                   "depends_on": {"type": "array", "items": {"type": "string"}, "description": "ids or unique suffixes this item waits for (deep)"},
                   "depends_on_shallow": {"type": "boolean"}, "context": strs.clone(),
                   "model": {"type": "string"}, "tags": strs.clone(), "usecase": {"type": "string"},
                   "question": {"type": "string", "description": "file the item AS a question for a human (the six-part shape); it queues for work once answered"}}), &["title"], false),
        tool("workitem_ask", "Ask the human (iter ask)", "Record a question on the calling work item; it parks in `question` when this turn ends and queues again once answered. Finish your turn after calling this.",
            json!({"project": p_project(), "workid": p_workid(), "question": {"type": "string"}}), &["question"], false),
        tool("workitem_reject", "Reject the calling item (iter reject)", "The calling work item is invalid: it parks for human review with the reason recorded; no retry is burned.",
            json!({"project": p_project(), "workid": p_workid(), "reason": {"type": "string"}}), &["reason"], false),
        tool("workitem_wait", "Wait on other items (iter wait)", "Declare that the calling item cannot finish until other items land: links them as deep dependencies; the close gate then queues this item behind them.",
            json!({"project": p_project(), "workid": p_workid(), "on": strs.clone(), "reason": {"type": "string"}}), &["on"], false),
        tool("workitem_doc", "Append a note (iter doc)", "Append a `doc` detail row to a work item (the calling one by default; allowed on closed items too).",
            json!({"project": p_project(), "workid": p_workid(), "id": {"type": "string", "description": "target item (default: the calling item)"}, "text": {"type": "string"}}), &["text"], false),
        tool("workitem_block", "Block on the cluster restart (iter block)", "Park the calling item until the nightly cluster restart finishes, giving its attempt back. Refused when tonight's rebuild was skipped.",
            json!({"project": p_project(), "workid": p_workid(), "reason": {"type": "string"}}), &[], false),
        tool("capability", "Capabilities (iter capability)", "Without `name`: the list of capability documents. With `name`: that capability's full text.",
            json!({"name": {"type": "string"}}), &[], true),
        tool("locks_list", "Locks", "The project's lock rows: path, holder work item, expiry, whether the holder's lease is live.",
            json!({"project": p_project()}), &[], true),
        // ----- the map -----
        tool("graph_stats", "Graph stats", "Counts of the project graph: nodes by type/level, edges by kind, file sync states, nodes unreachable from the project node, red tests.", json!({"project": p_project()}), &[], true),
        tool("graph_lookup", "Find a map node", "A node of the architecture map by `path` ({topdir}/…/x.code.iter.md) or by `name` (+ optional nodetype).",
            json!({"project": p_project(), "path": {"type": "string"}, "name": {"type": "string"}, "nodetype": {"type": "string"}}), &[], true),
        tool("graph_node", "Get a map node", "One node of the map by id.", json!({"project": p_project(), "id": {"type": "string"}}), &["id"], true),
        tool("graph_neighbors", "Map neighbours", "Nodes linked to a node, to a depth, following links out, in, or both.",
            json!({"project": p_project(), "id": {"type": "string"}, "depth": {"type": "integer"}, "direction": {"type": "string", "enum": ["out", "in", "any"]}}), &["id"], true),
        tool("graph_owner", "Owner of a path", "The code nodes whose code directories contain a path, deepest first.",
            json!({"project": p_project(), "path": {"type": "string"}}), &["path"], true),
        tool("graph_usecase", "A use case's parts", "A use case (id or name) with the code nodes it uses, everything they own, and its actors.",
            json!({"project": p_project(), "ucid": {"type": "string"}}), &["ucid"], true),
        tool("graph_node_create", "Create a graph node",
            "Create a node (and its file, written by an engine): nodetype project|code|test|bizreq|techreq|philosophy|usecase|actor; code nodes take level context|container|component|connection. `attach_to` (a node id) hangs it under a parent by `attach_kind` (codenodes|tests|reqs|uses|drives|touches|supplies|connects; inferred when omitted). The file path follows the designer folder rules. Returns {node, touched}.",
            json!({"project": p_project(), "nodetype": {"type": "string"}, "level": {"type": "string"}, "name": {"type": "string"}, "desc": {"type": "string", "description": "~100 words: enough to decide whether to read the whole node"},
                   "body": {"type": "string", "description": "markdown body (a bizreq/techreq: the one requirement + rationale)"}, "attach_to": {"type": "string"}, "attach_kind": {"type": "string"},
                   "front": {"type": "object", "description": "type-specific keys (status, owner, flowmap, connects, …)"}, "children": {"type": "object", "description": "codedirs/codenodes/tests/reqs path lists"}}), &["nodetype", "name"], false),
        tool("graph_node_update", "Update a graph node",
            "Change a node's name, desc, body, teststate, level, front keys (null removes one) or children lists (a given list replaces the old one). `expect_version` refuses the write when someone changed the node since you read it. Renaming never moves the file.",
            json!({"project": p_project(), "id": {"type": "string"}, "expect_version": {"type": "integer"}, "name": {"type": "string"}, "desc": {"type": "string"}, "body": {"type": "string"},
                   "teststate": {"type": "string", "enum": ["inherit", "include", "omit", "block"]}, "level": {"type": "string"}, "front": {"type": "object"}, "children": {"type": "object"}}), &["id"], false),
        tool("graph_edge_add", "Add a graph edge",
            "Add an edge between two nodes (ids). kind: codenodes (owns a code node), tests, reqs, uses (use case → code), drives (actor → use case), touches (actor → code), supplies (code → connection), connects (connection → code). It is written into the owning node's file.",
            json!({"project": p_project(), "from": {"type": "string"}, "to": {"type": "string"}, "kind": {"type": "string"}}), &["from", "to", "kind"], false),
        tool("graph_edge_remove", "Remove a graph edge",
            "Remove an edge (from, to, kind) with a reason. Refused when only a glob pattern of the owning node matches the target: change that pattern with graph_node_update instead.",
            json!({"project": p_project(), "from": {"type": "string"}, "to": {"type": "string"}, "kind": {"type": "string"}, "reason": {"type": "string"}}), &["from", "to", "kind", "reason"], false),
        // ----- requirements (sections of bizreq / techreq files) -----
        tool("req_create", "Add a requirement",
            "Add one requirement (a `## ` section) to a requirement file: name the file (`file`, a bizreq/techreq node id) or the code/project node it belongs to plus `type` (bizreq | techreq) — that node's file is created in its reqs/ folder when it has none. `after` (a req id of that file) places it; default: last. Returns {req, file}.",
            json!({"project": p_project(), "file": {"type": "string"}, "node": {"type": "string"}, "type": {"type": "string", "enum": ["bizreq", "techreq"]},
                   "key": {"type": "string", "description": "e.g. PDY-TECH-034 (optional)"}, "title": {"type": "string"},
                   "text": {"type": "string", "description": "markdown; use ### or lower for sub-headings"}, "status": {"type": "string", "enum": ["draft", "agreed", "done"]},
                   "after": {"type": "string"}}), &["title"], false),
        tool("req_update", "Update a requirement",
            "Change a requirement's key, title, text or status (the other sections of its file are untouched). `expect_version` = the file node's node_version: refused when the file changed since you read it.",
            json!({"project": p_project(), "id": {"type": "string"}, "key": {"type": "string"}, "title": {"type": "string"}, "text": {"type": "string"},
                   "status": {"type": "string", "enum": ["draft", "agreed", "done"]}, "expect_version": {"type": "integer"}}), &["id"], false),
        tool("req_move", "Move a requirement",
            "Move a requirement to another requirement file of the project (same id), or reorder it within its file; `after` (a req id of the target file) places it, default last.",
            json!({"project": p_project(), "id": {"type": "string"}, "to_file": {"type": "string"}, "after": {"type": "string"}}), &["id", "to_file"], false),
        tool("req_delete", "Delete a requirement", "Remove one requirement from its file, with a reason (recorded in the commit).",
            json!({"project": p_project(), "id": {"type": "string"}, "reason": {"type": "string"}}), &["id", "reason"], false),
        tool("testlogs", "Test logs", "The test results posted for the project's test nodes, newest first: outcome, bucket counts, failing details, engine, work item.",
            json!({"project": p_project(), "node": {"type": "string", "description": "only this test node (id)"}, "limit": {"type": "integer", "description": "default 50, max 1000"}}), &[], true),
        tool("settings_graph", "Settings graph", "The settings graph: nodes (iter_data, engines, projects, agents, tooling, users, accounts, providers, work item states) and edges (serves, bills, holds, member, runs, …) with their settings. Admins see all; others the parts touching their projects.",
            json!({}), &[], true),
        // ----- GraphRAG -----
        tool("rag_search", "Search the project's documents",
            "Search the project's GraphRAG index (uploaded documents and *.iter.md node files) by meaning and by keyword at once. Each result is a FULL chunk of text with its chapter and document summaries and its place in the map: a node file's neighbours and linked documents, or the nodes that link an uploaded document. Use it before reading files blind, and before asking a human something the project's documents may already answer.",
            json!({"project": p_project(), "query": {"type": "string", "description": "a question or description in plain words"},
                   "k": {"type": "integer", "description": "results, default 8, max 50"},
                   "kinds": {"type": "array", "items": {"type": "string", "enum": ["file", "node", "guide"]}, "description": "file = uploads, node = *.iter.md files, guide = the built-in iter user guide (default: all)"},
                   "include_guide": {"type": "boolean", "description": "also search the iter user guide (how to use iter itself); default true"},
                   "nodetypes": {"type": "array", "items": {"type": "string"}, "description": "node files of these types only: project, code, test, bizreq, techreq, philosophy, usecase, actor"},
                   "doc": {"type": "string", "description": "search inside one document"},
                   "mode": {"type": "string", "enum": ["hybrid", "vector", "keyword"], "description": "hybrid (default): keyword + raw-text + summary rankings fused; keyword alone for exact names and codes"},
                   "vectors": {"type": "string", "enum": ["both", "raw", "summary"], "description": "which vectors the vector rankings use (default both)"},
                   "docs": {"type": "boolean", "description": "also rank whole documents by their summaries"},
                   "graph": {"type": "boolean", "description": "attach map neighbours to node hits (default true)"}}), &["query"], true),
        tool("rag_status", "GraphRAG status", "Documents by kind and state, chunks by summary state, work waiting for the Summary agent, the docs directory, the embedding model, live engines, the change-sweep schedule.",
            json!({"project": p_project()}), &[], true),
        tool("rag_docs", "GraphRAG documents", "Every document in the project's GraphRAG index, one per document (no text, no match score): kind file|node, title, path, state, chunk and chapter counts, its summary, and `locations` — the absolute path of its file on each engine's checkout.",
            json!({"project": p_project(), "kind": {"type": "string", "enum": ["file", "node"]}, "state": {"type": "string"}}), &[], true),
        tool("rag_doc", "One GraphRAG document", "A document with its chapter summaries and every chunk (text + summary).",
            json!({"project": p_project(), "id": {"type": "string"}}), &["id"], true),
        tool("rag_link_document", "Link a document to a map node",
            "Record that an uploaded document describes a map node: the node's `children.documents` gains the document (at once in the graph; an engine writes the node file), so searches that hit either one bring the other. `unlink: true` removes the link.",
            json!({"project": p_project(), "doc": {"type": "string", "description": "the GraphRAG document id (rag_docs lists them)"},
                   "node": {"type": "string", "description": "the node file, {topdir}/…/x.code.iter.md, or the node id (graph_lookup finds it)"},
                   "unlink": {"type": "boolean"}}), &["doc", "node"], false),
        tool("rag_add_document", "Add a document to GraphRAG",
            "Index a document: give `text` (a UTF-8 document, e.g. markdown) or `content_b64` (any supported file: pdf, docx, html, md, txt). It is searchable at once; the Summary agent adds summaries shortly after, and the original is committed to the project's docs directory unless store=false.",
            json!({"project": p_project(), "filename": {"type": "string", "description": "e.g. design-notes.md (decides the format and the stored name)"},
                   "text": {"type": "string"}, "content_b64": {"type": "string"}, "title": {"type": "string"}, "store": {"type": "boolean"}}), &["filename"], false),
    ]
}

// ---------- dispatch ----------

fn a<'a>(args: &'a Value, k: &str) -> &'a str {
    args.get(k).and_then(|x| x.as_str()).unwrap_or("").trim()
}
fn arr(args: &Value, k: &str) -> Vec<String> {
    args.get(k).and_then(|x| x.as_array()).map(|v| v.iter().filter_map(|x| x.as_str().map(|s| s.trim().to_string())).filter(|s| !s.is_empty()).collect()).unwrap_or_default()
}
fn enc(s: &str) -> String {
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
            let msg = v.get("error").and_then(|e| e.as_str()).map(String::from).unwrap_or_else(|| v.to_string());
            Err(format!("{} {}: {msg}", status.as_u16(), path))
        }
    }

    async fn items(&self, c: &Caller, project: &str) -> Result<Vec<Value>, String> {
        Ok(self.call(c, "GET", &format!("/api/projects/{}/workitems", enc(project)), None).await?.as_array().cloned().unwrap_or_default())
    }

    /// Full id from a full id or unique prefix/suffix (like `iter`'s resolve_id).
    async fn resolve(&self, c: &Caller, project: &str, needle: &str, all: Option<&[Value]>) -> Result<String, String> {
        let owned;
        let all = match all {
            Some(a) => a,
            None => {
                owned = self.items(c, project).await?;
                &owned
            }
        };
        let n = needle.trim();
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
}

fn project_of(c: &Caller, args: &Value) -> Result<String, String> {
    let p = if a(args, "project").is_empty() { c.project.clone() } else { a(args, "project").to_string() };
    if p.is_empty() { Err("no project: pass `project` or send the X-Iter-Project header".into()) } else { Ok(p) }
}
fn workid_of(c: &Caller, args: &Value) -> Result<String, String> {
    let w = if a(args, "workid").is_empty() { c.workid.clone() } else { a(args, "workid").to_string() };
    if w.is_empty() { Err("no calling work item: pass `workid` or send the X-Iter-Workid header".into()) } else { Ok(w) }
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

async fn call_tool(m: &Mcp, c: &Caller, name: &str, args: &Value) -> Result<Value, String> {
    match name {
        "capability" => {
            let rows = m.call(c, "GET", "/api/tooling", None).await?.as_array().cloned().unwrap_or_default();
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
        "settings_graph" => m.call(c, "GET", "/api/settings/graph", None).await,
        _ => {
            let project = project_of(c, args)?;
            let pp = format!("/api/projects/{}", enc(&project));
            project_tool(m, c, name, args, &project, &pp).await
        }
    }
}

async fn project_tool(m: &Mcp, c: &Caller, name: &str, args: &Value, project: &str, pp: &str) -> Result<Value, String> {
    match name {
        "status" => {
            let mut all = m.items(c, project).await?;
            let order = |s: &str| match s {
                "in-progress" => 0, "queued" => 1, "question" => 2, "paused" => 3, "parked" => 4, "scheduled" => 5, _ => 7,
            };
            all.retain(|i| !matches!(i["state"].as_str(), Some("complete") | Some("failed")));
            all.sort_by_key(|i| (order(i["state"].as_str().unwrap_or("")), i["priority"].as_i64().unwrap_or(5)));
            let deadlocks = m.call(c, "GET", &format!("{pp}/deadlocks"), None).await.unwrap_or(Value::Null);
            Ok(json!({"project": project, "open": all.iter().map(brief).collect::<Vec<_>>(), "open_count": all.len(),
                      "deadlocks": deadlocks.get("cycles").cloned().unwrap_or(json!([])),
                      "stale_lock_rows": deadlocks.get("stale_lock_rows").and_then(|s| s.as_array()).map(|a| a.len()).unwrap_or(0)}))
        }
        "workitem_list" => {
            let mut all = m.items(c, project).await?;
            let st = a(args, "state");
            let text = a(args, "text").to_lowercase();
            let tag = a(args, "tag");
            all.retain(|i| {
                (st.is_empty() || i["state"] == st)
                    && (text.is_empty() || i["name"].as_str().unwrap_or("").to_lowercase().contains(&text))
                    && (tag.is_empty() || i["tags"].as_array().map(|t| t.iter().any(|x| x["text"] == tag)).unwrap_or(false))
            });
            all.sort_by(|x, y| y["created"].as_str().unwrap_or("").cmp(x["created"].as_str().unwrap_or("")));
            let limit = args.get("limit").and_then(|l| l.as_u64()).unwrap_or(50).clamp(1, 500) as usize;
            let total = all.len();
            Ok(json!({"total": total, "items": all.iter().take(limit).map(brief).collect::<Vec<_>>()}))
        }
        "workitem_get" => {
            let id = m.resolve(c, project, a(args, "id"), None).await?;
            m.call(c, "GET", &format!("{pp}/workitems/{id}"), None).await
        }
        "workitem_details" => {
            let id = m.resolve(c, project, a(args, "id"), None).await?;
            Ok(json!({"id": id, "details": m.call(c, "GET", &format!("{pp}/workitems/{id}/details"), None).await?}))
        }
        "workitem_create" => create(m, c, args, project, pp).await,
        "workitem_ask" => {
            let q = a(args, "question");
            if q.is_empty() {
                return Err("the question is empty".into());
            }
            let wid = workid_of(c, args)?;
            let item = m.call(c, "GET", &format!("{pp}/workitems/{wid}"), None).await?;
            m.call(c, "POST", &format!("{pp}/workitems/{wid}/details"), Some(&json!({"key": "question", "valuetype": "json", "value": question_widget(q)}))).await?;
            set_state(m, c, pp, &wid, item, "question", None).await?;
            Ok(json!({"ok": true, "message": "question recorded — this work item parks in `question` when this turn ends and queues again once a human answers. Finish your turn now."}))
        }
        "workitem_reject" => {
            let reason = a(args, "reason");
            if reason.is_empty() {
                return Err("a reason is required".into());
            }
            let wid = workid_of(c, args)?;
            let item = m.call(c, "GET", &format!("{pp}/workitems/{wid}"), None).await?;
            let agent = item["agent"].as_str().unwrap_or("").to_string();
            let _ = m.call(c, "POST", &format!("{pp}/workitems/{wid}/details"), Some(&json!({"key": "doc", "valuetype": "text", "value": format!("rejected by the {agent} agent: {reason}")}))).await;
            let note: String = format!("rejected: {}", reason.chars().take(400).collect::<String>());
            set_state(m, c, pp, &wid, item, "parked", Some(&note)).await?;
            Ok(json!({"ok": true, "message": "rejected — this work item parks for human review when this turn ends. Finish your turn now."}))
        }
        "workitem_wait" => {
            let wid = workid_of(c, args)?;
            let all = m.items(c, project).await?;
            let mut ids = Vec::new();
            for n in arr(args, "on") {
                ids.push(m.resolve(c, project, &n, Some(&all)).await?);
            }
            if ids.is_empty() {
                return Err("`on` names no work item".into());
            }
            let mut item = m.call(c, "GET", &format!("{pp}/workitems/{wid}"), None).await?;
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
                return Ok(json!({"ok": true, "message": "already waiting on all of them — nothing to add"}));
            }
            item["blockedby"] = json!(merged);
            m.call(c, "PUT", &format!("{pp}/workitems/{wid}?expect_version={version}"), Some(&item)).await?;
            let reason: String = a(args, "reason").chars().take(400).collect();
            let _ = m
                .call(c, "POST", &format!("{pp}/workitems/{wid}/details"), Some(&json!({"key": "doc", "valuetype": "text", "value": format!(
                    "waiting on {} (the {} agent, attempt {}){}", added.join(", "), item["agent"].as_str().unwrap_or(""),
                    item["attempt"].as_u64().unwrap_or(0), if reason.is_empty() { String::new() } else { format!(": {reason}") })})))
                .await;
            Ok(json!({"ok": true, "added": added, "message": "linked as dependencies. Finish what you can, then end your turn listing what still waits on them as NOT DONE lines: the close gate queues this item behind them and the engine re-runs it once they close complete."}))
        }
        "workitem_doc" => {
            let text = args.get("text").and_then(|t| t.as_str()).unwrap_or("").trim_end();
            if text.trim().is_empty() {
                return Err("doc text is empty".into());
            }
            let target = if !a(args, "id").is_empty() { m.resolve(c, project, a(args, "id"), None).await? } else { workid_of(c, args)? };
            let row = m.call(c, "POST", &format!("{pp}/workitems/{target}/details"), Some(&json!({"key": "doc", "valuetype": "text", "value": text}))).await?;
            Ok(json!({"ok": true, "id": target, "order": row["order"]}))
        }
        "workitem_block" => block(m, c, args, project, pp).await,
        "locks_list" => m.call(c, "GET", &format!("{pp}/locks"), None).await,
        "graph_stats" => m.call(c, "GET", &format!("{pp}/graph/stats"), None).await,
        "graph_lookup" => {
            let q = format!("path={}&name={}&nodetype={}", enc(a(args, "path")), enc(a(args, "name")), enc(a(args, "nodetype")));
            m.call(c, "GET", &format!("{pp}/graph/lookup?{q}"), None).await
        }
        "graph_node" => m.call(c, "GET", &format!("{pp}/graph/nodes/{}", enc(a(args, "id"))), None).await,
        "graph_neighbors" => {
            let depth = args.get("depth").and_then(|d| d.as_u64()).unwrap_or(1);
            let dir = if a(args, "direction").is_empty() { "any" } else { a(args, "direction") };
            m.call(c, "GET", &format!("{pp}/graph/nodes/{}/neighbors?depth={depth}&direction={}", enc(a(args, "id")), enc(dir)), None).await
        }
        "graph_owner" => m.call(c, "GET", &format!("{pp}/graph/owner?path={}", enc(a(args, "path"))), None).await,
        "graph_usecase" => m.call(c, "GET", &format!("{pp}/graph/usecases/{}", enc(a(args, "ucid"))), None).await,
        "graph_node_create" => {
            let mut body = args.clone();
            if let Some(o) = body.as_object_mut() {
                o.remove("project");
            }
            m.call(c, "POST", &format!("{pp}/graph/nodes"), Some(&body)).await
        }
        "graph_node_update" => {
            let mut body = args.clone();
            if let Some(o) = body.as_object_mut() {
                o.remove("project");
                o.remove("id");
            }
            m.call(c, "PATCH", &format!("{pp}/graph/nodes/{}", enc(a(args, "id"))), Some(&body)).await
        }
        "graph_edge_add" => {
            let body = json!({"from": a(args, "from"), "to": a(args, "to"), "kind": a(args, "kind")});
            m.call(c, "POST", &format!("{pp}/graph/edges"), Some(&body)).await
        }
        "graph_edge_remove" => {
            let body = json!({"from": a(args, "from"), "to": a(args, "to"), "kind": a(args, "kind"), "reason": a(args, "reason")});
            m.call(c, "DELETE", &format!("{pp}/graph/edges"), Some(&body)).await
        }
        "req_create" => {
            let mut body = args.clone();
            if let Some(o) = body.as_object_mut() {
                o.remove("project");
            }
            m.call(c, "POST", &format!("{pp}/graph/reqs"), Some(&body)).await
        }
        "req_update" => {
            let mut body = args.clone();
            if let Some(o) = body.as_object_mut() {
                o.remove("project");
                o.remove("id");
            }
            m.call(c, "PATCH", &format!("{pp}/graph/reqs/{}", enc(a(args, "id"))), Some(&body)).await
        }
        "req_move" => {
            let mut body = json!({"to_file": a(args, "to_file")});
            if !a(args, "after").is_empty() {
                body["after"] = json!(a(args, "after"));
            }
            m.call(c, "POST", &format!("{pp}/graph/reqs/{}/move", enc(a(args, "id"))), Some(&body)).await
        }
        "req_delete" => {
            let body = json!({"reason": a(args, "reason")});
            m.call(c, "DELETE", &format!("{pp}/graph/reqs/{}", enc(a(args, "id"))), Some(&body)).await
        }
        "testlogs" => {
            let limit = args.get("limit").and_then(|l| l.as_u64()).unwrap_or(50);
            m.call(c, "GET", &format!("{pp}/testlogs?node={}&limit={limit}", enc(a(args, "node"))), None).await
        }
        "rag_search" => {
            let mut body = args.clone();
            if let Some(o) = body.as_object_mut() {
                o.remove("project");
            }
            m.call(c, "POST", &format!("{pp}/rag/search"), Some(&body)).await
        }
        "rag_status" => m.call(c, "GET", &format!("{pp}/rag"), None).await,
        "rag_docs" => {
            let q = format!("kind={}&state={}", enc(a(args, "kind")), enc(a(args, "state")));
            Ok(json!({"documents": m.call(c, "GET", &format!("{pp}/rag/docs?{q}"), None).await?}))
        }
        "rag_doc" => m.call(c, "GET", &format!("{pp}/rag/docs/{}", enc(a(args, "id"))), None).await,
        "rag_link_document" => {
            let body = json!({"node": a(args, "node"), "unlink": args.get("unlink").and_then(|u| u.as_bool()).unwrap_or(false)});
            m.call(c, "POST", &format!("{pp}/rag/docs/{}/links", enc(a(args, "doc"))), Some(&body)).await
        }
        "rag_add_document" => {
            use base64::Engine as _;
            let b64 = if !a(args, "content_b64").is_empty() {
                a(args, "content_b64").to_string()
            } else {
                let t = args.get("text").and_then(|t| t.as_str()).unwrap_or("");
                if t.trim().is_empty() {
                    return Err("give `text` or `content_b64`".into());
                }
                base64::engine::general_purpose::STANDARD.encode(t.as_bytes())
            };
            let body = json!({"filename": a(args, "filename"), "content_b64": b64, "title": a(args, "title"),
                              "store": args.get("store").and_then(|s| s.as_bool()).unwrap_or(true)});
            m.call(c, "POST", &format!("{pp}/rag/docs"), Some(&body)).await
        }
        other => Err(format!("unknown tool {other}")),
    }
}

fn brief(i: &Value) -> Value {
    json!({"id": i["id"], "name": i["name"], "state": i["state"], "agent": i["agent"], "priority": i["priority"],
           "tags": i["tags"].as_array().map(|t| t.iter().map(|x| x["text"].clone()).collect::<Vec<_>>()).unwrap_or_default(),
           "blockedby": i["blockedby"], "created": i["created"]})
}

async fn set_state(m: &Mcp, c: &Caller, pp: &str, wid: &str, mut item: Value, state: &str, note: Option<&str>) -> Result<(), String> {
    let version = item["version"].as_u64().unwrap_or(1);
    item["state"] = json!(state);
    if let Some(n) = note {
        item["lasterror"] = json!(n);
    }
    m.call(c, "PUT", &format!("{pp}/workitems/{wid}?expect_version={version}"), Some(&item)).await.map(|_| ())
}

/// `iter add`, the same body the CLI sends.
async fn create(m: &Mcp, c: &Caller, args: &Value, project: &str, pp: &str) -> Result<Value, String> {
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
    // inside an agent: the calling item is the parent
    let workid = if a(args, "workid").is_empty() { c.workid.clone() } else { a(args, "workid").to_string() };
    let all = m.items(c, project).await?;
    let mut blockedby = Vec::new();
    let mut deps = arr(args, "depends_on");
    deps.extend(arr(args, "blockedby"));
    for d in deps {
        blockedby.push(m.resolve(c, project, &d, Some(&all)).await?);
    }
    if !workid.is_empty() && blockedby.contains(&workid) {
        return Err("an item cannot depend on the item that creates it".into());
    }
    let calling_agent = if workid.is_empty() {
        String::new()
    } else {
        all.iter().find(|i| i["id"] == workid.as_str()).and_then(|i| i["agent"].as_str()).unwrap_or("").to_string()
    };
    let state = if !question.is_empty() {
        "question".to_string()
    } else {
        let proj = m.call(c, "GET", pp, None).await.unwrap_or(json!({}));
        let over = proj.pointer(&format!("/agents/{calling_agent}/childstate")).and_then(|x| x.as_str()).map(String::from);
        let def = if calling_agent.is_empty() {
            None
        } else {
            m.call(c, "GET", &format!("/api/agents/{}", enc(&calling_agent)), None).await.ok().and_then(|a| a["childstate"].as_str().map(String::from))
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
    let requestedby = if calling_agent.is_empty() {
        if c.role == "engine" { "agent".to_string() } else { format!("user:{}", c.sub) }
    } else {
        format!("agent:{calling_agent}")
    };
    let usecase = a(args, "usecase");
    let mut paths = arr(args, "codepaths");
    paths.extend(arr(args, "lockdirs"));
    if !a(args, "codepath").is_empty() {
        paths.insert(0, a(args, "codepath").to_string());
    }
    let lockdirs: Vec<String> = paths.iter().map(|p| lockdir(p)).collect();
    let body = json!({
        "name": title, "agent": agent, "state": state, "priority": args.get("priority").and_then(|p| p.as_i64()),
        "lockdirs": lockdirs,
        "blockedby": blockedby, "blockedby_shallow": args.get("depends_on_shallow").and_then(|b| b.as_bool()).unwrap_or(false),
        "context": arr(args, "context"), "model": a(args, "model"), "tags": tags,
        "usecase": if usecase.is_empty() { Value::Null } else { json!(usecase) },
        "createdby": if workid.is_empty() { requestedby.clone() } else { workid.clone() },
        "requestedby": requestedby, "prework": [], "postwork": [], "request": request,
    });
    let created = m.call(c, "POST", &format!("{pp}/workitems"), Some(&body)).await?;
    let id = created["id"].as_str().unwrap_or("").to_string();
    let already = created.get("already_open").and_then(|b| b.as_bool()).unwrap_or(false);
    if !already && !question.is_empty() {
        let _ = m.call(c, "POST", &format!("{pp}/workitems/{id}/details"), Some(&json!({"key": "question", "valuetype": "json", "value": question_widget(question)}))).await;
    }
    Ok(json!({"id": id, "state": created["state"], "agent": created["agent"], "priority": created["priority"],
              "already_open": already, "warnings": created.get("warnings").cloned().unwrap_or(json!([])),
              "message": if already { "already open: the same check and container is filed; your request text was recorded on it. Do not file it again." } else { "added" }}))
}

/// `iter block --cluster-restart`.
async fn block(m: &Mcp, c: &Caller, args: &Value, project: &str, pp: &str) -> Result<Value, String> {
    let wid = workid_of(c, args)?;
    let reason: String = {
        let r = a(args, "reason");
        if r.is_empty() { "the cluster is required for this work".into() } else { r.chars().take(400).collect() }
    };
    let proj: Option<iter_core::Project> = m.call(c, "GET", pp, None).await.ok().and_then(|v| serde_json::from_value(v).ok());
    if let Some(proj) = proj {
        let all: Vec<iter_core::WorkItem> = m.items(c, project).await?.into_iter().filter_map(|v| serde_json::from_value(v).ok()).collect();
        if let Some(clone) = iter_core::cluster::newest_clone(&proj.cluster_restart, &all) {
            let details: Vec<Value> = m.call(c, "GET", &format!("{pp}/workitems/{}/details", clone.id), None).await.ok().and_then(|v| v.as_array().cloned()).unwrap_or_default();
            if iter_core::cluster::rebuild_skipped(Some(clone), &details, chrono::Utc::now()) {
                return Err(format!(
                    "refused: tonight's cluster rebuild was skipped (restart window {} posted \"skipped\"), so there is no restart to wait for — the cluster stays up. Do the work now; if the cluster is really unavailable, say so with workitem_ask.",
                    &clone.id[clone.id.len().saturating_sub(12)..]
                ));
            }
        }
    }
    let mut item = m.call(c, "GET", &format!("{pp}/workitems/{wid}"), None).await?;
    let version = item["version"].as_u64().unwrap_or(1);
    let before = item["attempt"].as_u64().unwrap_or(0);
    iter_core::cluster::apply_block(&mut item, &reason);
    let after = item["attempt"].as_u64().unwrap_or(0);
    let _ = m
        .call(c, "POST", &format!("{pp}/workitems/{wid}/details"), Some(&json!({"key": "doc", "valuetype": "text", "value": format!(
            "blocked by cluster restart (the {} agent): {reason} — attempt put back from {before} to {after}; parked until the cluster is back up and healthy",
            item["agent"].as_str().unwrap_or(""))})))
        .await;
    m.call(c, "PUT", &format!("{pp}/workitems/{wid}?expect_version={version}"), Some(&item)).await?;
    Ok(json!({"ok": true, "attempt_before": before, "attempt_after": after, "message": format!(
        "blocked on the cluster restart — this item parks when this turn ends, tagged `{}`; the engine requeues it once the cluster is healthy. Finish your turn now.",
        iter_core::cluster::CLUSTER_RESTART_TAG)}))
}

#[cfg(test)]
mod tests {
    use super::*;

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
        assert_eq!(enc("{topdir}/a b"), "%7Btopdir%7D%2Fa%20b");
    }
}
