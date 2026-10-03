//! Project graph store, edits, file sync, designer build, test results and
//! the MCP graph tools — each test on its own throwaway ArangoDB database
//! (test_db.rs), driven through the real router (and its authz layer).

use crate::api::{AppState, NOSK};
use crate::storage::Storage;
use axum::Router;
use axum::body::Body;
use axum::http::Request;
use iter_core::nodefile::{self as nf, NodeDoc, NodeType};
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
        let st = Arc::new(AppState { store: Arc::new(a), secret: b"graph-test".to_vec() });
        crate::settings::bootstrap(st.store.as_ref()).await.unwrap();
        for (u, role) in [("adm", "admin"), ("eng1", "engine"), ("alice", "user")] {
            st.store.put("webui_user", u, NOSK, &json!({"user": u, "role": role, "tokenver": 1, "pwhash": ""})).await.unwrap();
        }
        let app = crate::api::router(st.clone()).merge(crate::mcp::routes(st.clone()));
        T { st, db, app }
    }
    fn tok(&self, user: &str) -> String {
        let role = match user {
            "adm" => "admin",
            "eng1" => "engine",
            _ => "user",
        };
        crate::auth::mint_token(&self.st.secret, user, role, 1, 3600).unwrap()
    }
    async fn call(&self, who: &str, method: &str, path: &str, body: Option<Value>) -> (u16, Value) {
        let mut b = Request::builder().method(method).uri(path).header("authorization", format!("Bearer {}", self.tok(who)));
        let req = match body {
            Some(v) => {
                b = b.header("content-type", "application/json");
                b.body(Body::from(serde_json::to_vec(&v).unwrap())).unwrap()
            }
            None => b.body(Body::empty()).unwrap(),
        };
        let resp = self.app.clone().oneshot(req).await.unwrap();
        let code = resp.status().as_u16();
        let bytes = axum::body::to_bytes(resp.into_body(), 64 << 20).await.unwrap();
        (code, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
    }
    async fn ok(&self, who: &str, method: &str, path: &str, body: Option<Value>) -> Value {
        let (c, v) = self.call(who, method, path, body).await;
        assert!((200..300).contains(&c), "{method} {path} as {who}: {c} {v}");
        v
    }
    fn store(&self) -> &dyn Storage {
        self.st.store.as_ref()
    }
    async fn done(self) {
        crate::test_db::drop(&self.db).await;
    }
    async fn project(&self, p: &str) {
        self.ok("adm", "PUT", &format!("/api/projects/{p}"), Some(json!({"state": "Running"}))).await;
    }
    /// project p served by engine e1 (owned by eng1)
    async fn served(&self, p: &str) {
        self.project(p).await;
        if self.store().get("engine", "e1", NOSK).await.unwrap().is_none() {
            self.ok("eng1", "PUT", "/api/engines/e1", Some(json!({"state": "Running"}))).await;
        }
        self.ok("adm", "POST", "/api/settings/edges", Some(json!({"from": "iter_engine:e1", "to": format!("project:{p}"), "settings": {"topdir": format!("/w/{p}")}}))).await;
    }
    async fn graph(&self, p: &str) -> Value {
        self.ok("adm", "GET", &format!("/api/projects/{p}/graph"), None).await
    }
    async fn node(&self, p: &str, id: &str) -> Value {
        self.ok("adm", "GET", &format!("/api/projects/{p}/graph/nodes/{id}"), None).await
    }
    async fn create(&self, p: &str, body: Value) -> Value {
        self.ok("adm", "POST", &format!("/api/projects/{p}/graph/nodes"), Some(body)).await["node"].clone()
    }
    async fn sync(&self, p: &str, body: Value) -> Value {
        self.ok("eng1", "POST", &format!("/api/projects/{p}/files/sync"), Some(body)).await
    }
    async fn pending(&self, p: &str) -> Vec<Value> {
        self.ok("eng1", "GET", &format!("/api/projects/{p}/files/pending"), None).await["pending"].as_array().cloned().unwrap()
    }
    /// a work item's request text (detail row "request")
    async fn request(&self, p: &str, wid: &str) -> String {
        let d = self.ok("adm", "GET", &format!("/api/projects/{p}/workitems/{wid}/details"), None).await;
        d.as_array().unwrap().iter().find(|r| r["key"] == "request").map(|r| r["value"].as_str().unwrap_or("").to_string()).unwrap_or_default()
    }
    async fn mcp(&self, who: &str, project: &str, tool: &str, args: Value) -> Value {
        let mut args = args;
        args["project"] = json!(project);
        let (c, v) = self.call(who, "POST", "/mcp", Some(json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": {"name": tool, "arguments": args}}))).await;
        assert_eq!(c, 200, "{v}");
        v["result"].clone()
    }
}

fn has_edge(g: &Value, from: &str, kind: &str, to: &str) -> bool {
    g["edges"].as_array().unwrap().iter().any(|e| e["from"] == from && e["kind"] == kind && e["to"] == to)
}

fn find<'a>(g: &'a Value, name: &str) -> &'a Value {
    g["nodes"].as_array().unwrap().iter().find(|n| n["name"] == name).unwrap_or_else(|| panic!("no node {name}"))
}

fn id(v: &Value) -> String {
    v["id"].as_str().unwrap().to_string()
}

/// A node file as an engine would send it.
fn file(t: NodeType, name: &str, path: &str, f: impl FnOnce(&mut NodeDoc)) -> NodeDoc {
    let mut d = NodeDoc::new(t, name, "stephen", "2026-10-01 10:00:00Z");
    d.path = path.into();
    f(&mut d);
    nf::conform(path, &nf::render(&d), "2026-10-01 10:00:00Z", "stephen").doc.unwrap()
}

fn fjson(d: &NodeDoc, base: u64) -> Value {
    let text = nf::render(d);
    json!({"path": d.path, "text": text, "hash": nf::content_hash(&text), "base_version": base})
}

// ---------- designer: the project node ----------

#[tokio::test]
async fn a_new_project_is_designed_with_its_project_node_and_global_reqs() {
    let t = T::new().await;
    t.project("p").await;
    let g = t.graph("p").await;
    let names: Vec<&str> = g["nodes"].as_array().unwrap().iter().map(|n| n["name"].as_str().unwrap()).collect();
    assert_eq!(g["nodes"].as_array().unwrap().len(), 4, "{names:?}");
    let p = find(&g, "p");
    assert_eq!((p["nodetype"].as_str(), p["path"].as_str()), (Some("project"), Some("{topdir}/global/p.project.iter.md")));
    assert_eq!(p["front"]["file_naming"], "sequence");
    for (n, t_, path) in [
        ("Philosophy", "philosophy", "{topdir}/global/requirements/philosophy.philosophy.iter.md"),
        // §2.8: the project's global pair is named after the project
        ("Business requirements", "bizreq", "{topdir}/global/requirements/p.bizreq.iter.md"),
        ("Technical requirements", "techreq", "{topdir}/global/requirements/p.techreq.iter.md"),
    ] {
        let r = find(&g, n);
        assert_eq!((r["nodetype"].as_str(), r["path"].as_str(), r["file_state"].as_str()), (Some(t_), Some(path), Some("designed")));
        assert!(has_edge(&g, &id(p), "reqs", &id(r)));
    }
    assert!(g["nodes"].as_array().unwrap().iter().all(|n| n["file_state"] == "designed"));
    assert_eq!(g["stats"]["designed"], 4);
    // nothing to write: there is no repo
    assert!(t.ok("adm", "GET", "/api/projects/p/files/pending", None).await["pending"].as_array().unwrap().is_empty());
    // a project created from the settings graph gets one too
    t.ok("adm", "POST", "/api/settings/nodes", Some(json!({"type": "project", "name": "q"}))).await;
    let a = crate::nodes::arango(t.store()).unwrap();
    let n = a.aql("FOR n IN node FILTER n.project == 'q' RETURN n.nodetype", json!({})).await.unwrap();
    assert_eq!(n.len(), 4);
    // a project created before iter5 (no node) gets it on first read
    t.store().put("project", "old", NOSK, &json!({"name": "old", "state": "Running"})).await.unwrap();
    crate::settings::on_project_created(t.store(), "old", None).await.unwrap();
    assert_eq!(t.graph("old").await["nodes"].as_array().unwrap().len(), 4);
    t.done().await;
}

// ---------- node CRUD ----------

