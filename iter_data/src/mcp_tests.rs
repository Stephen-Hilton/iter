//! The MCP gateway end to end: the JSON-RPC protocol layer, the header
//! defaults, authn/authz, and the work-item and GraphRAG tools — each test on
//! its own throwaway ArangoDB database (test_db.rs), driven through the real
//! router. (The graph tools are covered by graph_tests::mcp_graph_tools.)

use crate::api::{AppState, NOSK};
use axum::Router;
use axum::body::Body;
use axum::http::Request;
use serde_json::{Value, json};
use std::sync::Arc;
use tower::ServiceExt;

struct T {
    st: Arc<AppState>,
    db: String,
    app: Router,
}

impl T {
    async fn new() -> T {
        let (a, db) = crate::test_db::store().await;
        let st = Arc::new(AppState::for_tests(Arc::new(a), b"mcp-test"));
        crate::settings::bootstrap(st.store.as_ref()).await.unwrap();
        for (u, role) in [("adm", "admin"), ("alice", "user")] {
            st.store.put("webui_user", u, NOSK, &json!({"user": u, "role": role, "tokenver": 1, "pwhash": ""})).await.unwrap();
        }
        let app = crate::api::router(st.clone()).merge(crate::mcp::routes(st.clone()));
        T { st, db, app }
    }
    async fn tok(&self, who: &str) -> String {
        match who {
            "eng1" => crate::keys::test_session(&self.st, "e1").await,
            "eng2" => crate::keys::test_session(&self.st, "e2").await,
            "adm" => crate::auth::mint_token(&self.st.secret, who, "admin", 1, 3600).unwrap(),
            _ => crate::auth::mint_token(&self.st.secret, who, "user", 1, 3600).unwrap(),
        }
    }
    async fn done(self) {
        crate::test_db::drop(&self.db).await;
    }
    /// raw request: (status, body) — `auth` is the whole Authorization header value
    async fn raw(&self, method: &str, auth: Option<&str>, headers: &[(&str, &str)], body: Option<Value>) -> (u16, Value) {
        let mut b = Request::builder().method(method).uri("/mcp");
        if let Some(a) = auth {
            b = b.header("authorization", a);
        }
        for (k, v) in headers {
            b = b.header(*k, *v);
        }
        let req = match body {
            Some(v) => b.header("content-type", "application/json").body(Body::from(serde_json::to_vec(&v).unwrap())).unwrap(),
            None => b.body(Body::empty()).unwrap(),
        };
        let resp = self.app.clone().oneshot(req).await.unwrap();
        let code = resp.status().as_u16();
        let bytes = axum::body::to_bytes(resp.into_body(), 64 << 20).await.unwrap();
        (code, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
    }
    /// a JSON-RPC request as `who`
    async fn rpc(&self, who: &str, headers: &[(&str, &str)], msg: Value) -> (u16, Value) {
        let auth = format!("Bearer {}", self.tok(who).await);
        self.raw("POST", Some(&auth), headers, Some(msg)).await
    }
    /// a tool call with the project / work item given as headers; the `result`
    async fn tool_h(&self, who: &str, project: &str, workid: &str, name: &str, args: Value) -> Value {
        let mut h = vec![("x-iter-project", project)];
        if !workid.is_empty() {
            h.push(("x-iter-workid", workid));
        }
        let (c, v) = self.rpc(who, &h, json!({"jsonrpc": "2.0", "id": 7, "method": "tools/call", "params": {"name": name, "arguments": args}})).await;
        assert_eq!(c, 200, "{v}");
        assert_eq!(v["id"], 7);
        v["result"].clone()
    }
    async fn tool(&self, who: &str, project: &str, name: &str, args: Value) -> Value {
        self.tool_h(who, project, "", name, args).await
    }
    /// the tool succeeded: its structured content
    async fn ok(&self, who: &str, project: &str, name: &str, args: Value) -> Value {
        let r = self.tool(who, project, name, args).await;
        assert_eq!(r["isError"], false, "{name}: {r}");
        r["structuredContent"].clone()
    }
    /// an API call (not MCP) that must succeed
    async fn api(&self, method: &str, path: &str, body: Value, tok: String) {
        let req = Request::builder().method(method).uri(path).header("authorization", format!("Bearer {tok}")).header("content-type", "application/json").body(Body::from(body.to_string())).unwrap();
        let r = self.app.clone().oneshot(req).await.unwrap();
        assert!(r.status().is_success(), "{path}: {}", r.status());
    }
    async fn served(&self, p: &str) {
        let adm = self.tok("adm").await;
        self.api("PUT", &format!("/api/projects/{p}"), json!({"state": "Running"}), adm.clone()).await;
        if self.st.store.get("engine", "e1", NOSK).await.unwrap().is_none() {
            self.api("PUT", "/api/engines/e1", json!({"state": "Running"}), self.tok("eng1").await).await;
        }
        self.api("POST", "/api/settings/edges", json!({"from": "iter_engine:e1", "to": format!("project:{p}"), "settings": {"topdir": format!("/w/{p}")}}), adm).await;
    }
    /// a queued work item filed over MCP; its id
    async fn item(&self, p: &str, title: &str) -> String {
        let r = self.ok("eng1", p, "workitem_create", json!({"title": title, "request": "do the thing; done when it is done"})).await;
        r["id"].as_str().unwrap().to_string()
    }
}

fn text_of(r: &Value) -> String {
    r["content"][0]["text"].as_str().unwrap_or("").to_string()
}

#[tokio::test]
async fn protocol_layer() {
    let t = T::new().await;
    // initialize: negotiates a known version, falls back to the newest for an unknown one, carries the instructions
    let (c, v) = t.rpc("eng1", &[], json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {"protocolVersion": "2025-03-26"}})).await;
    assert_eq!(c, 200);
    assert_eq!(v["result"]["protocolVersion"], "2025-03-26");
    assert_eq!(v["result"]["serverInfo"]["name"], "iter_data");
    assert_eq!(v["result"]["capabilities"]["tools"]["listChanged"], false);
    assert!(v["result"]["instructions"].as_str().unwrap().contains("rag_search"));
    let (_, v) = t.rpc("eng1", &[], json!({"jsonrpc": "2.0", "id": 2, "method": "initialize", "params": {"protocolVersion": "1999-01-01"}})).await;
    assert_eq!(v["result"]["protocolVersion"], "2025-06-18");
    assert_eq!(v["id"], 2);
    // string ids are echoed as they came
    let (_, v) = t.rpc("eng1", &[], json!({"jsonrpc": "2.0", "id": "abc", "method": "ping"})).await;
    assert_eq!(v, json!({"jsonrpc": "2.0", "id": "abc", "result": {}}));
    // a notification has no answer
    let (c, _) = t.rpc("eng1", &[], json!({"jsonrpc": "2.0", "method": "notifications/initialized"})).await;
    assert_eq!(c, 202);
    // a batch answers each request and skips notifications
    let (c, v) = t.rpc("eng1", &[], json!([
        {"jsonrpc": "2.0", "id": 1, "method": "ping"},
        {"jsonrpc": "2.0", "method": "notifications/initialized"},
        {"jsonrpc": "2.0", "id": 2, "method": "resources/list"},
        {"jsonrpc": "2.0", "id": 3, "method": "prompts/list"},
    ])).await;
    assert_eq!(c, 200);
    assert_eq!(v.as_array().unwrap().len(), 3);
    assert_eq!(v[1]["result"]["resources"], json!([]));
    // unknown method / unknown tool are JSON-RPC errors
    let (_, v) = t.rpc("eng1", &[], json!({"jsonrpc": "2.0", "id": 4, "method": "nope/nothing"})).await;
    assert_eq!(v["error"]["code"], -32601);
    let (_, v) = t.rpc("eng1", &[], json!({"jsonrpc": "2.0", "id": 5, "method": "tools/call", "params": {"name": "not_a_tool"}})).await;
    assert_eq!(v["error"]["code"], -32602);
    assert!(v["error"]["message"].as_str().unwrap().contains("not_a_tool"));
    // stateless: no stream, no session
    for m in ["GET", "DELETE"] {
        let auth = format!("Bearer {}", t.tok("eng1").await);
        assert_eq!(t.raw(m, Some(&auth), &[], None).await.0, 405, "{m}");
    }
    // the bearer token is checked before anything: none, garbage, and a good one for the wrong secret
    let ping = json!({"jsonrpc": "2.0", "id": 1, "method": "ping"});
    assert_eq!(t.raw("POST", None, &[], Some(ping.clone())).await.0, 401);
    assert_eq!(t.raw("POST", Some("Bearer garbage"), &[], Some(ping.clone())).await.0, 401);
    let forged = crate::auth::mint_token(b"another-secret", "adm", "admin", 1, 3600).unwrap();
    assert_eq!(t.raw("POST", Some(&format!("Bearer {forged}")), &[], Some(ping)).await.0, 401);
    t.done().await;
}

