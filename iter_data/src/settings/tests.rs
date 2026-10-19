//! Settings graph, assignments, server-side get_next and project authz —
//! each test on its own throwaway ArangoDB database (test_db.rs), driven
//! through the real router (and its authz layer) with minted tokens.

use super::*;
use crate::api::AppState;
use axum::body::Body;
use axum::http::Request;
use tower::ServiceExt;
use iter_core::{LockRow, WorkItem};

struct T {
    st: Arc<AppState>,
    db: String,
    app: Router,
}

impl T {
    async fn new() -> T {
        let (a, db) = crate::test_db::store().await;
        let st = Arc::new(AppState::for_tests(Arc::new(a), b"settings-test"));
        bootstrap(st.store.as_ref()).await.unwrap();
        for (u, role) in [("adm", "admin"), ("alice", "user"), ("bob", "user"), ("vic", "viewer")] {
            st.store.put("webui_user", u, NOSK, &json!({"user": u, "role": role, "tokenver": 1, "pwhash": ""})).await.unwrap();
        }
        let app = crate::api::router(st.clone()).merge(crate::mcp::routes(st.clone()));
        T { st, db, app }
    }
    /// A bearer for `who`: an engine session for "eng1" (engine e1), "eng2"
    /// (e2) or "eng:<engine>"; else a person's token.
    async fn tok(&self, who: &str) -> String {
        let engine = match who {
            "eng1" => Some("e1"),
            "eng2" => Some("e2"),
            w => w.strip_prefix("eng:"),
        };
        if let Some(e) = engine {
            return crate::keys::test_session(&self.st, e).await;
        }
        let role = match who {
            "adm" => "admin",
            "vic" => "viewer",
            _ => "user",
        };
        crate::auth::mint_token(&self.st.secret, who, role, 1, 3600).unwrap()
    }
    async fn call(&self, who: &str, method: &str, path: &str, body: Option<Value>) -> (u16, Value) {
        let mut b = Request::builder().method(method).uri(path).header("authorization", format!("Bearer {}", self.tok(who).await));
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
    async fn edges(&self) -> Vec<SysEdge> {
        load_edges(self.store()).await.unwrap()
    }
    async fn edge(&self, t: &str, from: &str, to: &str) -> Option<SysEdge> {
        self.edges().await.into_iter().find(|e| e.edge_type == t && e.from == from && e.to == to)
    }
    async fn done(self) {
        crate::test_db::drop(&self.db).await;
    }
    /// project p (Running) served by engine `e` (owned by token user `owner`)
    async fn served(&self, project: &str, engine: &str, owner: &str) {
        self.ok("adm", "PUT", &format!("/api/projects/{project}"), Some(json!({"state": "Running"}))).await;
        if self.store().get("engine", engine, NOSK).await.unwrap().is_none() {
            self.ok(owner, "PUT", &format!("/api/engines/{engine}"), Some(json!({"state": "Running"}))).await;
        }
        self.ok("adm", "POST", "/api/settings/edges", Some(json!({"from": format!("iter_engine:{engine}"), "to": format!("project:{project}"), "settings": {"topdir": format!("/w/{project}")}}))).await;
    }
    async fn item(&self, project: &str, body: Value) -> Value {
        let mut b = json!({"project": project, "state": "queued", "agent": "code", "priority": 40, "version": 1,
                           "dedup_checked": "2026-10-01T00:00:00Z", "ts": {"receive": "2026-10-01T00:00:00Z"}});
        merge(&mut b, &body);
        let id = body_str(&b, "id");
        self.store().put_versioned("workitem", project, &id, &b, 0).await.unwrap();
        b
    }
    async fn next(&self, who: &str, project: &str, engine: &str, extra: Value) -> (u16, Value) {
        let mut b = json!({"engine": engine, "lease_ttl_sec": 300, "max": 1});
        merge(&mut b, &extra);
        self.call(who, "POST", &format!("/api/projects/{project}/next"), Some(b)).await
    }
}

// ---------- settings graph ----------

#[tokio::test]
async fn graph_crud_validation_and_inferred_types() {
    let t = T::new().await;
    // seeded: providers, workitem types, placeholders, iter_data:self
    let g = t.ok("adm", "GET", "/api/settings/graph", None).await;
    let ids: Vec<&str> = g["nodes"].as_array().unwrap().iter().map(|n| n["id"].as_str().unwrap()).collect();
    for want in ["iter_data:self", "provider:claude", "provider:mock", "workitem_type:queued", "workitem_type:scheduled", "project:_deactivated", "account:_deactivated", "user:adm"] {
        assert!(ids.contains(&want), "{want} in {ids:?}");
    }
    assert_eq!(NODE_TYPES.len() - 1, ids.iter().filter(|i| i.ends_with(":_deactivated")).count());
    let ph = g["nodes"].as_array().unwrap().iter().find(|n| n["id"] == "agent:_deactivated").unwrap();
    assert_eq!((ph["placeholder"].as_bool(), ph["deactivated"].as_bool()), (Some(true), Some(true)));
    // user nodes never show secrets
    let u = g["nodes"].as_array().unwrap().iter().find(|n| n["id"] == "user:adm").unwrap();
    assert!(u["settings"].get("pwhash").is_none());

    // nodes: create, refuse duplicates / bad types / bad records
    t.ok("adm", "POST", "/api/settings/nodes", Some(json!({"type": "project", "name": "p1", "settings": {"desc": "x"}}))).await;
    // an engine node needs its host, and one engine per host
    assert_eq!(t.call("adm", "POST", "/api/settings/nodes", Some(json!({"type": "iter_engine", "name": "nohost", "settings": {}}))).await.0, 400);
    t.ok("adm", "POST", "/api/settings/nodes", Some(json!({"type": "iter_engine", "name": "mbp", "settings": {"host": "mbp.local"}}))).await;
    assert_eq!(t.call("adm", "POST", "/api/settings/nodes", Some(json!({"type": "iter_engine", "name": "mbp2", "settings": {"host": "MBP.local"}}))).await.0, 409);
    let acct = t.ok("adm", "POST", "/api/settings/nodes", Some(json!({"type": "account", "name": "main", "settings": {"token_envar": "CLAUDE_TOKEN_MAIN", "provider": "mock"}}))).await;
    assert_eq!(acct["id"], "account:main");
    assert!(t.edge("of", "account:main", "provider:mock").await.unwrap().is_active(), "an account is `of` its provider");
    assert_eq!(t.call("adm", "POST", "/api/settings/nodes", Some(json!({"type": "project", "name": "p1"}))).await.0, 409);
    assert_eq!(t.call("adm", "POST", "/api/settings/nodes", Some(json!({"type": "gadget", "name": "x"}))).await.0, 400);
    assert_eq!(t.call("adm", "POST", "/api/settings/nodes", Some(json!({"type": "iter_data", "name": "two"}))).await.0, 400);
    assert_eq!(t.call("adm", "POST", "/api/settings/nodes", Some(json!({"type": "workitem_type", "name": "zombie"}))).await.0, 400);
    assert_eq!(t.call("adm", "POST", "/api/settings/nodes", Some(json!({"type": "project", "name": "_deactivated"}))).await.0, 400);
    assert_eq!(t.call("adm", "POST", "/api/settings/nodes", Some(json!({"type": "project", "name": "p2", "settings": {"state": "Sleeping"}}))).await.0, 400);
    // a new project gets allows edges for every state; `failed` carries the retry policy
    for s in iter_core::STATES {
        assert!(t.edge("allows", "project:p1", &format!("workitem_type:{s}")).await.is_some(), "allows {s}");
    }
    assert_eq!(t.edge("allows", "project:p1", "workitem_type:failed").await.unwrap().settings["maxattempts"], 5);
    // writes are admin-only
    assert_eq!(t.call("alice", "POST", "/api/settings/nodes", Some(json!({"type": "project", "name": "p9"}))).await.0, 403);

    // edges: type inferred from the endpoints
    let e = t.ok("adm", "POST", "/api/settings/edges", Some(json!({"from": "iter_engine:mbp", "to": "project:p1", "tag": "p1_default", "settings": {"topdir": "~/dev/p1", "read_only": false}}))).await;
    assert_eq!((e["type"].as_str(), e["active"].as_bool(), e["tag"].as_str()), (Some("serves"), Some(true), Some("p1_default")));
    // validation: no type joins these, wrong declared type, unknown node, bad settings, duplicate
    assert_eq!(t.call("adm", "POST", "/api/settings/edges", Some(json!({"from": "project:p1", "to": "iter_engine:mbp"}))).await.0, 400);
    assert_eq!(t.call("adm", "POST", "/api/settings/edges", Some(json!({"type": "bills", "from": "iter_engine:mbp", "to": "project:p1"}))).await.0, 400);
    assert_eq!(t.call("adm", "POST", "/api/settings/edges", Some(json!({"from": "iter_engine:ghost", "to": "project:p1"}))).await.0, 404);
    assert_eq!(t.call("adm", "POST", "/api/settings/edges", Some(json!({"from": "account:main", "to": "project:p1", "settings": {"switch": 150}}))).await.0, 400);
    assert_eq!(t.call("adm", "POST", "/api/settings/edges", Some(json!({"from": "iter_engine:mbp", "to": "project:p1"}))).await.0, 409);
    assert_eq!(t.call("adm", "POST", "/api/settings/edges", Some(json!({"from": "nonsense", "to": "project:p1"}))).await.0, 400);
    let b = t.ok("adm", "POST", "/api/settings/edges", Some(json!({"from": "account:main", "to": "project:p1", "settings": {"order": 1, "switch": 80, "stop": 95}}))).await;
    assert_eq!(b["type"], "bills");
    // PATCH settings merges (null removes); PATCH node settings merges and re-validates
    let eid = e["id"].as_str().unwrap();
    let e2 = t.ok("adm", "PATCH", &format!("/api/settings/edges/{eid}"), Some(json!({"settings": {"read_only": true}, "tag": "ro"}))).await;
    assert_eq!((e2["settings"]["topdir"].as_str(), e2["settings"]["read_only"].as_bool(), e2["tag"].as_str()), (Some("~/dev/p1"), Some(true), Some("ro")));
    assert_eq!(t.call("adm", "PATCH", &format!("/api/settings/edges/{eid}"), Some(json!({"settings": {"read_only": "maybe"}}))).await.0, 400);
    let n = t.ok("adm", "PATCH", "/api/settings/nodes/project:p1", Some(json!({"settings": {"desc": "changed", "gitrepo": "git@x"}}))).await;
    assert_eq!((n["settings"]["desc"].as_str(), n["settings"]["gitrepo"].as_str()), (Some("changed"), Some("git@x")));
    assert_eq!(t.call("adm", "PATCH", "/api/settings/nodes/project:p1", Some(json!({"settings": {"state": "Bogus"}}))).await.0, 400);
    // the stored record is the project record the rest of the API reads
    assert_eq!(t.ok("adm", "GET", "/api/projects/p1", None).await["desc"], "changed");
    // admin switch: active false keeps the edge, inactive
    let off = t.ok("adm", "PATCH", &format!("/api/settings/edges/{eid}"), Some(json!({"active": false}))).await;
    assert_eq!((off["active"].as_bool(), off["enabled"].as_bool()), (Some(false), Some(false)));
    // edge delete
    t.ok("adm", "DELETE", &format!("/api/settings/edges/{}", b["id"].as_str().unwrap()), None).await;
    assert_eq!(t.call("adm", "DELETE", &format!("/api/settings/edges/{}", b["id"].as_str().unwrap()), None).await.0, 404);
    t.done().await;
}

#[tokio::test]
async fn placeholders_moves_and_copy() {
    let t = T::new().await;
    for (ty, name) in [("project", "p1"), ("project", "p2"), ("iter_engine", "e1"), ("iter_engine", "e2")] {
        let settings = if ty == "iter_engine" { json!({"host": format!("{name}.local")}) } else { json!({}) };
        t.ok("adm", "POST", "/api/settings/nodes", Some(json!({"type": ty, "name": name, "settings": settings}))).await;
    }
    let e = t.ok("adm", "POST", "/api/settings/edges", Some(json!({"from": "iter_engine:e1", "to": "project:p1", "tag": "t", "settings": {"topdir": "/a"}}))).await;
    let id = e["id"].as_str().unwrap().to_string();
    // drag the engine end onto the placeholder: kept, inactive, settings intact
    let m = t.ok("adm", "PATCH", &format!("/api/settings/edges/{id}"), Some(json!({"from": "iter_engine:_deactivated"}))).await;
    assert_eq!((m["active"].as_bool(), m["settings"]["topdir"].as_str()), (Some(false), Some("/a")));
    // a placeholder of the wrong type does not fit the edge type
    assert_eq!(t.call("adm", "PATCH", &format!("/api/settings/edges/{id}"), Some(json!({"from": "account:_deactivated"}))).await.0, 400);
    // back onto a real engine: active again
    let m = t.ok("adm", "PATCH", &format!("/api/settings/edges/{id}"), Some(json!({"from": "iter_engine:e2"}))).await;
    assert_eq!((m["active"].as_bool(), m["from"].as_str()), (Some(true), Some("iter_engine:e2")));
    // copy/paste onto another project: same type, tag and settings
    let c = t.ok("adm", "POST", &format!("/api/settings/edges/{id}/copy"), Some(json!({"to": "project:p2"}))).await;
    assert_eq!((c["type"].as_str(), c["tag"].as_str(), c["settings"]["topdir"].as_str(), c["from"].as_str(), c["to"].as_str()),
               (Some("serves"), Some("t"), Some("/a"), Some("iter_engine:e2"), Some("project:p2")));
    assert_ne!(c["id"], e["id"]);
    // pasting where it already exists is a duplicate
    assert_eq!(t.call("adm", "POST", &format!("/api/settings/edges/{id}/copy"), Some(json!({}))).await.0, 409);
    // a copy onto a node of the wrong type is refused
    assert_eq!(t.call("adm", "POST", &format!("/api/settings/edges/{id}/copy"), Some(json!({"to": "iter_engine:e1"}))).await.0, 400);
    // deleting a node moves its edges to the placeholder (inactive, settings kept)
    let d = t.ok("adm", "DELETE", "/api/settings/nodes/iter_engine:e2", None).await;
    assert_eq!(d["edges_moved_to_placeholder"], 3, "two serves + the hosts edge");
    for ed in t.edges().await.into_iter().filter(|x| x.edge_type == "serves") {
        assert_eq!(ed.from, "iter_engine:_deactivated");
        assert!(!ed.is_active());
        assert_eq!(ed.settings["topdir"], "/a");
    }
    // ?edges=delete leaves nothing behind: the node's edges go too, none park on the placeholder
    t.ok("adm", "POST", "/api/settings/nodes", Some(json!({"type": "iter_engine", "name": "e3", "settings": {"host": "e3.local"}}))).await;
    t.ok("adm", "POST", "/api/settings/edges", Some(json!({"from": "iter_engine:e3", "to": "project:p1", "settings": {"topdir": "/c"}}))).await;
    t.ok("adm", "DELETE", "/api/settings/nodes/iter_engine:e3?edges=delete", None).await;
    assert!(t.edges().await.iter().all(|x| x.from != "iter_engine:e3" && x.to != "iter_engine:e3"));
    assert_eq!(t.edges().await.iter().filter(|x| x.from == "iter_engine:_deactivated").count(), 2, "only e2's two serves edges are parked");
    // the legacy delete routes do the same
    t.ok("adm", "DELETE", "/api/projects/p2", None).await;
    assert!(t.edges().await.iter().filter(|x| x.to == "project:p2").count() == 0);
    assert!(t.edges().await.iter().any(|x| x.edge_type == "allows" && x.from == "project:_deactivated"));
    // placeholders and iter_data:self are permanent and not editable
    for id in ["iter_engine:_deactivated", "project:_deactivated", "iter_data:self"] {
        assert_eq!(t.call("adm", "DELETE", &format!("/api/settings/nodes/{id}"), None).await.0, 400, "{id}");
        assert_eq!(t.call("adm", "PATCH", &format!("/api/settings/nodes/{id}"), Some(json!({"settings": {"x": 1}}))).await.0, 400, "{id}");
    }
    // many deactivated edges may share a placeholder (no duplicate rule there)
    t.ok("adm", "POST", "/api/settings/edges", Some(json!({"from": "iter_engine:_deactivated", "to": "project:p1"}))).await;
    t.done().await;
}

// ---------- a sample world ----------

/// Records and the edges that join them: engine mbp serves projects pdy (read-write) and
/// side (read-only), two accounts bill pdy and are held by mbp, every agent runs on every
/// project, every project allows every state.
async fn sample_world(store: &dyn Storage) {
    bootstrap(store).await.unwrap();
    let pdy = json!({"name": "pdy", "state": "Running",
        "failure": {"maxattempts": 3, "first_retry_second": 30, "retry_backoff_exponent": 2}});
    let side = json!({"name": "side", "state": "Stopped"});
    store.put("project", "pdy", NOSK, &pdy).await.unwrap();
    store.put("project", "side", NOSK, &side).await.unwrap();
    store.put("engine", "mbp", NOSK, &json!({"name": "mbp", "host": "mbp.local", "state": "Running"})).await.unwrap();
    for a in ["code", "plan", "test"] {
        store.put("agent", a, NOSK, &json!({"name": a, "promptbody": ""})).await.unwrap();
    }
    store.put("agent_tooling", "_shared", NOSK, &json!({"name": "_shared", "kind": "shared", "body": "rules"})).await.unwrap();
    store.put("agent_tooling", "aws", NOSK, &json!({"name": "aws", "kind": "capability", "body": "x"})).await.unwrap();
    let eid = node_id("iter_engine", "mbp");
    ensure_edge(store, "hosts", ITER_DATA_SELF, &eid, json!({})).await.unwrap();
    for (p, row, ro) in [("pdy", &pdy, false), ("side", &side, true)] {
        let pid = node_id("project", p);
        ensure_edge(store, "hosts", ITER_DATA_SELF, &pid, json!({})).await.unwrap();
        ensure_edge(store, "serves", &eid, &pid, json!({"topdir": format!("~/dev/{p}"), "read_only": ro})).await.unwrap();
        for s in iter_core::STATES {
            ensure_edge(store, "allows", &pid, &node_id("workitem_type", s), allows_settings(s, Some(row))).await.unwrap();
        }
        for a in ["code", "plan", "test"] {
            let ovr = if p == "pdy" && a == "code" { json!({"model": "opus", "max": 2}) } else { json!({}) };
            ensure_edge(store, "runs", &node_id("agent", a), &pid, ovr).await.unwrap();
        }
    }
    for (name, envar, order, switch, stop) in [("Dev1", "DEV1_TOKEN", 1, 80, 95), ("Dev2", "DEV2_TOKEN", 2, 90, 98)] {
        store.put("account", name, NOSK, &json!({"name": name, "token_envar": envar, "provider": "claude", "desc": ""})).await.unwrap();
        let aid = node_id("account", name);
        ensure_edge(store, "of", &aid, &node_id("provider", "claude"), json!({})).await.unwrap();
        ensure_edge(store, "bills", &aid, &node_id("project", "pdy"), json!({"order": order, "switch": switch, "stop": stop, "model": ""})).await.unwrap();
        ensure_edge(store, "holds", &eid, &aid, json!({})).await.unwrap();
    }
    for a in ["code", "plan", "test"] {
        let aid = node_id("agent", a);
        ensure_edge(store, "handles", &aid, &node_id("workitem_type", "queued"), json!({})).await.unwrap();
        ensure_edge(store, "uses", &node_id("agent_tools", "_shared"), &aid, json!({})).await.unwrap();
    }
}

// ---------- assignments ----------

#[tokio::test]
async fn assignments_follow_active_edges() {
    let (a, db) = crate::test_db::store().await;
    let store: &dyn Storage = &a;
    sample_world(store).await;
    let asg = assignments(store, "mbp").await.unwrap();
    assert_eq!(asg.engine, "mbp");
    let names: Vec<&str> = asg.projects.iter().map(|p| p.project.as_str()).collect();
    assert_eq!(names, ["pdy", "side"]);
    let pdy = &asg.projects[0];
    assert_eq!((pdy.topdir.as_str(), pdy.read_only, pdy.state.as_str()), ("~/dev/pdy", false, "Running"));
    assert_eq!(asg.projects[1].state, "Stopped");
    assert!(asg.projects[1].read_only);
    let accts: Vec<(&str, i64, u8, u8)> = pdy.accounts.iter().map(|a| (a.name.as_str(), a.order, a.switch, a.stop)).collect();
    assert_eq!(accts, [("Dev1", 1, 80, 95), ("Dev2", 2, 90, 98)]);
    assert_eq!(pdy.accounts[0].provider, "claude");
    assert_eq!(pdy.accounts[0].token_envar, "DEV1_TOKEN");
    assert_eq!(pdy.agents["code"]["model"], "opus");
    assert_eq!(asg.accounts.iter().map(|a| a.name.as_str()).collect::<Vec<_>>(), ["Dev1", "Dev2"]);
    // holds override of the token env var
    let mut h = load_edges(store).await.unwrap().into_iter().find(|e| e.edge_type == "holds" && e.to == "account:Dev2").unwrap();
    h.settings = json!({"token_envar": "MY_DEV2"});
    put_edge(store, &h).await.unwrap();
    // an account not held by the engine is not offered, even when billed
    let mut h1 = load_edges(store).await.unwrap().into_iter().find(|e| e.edge_type == "holds" && e.to == "account:Dev1").unwrap();
    h1.active = false;
    put_edge(store, &h1).await.unwrap();
    // a deactivated serves edge drops the project
    let mut s = load_edges(store).await.unwrap().into_iter().find(|e| e.edge_type == "serves" && e.to == "project:side").unwrap();
    s.from = placeholder_id("iter_engine");
    s.active = false;
    put_edge(store, &s).await.unwrap();
    // a disabled agent (runs edge off) is not listed
    let mut r = load_edges(store).await.unwrap().into_iter().find(|e| e.edge_type == "runs" && e.from == "agent:plan" && e.to == "project:pdy").unwrap();
    r.active = false;
    put_edge(store, &r).await.unwrap();
    let asg = assignments(store, "mbp").await.unwrap();
    assert_eq!(asg.projects.len(), 1);
    assert_eq!(asg.projects[0].accounts.iter().map(|a| (a.name.as_str(), a.token_envar.as_str())).collect::<Vec<_>>(), [("Dev2", "MY_DEV2")]);
    assert!(!asg.projects[0].agents.contains_key("plan") && asg.projects[0].agents.contains_key("test"));
    assert!(assignments(store, "nobody").await.unwrap().projects.is_empty());
    crate::test_db::drop(&db).await;
}

#[tokio::test]
async fn assignments_route_is_owner_or_admin() {
    let t = T::new().await;
    t.served("p1", "e1", "eng1").await;
    t.served("p2", "e2", "eng2").await;
    let mine = t.ok("eng1", "GET", "/api/engines/e1/assignments", None).await;
    assert_eq!(mine["projects"][0]["project"], "p1");
    assert_eq!(mine["projects"][0]["topdir"], "/w/p1");
    assert_eq!(t.call("eng1", "GET", "/api/engines/e2/assignments", None).await.0, 403);
    assert_eq!(t.call("alice", "GET", "/api/engines/e1/assignments", None).await.0, 403);
    t.ok("adm", "GET", "/api/engines/e2/assignments", None).await;
    assert_eq!(t.call("adm", "GET", "/api/engines/none/assignments", None).await.0, 404);
    // an engine registers as itself: no owning user, no owns edge
    assert_eq!(t.store().get("engine", "e1", NOSK).await.unwrap().unwrap()["user"].as_str().unwrap_or(""), "");
    assert!(!t.edges().await.iter().any(|e| e.edge_type == "owns" && e.to == "iter_engine:e1"));
    // an engine reads only its own assignments, even of a record no person owns
    t.ok("adm", "PUT", "/api/engines/e9", Some(json!({"state": "Running"}))).await;
    assert_eq!(t.call("eng1", "GET", "/api/engines/e9/assignments", None).await.0, 403);
    t.done().await;
}

// ---------- server-side get_next ----------

#[tokio::test]
async fn next_orders_by_priority_and_receive_and_honours_deps() {
    let t = T::new().await;
    t.served("p", "e1", "eng1").await;
    t.item("p", json!({"id": "late-p40", "priority": 40, "ts": {"receive": "2026-10-01T02:00:00Z"}})).await;
    t.item("p", json!({"id": "early-p40", "priority": 40, "ts": {"receive": "2026-10-01T01:00:00Z"}})).await;
    t.item("p", json!({"id": "p5-blocked", "priority": 5, "blockedby": ["early-p40"]})).await;
    t.item("p", json!({"id": "p50", "priority": 50})).await;
    let order: Vec<String> = {
        let mut v = Vec::new();
        for _ in 0..3 {
            let (c, r) = t.next("eng1", "p", "e1", json!({})).await;
            assert_eq!(c, 200, "{r}");
            v.push(r["item"]["id"].as_str().unwrap().to_string());
        }
        v
    };
    // p5 waits on early-p40 (in progress now): it never runs before it
    assert_eq!(order, ["early-p40", "late-p40", "p50"]);
    let (_, r) = t.next("eng1", "p", "e1", json!({})).await;
    assert_eq!((r["item"].is_null(), r["reason"].as_str()), (true, Some("blocked")));
    // the claim: versioned, in-progress, engine, attempt+1, lease, start ts
    let w = t.store().get("workitem", "p", "early-p40").await.unwrap().unwrap();
    assert_eq!((w["state"].as_str(), w["engine"].as_str(), w["attempt"].as_u64(), w["version"].as_u64()), (Some("in-progress"), Some("e1"), Some(1), Some(2)));
    assert!(!body_str(&w, "lease").is_empty() && !w["ts"]["start"].as_str().unwrap_or("").is_empty());
    // once the blocker completes, p5 goes first
    let mut done = w.clone();
    done["state"] = json!("complete");
    done["version"] = json!(3);
    t.store().put_versioned("workitem", "p", "early-p40", &done, 2).await.unwrap();
    let (_, r) = t.next("eng1", "p", "e1", json!({})).await;
    assert_eq!(r["item"]["id"], "p5-blocked");
    let (_, r) = t.next("eng1", "p", "e1", json!({})).await;
    assert_eq!(r["reason"], "none-queued");
    t.done().await;
}

#[tokio::test]
async fn next_skips_held_items_and_filters_agents() {
    let t = T::new().await;
    t.served("p", "e1", "eng1").await;
    t.item("p", json!({"id": "approval", "priority": 1, "needs_approval": true})).await;
    t.item("p", json!({"id": "backoff", "priority": 1, "retry_after": "2999-01-01T00:00:00Z"})).await;
    t.item("p", json!({"id": "untriaged", "priority": 1, "dedup_checked": ""})).await;
    t.item("p", json!({"id": "paused", "priority": 1, "state": "paused"})).await;
    t.item("p", json!({"id": "plan-item", "priority": 2, "agent": "plan"})).await;
    t.item("p", json!({"id": "shell", "priority": 3, "agent": "exec", "exec_shell": "true"})).await;
    t.item("p", json!({"id": "code-item", "priority": 4, "agent": "code"})).await;
    // agents_allowed: only `code` — the plan item waits, shell items are outside the caps
    let (_, r) = t.next("eng1", "p", "e1", json!({"agents_allowed": ["code"]})).await;
    assert_eq!(r["item"]["id"], "shell");
    let (_, r) = t.next("eng1", "p", "e1", json!({"agents_allowed": ["code"]})).await;
    assert_eq!(r["item"]["id"], "code-item");
    let (_, r) = t.next("eng1", "p", "e1", json!({"agents_allowed": ["code"]})).await;
    assert_eq!(r["reason"], "blocked");
    // an agent with a record but no active runs edge is disabled on the project
    t.ok("adm", "PUT", "/api/agents/plan", Some(json!({"promptbody": ""}))).await;
    let runs = t.edge("runs", "agent:plan", "project:p").await.unwrap();
    assert!(runs.is_active(), "a new agent runs on every project");
    t.ok("adm", "PATCH", &format!("/api/settings/edges/{}", runs.id), Some(json!({"active": false}))).await;
    let (_, r) = t.next("eng1", "p", "e1", json!({})).await;
    assert_eq!(r["reason"], "blocked");
    t.ok("adm", "PATCH", &format!("/api/settings/edges/{}", runs.id), Some(json!({"active": true}))).await;
    let (_, r) = t.next("eng1", "p", "e1", json!({})).await;
    assert_eq!(r["item"]["id"], "plan-item");
    // run_now and test-sweep runs jump the priority order
    t.item("p", json!({"id": "urgent-p0", "priority": 0})).await;
    t.item("p", json!({"id": "run-now-p90", "priority": 90, "run_now": true})).await;
    t.item("p", json!({"id": "sweep-run", "priority": 99, "agent": "test", "exec_shell": "iter sweep", "system": "test-sweep"})).await;
    let mut got = Vec::new();
    for _ in 0..3 {
        got.push(t.next("eng1", "p", "e1", json!({})).await.1["item"]["id"].as_str().unwrap().to_string());
    }
    assert_eq!(got, ["sweep-run", "run-now-p90", "urgent-p0"]);
    let rn = t.store().get("workitem", "p", "run-now-p90").await.unwrap().unwrap();
    assert_eq!(rn["run_now"], false, "the override is consumed by the claim");
    t.done().await;
}

#[tokio::test]
async fn next_respects_locks_reserves_and_rolls_back() {
    let t = T::new().await;
    t.served("p", "e1", "eng1").await;
    t.item("p", json!({"id": "holder", "priority": 1, "lockdirs": ["{topdir}/src/"]})).await;
    let (_, r) = t.next("eng1", "p", "e1", json!({})).await;
    assert_eq!(r["item"]["id"], "holder");
    let lease = r["item"]["lease"].as_str().unwrap().to_string();
    let rows = t.store().query("lock", "p").await.unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!((rows[0]["path"].as_str(), rows[0]["lease"].as_str(), rows[0]["engine"].as_str()), (Some("{topdir}/src/"), Some(lease.as_str()), Some("e1")));
    // an overlapping item waits on the live lock and reserves its scope
    t.item("p", json!({"id": "waiter", "priority": 20, "lockdirs": ["{topdir}/src/a/"]})).await;
    t.item("p", json!({"id": "later-overlap", "priority": 30, "lockdirs": ["{topdir}/src/a/b/"]})).await;
    t.item("p", json!({"id": "elsewhere", "priority": 40, "lockdirs": ["{topdir}/docs/"]})).await;
    let (_, r) = t.next("eng1", "p", "e1", json!({})).await;
    assert_eq!(r["item"]["id"], "elsewhere", "locked and reservation-gated items are skipped");
    let reserve: Vec<Value> = t.store().query("lock", "p").await.unwrap().into_iter().filter(|r| r["kind"] == "reserve").collect();
    assert_eq!((reserve.len(), reserve[0]["workid"].as_str()), (1, Some("waiter")));
    let (_, r) = t.next("eng1", "p", "e1", json!({})).await;
    assert_eq!(r["reason"], "locked");

    // rollback: a lock row taken between the pick and the claim's acquire
    t.item("p", json!({"id": "racer", "priority": 1, "lockdirs": ["{topdir}/x/", "{topdir}/y/"]})).await;
    let other = LockRow { project: "p".into(), path: "{topdir}/y/".into(), kind: "lock".into(), engine: "e9".into(), workid: "someone".into(),
        acquired: now_utc(), expires: "2999-01-01T00:00:00Z".into(), lease: "L".into() };
    t.store().put("lock", "p", &iter_core::lock_sk("lock", "{topdir}/y/"), &serde_json::to_value(&other).unwrap()).await.unwrap();
    let orig = t.store().get("workitem", "p", "racer").await.unwrap().unwrap();
    let item: WorkItem = serde_json::from_value(orig.clone()).unwrap();
    let out = crate::next::claim(&t.st, "p", "e1", &item, &orig, 300).await.ok().unwrap();
    assert!(matches!(out, crate::next::Claim::Locked));
    let back = t.store().get("workitem", "p", "racer").await.unwrap().unwrap();
    assert_eq!((back["state"].as_str(), back["lease"].as_str(), back["attempt"].as_u64(), back["version"].as_u64()), (Some("queued"), None, None, Some(3)));
    let rows = t.store().query("lock", "p").await.unwrap();
    assert!(!rows.iter().any(|r| r["workid"] == "racer"), "the claim's rows are released: {rows:?}");
    t.done().await;
}

#[tokio::test]
async fn next_refuses_unserved_stopped_and_foreign_engines() {
    let t = T::new().await;
    t.served("p", "e1", "eng1").await;
    t.served("q", "e2", "eng2").await;
    t.item("p", json!({"id": "a"})).await;
    // e2 does not serve p: the admin may ask on its behalf and is told so
    let (c, r) = t.next("adm", "p", "e2", json!({})).await;
    assert_eq!((c, r["reason"].as_str()), (200, Some("not-served")));
    // an engine token may only ask for its own engine (and only on served projects)
    assert_eq!(t.next("eng2", "q", "e1", json!({})).await.0, 403);
    assert_eq!(t.next("eng2", "p", "e2", json!({})).await.0, 403);
    // read-only serves edge: nothing runs
    let s = t.edge("serves", "iter_engine:e1", "project:p").await.unwrap();
    t.ok("adm", "PATCH", &format!("/api/settings/edges/{}", s.id), Some(json!({"settings": {"read_only": true}}))).await;
    assert_eq!(t.next("eng1", "p", "e1", json!({})).await.1["reason"], "not-served");
    t.ok("adm", "PATCH", &format!("/api/settings/edges/{}", s.id), Some(json!({"settings": {"read_only": false}}))).await;
    // stopped project
    t.ok("adm", "PUT", "/api/projects/p", Some(json!({"state": "Stopped"}))).await;
    assert_eq!(t.next("eng1", "p", "e1", json!({})).await.1["reason"], "project-stopped");
    t.ok("adm", "PUT", "/api/projects/p", Some(json!({"state": "Running"}))).await;
    // two engines race for one item: exactly one wins
    t.ok("eng:e3", "PUT", "/api/engines/e3", Some(json!({"state": "Running"}))).await;
    t.ok("adm", "POST", "/api/settings/edges", Some(json!({"from": "iter_engine:e3", "to": "project:p"}))).await;
    let (r1, r2) = tokio::join!(t.next("eng1", "p", "e1", json!({})), t.next("eng:e3", "p", "e3", json!({})));
    let won = [&r1.1, &r2.1].iter().filter(|r| !r["item"].is_null()).count();
    assert_eq!(won, 1, "{r1:?} {r2:?}");
    // deactivating the serves edge stops the engine from getting work
    t.item("p", json!({"id": "b"})).await;
    t.ok("adm", "PATCH", &format!("/api/settings/edges/{}", s.id), Some(json!({"from": "iter_engine:_deactivated"}))).await;
    let (c, v) = t.next("eng1", "p", "e1", json!({})).await;
    assert_eq!(c, 403, "an engine that no longer serves the project is refused at the project gate: {v}");
    t.done().await;
}

/// The switches (2026-10-08): server and engine Stopped start nothing new —
/// the assignments read Stopped and say why, next refuses, no GraphRAG work
/// is offered; an account switch rides on the assignments. Who may flip which.
#[tokio::test]
async fn switches_stop_new_work_at_every_level() {
    let t = T::new().await;
    t.served("p", "e1", "eng1").await;
    t.ok("adm", "POST", "/api/settings/nodes", Some(json!({"type": "account", "name": "main", "settings": {"token_envar": "MAIN_TOKEN"}}))).await;
    t.ok("adm", "POST", "/api/settings/edges", Some(json!({"from": "account:main", "to": "project:p", "settings": {"order": 1, "switch": 80, "stop": 95}}))).await;
    t.ok("adm", "POST", "/api/settings/edges", Some(json!({"from": "iter_engine:e1", "to": "account:main"}))).await;
    t.item("p", json!({"id": "a"})).await;
    let asg = |v: &Value| (v["projects"][0]["state"].as_str().unwrap_or("").to_string(), v["projects"][0]["stopped_by"].as_str().unwrap_or("").to_string());
    assert_eq!(t.ok("alice", "GET", "/api/server", None).await["active"], true, "no row = Active");
    assert_eq!(asg(&t.ok("eng1", "GET", "/api/engines/e1/assignments", None).await), ("Running".into(), "".into()));

    // the server: admin only
    assert_eq!(t.call("alice", "POST", "/api/switch", Some(json!({"target": "iter_data:self", "active": false}))).await.0, 403);
    assert_eq!(t.call("eng1", "POST", "/api/switch", Some(json!({"target": "server", "active": false}))).await.0, 403);
    t.ok("adm", "POST", "/api/switch", Some(json!({"target": "iter_data:self", "active": false}))).await;
    let sv = t.ok("alice", "GET", "/api/server", None).await;
    assert_eq!((sv["active"].as_bool(), sv["active_by"].as_str()), (Some(false), Some("adm")));
    assert_eq!(asg(&t.ok("eng1", "GET", "/api/engines/e1/assignments", None).await), ("Stopped".into(), "server".into()));
    assert_eq!(t.next("eng1", "p", "e1", json!({})).await.1["reason"], "server-stopped");
    let hb = t.ok("eng1", "POST", "/api/engines/e1/heartbeat", Some(json!({"state": "Running"}))).await;
    assert_eq!((hb["stopped_by"].as_str(), hb["rag_waiting"].clone()), (Some("server"), json!({})));
    assert_eq!(t.ok("adm", "GET", "/api/projects/p/status", None).await["server_active"], false);
    // the project record itself is untouched: switching the server back resumes it
    assert_eq!(t.store().get("project", "p", NOSK).await.unwrap().unwrap()["state"], "Running");
    t.ok("adm", "POST", "/api/switch", Some(json!({"target": "server", "active": true}))).await;

    // an engine: its owner or an admin, not another engine's token
    assert_eq!(t.call("eng2", "POST", "/api/switch", Some(json!({"target": "iter_engine:e1", "active": false}))).await.0, 403);
    assert_eq!(t.call("alice", "POST", "/api/switch", Some(json!({"target": "iter_engine:e1", "active": false}))).await.0, 403);
    t.ok("eng1", "POST", "/api/switch", Some(json!({"target": "iter_engine:e1", "active": false}))).await;
    assert_eq!(asg(&t.ok("eng1", "GET", "/api/engines/e1/assignments", None).await), ("Stopped".into(), "engine".into()));
    assert_eq!(t.next("eng1", "p", "e1", json!({})).await.1["reason"], "engine-stopped");
    assert_eq!(t.ok("adm", "GET", "/api/projects/p/status", None).await["engines"][0]["active"], false);
    // a heartbeat keeps the switch (it is not the engine's to set)
    t.ok("eng1", "POST", "/api/engines/e1/heartbeat", Some(json!({"state": "Running"}))).await;
    assert_eq!(t.store().get("engine", "e1", NOSK).await.unwrap().unwrap()["active"], false);
    let g = t.ok("adm", "GET", "/api/settings/graph", None).await;
    let node = |id: &str| g["nodes"].as_array().unwrap().iter().find(|n| n["id"] == id).cloned().unwrap();
    assert!(node("iter_engine:e1")["summary"].as_str().unwrap().starts_with("switched off"));
    t.ok("adm", "POST", "/api/switch", Some(json!({"target": "iter_engine:e1", "active": true}))).await;
    assert_eq!(t.next("eng1", "p", "e1", json!({})).await.1["item"]["id"], "a");

    // an account: admin only; it rides on the assignments
    assert_eq!(t.call("eng1", "POST", "/api/switch", Some(json!({"target": "account:main", "active": false}))).await.0, 403);
    let acct = |v: &Value| v["projects"][0]["accounts"][0].get("stopped").and_then(|s| s.as_bool()).unwrap_or(false);
    assert!(!acct(&t.ok("eng1", "GET", "/api/engines/e1/assignments", None).await));
    t.ok("adm", "POST", "/api/switch", Some(json!({"target": "account:main", "active": false}))).await;
    assert!(acct(&t.ok("eng1", "GET", "/api/engines/e1/assignments", None).await));
    // projects keep their state; unknown targets and other node types are refused
    assert_eq!(t.call("adm", "POST", "/api/switch", Some(json!({"target": "project:p", "active": false}))).await.0, 400);
    assert_eq!(t.call("adm", "POST", "/api/switch", Some(json!({"target": "agent:code", "active": false}))).await.0, 400);
    assert_eq!(t.call("adm", "POST", "/api/switch", Some(json!({"target": "iter_engine:ghost", "active": false}))).await.0, 404);
    t.done().await;
}

/// The Accounts pane's usage refresh (2026-10-08): probe_requested is set by
/// any writer and cleared by the engine's heartbeat; the engine's usage report
/// is status, not a setting, so the settings graph does not show it.
#[tokio::test]
async fn usage_refresh_round_trip_and_the_report_is_not_a_setting() {
    let t = T::new().await;
    t.served("p", "e1", "eng1").await;
    let r = t.ok("alice", "POST", "/api/engines/e1/probe", Some(json!({}))).await;
    let rec = t.ok("adm", "GET", "/api/engines/e1", None).await;
    assert_eq!(rec["probe_requested"], r["requested"]);
    assert_eq!(t.call("vic", "POST", "/api/engines/e1/probe", Some(json!({}))).await.0, 403, "a viewer cannot ask");
    t.ok("eng1", "POST", "/api/engines/e1/heartbeat", Some(json!({"state": "Running", "clear_probe": true,
        "accounts": [{"name": "main", "five_hour_pct": 12.0}], "usage": {"account": "main"}, "next": null}))).await;
    let rec = t.ok("adm", "GET", "/api/engines/e1", None).await;
    assert_eq!((rec["probe_requested"].as_str(), rec["accounts"][0]["five_hour_pct"].as_f64()), (Some(""), Some(12.0)));
    let g = t.ok("adm", "GET", "/api/settings/graph", None).await;
    let s = &g["nodes"].as_array().unwrap().iter().find(|n| n["id"] == "iter_engine:e1").unwrap()["settings"];
    assert!(s.get("accounts").is_none() && s.get("usage").is_none() && s.get("next").is_none(), "{s}");
    assert_eq!(s["host"].as_str().is_some() || s.get("state").is_some(), true, "settings still shown: {s}");
    t.done().await;
}

// ---------- authz ----------

#[tokio::test]
async fn project_routes_are_authorized_by_edges() {
    let t = T::new().await;
    t.served("p", "e1", "eng1").await;
    t.ok("adm", "PUT", "/api/projects/q", Some(json!({"state": "Running"}))).await;
    // engine token: served project ok, unserved 403 — on every project route family
    t.ok("eng1", "GET", "/api/projects/p/workitems", None).await;
    for path in ["/api/projects/q/workitems", "/api/projects/q", "/api/projects/q/locks", "/api/projects/q/graph", "/api/projects/q/rag", "/api/projects/q/datasync"] {
        let (c, v) = t.call("eng1", "GET", path, None).await;
        assert_eq!(c, 403, "{path}: {v}");
    }
    assert_eq!(t.call("eng1", "POST", "/api/projects/q/workitems", Some(json!({"name": "x", "agent": "code"}))).await.0, 403);
    // users need an active member edge
    assert_eq!(t.call("alice", "GET", "/api/projects/p/workitems", None).await.0, 403);
    t.ok("adm", "POST", "/api/settings/edges", Some(json!({"from": "user:alice", "to": "project:p"}))).await;
    t.ok("alice", "GET", "/api/projects/p/workitems", None).await;
    t.ok("alice", "POST", "/api/projects/p/workitems", Some(json!({"name": "from alice", "agent": "code", "request": "do it"}))).await;
    assert_eq!(t.call("alice", "GET", "/api/projects/q/workitems", None).await.0, 403);
    // a viewer member reads only
    t.ok("adm", "POST", "/api/settings/edges", Some(json!({"from": "user:bob", "to": "project:p", "settings": {"role": "viewer"}}))).await;
    t.ok("bob", "GET", "/api/projects/p/workitems", None).await;
    assert_eq!(t.call("bob", "POST", "/api/projects/p/workitems", Some(json!({"name": "x", "agent": "code"}))).await.0, 403);
    // a deactivated member edge is no membership
    let m = t.edge("member", "user:alice", "project:p").await.unwrap();
    t.ok("adm", "PATCH", &format!("/api/settings/edges/{}", m.id), Some(json!({"from": "user:_deactivated"}))).await;
    assert_eq!(t.call("alice", "GET", "/api/projects/p/workitems", None).await.0, 403);
    // admin sees everything
    t.ok("adm", "GET", "/api/projects/q/workitems", None).await;
    // project lists are filtered the same way
    let names = |v: &Value| v.as_array().unwrap().iter().map(|p| p["name"].as_str().unwrap().to_string()).collect::<Vec<_>>();
    assert_eq!(names(&t.ok("eng1", "GET", "/api/projects", None).await), ["p"]);
    assert_eq!(names(&t.ok("bob", "GET", "/api/projects", None).await), ["p"]);
    assert!(names(&t.ok("alice", "GET", "/api/projects", None).await).is_empty());
    assert_eq!(names(&t.ok("adm", "GET", "/api/projects", None).await).len(), 2);
    // a user may create a new project and becomes its member; editing stays admin-only
    t.ok("alice", "PUT", "/api/projects/mine", Some(json!({"state": "Running"}))).await;
    assert!(t.edge("member", "user:alice", "project:mine").await.unwrap().is_active());
    t.ok("alice", "GET", "/api/projects/mine/workitems", None).await;
    assert_eq!(t.call("alice", "PUT", "/api/projects/mine", Some(json!({"state": "Stopped"}))).await.0, 403);
    assert_eq!(t.call("vic", "PUT", "/api/projects/vics", Some(json!({"state": "Running"}))).await.0, 403);
    // an admin-created project needs no member edge for the admin
    assert!(t.edge("member", "user:adm", "project:q").await.is_none());
    // no / bad token: still 401 from the extractor, not 403
    let req = Request::builder().uri("/api/projects/p/workitems").body(Body::empty()).unwrap();
    assert_eq!(t.app.clone().oneshot(req).await.unwrap().status().as_u16(), 401);
    t.done().await;
}

#[tokio::test]
async fn mcp_inherits_project_authz() {
    let t = T::new().await;
    t.served("p", "e1", "eng1").await;
    t.ok("adm", "PUT", "/api/projects/q", Some(json!({"state": "Running"}))).await;
    let call = |project: &str| json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": {"name": "workitem_list", "arguments": {"project": project}}});
    let ok = t.ok("eng1", "POST", "/mcp", Some(call("p"))).await;
    assert_eq!(ok["result"]["isError"], false, "{ok}");
    let denied = t.ok("eng1", "POST", "/mcp", Some(call("q"))).await;
    assert_eq!(denied["result"]["isError"], true, "{denied}");
    assert!(denied.to_string().contains("403"), "{denied}");
    t.done().await;
}

#[tokio::test]
async fn engines_write_only_their_own_records_and_heartbeat_lists_waiting_work() {
    let t = T::new().await;
    t.served("p", "e1", "eng1").await;
    // another engine token may not heartbeat or PUT e1
    assert_eq!(t.call("eng2", "POST", "/api/engines/e1/heartbeat", Some(json!({"state": "Running"}))).await.0, 403);
    assert_eq!(t.call("eng2", "PUT", "/api/engines/e1", Some(json!({"state": "Stopped"}))).await.0, 403);
    // its own: fine, and the reply carries the waiting lists
    let hb = t.ok("eng1", "POST", "/api/engines/e1/heartbeat", Some(json!({"state": "Running"}))).await;
    for k in ["files_waiting", "build_waiting"] {
        assert!(hb[k].is_array(), "{k}: {hb}");
    }
    for k in ["rag_waiting", "datasync_waiting"] {
        assert!(hb.get(k).is_some(), "{k} kept: {hb}");
    }
    assert_eq!(hb["user"].as_str().unwrap_or(""), "", "an engine is no user's");
    // a PUT cannot hand the record to a person
    let put = t.ok("eng1", "PUT", "/api/engines/e1", Some(json!({"state": "Running", "user": "alice"}))).await;
    assert_eq!(put["user"].as_str().unwrap_or(""), "");
    // a record no person owns is still only its own engine's to write
    t.store().put("engine", "other", NOSK, &json!({"name": "other", "state": "Running"})).await.unwrap();
    assert_eq!(t.call("eng2", "POST", "/api/engines/other/heartbeat", Some(json!({}))).await.0, 403);
    assert_eq!(t.call("eng:other", "POST", "/api/engines/other/heartbeat", Some(json!({}))).await.0, 200);
    t.done().await;
}

#[tokio::test]
async fn non_admin_settings_reads_are_scoped() {
    let t = T::new().await;
    t.served("p", "e1", "eng1").await;
    t.ok("adm", "PUT", "/api/projects/q", Some(json!({"state": "Running"}))).await;
    t.ok("adm", "POST", "/api/settings/edges", Some(json!({"from": "user:alice", "to": "project:p"}))).await;
    let g = t.ok("alice", "GET", "/api/settings/graph", None).await;
    let ids: HashSet<String> = g["nodes"].as_array().unwrap().iter().map(|n| n["id"].as_str().unwrap().to_string()).collect();
    assert!(ids.contains("project:p") && ids.contains("user:alice") && !ids.contains("project:q"), "{ids:?}");
    assert!(g["edges"].as_array().unwrap().iter().all(|e| e["from"] != "project:q" && e["to"] != "project:q"));
    let full = t.ok("adm", "GET", "/api/settings/graph", None).await;
    assert!(full["edges"].as_array().unwrap().len() > g["edges"].as_array().unwrap().len());
    t.done().await;
}

// ---------- stable ids and renames ----------

#[tokio::test]
async fn renames_change_the_name_only_never_the_id() {
    let t = T::new().await;
    t.served("p1", "e1", "eng1").await;
    t.item("p1", json!({"id": "w-1", "name": "fix it"})).await;
    let serves = t.edge("serves", "iter_engine:e1", "project:p1").await.expect("serves edge");

    // every record carries its id and a display name (the id until renamed)
    let p = t.ok("adm", "GET", "/api/projects/p1", None).await;
    assert_eq!((p["id"].as_str(), p["name"].as_str()), (Some("p1"), Some("p1")));
    let u = t.ok("adm", "GET", "/api/users/bob", None).await;
    assert_eq!((u["id"].as_str(), u["user"].as_str(), u["name"].as_str()), (Some("bob"), Some("bob"), Some("bob")));

    // rename the project, the engine and a user in the settings graph
    let n = t.ok("adm", "PATCH", "/api/settings/nodes/project:p1", Some(json!({"name": "Shop API"}))).await;
    assert_eq!((n["id"].as_str(), n["key"].as_str(), n["name"].as_str()), (Some("project:p1"), Some("p1"), Some("Shop API")));
    t.ok("adm", "PATCH", "/api/settings/nodes/iter_engine:e1", Some(json!({"name": "Laptop"}))).await;
    // a generic editor may send the name among the settings
    t.ok("adm", "PATCH", "/api/settings/nodes/user:bob", Some(json!({"settings": {"name": "Beast"}}))).await;

    // the id, the edges, the work and the tokens are untouched
    let p = t.ok("adm", "GET", "/api/projects/p1", None).await;
    assert_eq!((p["id"].as_str(), p["name"].as_str()), (Some("p1"), Some("Shop API")));
    let after = t.edge("serves", "iter_engine:e1", "project:p1").await.expect("serves edge kept");
    assert_eq!((after.id.as_str(), after.is_active()), (serves.id.as_str(), true));
    let a = t.ok("eng1", "GET", "/api/engines/e1/assignments", None).await;
    assert_eq!(a["projects"][0]["project"], "p1");
    let items = t.ok("eng1", "GET", "/api/projects/p1/workitems", None).await;
    assert_eq!(items.as_array().unwrap().len(), 1);
    assert_eq!(t.ok("adm", "GET", "/api/engines/e1", None).await["name"], "Laptop");
    let g = t.ok("adm", "GET", "/api/settings/graph", None).await;
    let node = |id: &str| g["nodes"].as_array().unwrap().iter().find(|n| n["id"] == id).cloned().unwrap();
    assert_eq!(node("user:bob")["name"], "Beast");
    assert_eq!(node("iter_engine:e1")["name"], "Laptop");
    assert!(node("user:bob")["settings"].get("name").is_none(), "the name is beside the settings, not among them");

    // a PUT without a name keeps the display name; one with a name renames
    t.ok("adm", "PUT", "/api/projects/p1", Some(json!({"state": "Running", "desc": "d"}))).await;
    assert_eq!(t.ok("adm", "GET", "/api/projects/p1", None).await["name"], "Shop API");
    t.ok("adm", "PUT", "/api/projects/p1", Some(json!({"state": "Running", "name": "Shop"}))).await;
    assert_eq!(t.ok("adm", "GET", "/api/projects/p1", None).await["name"], "Shop");

    // names are unique per type, and may not shadow another record's id
    t.ok("adm", "PUT", "/api/projects/p2", Some(json!({"state": "Running"}))).await;
    assert_eq!(t.call("adm", "PATCH", "/api/settings/nodes/project:p2", Some(json!({"name": "Shop"}))).await.0, 409);
    assert_eq!(t.call("adm", "PATCH", "/api/settings/nodes/project:p2", Some(json!({"name": "p1"}))).await.0, 409);
    assert_eq!(t.call("adm", "PATCH", "/api/settings/nodes/project:p2", Some(json!({"name": "a/b"}))).await.0, 400);
    // a different type may share it
    t.ok("adm", "PATCH", "/api/settings/nodes/iter_engine:e1", Some(json!({"name": "Shop"}))).await;

    // a new node gets an id minted once from its name
    let c = t.ok("adm", "POST", "/api/settings/nodes", Some(json!({"type": "account", "name": "Team Max"}))).await;
    assert_eq!((c["id"].as_str(), c["name"].as_str()), (Some("account:Team-Max"), Some("Team Max")));
    let c2 = t.ok("adm", "POST", "/api/settings/nodes", Some(json!({"type": "account", "name": "Team-Max 2", "id": "max2"}))).await;
    assert_eq!(c2["id"], "account:max2");
    assert_eq!(t.call("adm", "POST", "/api/settings/nodes", Some(json!({"type": "account", "name": "Team Max"}))).await.0, 409);
    // edges may name their ends by display name; they store the ids
    let e = t.ok("adm", "POST", "/api/settings/edges", Some(json!({"from": "account:Team Max", "to": "project:Shop"}))).await;
    assert_eq!((e["from"].as_str(), e["to"].as_str(), e["type"].as_str()), (Some("account:Team-Max"), Some("project:p1"), Some("bills")));

    // a display name in a URL path resolves to the id before routing
    let s = t.store();
    assert_eq!(crate::names::resolve_path(s, "/api/projects/Shop/workitems").await.as_deref(), Some("/api/projects/p1/workitems"));
    assert_eq!(crate::names::resolve_path(s, "/api/projects/p1/workitems").await, None);
    assert_eq!(crate::names::resolve_path(s, "/api/projects/nothing/workitems").await, None);
    assert_eq!(crate::names::resolve_path(s, "/api/settings/nodes/user:Beast").await.as_deref(), Some("/api/settings/nodes/user:bob"));
    assert_eq!(crate::names::resolve_path(s, "/api/users/Beast/token").await.as_deref(), Some("/api/users/bob/token"));

    // sign in by id or by display name: the token carries the id
    t.ok("adm", "PUT", "/api/users/bob", Some(json!({"role": "user", "password": "pw-bob-12345"}))).await;
    assert_eq!(t.ok("adm", "GET", "/api/users/bob", None).await["name"], "Beast", "a PUT without a name keeps it");
    for who in ["bob", "Beast"] {
        let r = t.ok("adm", "POST", "/auth/login", Some(json!({"user": who, "password": "pw-bob-12345"}))).await;
        assert_eq!((r["user"].as_str(), r["name"].as_str()), (Some("bob"), Some("Beast")));
    }
    assert_eq!(t.call("adm", "POST", "/auth/login", Some(json!({"user": "Beast", "password": "wrong"}))).await.0, 401);

    // work-item states keep their names
    assert_eq!(t.call("adm", "PATCH", "/api/settings/nodes/workitem_type:queued", Some(json!({"name": "waiting"}))).await.0, 400);
    t.done().await;
}

// ---------- people's passwords (2026-10-17) ----------

/// A person's raw bearer call (no harness token minting).
async fn as_bearer(t: &T, bearer: &str, method: &str, path: &str, body: Option<Value>) -> (u16, Value) {
    let mut b = Request::builder().method(method).uri(path);
    if !bearer.is_empty() {
        b = b.header("authorization", format!("Bearer {bearer}"));
    }
    let req = match body {
        Some(v) => b.header("content-type", "application/json").body(Body::from(serde_json::to_vec(&v).unwrap())).unwrap(),
        None => b.body(Body::empty()).unwrap(),
    };
    let resp = t.app.clone().oneshot(req).await.unwrap();
    let code = resp.status().as_u16();
    let bytes = axum::body::to_bytes(resp.into_body(), 1 << 20).await.unwrap();
    (code, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
}

#[tokio::test]
async fn a_set_password_must_be_changed_and_a_change_needs_the_current_one() {
    let t = T::new().await;
    // an admin sets carol's password: too short is refused; a good one must be changed
    assert_eq!(t.call("adm", "PUT", "/api/users/carol", Some(json!({"role": "user", "password": "short"}))).await.0, 400);
    t.ok("adm", "PUT", "/api/users/carol", Some(json!({"role": "user", "password": "set-by-admin-1"}))).await;
    let (c, login) = as_bearer(&t, "", "POST", "/auth/login", Some(json!({"user": "carol", "password": "set-by-admin-1"}))).await;
    assert_eq!((c, login["must_change_password"].as_bool()), (200, Some(true)), "{login}");
    let held = login["token"].as_str().unwrap().to_string();
    // until then: everything is refused
    let (c, v) = as_bearer(&t, &held, "GET", "/api/projects", None).await;
    assert_eq!(c, 403, "{v}");
    assert!(v["error"].as_str().unwrap_or("").contains("password change required"), "{v}");
    // the change: the current password must be right, the new one long enough and different
    let pw = |cur: &str, new: &str| json!({"current_password": cur, "new_password": new});
    assert_eq!(as_bearer(&t, &held, "POST", "/api/users/carol/password", Some(pw("wrong-wrong-1", "carols-own-pw-1"))).await.0, 401);
    assert_eq!(as_bearer(&t, &held, "POST", "/api/users/carol/password", Some(pw("set-by-admin-1", "short"))).await.0, 400);
    assert_eq!(as_bearer(&t, &held, "POST", "/api/users/carol/password", Some(pw("set-by-admin-1", "set-by-admin-1"))).await.0, 400);
    assert_eq!(as_bearer(&t, &held, "POST", "/api/users/bob/password", Some(pw("set-by-admin-1", "carols-own-pw-1"))).await.0, 403, "only one's own");
    let (c, fresh) = as_bearer(&t, &held, "POST", "/api/users/carol/password", Some(pw("set-by-admin-1", "carols-own-pw-1"))).await;
    assert_eq!((c, fresh["must_change_password"].as_bool()), (200, Some(false)), "{fresh}");
    let fresh = fresh["token"].as_str().unwrap().to_string();
    // the old session ended, the new one works, and the new password signs in with nothing asked
    assert_eq!(as_bearer(&t, &held, "GET", "/api/projects", None).await.0, 401);
    assert_eq!(as_bearer(&t, &fresh, "GET", "/api/projects", None).await.0, 200);
    let (_, again) = as_bearer(&t, "", "POST", "/auth/login", Some(json!({"user": "carol", "password": "carols-own-pw-1"}))).await;
    assert_eq!(again["must_change_password"].as_bool(), Some(false));
    // a person never sets a password, a hash or the flag through the profile PUT
    let (c, v) = as_bearer(&t, &fresh, "PUT", "/api/users/carol", Some(json!({"role": "user", "password": "sneaky-new-pw-1"}))).await;
    assert_eq!(c, 400, "{v}");
    as_bearer(&t, &fresh, "PUT", "/api/users/carol", Some(json!({"role": "user", "pwhash": "$argon2id$forged", "must_change_password": true, "timezone": "UTC"}))).await;
    let row = t.store().get("webui_user", "carol", NOSK).await.unwrap().unwrap();
    assert!(row["pwhash"].as_str().unwrap().starts_with("$argon2") && row["pwhash"] != "$argon2id$forged", "{row}");
    assert_eq!((row["must_change_password"].as_bool(), row["timezone"].as_str()), (Some(false), Some("UTC")));
    // an admin's reset forces a change again and ends carol's sessions
    t.ok("adm", "PUT", "/api/users/carol", Some(json!({"role": "user", "password": "reset-by-admin-2"}))).await;
    assert_eq!(as_bearer(&t, &fresh, "GET", "/api/projects", None).await.0, 401);
    t.done().await;
}

#[tokio::test]
async fn sign_ins_are_limited_per_user_and_the_first_admin_must_change_theirs() {
    let t = T::new().await;
    t.ok("adm", "PUT", "/api/users/dave", Some(json!({"role": "user", "password": "dave-initial-1"}))).await;
    for _ in 0..crate::api::LOGIN_MAX_FAILURES {
        assert_eq!(as_bearer(&t, "", "POST", "/auth/login", Some(json!({"user": "dave", "password": "nope-nope-nope"}))).await.0, 401);
    }
    // the next attempt is refused before the password is even checked, right or wrong
    let (c, v) = as_bearer(&t, "", "POST", "/auth/login", Some(json!({"user": "dave", "password": "dave-initial-1"}))).await;
    assert_eq!(c, 429, "{v}");
    // another user is not held up by dave's failures (the address limit is per real peer; tests have none)
    t.ok("adm", "PUT", "/api/users/erin", Some(json!({"role": "user", "password": "erin-initial-1"}))).await;
    // the first admin, made on an empty database, must change the first-boot password
    t.store().delete("webui_user", "admin", NOSK).await.unwrap();
    for u in t.store().scan("webui_user").await.unwrap() {
        t.store().delete("webui_user", &crate::storage::body_str(&u, "user"), NOSK).await.unwrap();
    }
    unsafe { std::env::set_var("ITER_ADMIN_PASSWORD", "first-boot-pw-1") };
    crate::bootstrap_admin(t.store()).await;
    unsafe { std::env::remove_var("ITER_ADMIN_PASSWORD") };
    let row = t.store().get("webui_user", "admin", NOSK).await.unwrap().unwrap();
    assert_eq!(row["must_change_password"].as_bool(), Some(true), "{row}");
    t.done().await;
}

// ---------- one user form (2026-10-19) ----------

#[tokio::test]
async fn user_nodes_follow_the_user_form_rules() {
    let t = T::new().await;
    t.ok("adm", "PUT", "/api/users/alice", Some(json!({"role": "user", "email": "alice@example.com", "authz": {}, "settings": {}}))).await;
    // the settings graph never sets a password, the must-change flag or the token version
    for k in ["password", "pwhash", "must_change_password", "tokenver"] {
        let (c, v) = t.call("adm", "PATCH", "/api/settings/nodes/user:alice", Some(json!({"settings": {k: "x"}}))).await;
        assert_eq!(c, 400, "{k}: {v}");
        assert!(v.to_string().contains("/api/users/alice"), "{k}: {v}");
    }
    let (c, _) = t.call("adm", "POST", "/api/settings/nodes", Some(json!({"type": "user", "name": "carol", "settings": {"password": "a-long-password-1"}}))).await;
    assert_eq!(c, 400);
    // and no role the user form refuses
    let (c, _) = t.call("adm", "PATCH", "/api/settings/nodes/user:alice", Some(json!({"settings": {"role": "engine"}}))).await;
    assert_eq!(c, 400);
    t.ok("adm", "PATCH", "/api/settings/nodes/user:alice", Some(json!({"settings": {"role": "viewer"}}))).await;
    // another person sees who it is and the role, not the rest
    let other = t.ok("bob", "GET", "/api/users/alice", None).await;
    assert_eq!(other["role"], "viewer");
    assert!(other.get("email").is_none() && other.get("authz").is_none(), "{other}");
    let node = t.ok("bob", "GET", "/api/settings/nodes/user:alice", None).await;
    assert!(node["settings"].get("email").is_none(), "{node}");
    // the person themselves and the admins see everything but the hash
    let mine = t.ok("alice", "GET", "/api/users/alice", None).await;
    assert_eq!(mine["email"], "alice@example.com");
    assert!(mine.get("pwhash").is_none());
    assert_eq!(t.ok("adm", "GET", "/api/settings/nodes/user:alice", None).await["settings"]["email"], "alice@example.com");
    t.done().await;
}