#[tokio::test]
async fn node_crud_versions_attach_and_delete() {
    let t = T::new().await;
    t.project("p").await;
    let g = t.graph("p").await;
    let pid = id(find(&g, "p"));
    // a root context hangs under the project node by itself
    let ctx = t.create("p", json!({"nodetype": "code", "level": "context", "name": "Data Platform", "desc": "all data"})).await;
    assert_eq!(ctx["path"], "{topdir}/src/data_platform/data_platform.code.iter.md");
    assert_eq!((ctx["node_version"].as_u64(), ctx["file_state"].as_str()), (Some(1), Some("designed")));
    assert_eq!(ctx["children"]["codedirs"], json!(["{thisfiledir}/**"]));
    let api = t.create("p", json!({"nodetype": "code", "level": "container", "name": "API", "attach_to": id(&ctx)})).await;
    assert_eq!(api["path"], "{topdir}/src/data_platform/api/api.code.iter.md");
    let g = t.graph("p").await;
    assert!(has_edge(&g, &pid, "codenodes", &id(&ctx)) && has_edge(&g, &id(&ctx), "codenodes", &id(&api)));
    let p = find(&g, "p");
    assert_eq!(p["children"]["codenodes"], json!(["{topdir}/src/data_platform/data_platform.code.iter.md"]));
    // a test and a local req attach by inference
    let test = t.create("p", json!({"nodetype": "test", "name": "API smoke", "attach_to": id(&api)})).await;
    assert_eq!(test["path"], "{topdir}/src/data_platform/api/api_smoke.test.iter.md");
    assert_eq!(test["children"]["tests"], json!(["{thisfiledir}/tests/{thisfilestem}*.sh"]));
    let req = t.create("p", json!({"nodetype": "bizreq", "name": "Fast", "body": "Answers in 100ms.", "attach_to": id(&api)})).await;
    // §2.8: one bizreq file per code node, named after the node
    assert_eq!(req["path"], "{topdir}/src/data_platform/api/reqs/api.bizreq.iter.md");
    // a second one for the same node is refused: requirements go into that file
    let (c, v) = t.call("adm", "POST", "/api/projects/p/graph/nodes", Some(json!({"nodetype": "bizreq", "name": "Other", "attach_to": id(&api)}))).await;
    assert_eq!((c, v["existing"].as_str()), (409, req["id"].as_str()), "{v}");
    assert_eq!(req["front"]["status"], "draft");
    let g = t.graph("p").await;
    assert!(has_edge(&g, &id(&api), "tests", &id(&test)) && has_edge(&g, &id(&api), "reqs", &id(&req)));
    // bad requests
    assert_eq!(t.call("adm", "POST", "/api/projects/p/graph/nodes", Some(json!({"nodetype": "interface", "name": "x"}))).await.0, 400);
    assert_eq!(t.call("adm", "POST", "/api/projects/p/graph/nodes", Some(json!({"nodetype": "code", "level": "huge", "name": "x"}))).await.0, 400);
    assert_eq!(t.call("adm", "POST", "/api/projects/p/graph/nodes", Some(json!({"nodetype": "project", "name": "x"}))).await.0, 409);
    assert_eq!(t.call("adm", "POST", "/api/projects/p/graph/nodes", Some(json!({"nodetype": "usecase", "name": "x", "attach_to": id(&api), "attach_kind": "reqs"}))).await.0, 400);
    // PATCH: expect_version, version + last_modified bump, front null removes
    let path = format!("/api/projects/p/graph/nodes/{}", id(&api));
    let (c, cur) = t.call("adm", "PATCH", &path, Some(json!({"expect_version": 99, "desc": "x"}))).await;
    assert_eq!(c, 409);
    assert_eq!(cur["current"]["node_version"], t.node("p", &id(&api)).await["node_version"]);
    let v0 = t.node("p", &id(&api)).await["node_version"].as_u64().unwrap();
    let up = t.ok("adm", "PATCH", &path, Some(json!({"expect_version": v0, "desc": "the API", "front": {"owner": "bespoke"}, "teststate": "include"}))).await;
    assert_eq!((up["node_version"].as_u64(), up["desc"].as_str(), up["front"]["owner"].as_str()), (Some(v0 + 1), Some("the API"), Some("bespoke")));
    assert_eq!(up["path"], api["path"], "a rename or edit never moves the file");
    let up = t.ok("adm", "PATCH", &path, Some(json!({"name": "Public API", "front": {"owner": null}}))).await;
    assert!(up["front"].get("owner").is_none());
    assert_eq!(up["path"], api["path"]);
    let full = t.node("p", &id(&api)).await;
    assert!(full["text"].as_str().unwrap().contains("name: \"Public API\"") || full["text"].as_str().unwrap().contains("name: Public API"), "{}", full["text"]);
    // an unchanged PATCH does not bump
    let same = t.ok("adm", "PATCH", &path, Some(json!({"name": "Public API"}))).await;
    assert_eq!(same["node_version"], up["node_version"]);
    // children lists replace: dropping the test entry drops the edge
    t.ok("adm", "PATCH", &path, Some(json!({"children": {"tests": []}}))).await;
    assert!(!has_edge(&t.graph("p").await, &id(&api), "tests", &id(&test)));
    // delete needs a reason; a designed node goes at once and leaves its parent's list
    assert_eq!(t.call("adm", "DELETE", &path, Some(json!({}))).await.0, 400);
    let d = t.ok("adm", "DELETE", &path, Some(json!({"reason": "merged into data"}))).await;
    assert_eq!(d["file_state"], "removed");
    assert_eq!(d["touched"], json!([id(&ctx)]));
    let g = t.graph("p").await;
    assert!(g["nodes"].as_array().unwrap().iter().all(|n| n["id"] != id(&api)));
    assert_eq!(find(&g, "Data Platform")["children"]["codenodes"], json!([]));
    assert_eq!(t.call("adm", "GET", &path, None).await.0, 404);
    // the project node cannot be deleted
    assert_eq!(t.call("adm", "DELETE", &format!("/api/projects/p/graph/nodes/{pid}?reason=x"), None).await.0, 400);
    t.done().await;
}

#[tokio::test]
async fn file_naming_sequence_and_uuid12() {
    let t = T::new().await;
    t.project("p").await;
    let a = t.create("p", json!({"nodetype": "usecase", "name": "Sign up"})).await;
    let b = t.create("p", json!({"nodetype": "usecase", "name": "Sign up"})).await;
    let c = t.create("p", json!({"nodetype": "usecase", "name": "Sign-up!"})).await;
    assert_eq!(a["path"], "{topdir}/global/usecases/sign_up.usecase.iter.md");
    assert_eq!(b["path"], "{topdir}/global/usecases/sign_up01.usecase.iter.md");
    assert_eq!(c["path"], "{topdir}/global/usecases/sign-up.usecase.iter.md");
    let ctx1 = t.create("p", json!({"nodetype": "code", "level": "context", "name": "Web"})).await;
    let ctx2 = t.create("p", json!({"nodetype": "code", "level": "context", "name": "web"})).await;
    assert_eq!(ctx1["path"], "{topdir}/src/web/web.code.iter.md");
    assert_eq!(ctx2["path"], "{topdir}/src/web01/web01.code.iter.md");
    // the project node's file_naming switches the scheme
    let pid = id(find(&t.graph("p").await, "p"));
    t.ok("adm", "PATCH", &format!("/api/projects/p/graph/nodes/{pid}"), Some(json!({"front": {"file_naming": "uuid12"}}))).await;
    let d = t.create("p", json!({"nodetype": "usecase", "name": "Sign up"})).await;
    let tail: String = id(&d).replace('-', "").chars().rev().take(12).collect::<Vec<_>>().into_iter().rev().collect();
    assert_eq!(d["path"], format!("{{topdir}}/global/usecases/sign_up_{tail}.usecase.iter.md"));
    // connections and actors have their own folders
    let conn = t.create("p", json!({"nodetype": "code", "level": "connection", "name": "API call over HTTP"})).await;
    assert_eq!(conn["path"], "{topdir}/global/connections/api_call_over_http.code.iter.md");
    assert_eq!(conn["front"]["connects"], json!({"from": [], "to": []}));
    let actor = t.create("p", json!({"nodetype": "actor", "name": "Employer"})).await;
    assert_eq!(actor["path"], "{topdir}/global/usecases/employer.actor.iter.md");
    t.done().await;
}

// ---------- edges ----------

#[tokio::test]
async fn edges_add_remove_move_and_glob_refusal() {
    let t = T::new().await;
    t.project("p").await;
    let ctx = id(&t.create("p", json!({"nodetype": "code", "level": "context", "name": "Shop"})).await);
    let a = id(&t.create("p", json!({"nodetype": "code", "level": "container", "name": "Gateway", "attach_to": ctx})).await);
    let b = id(&t.create("p", json!({"nodetype": "code", "level": "container", "name": "Orders", "attach_to": ctx})).await);
    let c = id(&t.create("p", json!({"nodetype": "code", "level": "container", "name": "Billing", "attach_to": ctx})).await);
    let conn = id(&t.create("p", json!({"nodetype": "code", "level": "connection", "name": "HTTP API"})).await);
    let uc = id(&t.create("p", json!({"nodetype": "usecase", "name": "Checkout"})).await);
    let actor = id(&t.create("p", json!({"nodetype": "actor", "name": "Buyer"})).await);
    let add = |f: &str, k: &str, to: &str| json!({"from": f, "kind": k, "to": to});
    // supplies is stored on the connection (connects.from); connects on it too
    let r = t.ok("adm", "POST", "/api/projects/p/graph/edges", Some(add(&a, "supplies", &conn))).await;
    assert_eq!(r["touched"], json!([conn]));
    t.ok("adm", "POST", "/api/projects/p/graph/edges", Some(add(&conn, "connects", &b))).await;
    t.ok("adm", "POST", "/api/projects/p/graph/edges", Some(add(&uc, "uses", &b))).await;
    t.ok("adm", "POST", "/api/projects/p/graph/edges", Some(add(&actor, "drives", &uc))).await;
    t.ok("adm", "POST", "/api/projects/p/graph/edges", Some(add(&actor, "touches", &a))).await;
    let g = t.graph("p").await;
    for (f, k, to) in [(&a, "supplies", &conn), (&conn, "connects", &b), (&uc, "uses", &b), (&actor, "drives", &uc), (&actor, "touches", &a)] {
        assert!(has_edge(&g, f, k, to), "{k}");
    }
    let cn = t.node("p", &conn).await;
    assert_eq!(cn["front"]["connects"]["from"], json!(["{topdir}/src/shop/gateway/gateway.code.iter.md"]));
    // adding twice is a no-op; wrong kinds are refused
    let again = t.ok("adm", "POST", "/api/projects/p/graph/edges", Some(add(&a, "supplies", &conn))).await;
    assert_eq!(again["touched"], json!([]));
    assert_eq!(t.call("adm", "POST", "/api/projects/p/graph/edges", Some(add(&a, "reqs", &b))).await.0, 400, "reqs must point at a requirement");
    assert_eq!(t.call("adm", "POST", "/api/projects/p/graph/edges", Some(add(&a, "connects", &b))).await.0, 400, "only a connection connects");
    assert_eq!(t.call("adm", "POST", "/api/projects/p/graph/edges", Some(add(&a, "bogus", &b))).await.0, 400);
    // drives written on the use case side (actors) is the same edge; removing clears both sides
    t.ok("adm", "PATCH", &format!("/api/projects/p/graph/nodes/{uc}"), Some(json!({"front": {"actors": ["{topdir}/global/usecases/buyer.actor.iter.md"]}}))).await;
    let rm = |f: &str, k: &str, to: &str, reason: &str| json!({"from": f, "kind": k, "to": to, "reason": reason});
    assert_eq!(t.call("adm", "DELETE", "/api/projects/p/graph/edges", Some(rm(&actor, "drives", &uc, ""))).await.0, 400, "needs a reason");
    let r = t.ok("adm", "DELETE", "/api/projects/p/graph/edges", Some(rm(&actor, "drives", &uc, "wrong actor"))).await;
    assert_eq!(r["touched"].as_array().unwrap().len(), 2);
    assert!(!has_edge(&t.graph("p").await, &actor, "drives", &uc));
    assert_eq!(t.call("adm", "DELETE", "/api/projects/p/graph/edges", Some(rm(&actor, "drives", &uc, "again"))).await.0, 404);
    // move an endpoint: connects conn → b becomes conn → c
    let mv = t.ok("adm", "POST", "/api/projects/p/graph/edges/move", Some(json!({"from": conn, "to": b, "kind": "connects", "new_to": c}))).await;
    assert_eq!(mv["edge"]["to"], c);
    let g = t.graph("p").await;
    assert!(has_edge(&g, &conn, "connects", &c) && !has_edge(&g, &conn, "connects", &b));
    // a move onto an invalid target changes nothing
    assert_eq!(t.call("adm", "POST", "/api/projects/p/graph/edges/move", Some(json!({"from": conn, "to": c, "kind": "connects", "new_to": uc}))).await.0, 400);
    assert!(has_edge(&t.graph("p").await, &conn, "connects", &c));
    // a glob entry owns its matches: removing one is refused (409) and explained
    t.ok("adm", "PATCH", &format!("/api/projects/p/graph/nodes/{ctx}"), Some(json!({"children": {"codenodes": ["{thisfiledir}/*/*.code.iter.md"]}}))).await;
    let g = t.graph("p").await;
    assert!(has_edge(&g, &ctx, "codenodes", &a) && has_edge(&g, &ctx, "codenodes", &b) && has_edge(&g, &ctx, "codenodes", &c));
    let (code, body) = t.call("adm", "DELETE", "/api/projects/p/graph/edges", Some(rm(&ctx, "codenodes", &a, "split"))).await;
    assert_eq!(code, 409);
    assert_eq!(body["refused"], "glob");
    assert_eq!(body["entry"], "{thisfiledir}/*/*.code.iter.md");
    assert!(body["error"].as_str().unwrap().contains("pattern"));
    t.done().await;
}