#[tokio::test]
async fn tools_list_is_complete_and_dispatchable() {
    let t = T::new().await;
    let (_, v) = t.rpc("eng1", &[], json!({"jsonrpc": "2.0", "id": 1, "method": "tools/list"})).await;
    let tools = v["result"]["tools"].as_array().unwrap().clone();
    let names: Vec<&str> = tools.iter().map(|x| x["name"].as_str().unwrap()).collect();
    assert_eq!(names.len(), 34, "the README and mcp.code.iter.md count the tools: {names:?}");
    for x in &tools {
        let n = x["name"].as_str().unwrap();
        assert!(x["title"].as_str().is_some_and(|s| !s.is_empty()), "{n}: title");
        assert!(x["description"].as_str().unwrap().len() > 20, "{n}: description");
        let props = x["inputSchema"]["properties"].as_object().unwrap();
        for r in x["inputSchema"]["required"].as_array().unwrap() {
            assert!(props.contains_key(r.as_str().unwrap()), "{n}: required `{r}` is not a property");
        }
        for (k, p) in props {
            assert!(p["type"].is_string(), "{n}.{k}: type");
        }
        // read-only tools say so, and no tool claims to destroy anything
        assert_eq!(x["annotations"]["destructiveHint"], false, "{n}");
    }
    // every tool is wired to a handler: called with no arguments, none answers "unknown tool"
    for n in &names {
        let r = t.tool("eng1", "", n, json!({})).await;
        assert!(!text_of(&r).contains("unknown tool"), "{n} has no handler: {r}");
        // and every failure is a tool error carrying its reason, not a protocol error
        if r["isError"] == true {
            assert!(r["structuredContent"]["error"].as_str().is_some_and(|e| !e.is_empty()), "{n}: {r}");
        }
    }
    t.done().await;
}