// ---------- file sync ----------

#[tokio::test]
async fn files_sync_new_changed_collision_deleted_and_full() {
    let t = T::new().await;
    t.served("p").await;
    // the project was designed before the serves edge: its seeds go when the repo's project node arrives
    assert_eq!(t.graph("p").await["nodes"].as_array().unwrap().len(), 4);
    let proj = file(NodeType::Project, "Shop", "{topdir}/global/shop.project.iter.md", |d| d.children.codenodes = vec!["{topdir}/src/*/*.code.iter.md".into()]);
    let web = file(NodeType::Code, "Web", "{topdir}/src/web/web.code.iter.md", |d| d.level = Some("context".into()));
    let db = file(NodeType::Code, "DB", "{topdir}/src/db/db.code.iter.md", |d| d.level = Some("context".into()));
    let mem = json!({"path": "{topdir}/src/web/web.agentmem.iter.md", "text": "notes", "hash": "", "base_version": 0});
    let doc = json!({"path": "{topdir}/README.md", "text": "x", "hash": "", "base_version": 0});
    let r = t.sync("p", json!({"engine": "e1", "full": true, "files": [fjson(&proj, 0), fjson(&web, 0), fjson(&db, 0), mem, doc]})).await;
    assert_eq!(r["applied"].as_array().unwrap().len(), 3, "{r}");
    assert_eq!(r["ignored"].as_array().unwrap().len(), 2);
    assert!(r["rewrite"].as_array().unwrap().is_empty(), "conformed files are not rewritten: {r}");
    let g = t.graph("p").await;
    assert!(has_edge(&g, &proj.id, "codenodes", &web.id) && has_edge(&g, &proj.id, "codenodes", &db.id));
    let n = t.node("p", &web.id).await;
    assert_eq!((n["node_version"].as_u64(), n["file_version"].as_u64(), n["file_state"].as_str()), (Some(1), Some(1), Some("synced")));
    // unchanged → nothing; changed → version bump
    let r = t.sync("p", json!({"engine": "e1", "files": [fjson(&web, 1)]})).await;
    assert!(r["applied"].as_array().unwrap().is_empty());
    let mut web2 = web.clone();
    web2.desc = "the web tier".into();
    web2.timestamps.last_modified = "2026-10-01 11:00:00Z".into();
    t.sync("p", json!({"engine": "e1", "files": [fjson(&web2, 1)]})).await;
    let n = t.node("p", &web.id).await;
    assert_eq!((n["node_version"].as_u64(), n["file_version"].as_u64(), n["desc"].as_str()), (Some(2), Some(2), Some("the web tier")));
    // a text that is not canonical is applied and the engine told to rewrite it
    let messy = nf::render(&web2).replace("desc: \"the web tier\"", "desc:    \"the web tier\"");
    let r = t.sync("p", json!({"engine": "e1", "files": [{"path": web.path, "text": messy, "hash": nf::content_hash(&messy), "base_version": 2}]})).await;
    assert_eq!(r["rewrite"][0]["text"], nf::render(&web2), "{r}");
    // a copied file (same id, the original still there) gets a new id
    let mut copy = web2.clone();
    copy.path = "{topdir}/src/web2/web.code.iter.md".into();
    let r = t.sync("p", json!({"engine": "e1", "files": [fjson(&copy, 0)]})).await;
    let rw = &r["rewrite"][0];
    assert_eq!(rw["path"], copy.path);
    let new_id = nf::parse(&copy.path, rw["text"].as_str().unwrap()).unwrap().id;
    assert_ne!(new_id, web.id);
    assert_eq!(r["conflicts"][0]["kind"], "id-collision");
    assert_eq!(t.node("p", &new_id).await["path"], copy.path);
    assert_eq!(t.node("p", &web.id).await["path"], web.path, "the original keeps its id");
    // two files of one batch with one id: the second is renamed
    let x1 = file(NodeType::Test, "t", "{topdir}/src/web/a.test.iter.md", |_| {});
    let mut x2 = x1.clone();
    x2.path = "{topdir}/src/web/b.test.iter.md".into();
    let r = t.sync("p", json!({"engine": "e1", "files": [fjson(&x1, 0), fjson(&x2, 0)]})).await;
    assert_eq!(r["rewrite"].as_array().unwrap().len(), 1);
    assert_eq!(r["applied"].as_array().unwrap().len(), 2);
    // a moved file (old path deleted, same id) keeps its id
    let mut moved = db.clone();
    moved.path = "{topdir}/lib/db/db.code.iter.md".into();
    let r = t.sync("p", json!({"engine": "e1", "files": [fjson(&moved, 1)], "deleted": [db.path]})).await;
    assert!(r["rewrite"].as_array().unwrap().is_empty(), "{r}");
    assert_eq!(t.node("p", &db.id).await["path"], moved.path);
    assert!(!has_edge(&t.graph("p").await, &proj.id, "codenodes", &db.id), "the project glob no longer matches it");
    // deleted path → node deleted
    let r = t.sync("p", json!({"engine": "e1", "deleted": [copy.path]})).await;
    assert_eq!(r["removed"], json!([new_id]));
    assert_eq!(t.call("adm", "GET", &format!("/api/projects/p/graph/nodes/{new_id}"), None).await.0, 404);
    // full rescan: every once-written node not sent goes; a node never written stays
    let draft = t.create("p", json!({"nodetype": "usecase", "name": "Draft"})).await;
    assert_eq!(draft["file_state"], "pending_write");
    let r = t.sync("p", json!({"engine": "e1", "full": true, "files": [fjson(&proj, 1), fjson(&web2, 2)]})).await;
    let removed: Vec<&str> = r["removed"].as_array().unwrap().iter().map(|x| x.as_str().unwrap()).collect();
    assert!(removed.contains(&db.id.as_str()) && removed.contains(&x1.id.as_str()) && removed.len() == 3, "{removed:?}");
    let g = t.graph("p").await;
    let names: Vec<&str> = g["nodes"].as_array().unwrap().iter().map(|n| n["name"].as_str().unwrap()).collect();
    assert_eq!(names.len(), 3, "{names:?}");
    assert!(names.contains(&"Draft"));
    t.done().await;
}

#[tokio::test]
async fn files_sync_conflicts_newer_wins_tie_goes_to_the_server() {
    let t = T::new().await;
    t.served("p").await;
    let web = file(NodeType::Code, "Web", "{topdir}/src/web/web.code.iter.md", |d| d.level = Some("context".into()));
    t.sync("p", json!({"engine": "e1", "files": [fjson(&web, 0)]})).await;
    // a server edit not yet written (now = later than the file's 2026-10-01 stamp)
    t.ok("adm", "PATCH", &format!("/api/projects/p/graph/nodes/{}", web.id), Some(json!({"desc": "server says"}))).await;
    let n = t.node("p", &web.id).await;
    assert_eq!((n["node_version"].as_u64(), n["file_state"].as_str()), (Some(2), Some("pending_write")));
    // the file changed too, older timestamp → server wins, the engine is told to rewrite
    let mut f1 = web.clone();
    f1.desc = "file says".into();
    f1.timestamps.last_modified = "2026-10-01 12:00:00Z".into();
    let r = t.sync("p", json!({"engine": "e1", "files": [fjson(&f1, 1)]})).await;
    assert_eq!(r["conflicts"][0]["winner"], "server", "{r}");
    let rw = r["rewrite"][0]["text"].as_str().unwrap();
    assert!(rw.contains("server says"));
    let n = t.node("p", &web.id).await;
    assert_eq!((n["desc"].as_str(), n["file_state"].as_str(), n["file_version"].as_u64()), (Some("server says"), Some("synced"), Some(2)));
    // another server edit, then a file with a NEWER timestamp → the file wins
    t.ok("adm", "PATCH", &format!("/api/projects/p/graph/nodes/{}", web.id), Some(json!({"desc": "server again"}))).await;
    let mut f2 = web.clone();
    f2.desc = "file wins".into();
    f2.timestamps.last_modified = "2099-01-01 00:00:00Z".into();
    let r = t.sync("p", json!({"engine": "e1", "files": [fjson(&f2, 2)]})).await;
    assert_eq!(r["conflicts"][0]["winner"], "file", "{r}");
    assert!(r["rewrite"].as_array().unwrap().is_empty());
    let n = t.node("p", &web.id).await;
    assert_eq!((n["desc"].as_str(), n["file_state"].as_str(), n["node_version"].as_u64()), (Some("file wins"), Some("synced"), Some(4)));
    assert!(t.pending("p").await.is_empty());
    // a tie goes to the server
    t.ok("adm", "PATCH", &format!("/api/projects/p/graph/nodes/{}", web.id), Some(json!({"desc": "tie server"}))).await;
    let srv = t.node("p", &web.id).await;
    let mut f3 = web.clone();
    f3.desc = "tie file".into();
    f3.timestamps.last_modified = srv["timestamps"]["last_modified"].as_str().unwrap().to_string();
    let r = t.sync("p", json!({"engine": "e1", "files": [fjson(&f3, 4)]})).await;
    assert_eq!(r["conflicts"][0]["winner"], "server");
    // the losers are kept
    let c = t.ok("adm", "GET", "/api/projects/p/graph/conflicts", None).await;
    let rows = c["conflicts"].as_array().unwrap();
    assert_eq!(rows.len(), 3);
    assert!(rows.iter().any(|r| r["winner"] == "file" && r["loser"]["desc"] == "server again"));
    assert!(rows.iter().any(|r| r["winner"] == "server" && r["loser"]["text"].as_str().unwrap().contains("file says")));
    // the engine wrote the pending file but its ack is not in yet: the same
    // content coming back is no conflict
    t.ok("adm", "PATCH", &format!("/api/projects/p/graph/nodes/{}", web.id), Some(json!({"desc": "written"}))).await;
    let op = t.pending("p").await.into_iter().next().unwrap();
    let text = op["text"].as_str().unwrap();
    let r = t.sync("p", json!({"engine": "e1", "files": [{"path": web.path, "text": text, "hash": nf::content_hash(text), "base_version": 4}]})).await;
    assert!(r["conflicts"].as_array().unwrap().is_empty(), "{r}");
    assert!(t.pending("p").await.is_empty());
    t.done().await;
}

#[tokio::test]
async fn pending_writes_deletes_moves_and_acks() {
    let t = T::new().await;
    t.served("p").await;
    let proj = file(NodeType::Project, "Shop", "{topdir}/global/shop.project.iter.md", |_| {});
    t.sync("p", json!({"engine": "e1", "full": true, "files": [fjson(&proj, 0)]})).await;
    let hb = t.ok("eng1", "POST", "/api/engines/e1/heartbeat", Some(json!({"state": "Running"}))).await;
    assert_eq!(hb["files_waiting"], json!([]));
    // a node created in the graph is a pending write at once
    let ctx = t.create("p", json!({"nodetype": "code", "level": "context", "name": "Web"})).await;
    let ops = t.pending("p").await;
    assert_eq!(ops.len(), 2, "the new node and its parent: {ops:?}");
    let op = ops.iter().find(|o| o["id"] == ctx["id"]).unwrap();
    assert_eq!((op["op"].as_str(), op["path"].as_str()), (Some("write"), Some("{topdir}/src/web/web.code.iter.md")));
    let text = op["text"].as_str().unwrap();
    assert_eq!(nf::conform(op["path"].as_str().unwrap(), text, "2030-01-01 00:00:00Z", "x").text, text, "the server writes conformed text");
    let hb = t.ok("eng1", "POST", "/api/engines/e1/heartbeat", Some(json!({"state": "Running"}))).await;
    assert_eq!(hb["files_waiting"], json!(["p"]));
    // ack both
    let acks: Vec<Value> = ops.iter().map(|o| json!({"id": o["id"], "node_version": o["node_version"], "path": o["path"], "hash": o["hash"], "commit": "abc"})).collect();
    let r = t.ok("eng1", "POST", "/api/projects/p/files/ack", Some(json!({"engine": "e1", "acks": acks}))).await;
    assert_eq!((r["acked"].as_array().unwrap().len(), r["pending"].as_u64()), (2, Some(0)));
    let n = t.node("p", &id(&ctx)).await;
    assert_eq!((n["file_state"].as_str(), n["file_version"].as_u64(), n["last_commit"].as_str()), (Some("synced"), n["node_version"].as_u64(), Some("abc")));
    // a stale ack (an older version reached the file) keeps the write pending
    t.ok("adm", "PATCH", &format!("/api/projects/p/graph/nodes/{}", id(&ctx)), Some(json!({"desc": "one"}))).await;
    let v1 = t.pending("p").await[0]["node_version"].as_u64().unwrap();
    t.ok("adm", "PATCH", &format!("/api/projects/p/graph/nodes/{}", id(&ctx)), Some(json!({"desc": "two"}))).await;
    let r = t.ok("eng1", "POST", "/api/projects/p/files/ack", Some(json!({"engine": "e1", "acks": [{"id": id(&ctx), "node_version": v1, "hash": "h"}]}))).await;
    assert_eq!(r["stale"], json!([id(&ctx)]));
    let ops = t.pending("p").await;
    assert_eq!(ops.len(), 1);
    t.ok("eng1", "POST", "/api/projects/p/files/ack", Some(json!({"engine": "e1", "acks": [{"id": id(&ctx), "node_version": ops[0]["node_version"]}]}))).await;
    // move: op move with old_path, ack clears it
    let on_disk = t.node("p", &id(&ctx)).await;
    let proj_disk = t.node("p", &proj.id).await;
    let mv = t.ok("adm", "POST", &format!("/api/projects/p/graph/nodes/{}/move", id(&ctx)), Some(json!({"path": "{topdir}/apps/web/web.code.iter.md"}))).await;
    assert_eq!(mv["node"]["path"], "{topdir}/apps/web/web.code.iter.md");
    assert!(mv["touched"].as_array().unwrap().len() == 2, "the project node's exact entry follows: {mv}");
    let pn = t.node("p", &proj.id).await;
    assert_eq!(pn["children"]["codenodes"], json!(["{topdir}/apps/web/web.code.iter.md"]));
    let ops = t.pending("p").await;
    let op = ops.iter().find(|o| o["id"] == ctx["id"]).unwrap();
    assert_eq!((op["op"].as_str(), op["old_path"].as_str()), (Some("move"), Some("{topdir}/src/web/web.code.iter.md")));
    assert_eq!(t.call("adm", "POST", &format!("/api/projects/p/graph/nodes/{}/move", id(&ctx)), Some(json!({"path": "/etc/x.code.iter.md"}))).await.0, 400);
    assert_eq!(t.call("adm", "POST", &format!("/api/projects/p/graph/nodes/{}/move", id(&ctx)), Some(json!({"path": "{topdir}/apps/web/web.test.iter.md"}))).await.0, 400);
    // the engine's scan of the not-yet-moved file is not a deletion or a collision
    let old_text = on_disk["text"].as_str().unwrap();
    let ptext = proj_disk["text"].as_str().unwrap();
    let r = t.sync("p", json!({"engine": "e1", "full": true, "files": [{"path": proj.path, "text": ptext, "hash": nf::content_hash(ptext), "base_version": proj_disk["file_version"]},
        {"path": "{topdir}/src/web/web.code.iter.md", "text": old_text, "hash": nf::content_hash(old_text), "base_version": on_disk["file_version"]}]})).await;
    assert!(r["removed"].as_array().unwrap().is_empty() && r["rewrite"].as_array().unwrap().is_empty(), "{r}");
    assert_eq!(t.node("p", &id(&ctx)).await["file_state"], "pending_write");
    let acks: Vec<Value> = t.pending("p").await.iter().map(|o| json!({"id": o["id"], "node_version": o["node_version"], "path": o["path"], "hash": o["hash"]})).collect();
    t.ok("eng1", "POST", "/api/projects/p/files/ack", Some(json!({"engine": "e1", "acks": acks}))).await;
    assert_eq!(t.node("p", &id(&ctx)).await["old_path"], "");
    // delete: pending_delete → op delete → ack → gone
    t.ok("adm", "DELETE", &format!("/api/projects/p/graph/nodes/{}", id(&ctx)), Some(json!({"reason": "not needed"}))).await;
    assert!(t.graph("p").await["nodes"].as_array().unwrap().iter().all(|n| n["id"] != ctx["id"]), "hidden at once");
    let ops = t.pending("p").await;
    let del = ops.iter().find(|o| o["op"] == "delete").unwrap();
    assert_eq!(del["path"], "{topdir}/apps/web/web.code.iter.md");
    let acks: Vec<Value> = ops.iter().map(|o| json!({"id": o["id"], "node_version": o["node_version"], "hash": o["hash"]})).collect();
    t.ok("eng1", "POST", "/api/projects/p/files/ack", Some(json!({"engine": "e1", "acks": acks}))).await;
    assert!(t.pending("p").await.is_empty());
    assert_eq!(t.call("adm", "GET", &format!("/api/projects/p/graph/nodes/{}", id(&ctx)), None).await.0, 404);
    let unk = t.ok("eng1", "POST", "/api/projects/p/files/ack", Some(json!({"engine": "e1", "acks": [{"id": "nope", "node_version": 1}]}))).await;
    assert_eq!(unk["unknown"], json!(["nope"]));
    t.done().await;
}

// ---------- designer → build ----------

#[tokio::test]
async fn build_flips_designed_nodes_and_queues_the_plan() {
    let t = T::new().await;
    t.project("p").await;
    t.ok("eng1", "PUT", "/api/engines/e1", Some(json!({"state": "Running"}))).await;
    t.create("p", json!({"nodetype": "code", "level": "context", "name": "Web"})).await;
    // engines may not touch an unserved project
    assert_eq!(t.call("eng1", "GET", "/api/projects/p/files/pending", None).await.0, 403);
    assert_eq!(t.ok("adm", "GET", "/api/projects/p/build", None).await["designed"], 5);
    assert_eq!(t.call("adm", "POST", "/api/projects/p/build", Some(json!({"engine": "nope", "topdir": "/w/p"}))).await.0, 404);
    assert_eq!(t.call("alice", "POST", "/api/projects/p/build", Some(json!({"engine": "e1", "topdir": "/w/p"}))).await.0, 403);
    let b = t.ok("adm", "POST", "/api/projects/p/build", Some(json!({"engine": "e1", "topdir": "/w/p", "queue_plan": true, "plan_note": "start small"}))).await;
    assert_eq!((b["build"]["state"].as_str(), b["flipped"].as_u64(), b["pending"].as_u64()), (Some("requested"), Some(5), Some(5)));
    let edges = crate::settings::load_edges(t.store()).await.unwrap();
    let serves = edges.iter().find(|e| e.edge_type == "serves" && e.to == "project:p").unwrap();
    assert!(serves.is_active());
    assert_eq!(serves.setting_str("topdir"), "/w/p");
    let hb = t.ok("eng1", "POST", "/api/engines/e1/heartbeat", Some(json!({"state": "Running"}))).await;
    assert_eq!((hb["build_waiting"].clone(), hb["files_waiting"].clone()), (json!(["p"]), json!(["p"])));
    assert!(t.graph("p").await["nodes"].as_array().unwrap().iter().all(|n| n["file_state"] == "pending_write"));
    // the engine writes everything, acks, reports the build done
    let ops = t.pending("p").await;
    assert_eq!(ops.len(), 5);
    let acks: Vec<Value> = ops.iter().map(|o| json!({"id": o["id"], "node_version": o["node_version"], "hash": o["hash"], "commit": "c0"})).collect();
    t.ok("eng1", "POST", "/api/projects/p/files/ack", Some(json!({"engine": "e1", "acks": acks}))).await;
    let done = t.ok("eng1", "POST", "/api/projects/p/build/done", Some(json!({"engine": "e1", "commit": "c0"}))).await;
    assert_eq!(done["build"]["state"], "done");
    let plan = &done["plan"];
    let pid = id(find(&t.graph("p").await, "p"));
    assert_eq!((plan["agent"].as_str(), plan["priority"].as_i64(), plan["node"].as_str()), (Some("plan"), Some(5), Some(pid.as_str())));
    assert!(t.request("p", plan["id"].as_str().unwrap()).await.contains("start small"));
    assert_eq!(t.call("eng1", "POST", "/api/projects/p/build/done", Some(json!({"engine": "e1"}))).await.0, 409, "only once");
    let hb = t.ok("eng1", "POST", "/api/engines/e1/heartbeat", Some(json!({"state": "Running"}))).await;
    assert_eq!((hb["build_waiting"].clone(), hb["files_waiting"].clone()), (json!([]), json!([])));
    // the repo's first full sync brings the same files back: nothing changes
    let mut files = Vec::new();
    for o in &ops {
        files.push(json!({"path": o["path"], "text": o["text"], "hash": o["hash"], "base_version": o["node_version"]}));
    }
    let r = t.sync("p", json!({"engine": "e1", "full": true, "files": files})).await;
    assert!(r["removed"].as_array().unwrap().is_empty() && r["conflicts"].as_array().unwrap().is_empty(), "{r}");
    assert_eq!(t.graph("p").await["nodes"].as_array().unwrap().len(), 5);
    t.done().await;
}

#[tokio::test]
async fn a_repo_project_node_replaces_the_untouched_seeds() {
    let t = T::new().await;
    t.project("p").await; // designed: seeded project node + 3 reqs
    t.served("p").await; // then an admin points an engine at an existing repo
    let proj = file(NodeType::Project, "Real", "{topdir}/global/real.project.iter.md", |_| {});
    t.sync("p", json!({"engine": "e1", "full": true, "files": [fjson(&proj, 0)]})).await;
    let g = t.graph("p").await;
    let names: Vec<&str> = g["nodes"].as_array().unwrap().iter().map(|n| n["name"].as_str().unwrap()).collect();
    assert_eq!(names, vec!["Real"]);
    t.done().await;
}

// ---------- tests ----------