#[tokio::test]
async fn project_and_work_item_defaults_come_from_the_headers() {
    let t = T::new().await;
    t.served("p").await;
    // no project anywhere: a clear refusal naming both ways to give one
    let r = t.tool("eng1", "", "status", json!({})).await;
    assert_eq!(r["isError"], true);
    assert!(text_of(&r).contains("X-Iter-Project"), "{r}");
    // the header is the default; an argument overrides it
    let s = t.ok("eng1", "p", "status", json!({})).await;
    assert_eq!(s["project"], "p");
    let r = t.tool("eng1", "p", "status", json!({"project": "other"})).await;
    assert_eq!(r["isError"], true, "an engine that does not serve `other` is refused: {r}");
    // no calling work item: ask / reject / wait / block / doc say what is missing
    for (tool, args) in [("workitem_ask", json!({"question": "q?"})), ("workitem_reject", json!({"reason": "r"})), ("workitem_wait", json!({"on": ["x"]})), ("workitem_block", json!({})), ("workitem_doc", json!({"text": "note"}))] {
        let r = t.tool("eng1", "p", tool, args).await;
        assert_eq!(r["isError"], true, "{tool}: {r}");
        assert!(text_of(&r).contains("X-Iter-Workid"), "{tool}: {r}");
    }
    t.done().await;
}