#[tokio::test]
async fn test_results_logs_failures_and_run_tests() {
    let t = T::new().await;
    t.served("p").await;
    let web = t.create("p", json!({"nodetype": "code", "level": "context", "name": "Web"})).await;
    let tn = t.create("p", json!({"nodetype": "test", "name": "Web smoke", "attach_to": id(&web)})).await;
    let tid = id(&tn);
    let path = format!("/api/projects/p/graph/nodes/{tid}/testresult");
    // results only on test nodes
    assert_eq!(t.call("eng1", "POST", &format!("/api/projects/p/graph/nodes/{}/testresult", id(&web)), Some(json!({"overall_success": true, "normal": {"total": 1, "pass": 1, "err": 0}}))).await.0, 400);
    let red = json!({"name": "Web smoke", "id": tid, "overall_success": false, "normal": {"total": 3, "pass": 2, "err": 1},
                     "longtail": {"total": 0, "pass": 0, "err": 0}, "failure": {"total": 0, "pass": 0, "err": 0},
                     "details": [{"name": "login", "bucket": "normal", "pass": false, "msg": "500"}]});
    let r = t.ok("eng1", "POST", &path, Some(json!({"result": red, "exit_code": 1, "engine": "e1"}))).await;
    assert_eq!(r["test"]["result"], "red");
    let wid = r["workitem"]["id"].as_str().unwrap().to_string();
    let n = t.node("p", &tid).await;
    assert_eq!(n["front"]["last_result"]["normal"]["err"], 1);
    assert!(!n["timestamps"]["last_tested"].as_str().unwrap().is_empty());
    assert_eq!((n["test"]["outcome"].as_str(), n["file_state"].as_str()), (Some("fail"), Some("pending_write")));
    let item = t.ok("adm", "GET", &format!("/api/projects/p/workitems/{wid}"), None).await;
    assert_eq!((item["agent"].as_str(), item["node"].as_str()), (Some("code"), Some(tid.as_str())));
    assert!(t.request("p", &wid).await.contains("login (normal): 500"));
    // the same red test again is the same open item
    let r2 = t.ok("eng1", "POST", &path, Some(red.clone())).await;
    assert_eq!(r2["workitem"]["id"], json!(wid));
    // green: no item
    let green = json!({"overall_success": true, "normal": {"total": 3, "pass": 3, "err": 0}});
    let r3 = t.ok("eng1", "POST", &path, Some(green)).await;
    assert!(r3["workitem"].is_null());
    assert_eq!(t.node("p", &tid).await["test"]["result"], "green");
    // logs, newest first, per node
    let logs = t.ok("adm", "GET", &format!("/api/projects/p/testlogs?node={tid}&limit=10"), None).await;
    let rows = logs["logs"].as_array().unwrap();
    assert_eq!(rows.len(), 3);
    assert_eq!((rows[0]["outcome"].as_str(), rows[2]["outcome"].as_str()), (Some("pass"), Some("fail")));
    assert_eq!(rows[2]["details"][0]["msg"], "500");
    assert_eq!(t.ok("adm", "GET", "/api/projects/p/testlogs?limit=1", None).await["logs"].as_array().unwrap().len(), 1);
    // run tests from the graph: a test work item on the node
    let w = t.ok("adm", "POST", "/api/projects/p/graph/run_tests", Some(json!({"node": id(&web)}))).await;
    assert_eq!((w["agent"].as_str(), w["node"].as_str()), (Some("test"), web["id"].as_str()));
    assert!(w["exec_shell"].as_str().unwrap().contains("--node"));
    assert_eq!(t.call("adm", "POST", "/api/projects/p/graph/run_tests", Some(json!({"node": "nope"}))).await.0, 404);
    t.done().await;
}

// ---------- view + MCP ----------

#[tokio::test]
async fn graph_view_shape() {
    let t = T::new().await;
    t.project("p").await;
    let ctx = t.create("p", json!({"nodetype": "code", "level": "context", "name": "Shop"})).await;
    let api = t.create("p", json!({"nodetype": "code", "level": "container", "name": "API", "attach_to": id(&ctx)})).await;
    let lib = t.create("p", json!({"nodetype": "code", "level": "component", "name": "Lib", "attach_to": id(&api)})).await;
    let actor = t.create("p", json!({"nodetype": "actor", "name": "Buyer"})).await;
    let uc = t.create("p", json!({"nodetype": "usecase", "name": "Checkout", "front": {"flowmap": {"summary": "buy",
        "sequence": ["actor:Buyer", "{topdir}/src/shop/api/api.code.iter.md"],
        "process_flow": [{"step": 1, "from": "actor:Buyer", "to": "src/shop/api", "what": "orders"}, {"step": 2, "from": "src/shop/api", "to": "nowhere", "what": "x"}]}}})).await;
    t.ok("adm", "POST", "/api/projects/p/graph/edges", Some(json!({"from": id(&uc), "kind": "uses", "to": id(&api)}))).await;
    t.ok("adm", "POST", "/api/projects/p/graph/edges", Some(json!({"from": id(&actor), "kind": "drives", "to": id(&uc)}))).await;
    let v = t.ok("adm", "GET", "/api/projects/p/graph/view", None).await;
    let node = |i: &Value| v["nodes"].as_array().unwrap().iter().find(|n| n["id"] == i["id"]).cloned().unwrap();
    let pid = id(find(&t.graph("p").await, "p"));
    assert_eq!((node(&ctx)["level"].as_str(), node(&ctx)["parent"].as_str()), (Some("context"), Some(pid.as_str())));
    assert_eq!(node(&lib)["parent"], api["id"]);
    assert_eq!(node(&lib)["owners"], json!([id(&api), id(&ctx), pid]));
    assert_eq!(node(&actor)["level"], "actor");
    assert_eq!(node(&lib)["file_state"], "designed");
    let u = &v["usecases"][0];
    assert_eq!(u["parts"], json!([id(&api)]));
    assert_eq!(u["actors"], json!([id(&actor)]));
    assert_eq!(u["members"], json!([id(&api), id(&lib)]));
    assert_eq!(u["sequence"], json!([id(&actor), id(&api)]));
    assert_eq!(u["unresolved"], json!(["nowhere"]));
    assert_eq!(node(&api)["usecases"][0]["steps"], json!([1]));
    let flows: Vec<&Value> = v["edges"].as_array().unwrap().iter().filter(|e| e["type"] == "flow").collect();
    assert_eq!(flows.len(), 1);
    assert!(v["edges"].as_array().unwrap().iter().any(|e| e["type"] == "uses" && e["source"] == uc["id"]));
    assert_eq!(v["summary"]["file_sync"]["designed"], 9);
    assert_eq!(v["summary"]["counts"]["nodes_per_level"]["component"], 1);
    assert!(v.to_string().find("interface").is_none(), "no interfaces anywhere");
    // the use case route
    let r = t.ok("adm", "GET", "/api/projects/p/graph/usecases/Checkout", None).await;
    assert_eq!(r["tops"], json!([id(&api)]));
    assert_eq!(r["nodes"].as_array().unwrap().len(), 2);
    // neighbours, lookup, owner
    let nb = t.ok("adm", "GET", &format!("/api/projects/p/graph/nodes/{}/neighbors?depth=2", id(&ctx)), None).await;
    assert_eq!(nb["neighbors"].as_array().unwrap().len(), 2);
    let lk = t.ok("adm", "GET", "/api/projects/p/graph/lookup?path=%7Btopdir%7D/src/shop/api/api.code.iter.md", None).await;
    assert_eq!(lk["matches"][0]["id"], api["id"]);
    let ow = t.ok("adm", "GET", "/api/projects/p/graph/owner?path=%7Btopdir%7D/src/shop/api/lib/x.rs", None).await;
    let ids: Vec<&str> = ow["owners"].as_array().unwrap().iter().map(|o| o["name"].as_str().unwrap()).collect();
    assert_eq!(ids, ["Lib", "API", "Shop"]);
    t.done().await;
}