#[tokio::test]
async fn work_item_tools() {
    let t = T::new().await;
    t.served("p").await;
    // create: validation first
    let r = t.tool("eng1", "p", "workitem_create", json!({"title": "no request"})).await;
    assert_eq!(r["isError"], true);
    assert!(text_of(&r).contains("request"), "{r}");
    let r = t.tool("eng1", "p", "workitem_create", json!({"request": "no title"})).await;
    assert_eq!(r["isError"], true);
    // create → queued, filed by an engine (no calling item), with tags and a lock scope
    let a = t.ok("eng1", "p", "workitem_create", json!({"title": "Alpha job", "request": "do alpha", "agent": "code", "priority": 4, "tags": ["red:#ff0000", "plain"], "codepaths": ["src/a"]})).await;
    assert_eq!(a["state"], "queued");
    assert_eq!(a["agent"], "code");
    assert_eq!(a["priority"], 4);
    assert_eq!(a["message"], "added");
    let aid = a["id"].as_str().unwrap().to_string();
    let b = t.item("p", "Beta job").await;
    // get: by full id and by the unique 12-char suffix; an unknown one names the project
    let full = t.ok("eng1", "p", "workitem_get", json!({"id": aid})).await;
    assert_eq!(full["name"], "Alpha job");
    assert_eq!(full["lockdirs"], json!(["{topdir}/src/a"]));
    assert_eq!(full["tags"][0], json!({"text": "red", "color": "#ff0000"}));
    let by_suffix = t.ok("eng1", "p", "workitem_get", json!({"id": &aid[aid.len() - 12..]})).await;
    assert_eq!(by_suffix["id"], full["id"]);
    let r = t.tool("eng1", "p", "workitem_get", json!({"id": "zzzz-nothing"})).await;
    assert_eq!(r["isError"], true);
    assert!(text_of(&r).contains("no work item in 'p'"), "{r}");
    // list: filters, total, limit
    let l = t.ok("eng1", "p", "workitem_list", json!({})).await;
    assert_eq!(l["total"], 2);
    let l = t.ok("eng1", "p", "workitem_list", json!({"text": "ALPHA"})).await;
    assert_eq!(l["items"].as_array().unwrap().len(), 1);
    assert_eq!(l["items"][0]["tags"], json!(["red", "plain"]));
    let l = t.ok("eng1", "p", "workitem_list", json!({"tag": "plain"})).await;
    assert_eq!(l["items"][0]["name"], "Alpha job");
    let l = t.ok("eng1", "p", "workitem_list", json!({"state": "complete"})).await;
    assert_eq!(l["total"], 0);
    let l = t.ok("eng1", "p", "workitem_list", json!({"limit": 1})).await;
    assert_eq!((l["total"].as_u64(), l["items"].as_array().unwrap().len()), (Some(2), 1));
    // status: both open, nothing stuck
    let s = t.ok("eng1", "p", "status", json!({})).await;
    assert_eq!(s["open_count"], 2);
    assert_eq!(s["deadlocks"], json!([]));
    // details: the request is the first row
    let d = t.ok("eng1", "p", "workitem_details", json!({"id": aid})).await;
    assert_eq!(d["id"], aid);
    assert!(d["details"].as_array().unwrap().iter().any(|r| r["key"] == "request" && r["value"] == "do alpha"), "{d}");
    // a dependency: Gamma waits on Alpha, and cannot depend on a stranger
    let g = t.ok("eng1", "p", "workitem_create", json!({"title": "Gamma job", "request": "after alpha", "depends_on": [&aid[aid.len() - 12..]]})).await;
    let gid = g["id"].as_str().unwrap().to_string();
    assert_eq!(t.ok("eng1", "p", "workitem_get", json!({"id": gid})).await["blockedby"], json!([aid]));
    let r = t.tool("eng1", "p", "workitem_create", json!({"title": "Delta", "request": "x", "depends_on": ["nope-nope"]})).await;
    assert_eq!(r["isError"], true);
    // doc: appends to the calling item by default and to a named one, rejects empty text
    let d = t.ok("eng1", "p", "workitem_doc", json!({"id": b, "text": "a note on beta\n\n"})).await;
    assert_eq!(d["id"], b);
    let d = t.tool_h("eng1", "p", &aid, "workitem_doc", json!({"text": "a note on alpha"})).await;
    assert_eq!(d["structuredContent"]["id"], aid, "{d}");
    let notes = t.ok("eng1", "p", "workitem_details", json!({"id": aid})).await;
    assert!(notes["details"].as_array().unwrap().iter().any(|r| r["key"] == "doc" && r["value"] == "a note on alpha"));
    assert_eq!(t.tool("eng1", "p", "workitem_doc", json!({"id": b, "text": "  "})).await["isError"], true);
    // a child filed from inside an item takes that item as its parent
    let child = t.tool_h("eng1", "p", &aid, "workitem_create", json!({"title": "Alpha child", "request": "sub-task"})).await;
    assert_eq!(child["isError"], false, "{child}");
    let cid = child["structuredContent"]["id"].as_str().unwrap().to_string();
    let c = t.ok("eng1", "p", "workitem_get", json!({"id": cid})).await;
    assert_eq!(c["createdby"], aid);
    assert_eq!(c["requestedby"], "agent:code");
    // an item may not depend on the item that creates it
    let r = t.tool_h("eng1", "p", &aid, "workitem_create", json!({"title": "Loop", "request": "x", "depends_on": [aid]})).await;
    assert_eq!(r["isError"], true);
    // locks: an answer, not an error
    assert_eq!(t.tool("eng1", "p", "locks_list", json!({})).await["isError"], false);
    t.done().await;
}

#[tokio::test]
async fn ask_reject_wait_act_on_the_calling_item() {
    let t = T::new().await;
    t.served("p").await;
    let a = t.item("p", "Asker").await;
    let b = t.item("p", "Blocker").await;
    // wait: links the dependency, a second identical call changes nothing, itself and unknown ids are refused
    let r = t.tool_h("eng1", "p", &a, "workitem_wait", json!({"on": [&b[b.len() - 12..]], "reason": "needs b first"})).await;
    assert_eq!(r["structuredContent"]["added"], json!([b]), "{r}");
    assert_eq!(t.ok("eng1", "p", "workitem_get", json!({"id": a})).await["blockedby"], json!([b]));
    let r = t.tool_h("eng1", "p", &a, "workitem_wait", json!({"on": [b]})).await;
    assert!(r["structuredContent"]["message"].as_str().unwrap().contains("already waiting"), "{r}");
    assert_eq!(t.tool_h("eng1", "p", &a, "workitem_wait", json!({"on": [a]})).await["isError"], true);
    assert_eq!(t.tool_h("eng1", "p", &a, "workitem_wait", json!({"on": []})).await["isError"], true);
    let d = t.ok("eng1", "p", "workitem_details", json!({"id": a})).await;
    assert!(d["details"].as_array().unwrap().iter().any(|r| r["key"] == "doc" && r["value"].as_str().unwrap().contains("needs b first")), "{d}");
    // ask: an empty question is refused; a real one parks the item in `question` with the widget row
    assert_eq!(t.tool_h("eng1", "p", &b, "workitem_ask", json!({"question": " "})).await["isError"], true);
    let r = t.tool_h("eng1", "p", &b, "workitem_ask", json!({"question": "Which database?\nPostgres or Arango?"})).await;
    assert_eq!(r["isError"], false, "{r}");
    assert!(r["structuredContent"]["message"].as_str().unwrap().contains("Finish your turn"));
    assert_eq!(t.ok("eng1", "p", "workitem_get", json!({"id": b})).await["state"], "question");
    let d = t.ok("eng1", "p", "workitem_details", json!({"id": b})).await;
    let q = d["details"].as_array().unwrap().iter().find(|r| r["key"] == "question").unwrap_or_else(|| panic!("{d}"));
    assert_eq!(q["value"]["title"], "Which database?");
    assert_eq!(q["value"]["fields"][0]["key"], "answer");
    // reject: a reason is required; it parks the item with the reason and writes a doc row
    assert_eq!(t.tool_h("eng1", "p", &a, "workitem_reject", json!({"reason": ""})).await["isError"], true);
    let r = t.tool_h("eng1", "p", &a, "workitem_reject", json!({"reason": "contradicts the spec"})).await;
    assert_eq!(r["isError"], false, "{r}");
    let item = t.ok("eng1", "p", "workitem_get", json!({"id": a})).await;
    assert_eq!(item["state"], "parked");
    assert_eq!(item["lasterror"], "rejected: contradicts the spec");
    // a question filed as an item: state `question`, never runnable work, and the widget recorded
    let q = t.ok("eng1", "p", "workitem_create", json!({"title": "Which region?", "question": "Which AWS region should this deploy to?"})).await;
    assert_eq!(q["state"], "question");
    let d = t.ok("eng1", "p", "workitem_details", json!({"id": q["id"]})).await;
    assert!(d["details"].as_array().unwrap().iter().any(|r| r["key"] == "question"), "{d}");
    // block: with no cluster restart configured it parks the calling item and says when it returns
    let c = t.item("p", "Cluster user").await;
    let r = t.tool_h("eng1", "p", &c, "workitem_block", json!({"reason": "needs the cluster"})).await;
    assert_eq!(r["isError"], false, "{r}");
    assert!(r["structuredContent"]["message"].as_str().unwrap().contains("Finish your turn"));
    t.done().await;
}