#[tokio::test]
async fn mcp_graph_tools() {
    let t = T::new().await;
    t.served("p").await;
    let r = t.mcp("eng1", "p", "graph_node_create", json!({"nodetype": "code", "level": "context", "name": "Web"})).await;
    assert_eq!(r["isError"], false, "{r}");
    let web = r["structuredContent"]["node"]["id"].as_str().unwrap().to_string();
    let r = t.mcp("eng1", "p", "graph_node_create", json!({"nodetype": "code", "level": "container", "name": "UI", "attach_to": web})).await;
    let ui = r["structuredContent"]["node"]["id"].as_str().unwrap().to_string();
    let r = t.mcp("eng1", "p", "graph_node_update", json!({"id": ui, "desc": "the UI"})).await;
    assert_eq!(r["structuredContent"]["desc"], "the UI");
    let r = t.mcp("eng1", "p", "graph_edge_remove", json!({"from": web, "to": ui, "kind": "codenodes", "reason": "split"})).await;
    assert_eq!(r["isError"], false, "{r}");
    let r = t.mcp("eng1", "p", "graph_edge_add", json!({"from": web, "to": ui, "kind": "codenodes"})).await;
    assert_eq!(r["structuredContent"]["touched"], json!([web]));
    let r = t.mcp("eng1", "p", "graph_edge_add", json!({"from": web, "to": ui, "kind": "reqs"})).await;
    assert_eq!(r["isError"], true);
    let r = t.mcp("eng1", "p", "testlogs", json!({})).await;
    assert_eq!(r["structuredContent"]["logs"], json!([]));
    let r = t.mcp("adm", "p", "settings_graph", json!({})).await;
    assert!(r["structuredContent"]["nodes"].is_array(), "{r}");
    // the same authz: an engine that does not serve q is refused
    t.project("q").await;
    let r = t.mcp("eng1", "q", "graph_node_create", json!({"nodetype": "usecase", "name": "x"})).await;
    assert_eq!(r["isError"], true);
    // tools/list carries them, and no interface tool or nodetype
    let (_, l) = t.call("eng1", "POST", "/mcp", Some(json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list"}))).await;
    let names: Vec<&str> = l["result"]["tools"].as_array().unwrap().iter().map(|x| x["name"].as_str().unwrap()).collect();
    for n in ["graph_node_create", "graph_node_update", "graph_edge_add", "graph_edge_remove", "testlogs", "settings_graph"] {
        assert!(names.contains(&n), "{n}");
    }
    assert!(!l.to_string().contains("interface"));
    t.done().await;
}

// ---------- project delete ----------

#[tokio::test]
async fn deleting_a_project_purges_its_graph_and_a_recreate_starts_clean() {
    let t = T::new().await;
    t.project("p").await;
    t.project("keep").await;
    let web = t.create("p", json!({"nodetype": "code", "level": "context", "name": "Web"})).await;
    let tn = t.create("p", json!({"nodetype": "test", "name": "Web smoke", "attach_to": id(&web)})).await;
    let green = json!({"name": "Web smoke", "id": id(&tn), "overall_success": true, "normal": {"total": 1, "pass": 1, "err": 0},
                       "longtail": {"total": 0, "pass": 0, "err": 0}, "failure": {"total": 0, "pass": 0, "err": 0}});
    t.ok("adm", "POST", &format!("/api/projects/p/graph/nodes/{}/testresult", id(&tn)), Some(green)).await;
    let a = t.store().arango().unwrap();
    a.aql("INSERT {project: 'p', at: '2026-10-02 10:00:00Z', id: 'x'} INTO node_conflict", json!({})).await.unwrap();
    let count = |coll: &'static str, p: &'static str| async move {
        a.aql(&format!("RETURN LENGTH(FOR d IN {coll} FILTER d.project == @p RETURN 1)"), json!({"p": p})).await.unwrap()[0].as_u64().unwrap()
    };
    for coll in ["node", "link", "test_log", "node_conflict"] {
        assert!(count(coll, "p").await > 0, "{coll} seeded");
    }
    let keep_nodes = count("node", "keep").await;
    let r = t.ok("adm", "DELETE", "/api/projects/p", None).await;
    assert_eq!(r["deleted"], true);
    assert!(r["graph_purged"].as_u64().unwrap() > 0, "{r}");
    for coll in ["node", "link", "test_log", "node_conflict"] {
        assert_eq!(count(coll, "p").await, 0, "{coll} left behind");
    }
    assert_eq!(count("node", "keep").await, keep_nodes, "another project's graph is untouched");
    // same name again: one project node + the three default reqs, nothing doubled
    t.project("p").await;
    let g = t.graph("p").await;
    let types: Vec<&str> = g["nodes"].as_array().unwrap().iter().map(|n| n["nodetype"].as_str().unwrap()).collect();
    assert_eq!(types.iter().filter(|t| **t == "project").count(), 1, "{types:?}");
    assert_eq!(types.len(), 4, "{types:?}");
    // deleting through the settings graph purges too
    t.create("p", json!({"nodetype": "code", "level": "context", "name": "Again"})).await;
    t.ok("adm", "DELETE", "/api/settings/nodes/project:p", None).await;
    assert_eq!(count("node", "p").await, 0);
    t.done().await;
}

// ---------- requirements: req nodes inside bizreq / techreq files (§2.8) ----------

use iter_core::nodefile::reqs::{ReqItem, parse_reqs, render_reqs};

fn ri(id: &str, key: &str, title: &str, status: &str, text: &str) -> ReqItem {
    ReqItem { id: id.into(), key: key.into(), title: title.into(), status: status.into(), text: text.into(), ..Default::default() }
}

/// A requirement file exactly as given (no conform: the server must cope).
fn raw_req_file(t: NodeType, name: &str, path: &str, body: &str) -> Value {
    let mut d = NodeDoc::new(t, name, "stephen", "2026-10-01 10:00:00Z");
    d.path = path.into();
    d.body = body.into();
    let text = nf::render(&d);
    json!({"path": path, "text": text, "hash": nf::content_hash(&text), "base_version": 0})
}

fn reqs_of(g: &Value) -> Vec<Value> {
    let mut v: Vec<Value> = g["nodes"].as_array().unwrap().iter().filter(|n| n["nodetype"] == "req").cloned().collect();
    v.sort_by_key(|r| (r["file"].as_str().unwrap_or("").to_string(), r["order"].as_u64().unwrap_or(0)));
    v
}

async fn body_of(t: &T, p: &str, file: &str) -> String {
    t.node(p, file).await["body"].as_str().unwrap().to_string()
}

const RA: &str = "6f2c4c9e-0000-4000-8000-00000000000a";
const RB: &str = "6f2c4c9e-0000-4000-8000-00000000000b";
const RD: &str = "6f2c4c9e-0000-4000-8000-00000000000d";

#[tokio::test]
async fn req_nodes_are_derived_from_a_synced_multi_section_file() {
    let t = T::new().await;
    t.served("p").await;
    let proj = file(NodeType::Project, "p", "{topdir}/global/p.project.iter.md", |d| d.children.codenodes = vec!["{topdir}/src/api/api.code.iter.md".into()]);
    let api = file(NodeType::Code, "API", "{topdir}/src/api/api.code.iter.md", |d| {
        d.level = Some("context".into());
        d.children.reqs = vec!["{thisfiledir}/reqs/api.techreq.iter.md".into()];
    });
    let fpath = "{topdir}/src/api/reqs/api.techreq.iter.md";
    let body = format!(
        "Rules for the API.\n\n## PDY-TECH-034 — JWT identifies; only Ed25519 authorizes money\n<!-- req: id={RA} status=agreed -->\nEvery caller presents a JSON Web Token. The token names who calls.\n\n### Detail\nmore\n\n## Second requirement\n<!-- req: id={RB} -->\nPlain text.\n\n## Third, no marker yet\nNew one.\n"
    );
    let r = t.sync("p", json!({"engine": "e1", "files": [fjson(&proj, 0), fjson(&api, 0), raw_req_file(NodeType::Techreq, "API technical requirements", fpath, &body)]})).await;
    // the marker-less section got an id: the engine is asked to rewrite the file
    assert!(r["rewrite"].as_array().unwrap().iter().any(|w| w["path"] == fpath), "{r}");
    let g = t.graph("p").await;
    let fnode = g["nodes"].as_array().unwrap().iter().find(|n| n["path"] == fpath).unwrap().clone();
    let fid = id(&fnode);
    let rs = reqs_of(&g);
    assert_eq!(rs.len(), 3, "{rs:?}");
    let a = &rs[0];
    assert_eq!((a["id"].as_str(), a["key"].as_str(), a["title"].as_str(), a["status"].as_str()), (Some(RA), Some("PDY-TECH-034"), Some("JWT identifies; only Ed25519 authorizes money"), Some("agreed")));
    assert_eq!(a["name"], "PDY-TECH-034 — JWT identifies; only Ed25519 authorizes money");
    assert_eq!(a["desc"], "Every caller presents a JSON Web Token.");
    assert!(a["text"].as_str().unwrap().contains("### Detail\nmore"));
    assert_eq!((a["order"].as_u64(), a["file"].as_str(), a["file_type"].as_str()), (Some(0), Some(fid.as_str()), Some("techreq")));
    assert_eq!(a["path"], format!("{fpath}#{RA}"));
    assert_eq!(a["file_state"], "synced");
    assert_eq!((rs[1]["id"].as_str(), rs[1]["key"].as_str(), rs[1]["status"].as_str()), (Some(RB), Some(""), Some("draft")));
    assert_eq!((rs[2]["title"].as_str(), rs[2]["order"].as_u64()), (Some("Third, no marker yet"), Some(2)));
    let third = id(&rs[2]);
    assert!(nf::is_valid_id(&third));
    for r in &rs {
        assert!(has_edge(&g, &fid, "contains", &id(r)));
    }
    assert!(has_edge(&g, &id(&api_of(&g)), "reqs", &fid));
    // stats
    assert_eq!(g["stats"]["by_nodetype"]["req"], 3);
    assert_eq!(g["stats"]["by_kind"]["contains"], 3);
    assert_eq!(g["stats"]["reqs"], 3);
    // a req node by id, its edges; the file's out edges
    let rn = t.node("p", RA).await;
    assert_eq!((rn["nodetype"].as_str(), rn["edges_in"].clone(), rn["edges_out"].clone()), (Some("req"), json!([{"kind": "contains", "from": fid}]), json!([])));
    let fv = t.node("p", &fid).await;
    assert_eq!(fv["edges_out"].as_array().unwrap().iter().filter(|e| e["kind"] == "contains").count(), 3);
    // neighbours, lookup by path and by name
    let nb = t.ok("adm", "GET", &format!("/api/projects/p/graph/nodes/{fid}/neighbors?depth=1&direction=out"), None).await;
    assert_eq!(nb["neighbors"].as_array().unwrap().iter().filter(|n| n["via"] == "contains" && n["vertex"]["nodetype"] == "req").count(), 3);
    let nb = t.ok("adm", "GET", &format!("/api/projects/p/graph/nodes/{RB}/neighbors?depth=2&direction=in"), None).await;
    assert_eq!(nb["neighbors"].as_array().unwrap().len(), 2, "file, then the code node: {nb}");
    let lk = t.ok("adm", "GET", &format!("/api/projects/p/graph/lookup?path={}", enc(&format!("{fpath}#{RA}"))), None).await;
    assert_eq!(lk["matches"][0]["id"], RA);
    let lk = t.ok("adm", "GET", "/api/projects/p/graph/lookup?name=Second%20requirement&nodetype=req", None).await;
    assert_eq!(lk["matches"][0]["id"], RB);
    let lk = t.ok("adm", "GET", "/api/projects/p/graph/lookup?name=PDY-TECH-034", None).await;
    assert_eq!(lk["matches"][0]["id"], RA);
    // req nodes are stored (node collection) but are never files
    let ar = crate::nodes::arango(t.store()).unwrap();
    let stored = ar.aql("FOR n IN node FILTER n.project == 'p' AND n.nodetype == 'req' SORT n.order RETURN n", json!({})).await.unwrap();
    assert_eq!(stored.len(), 3);
    assert_eq!((stored[0]["id"].as_str(), stored[0]["file"].as_str()), (Some(RA), Some(fid.as_str())));
    let links = ar.aql("FOR l IN link FILTER l.project == 'p' AND l.kind == 'contains' RETURN l", json!({})).await.unwrap();
    assert_eq!(links.len(), 3);
    assert!(t.pending("p").await.is_empty(), "nothing waits: the rewrite went back in the sync reply");
    // the engine re-syncs the file with one section gone (and an edit in another)
    let (pre, mut items) = parse_reqs(&fnode["body"].as_str().unwrap().to_string());
    items.remove(1);
    items[0].text = "Every caller presents a JWT.".into();
    let mut d = doc_of(&fnode);
    d.body = render_reqs(&pre, &items);
    let r = t.sync("p", json!({"engine": "e1", "files": [fjson(&d, fnode["file_version"].as_u64().unwrap())]})).await;
    assert!(r["errors"].as_array().unwrap().is_empty() && r["conflicts"].as_array().unwrap().is_empty(), "{r}");
    let g = t.graph("p").await;
    let rs = reqs_of(&g);
    assert!(rs.iter().all(|r| r["id"] != RB), "the removed section's node is gone");
    assert_eq!(rs.len(), 2);
    assert_eq!((rs[0]["id"].as_str(), rs[0]["text"].as_str(), rs[0]["desc"].as_str()), (Some(RA), Some("Every caller presents a JWT."), Some("Every caller presents a JWT.")));
    assert_eq!((rs[1]["id"].as_str(), rs[1]["order"].as_u64()), (Some(third.as_str()), Some(1)), "ids survive the edit, order follows the file");
    let stored = ar.aql("FOR n IN node FILTER n.project == 'p' AND n.nodetype == 'req' RETURN n.id", json!({})).await.unwrap();
    assert!(!stored.contains(&json!(RB)));
    t.done().await;
}

fn api_of(g: &Value) -> Value {
    find(g, "API").clone()
}

/// The file content of a stored node (as the engine would hold it).
fn doc_of(v: &Value) -> NodeDoc {
    serde_json::from_value(v.clone()).unwrap()
}

fn enc(s: &str) -> String {
    s.replace('%', "%25").replace('{', "%7B").replace('}', "%7D").replace('#', "%23").replace(' ', "%20")
}

#[tokio::test]
async fn req_edits_rewrite_exactly_their_section() {
    let t = T::new().await;
    t.served("p").await;
    let api = t.create("p", json!({"nodetype": "code", "level": "context", "name": "API"})).await;
    let fpath = "{topdir}/src/api/reqs/api.techreq.iter.md";
    // node + type: the node's techreq file is created and named in its children.reqs
    let r1 = t.ok("adm", "POST", "/api/projects/p/graph/reqs", Some(json!({"node": id(&api), "type": "techreq", "key": "T-1", "title": "Answers fast", "text": "Within 100 ms. Always.", "status": "agreed"}))).await;
    let file = r1["file"].clone();
    let fid = id(&file);
    assert_eq!((file["path"].as_str(), file["nodetype"].as_str(), file["file_state"].as_str()), (Some(fpath), Some("techreq"), Some("pending_write")));
    let a = r1["req"].clone();
    assert_eq!((a["key"].as_str(), a["title"].as_str(), a["status"].as_str(), a["desc"].as_str(), a["file"].as_str()), (Some("T-1"), Some("Answers fast"), Some("agreed"), Some("Within 100 ms."), Some(fid.as_str())));
    let mut touched: Vec<String> = r1["touched"].as_array().unwrap().iter().map(|x| x.as_str().unwrap().to_string()).collect();
    touched.sort();
    let mut want = vec![fid.clone(), id(&api)];
    want.sort();
    assert_eq!(touched, want);
    let apin = t.node("p", &id(&api)).await;
    assert_eq!(apin["children"]["reqs"], json!(["{thisfiledir}/reqs/api.techreq.iter.md"]));
    let g = t.graph("p").await;
    assert!(has_edge(&g, &id(&api), "reqs", &fid) && has_edge(&g, &fid, "contains", &id(&a)));
    // the same node + type again: the same file, no new one
    let r2 = t.ok("adm", "POST", "/api/projects/p/graph/reqs", Some(json!({"node": id(&api), "type": "techreq", "title": "Second", "text": "Two."}))).await;
    assert_eq!(r2["file"]["id"], fid);
    assert_eq!(r2["req"]["status"], "draft");
    assert_eq!(r2["touched"], json!([fid]));
    // by file, placed with after
    let r3 = t.ok("adm", "POST", "/api/projects/p/graph/reqs", Some(json!({"file": fid, "title": "Between", "text": "In the middle.", "after": id(&a)}))).await;
    let (b, c) = (id(&r2["req"]), id(&r3["req"]));
    let order = |body: &str| parse_reqs(body).1.iter().map(|x| x.id.clone()).collect::<Vec<_>>();
    let before = body_of(&t, "p", &fid).await;
    assert_eq!(order(&before), vec![id(&a), c.clone(), b.clone()]);
    assert_eq!(t.node("p", &b).await["order"], 2);
    // PATCH one section: exactly that section changes, ids stay
    let v0 = t.node("p", &fid).await["node_version"].as_u64().unwrap();
    let (code, cur) = t.call("adm", "PATCH", &format!("/api/projects/p/graph/reqs/{c}"), Some(json!({"title": "x", "expect_version": v0 + 7}))).await;
    assert_eq!(code, 409);
    assert_eq!(cur["current"]["file"]["node_version"], v0);
    let up = t.ok("adm", "PATCH", &format!("/api/projects/p/graph/reqs/{c}"), Some(json!({"title": "In between", "key": "T-2", "expect_version": v0}))).await;
    assert_eq!((up["req"]["name"].as_str(), up["file"]["node_version"].as_u64()), (Some("T-2 — In between"), Some(v0 + 1)));
    let after = body_of(&t, "p", &fid).await;
    let (pb, ib) = parse_reqs(&before);
    let (pa, ia) = parse_reqs(&after);
    assert_eq!(pb, pa);
    assert_eq!((ib[0].clone(), ib[2].clone()), (ia[0].clone(), ia[2].clone()), "other sections untouched");
    assert_eq!((ia[1].id.as_str(), ia[1].key.as_str(), ia[1].title.as_str(), ia[1].text.as_str()), (c.as_str(), "T-2", "In between", "In the middle."));
    let mut exp = ib.clone();
    exp[1].key = "T-2".into();
    exp[1].title = "In between".into();
    assert_eq!(after, render_reqs(&pb, &exp), "the body is the old one with that section changed");
    t.ok("adm", "PATCH", &format!("/api/projects/p/graph/reqs/{c}"), Some(json!({"text": "Now in the middle.\n\n### Why\nBecause.", "status": "done"}))).await;
    let rn = t.node("p", &c).await;
    assert_eq!((rn["status"].as_str(), rn["text"].as_str(), rn["desc"].as_str()), (Some("done"), Some("Now in the middle.\n\n### Why\nBecause."), Some("Now in the middle.")));
    // an unchanged PATCH does not bump
    let v = t.node("p", &fid).await["node_version"].clone();
    t.ok("adm", "PATCH", &format!("/api/projects/p/graph/reqs/{c}"), Some(json!({"status": "done"}))).await;
    assert_eq!(t.node("p", &fid).await["node_version"], v);
    // refusals
    for bad in [json!({"status": "maybe"}), json!({"text": "a\n## b"}), json!({"title": "  "}), json!({"key": "a b"}), json!({"expect_version": "x"})] {
        assert_eq!(t.call("adm", "PATCH", &format!("/api/projects/p/graph/reqs/{c}"), Some(bad.clone())).await.0, 400, "{bad}");
    }
    assert_eq!(t.call("adm", "PATCH", "/api/projects/p/graph/reqs/nope", Some(json!({"title": "x"}))).await.0, 404);
    for bad in [json!({"title": "x"}), json!({"file": fid, "node": id(&api), "type": "techreq", "title": "x"}), json!({"node": id(&api), "type": "philosophy", "title": "x"}),
                json!({"file": id(&api), "title": "x"}), json!({"file": fid}), json!({"file": fid, "title": "x", "after": "nope"})] {
        assert_eq!(t.call("adm", "POST", "/api/projects/p/graph/reqs", Some(bad.clone())).await.0, 400, "{bad}");
    }
    // the write waits for the engine as one file op; req nodes are never files
    let pend = t.pending("p").await;
    let op = pend.iter().find(|o| o["id"] == fid).unwrap();
    assert_eq!((op["op"].as_str(), op["path"].as_str()), (Some("write"), Some(fpath)));
    assert!(op["text"].as_str().unwrap().contains(&format!("## T-2 — In between\n<!-- req: id={c} status=done -->\nNow in the middle.")));
    let rids = [id(&a), b.clone(), c.clone()];
    assert!(pend.iter().all(|o| !rids.contains(&o["id"].as_str().unwrap().to_string())));
    // DELETE needs a reason; then the section and its node are gone
    assert_eq!(t.call("adm", "DELETE", &format!("/api/projects/p/graph/reqs/{b}"), None).await.0, 400);
    let d = t.ok("adm", "DELETE", &format!("/api/projects/p/graph/reqs/{b}"), Some(json!({"reason": "dup"}))).await;
    assert_eq!((d["removed"].as_bool(), d["touched"].clone()), (Some(true), json!([fid])));
    assert_eq!(order(&body_of(&t, "p", &fid).await), vec![id(&a), c.clone()]);
    assert_eq!(t.call("adm", "GET", &format!("/api/projects/p/graph/nodes/{b}"), None).await.0, 404);
    assert!(t.node("p", &fid).await["change"].as_str().unwrap().contains("dup"));
    // a viewer may not write
    t.ok("adm", "POST", "/api/settings/edges", Some(json!({"from": "user:alice", "to": "project:p", "type": "member", "settings": {"role": "viewer"}}))).await;
    assert_eq!(t.call("alice", "POST", "/api/projects/p/graph/reqs", Some(json!({"file": fid, "title": "x"}))).await.0, 403);
    t.done().await;
}

#[tokio::test]
async fn reqs_move_between_files_keeping_their_id() {
    let t = T::new().await;
    t.project("p").await; // designed: no engine
    t.project("q").await;
    let g = t.graph("p").await;
    let pid = id(find(&g, "p"));
    let biz = id(find(&g, "Business requirements"));
    let tech = id(find(&g, "Technical requirements"));
    // node = the project: its global file (already attached), no new one
    let add = |f: Value| t.ok("adm", "POST", "/api/projects/p/graph/reqs", Some(f));
    let ra = add(json!({"node": pid, "type": "bizreq", "key": "B-1", "title": "A", "text": "a."})).await;
    assert_eq!((ra["file"]["id"].as_str(), ra["file"]["file_state"].as_str()), (Some(biz.as_str()), Some("designed")));
    assert_eq!(ra["touched"], json!([biz]));
    let a = id(&ra["req"]);
    let b = id(&add(json!({"file": biz, "title": "B", "text": "b."})).await["req"]);
    let c = id(&add(json!({"file": biz, "title": "C", "text": "c."})).await["req"]);
    let x = id(&add(json!({"node": pid, "type": "techreq", "title": "X", "text": "x."})).await["req"]);
    assert_eq!(t.graph("p").await["nodes"].as_array().unwrap().iter().filter(|n| n["nodetype"] != "req").count(), 4, "no new files");
    let order = |body: String| parse_reqs(&body).1.iter().map(|x| x.id.clone()).collect::<Vec<_>>();
    let (vb, vt) = (t.node("p", &biz).await["node_version"].as_u64().unwrap(), t.node("p", &tech).await["node_version"].as_u64().unwrap());
    // move B into the techreq file after X: same id, both files rewritten
    let mv = t.ok("adm", "POST", &format!("/api/projects/p/graph/reqs/{b}/move"), Some(json!({"to_file": tech, "after": x}))).await;
    assert_eq!((mv["req"]["id"].as_str(), mv["req"]["file"].as_str(), mv["req"]["file_type"].as_str()), (Some(b.as_str()), Some(tech.as_str()), Some("techreq")));
    assert_eq!((mv["file"]["id"].as_str(), mv["from_file"]["id"].as_str()), (Some(tech.as_str()), Some(biz.as_str())));
    assert_eq!(order(body_of(&t, "p", &biz).await), vec![a.clone(), c.clone()]);
    assert_eq!(order(body_of(&t, "p", &tech).await), vec![x.clone(), b.clone()]);
    let (fb, ft) = (t.node("p", &biz).await, t.node("p", &tech).await);
    assert_eq!((fb["node_version"].as_u64(), ft["node_version"].as_u64()), (Some(vb + 1), Some(vt + 1)));
    assert_eq!((fb["file_state"].as_str(), ft["file_state"].as_str()), (Some("designed"), Some("designed")));
    let moved = t.node("p", &b).await;
    assert_eq!((moved["title"].as_str(), moved["text"].as_str(), moved["path"].as_str()), (Some("B"), Some("b."), Some(format!("{}#{b}", ft["path"].as_str().unwrap()).as_str())));
    let g = t.graph("p").await;
    assert!(has_edge(&g, &tech, "contains", &b) && !has_edge(&g, &biz, "contains", &b));
    // drag the contains edge's file end back onto the bizreq file, after A
    let em = t.ok("adm", "POST", "/api/projects/p/graph/edges/move", Some(json!({"from": tech, "to": b, "kind": "contains", "new_from": biz, "after": a}))).await;
    assert_eq!(em["edge"], json!({"from": biz, "kind": "contains", "to": b}));
    assert_eq!(order(body_of(&t, "p", &biz).await), vec![a.clone(), b.clone(), c.clone()]);
    assert_eq!(order(body_of(&t, "p", &tech).await), vec![x.clone()]);
    // reorder within one file (no after = last)
    t.ok("adm", "POST", &format!("/api/projects/p/graph/reqs/{a}/move"), Some(json!({"to_file": biz}))).await;
    assert_eq!(order(body_of(&t, "p", &biz).await), vec![b.clone(), c.clone(), a.clone()]);
    t.ok("adm", "POST", &format!("/api/projects/p/graph/reqs/{a}/move"), Some(json!({"to_file": biz, "after": b}))).await;
    assert_eq!(order(body_of(&t, "p", &biz).await), vec![b.clone(), a.clone(), c.clone()]);
    // refusals
    let philo = id(find(&t.graph("p").await, "Philosophy"));
    let qbiz = id(find(&t.graph("q").await, "Business requirements"));
    let tr = &t;
    let call = move |path: String, body: Value| async move { tr.call("adm", "POST", &path, Some(body)).await };
    let mvp = format!("/api/projects/p/graph/reqs/{a}/move");
    assert_eq!(call(mvp.clone(), json!({"to_file": philo})).await.0, 400, "not a req file");
    assert_eq!(call(mvp.clone(), json!({"to_file": pid})).await.0, 400);
    let (code, v) = call(mvp.clone(), json!({"to_file": qbiz})).await;
    assert_eq!(code, 400);
    assert!(v["error"].as_str().unwrap().contains("project q"), "{v}");
    assert_eq!(call(mvp.clone(), json!({"to_file": "nope"})).await.0, 404);
    assert_eq!(call(mvp.clone(), json!({})).await.0, 400);
    assert_eq!(call(mvp.clone(), json!({"to_file": tech, "after": c})).await.0, 400, "after must be in the target");
    assert_eq!(call(mvp.clone(), json!({"to_file": biz, "after": a})).await.0, 400);
    assert_eq!(call("/api/projects/p/graph/reqs/nope/move".into(), json!({"to_file": biz})).await.0, 404);
    let em = "/api/projects/p/graph/edges/move".to_string();
    assert_eq!(call(em.clone(), json!({"from": biz, "to": a, "kind": "contains", "new_to": c})).await.0, 400);
    assert_eq!(call(em.clone(), json!({"from": tech, "to": a, "kind": "contains", "new_from": biz})).await.0, 404, "a is not in tech");
    assert_eq!(call("/api/projects/p/graph/edges".into(), json!({"from": tech, "to": a, "kind": "contains"})).await.0, 400);
    // nothing changed by the refusals
    assert_eq!(order(body_of(&t, "p", &biz).await), vec![b.clone(), a.clone(), c.clone()]);
    // graph/view: req nodes under their file; the file carries its table
    let v = t.ok("adm", "GET", "/api/projects/p/graph/view", None).await;
    let vn = |i: &str| v["nodes"].as_array().unwrap().iter().find(|n| n["id"] == i).unwrap().clone();
    let fv = vn(&biz);
    assert_eq!(fv["req_count"], 3);
    let rows: Vec<&str> = fv["reqs"].as_array().unwrap().iter().map(|r| r["id"].as_str().unwrap()).collect();
    assert_eq!(rows, [b.as_str(), a.as_str(), c.as_str()]);
    assert_eq!(fv["reqs"][1], json!({"id": a, "key": "B-1", "title": "A", "status": "draft"}));
    assert_eq!(vn(&tech)["req_count"], 1);
    assert!(vn(&philo).get("req_count").is_none());
    let rv = vn(&a);
    assert_eq!((rv["type_label"].as_str(), rv["level"].as_str(), rv["parent"].as_str(), rv["file_state"].as_str()), (Some("Requirement"), Some("req"), Some(biz.as_str()), Some("designed")));
    assert_eq!(rv["owners"], json!([biz, pid]));
    assert!(v["edges"].as_array().unwrap().iter().any(|e| e["type"] == "contains" && e["source"] == biz && e["target"] == a));
    assert_eq!(v["summary"]["counts"]["nodes_per_level"]["req"], 4);
    assert_eq!(v["summary"]["file_sync"]["designed"], 4, "req nodes are not files");
    // build: the files go out with their sections; never a req node
    t.ok("eng1", "PUT", "/api/engines/e1", Some(json!({"state": "Running"}))).await;
    t.ok("adm", "POST", "/api/projects/p/build", Some(json!({"engine": "e1", "topdir": "/w/p"}))).await;
    let pend = t.pending("p").await;
    assert_eq!(pend.len(), 4);
    assert!(pend.iter().any(|o| o["id"] == biz && o["text"].as_str().unwrap().contains(&format!("<!-- req: id={a} status=draft -->"))));
    assert_eq!(t.node("p", &a).await["file_state"], "pending_write");
    t.done().await;
}

#[tokio::test]
async fn a_req_id_in_two_files_stays_with_the_first_by_path() {
    let t = T::new().await;
    t.served("p").await;
    let pa = "{topdir}/src/a/reqs/a.techreq.iter.md";
    let pb = "{topdir}/src/b/reqs/b.techreq.iter.md";
    let body = |title: &str| render_reqs("", &[ri(RD, "", title, "agreed", "Same id."), ri(RA, "", "Own", "draft", "Only here.")]);
    let fa = raw_req_file(NodeType::Techreq, "A tech", pa, &body("In A"));
    let fbv = raw_req_file(NodeType::Techreq, "B tech", pb, &render_reqs("", &[ri(RD, "", "In B", "agreed", "Same id.")]));
    // b is synced first: the path order decides, not the arrival order
    let r = t.sync("p", json!({"engine": "e1", "files": [fbv]})).await;
    assert!(r["errors"].as_array().unwrap().is_empty());
    t.sync("p", json!({"engine": "e1", "files": [fa]})).await;
    let g = t.graph("p").await;
    let file_at = |p: &str| g["nodes"].as_array().unwrap().iter().find(|n| n["path"] == p).unwrap().clone();
    let (na, nb) = (file_at(pa), file_at(pb));
    let rd = t.node("p", RD).await;
    assert_eq!((rd["file"].as_str(), rd["title"].as_str()), (na["id"].as_str(), Some("In A")));
    // b's copy got a fresh id and waits to be written
    assert_eq!(nb["file_state"], "pending_write");
    let (_, items) = parse_reqs(nb["body"].as_str().unwrap());
    assert_eq!(items.len(), 1);
    assert!(items[0].id != RD && nf::is_valid_id(&items[0].id));
    assert_eq!((items[0].title.as_str(), items[0].status.as_str()), ("In B", "agreed"));
    assert_eq!(t.node("p", &items[0].id).await["file"], nb["id"]);
    let rs = reqs_of(&g);
    assert_eq!(rs.len(), 3);
    let pend = t.pending("p").await;
    assert_eq!(pend.len(), 1);
    assert_eq!(pend[0]["path"], pb);
    assert!(!pend[0]["text"].as_str().unwrap().contains(RD));
    // within one file a duplicate is conformed away
    let dup = render_reqs("", &[ri(RB, "", "One", "", "1."), ri(RB, "", "Two", "", "2.")]);
    let pc = "{topdir}/src/c/reqs/c.bizreq.iter.md";
    t.sync("p", json!({"engine": "e1", "files": [raw_req_file(NodeType::Bizreq, "C biz", pc, &dup)]})).await;
    let g = t.graph("p").await;
    let cid = g["nodes"].as_array().unwrap().iter().find(|n| n["path"] == pc).unwrap()["id"].as_str().unwrap().to_string();
    let mine: Vec<&Value> = g["nodes"].as_array().unwrap().iter().filter(|n| n["nodetype"] == "req" && n["file"] == cid.as_str()).collect();
    assert_eq!(mine.len(), 2);
    assert_eq!(mine.iter().filter(|r| r["id"] == RB).count(), 1);
    t.done().await;
}

#[tokio::test]
async fn mcp_req_tools() {
    let t = T::new().await;
    t.served("p").await;
    let r = t.mcp("eng1", "p", "graph_node_create", json!({"nodetype": "code", "level": "context", "name": "Web"})).await;
    let web = r["structuredContent"]["node"]["id"].as_str().unwrap().to_string();
    let r = t.mcp("eng1", "p", "req_create", json!({"node": web, "type": "bizreq", "key": "W-1", "title": "Loads", "text": "Loads in 1s."})).await;
    assert_eq!(r["isError"], false, "{r}");
    let biz = r["structuredContent"]["file"]["id"].as_str().unwrap().to_string();
    let w1 = r["structuredContent"]["req"]["id"].as_str().unwrap().to_string();
    assert_eq!(r["structuredContent"]["file"]["path"], "{topdir}/src/web/reqs/web.bizreq.iter.md");
    let r = t.mcp("eng1", "p", "req_create", json!({"node": web, "type": "techreq", "title": "Uses TLS", "text": "Always."})).await;
    let tech = r["structuredContent"]["file"]["id"].as_str().unwrap().to_string();
    let v = t.node("p", &biz).await["node_version"].as_u64().unwrap();
    let r = t.mcp("eng1", "p", "req_update", json!({"id": w1, "status": "agreed", "expect_version": v})).await;
    assert_eq!(r["structuredContent"]["req"]["status"], "agreed", "{r}");
    let r = t.mcp("eng1", "p", "req_update", json!({"id": w1, "title": "x", "expect_version": v})).await;
    assert_eq!(r["isError"], true, "stale version");
    let r = t.mcp("eng1", "p", "req_move", json!({"id": w1, "to_file": tech})).await;
    assert_eq!(r["structuredContent"]["req"]["file"], tech.as_str(), "{r}");
    assert_eq!(t.node("p", &w1).await["file_type"], "techreq");
    let r = t.mcp("eng1", "p", "req_delete", json!({"id": w1, "reason": "covered elsewhere"})).await;
    assert_eq!(r["structuredContent"]["removed"], true, "{r}");
    assert_eq!(t.call("adm", "GET", &format!("/api/projects/p/graph/nodes/{w1}"), None).await.0, 404);
    let r = t.mcp("eng1", "p", "req_delete", json!({"id": w1, "reason": "again"})).await;
    assert_eq!(r["isError"], true);
    // same authz: an engine that does not serve q is refused
    t.project("q").await;
    let qp = id(find(&t.graph("q").await, "q"));
    let r = t.mcp("eng1", "q", "req_create", json!({"node": qp, "type": "bizreq", "title": "x"})).await;
    assert_eq!(r["isError"], true);
    let (_, l) = t.call("eng1", "POST", "/mcp", Some(json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list"}))).await;
    let names: Vec<&str> = l["result"]["tools"].as_array().unwrap().iter().map(|x| x["name"].as_str().unwrap()).collect();
    for n in ["req_create", "req_update", "req_move", "req_delete"] {
        assert!(names.contains(&n), "{n}");
    }
    t.done().await;
}