#[tokio::test]
async fn authorization_is_the_apis() {
    let t = T::new().await;
    t.served("p").await;
    let id = t.item("p", "Private job").await;
    // an engine of another machine does not see the project's items; a user outside it neither
    for who in ["eng2", "alice"] {
        for (tool, args) in [("status", json!({})), ("workitem_list", json!({})), ("workitem_get", json!({"id": id})), ("graph_stats", json!({})), ("rag_status", json!({})), ("testlogs", json!({}))] {
            let r = t.tool(who, "p", tool, args).await;
            assert_eq!(r["isError"], true, "{who} {tool}: {r}");
            assert!(text_of(&r).contains("40"), "{who} {tool} names the HTTP status: {r}");
        }
        assert_eq!(t.tool(who, "p", "workitem_create", json!({"title": "x", "request": "y"})).await["isError"], true, "{who}");
    }
    // an admin sees everything; settings_graph is open to the signed-in
    assert_eq!(t.tool("adm", "p", "workitem_list", json!({})).await["isError"], false);
    assert_eq!(t.tool("alice", "p", "settings_graph", json!({})).await["isError"], false);
    t.done().await;
}

#[tokio::test]
async fn capabilities_and_settings() {
    let t = T::new().await;
    let caps = t.ok("eng1", "", "capability", json!({})).await;
    let list = caps["capabilities"].as_array().unwrap();
    assert!(!list.is_empty(), "the bootstrapped settings carry capability documents: {caps}");
    // with a name: the full text; the leading underscore and `.md` are optional; a stranger is an error
    let first = list[0]["name"].as_str().unwrap().to_string();
    let one = t.ok("eng1", "", "capability", json!({"name": first})).await;
    assert!(one["body"].as_str().is_some_and(|b| !b.is_empty()), "{one}");
    let r = t.tool("eng1", "", "capability", json!({"name": "no-such-capability"})).await;
    assert_eq!(r["isError"], true);
    assert!(text_of(&r).contains("no capability named"), "{r}");
    let g = t.ok("adm", "", "settings_graph", json!({})).await;
    assert!(g["nodes"].as_array().unwrap().iter().any(|n| n["type"] == "iter_data"), "{g}");
    t.done().await;
}

#[tokio::test]
async fn graphrag_tools() {
    let t = T::new().await;
    t.served("p").await;
    let r = t.tool("eng1", "p", "rag_add_document", json!({"filename": "x.md"})).await;
    assert_eq!(r["isError"], true);
    assert!(text_of(&r).contains("text"), "give `text` or `content_b64`: {r}");
    let added = t.tool("eng1", "p", "rag_add_document", json!({"filename": "tides.md", "title": "Tides", "store": false,
        "text": "# Tides\n\nThe tide at Port Marlowe turns every six hours. Harbour pilots call the slack water the stillgate."})).await;
    assert_eq!(added["isError"], false, "{added}");
    let st = t.ok("eng1", "p", "rag_status", json!({})).await;
    assert!(st.is_object(), "{st}");
    let docs = t.ok("eng1", "p", "rag_docs", json!({"kind": "file"})).await;
    let list = docs["documents"].as_array().unwrap();
    assert_eq!(list.len(), 1, "{docs}");
    assert_eq!(list[0]["title"], "Tides");
    let did = list[0]["id"].as_str().unwrap().to_string();
    let doc = t.ok("eng1", "p", "rag_doc", json!({"id": did})).await;
    assert!(doc.to_string().contains("stillgate"), "{doc}");
    // keyword mode is deterministic (no embedding model needed to rank): the full chunk text comes back
    let hits = t.tool("eng1", "p", "rag_search", json!({"query": "stillgate", "mode": "keyword", "include_guide": false})).await;
    assert_eq!(hits["isError"], false, "{hits}");
    assert!(hits.to_string().contains("Harbour pilots call the slack water the stillgate"), "{hits}");
    assert_eq!(t.tool("eng1", "p", "rag_search", json!({})).await["isError"], true, "a query is required");
    t.done().await;
}
