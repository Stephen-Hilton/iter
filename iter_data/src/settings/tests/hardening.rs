//! MCP and GraphRAG hardening (docs/build_plans/mcp_graphrag_hardening.md,
//! MG-1 to MG-21), through the real router and MCP as deployed.

use super::*;

/// One MCP `tools/call` as `who`; returns (isError, structuredContent or the error text).
pub(super) async fn tool(t: &T, who: &str, name: &str, args: Value) -> (bool, Value) {
    tool_h(t, who, name, args, &[]).await
}

async fn tool_h(t: &T, who: &str, name: &str, args: Value, headers: &[(&str, &str)]) -> (bool, Value) {
    let msg = json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": {"name": name, "arguments": args}});
    let (c, v) = t.call_h(who, "POST", "/mcp", Some(msg), headers).await;
    assert_eq!(c, 200, "{name}: {v}");
    let r = &v["result"];
    let is_error = r["isError"].as_bool().unwrap_or(true);
    let body = if r.get("structuredContent").is_some() { r["structuredContent"].clone() } else { r["content"][0]["text"].clone() };
    (is_error, body)
}

/// The engine record's last_seen, `ago` seconds back.
async fn seen(t: &T, engine: &str, ago: i64) {
    let mut e = t.store().get("engine", engine, NOSK).await.unwrap().unwrap();
    e["last_seen"] = json!((chrono::Utc::now() - chrono::Duration::seconds(ago)).format("%Y-%m-%dT%H:%M:%SZ").to_string());
    e["ticksec"] = json!(5);
    t.store().put("engine", engine, NOSK, &e).await.unwrap();
}

// ---------- MG-15 ----------

#[tokio::test]
async fn rag_status_lists_serving_engines() {
    let t = T::new().await;
    t.served("p", "e1", "eng1").await;
    t.served("p", "e2", "eng2").await;
    t.served("q", "e3", "eng:e3").await;
    seen(&t, "e1", 0).await;
    seen(&t, "e2", 600).await;
    seen(&t, "e3", 0).await;
    let st = t.ok("adm", "GET", "/api/projects/p/rag", None).await;
    let engines = st["engines"].as_array().unwrap();
    let by = |n: &str| engines.iter().find(|e| e["name"] == n).cloned();
    assert_eq!(by("e1").map(|e| e["live"].clone()), Some(json!(true)), "{st}");
    assert_eq!(by("e2").map(|e| e["live"].clone()), Some(json!(false)), "{st}");
    assert!(by("e3").is_none(), "e3 does not serve p: {st}");
    for k in ["name", "display", "account", "hold", "last_seen", "age_sec", "live"] {
        assert!(by("e1").unwrap().get(k).is_some(), "{k}");
    }
    t.done().await;
}

/// alice (a person) and engine e1 (eng1) both work on project `p`.
async fn world(project: &str) -> T {
    let t = T::new().await;
    t.served(project, "e1", "eng1").await;
    t.ok("adm", "POST", "/api/settings/edges", Some(json!({"from": "user:alice", "to": format!("project:{project}")}))).await;
    t
}

fn ids_of(v: &Value) -> Vec<String> {
    v["items"].as_array().unwrap().iter().map(|i| i["id"].as_str().unwrap().to_string()).collect()
}

// ---------- MG-11 ----------

#[tokio::test]
async fn hostile_ids_never_change_the_route() {
    let t = world("hostile").await;
    let real = "0000aaaa-1111-2222-3333-444455556666";
    t.item("hostile", json!({"id": real, "name": "the real one", "state": "in-progress", "engine": "e1"})).await;
    let locks_before = t.store().query("lock", "hostile").await.unwrap().len();
    let mut hostile: Vec<String> = ["/explain?", "?x=1", "#frag"].iter().map(|s| format!("{real}{s}")).collect();
    hostile.extend(["../locks/release_all", "..", "%2F", "", " ", "a\nb"].iter().map(|s| s.to_string()));
    hostile.push("a".repeat(201));
    let base = |rest: &str| format!("/api/projects/{{name}}{rest}");
    let wi = vec![base("/workitems"), base("/workitems/{id}"), base("/workitems/{id}/details"), base("/whoami"), base("")];
    let tools: Vec<(&str, &str, Value, Vec<String>)> = vec![
        ("workitem_ask", "workid", json!({"question": "q?"}), wi.clone()),
        ("workitem_reject", "workid", json!({"reason": "r"}), [wi.clone(), vec![base("/workitems/{id}/reject")]].concat()),
        ("workitem_wait", "workid", json!({"on": [real]}), wi.clone()),
        ("workitem_block", "workid", json!({}), wi.clone()),
        ("workitem_doc", "workid", json!({"text": "t"}), wi.clone()),
        ("workitem_doc", "id", json!({"text": "t"}), wi.clone()),
        ("workitem_get", "id", json!({}), wi.clone()),
        ("graph_node", "id", json!({}), vec![base("/graph/nodes/{id}")]),
        ("graph_neighbors", "id", json!({}), vec![base("/graph/nodes/{id}/neighbors")]),
        ("graph_usecase", "ucid", json!({}), vec![base("/graph/usecases/{ucid}")]),
        ("req_update", "id", json!({"title": "x"}), vec![base("/graph/reqs/{id}")]),
        ("req_move", "id", json!({"to_file": "f"}), vec![base("/graph/reqs/{id}/move")]),
        ("req_delete", "id", json!({"reason": "r"}), vec![base("/graph/reqs/{id}")]),
        ("rag_doc", "id", json!({}), vec![base("/rag/docs/{id}")]),
        ("rag_link_document", "doc", json!({"node": "n"}), vec![base("/rag/docs/{id}/links")]),
        // the people-only tools
        ("workitem_answer", "id", json!({"answer": "a"}), wi.clone()),
        ("workitem_set_state", "id", json!({"state": "parked"}), wi.clone()),
        ("workitem_stop", "id", json!({}), wi.clone()),
        ("workitem_priority", "id", json!({"priority": 1}), wi.clone()),
        ("workitem_reopen", "id", json!({}), wi.clone()),
        ("workitem_edit", "id", json!({"name": "x"}), wi.clone()),
        ("workitem_clone", "id", json!({}), wi.clone()),
        ("workitem_duplicate_of", "id", json!({"survivor": real}), wi.clone()),
        ("workitem_duplicate_of", "survivor", json!({"id": real}), wi.clone()),
        ("workitem_explain", "id", json!({}), wi.clone()),
        ("workitem_delete", "id", json!({}), wi.clone()),
        ("rag_delete_document", "id", json!({}), vec![base("/rag/docs/{id}")]),
        ("rag_resummarize_document", "id", json!({}), vec![base("/rag/docs/{id}/resummarize")]),
    ];
    for (name, arg, extra, allowed) in &tools {
        for h in &hostile {
            crate::api::matched_routes().lock().unwrap().retain(|r| r.0 != "hostile");
            let mut args = extra.clone();
            args["project"] = json!("hostile");
            args[*arg] = json!(h);
            let (is_error, out) = tool(&t, "alice", name, args).await;
            assert!(is_error, "{name} {arg}={h:?} was accepted: {out}");
            let hit: Vec<(String, String, String)> = crate::api::matched_routes().lock().unwrap().iter().filter(|r| r.0 == "hostile").cloned().collect();
            for (_, method, template) in hit {
                assert!(allowed.contains(&template), "{name} {arg}={h:?} reached {method} {template}");
            }
        }
    }
    let item = t.store().get("workitem", "hostile", real).await.unwrap().unwrap();
    assert!(item.get("explain_requested").is_none(), "{item}");
    assert_eq!(t.store().query("lock", "hostile").await.unwrap().len(), locks_before);
    let rows = t.ok("adm", "GET", &format!("/api/projects/hostile/workitems/{real}/details"), None).await;
    assert_eq!(rows, json!([]), "no hostile id reached the real item");
    // a unique suffix of the real id still works, and reaches only it
    let (is_error, out) = tool(&t, "alice", "workitem_doc", json!({"project": "hostile", "id": &real[real.len() - 12..], "text": "by suffix"})).await;
    assert!(!is_error && out["id"] == real, "{out}");
    // an empty graph id is refused before any call
    let (is_error, out) = tool(&t, "alice", "graph_node", json!({"project": "hostile", "id": ""})).await;
    assert!(is_error && out.to_string().contains("id is required"), "{out}");
    t.done().await;
}

// ---------- MG-8, MG-10 ----------

#[tokio::test]
async fn list_pages_through_everything() {
    let t = world("big").await;
    let rows: Vec<Value> = (0..1203)
        .map(|i| json!({"id": format!("w{i:05}"), "project": "big", "name": format!("item {i}"), "state": "queued", "agent": "code",
                        "priority": 40 + (i % 7), "version": 1, "ts": {"receive": format!("2026-10-{:02}T00:00:00Z", 1 + i % 20)}}))
        .collect();
    for chunk in rows.chunks(64) {
        let mut set = tokio::task::JoinSet::new();
        for r in chunk {
            let (store, r) = (t.st.store.clone(), r.clone());
            set.spawn(async move { store.put("workitem", "big", r["id"].as_str().unwrap(), &r).await.unwrap() });
        }
        while set.join_next().await.is_some() {}
    }
    assert_eq!(t.ok("adm", "GET", "/api/projects/big/workitems", None).await.as_array().unwrap().len(), 1203, "seeded rows read back");
    let mut seen: Vec<String> = Vec::new();
    let mut cursor = Value::Null;
    let mut pages = Vec::new();
    loop {
        let (e, out) = tool(&t, "alice", "workitem_list", json!({"project": "big", "limit": 500, "cursor": cursor})).await;
        assert!(!e, "{out}");
        // counted at call time: the item added after page 1 is in it from then on
        assert_eq!(out["total"], if pages.is_empty() { 1203 } else { 1204 });
        pages.push(out["items"].as_array().unwrap().len());
        seen.extend(ids_of(&out));
        if pages.len() == 1 {
            // created between pages: newest, so it sorts before the cursor and shifts nothing
            t.item("big", json!({"id": "zz-new", "ts": {"receive": "2026-12-31T00:00:00Z"}})).await;
        }
        if out["next_cursor"].is_null() {
            break;
        }
        cursor = out["next_cursor"].clone();
    }
    assert_eq!(pages, vec![500, 500, 203]);
    let mut distinct = seen.clone();
    distinct.sort();
    distinct.dedup();
    assert_eq!(distinct.len(), 1203);
    assert!(!seen.contains(&"zz-new".to_string()));
    // a cursor sent with another filter
    let (_, first) = tool(&t, "alice", "workitem_list", json!({"project": "big", "limit": 500})).await;
    let (e, out) = tool(&t, "alice", "workitem_list", json!({"project": "big", "limit": 500, "state": "queued", "cursor": first["next_cursor"]})).await;
    assert!(e && out.to_string().contains("400: cursor belongs to another query"), "{out}");
    t.done().await;
}

#[tokio::test]
async fn sort_default_and_any_field() {
    let t = world("s").await;
    for (id, recv, prio) in [("a", Some("2026-10-02"), 41), ("b", Some("2026-10-03"), 40), ("c", None, 42), ("d", Some("2026-10-03"), 43)] {
        let mut b = json!({"id": id, "priority": prio, "requestedby": format!("user:{id}"), "ts": {"start": format!("s-{id}")}});
        if let Some(r) = recv {
            b["ts"]["receive"] = json!(r);
        }
        t.item("s", b).await;
        if recv.is_none() {
            let mut row = t.store().get("workitem", "s", id).await.unwrap().unwrap();
            row["ts"] = json!({"start": "s-c"});
            t.store().put("workitem", "s", id, &row).await.unwrap();
        }
    }
    let (_, out) = tool(&t, "alice", "workitem_list", json!({"project": "s"})).await;
    assert_eq!(ids_of(&out), ["b", "d", "a", "c"], "ts.receive desc, id asc, missing last: {out}");
    assert_eq!(out["sort"], json!([{"field": "ts.receive", "dir": "desc"}, {"field": "id", "dir": "asc"}]));
    let mut got = Vec::new();
    let mut cursor = Value::Null;
    loop {
        let (_, out) = tool(&t, "alice", "workitem_list", json!({"project": "s", "limit": 1, "cursor": cursor, "sort": [{"field": "priority", "dir": "asc"}]})).await;
        got.extend(ids_of(&out));
        if out["next_cursor"].is_null() {
            break;
        }
        cursor = out["next_cursor"].clone();
    }
    assert_eq!(got, ["b", "a", "c", "d"]);
    let (_, out) = tool(&t, "alice", "workitem_list", json!({"project": "s", "fields": ["ts.start", "requestedby"], "filter": [{"field": "id", "op": "eq", "value": "a"}]})).await;
    let item = &out["items"][0];
    let mut keys: Vec<&str> = item.as_object().unwrap().keys().map(String::as_str).collect();
    keys.sort();
    assert_eq!(keys, ["agent", "blockedby", "id", "name", "priority", "received", "requestedby", "state", "tags", "ts.start"]);
    assert_eq!((item["ts.start"].as_str(), item["received"].as_str()), (Some("s-a"), Some("2026-10-02")));
    let (_, out) = tool(&t, "alice", "workitem_list", json!({"project": "s", "filter": [{"field": "prioritee", "op": "gt", "value": 1}]})).await;
    assert_eq!((out["total"].as_u64(), out["unknown_fields"].clone()), (Some(0), json!(["prioritee"])));
    t.done().await;
}

// ---------- MG-12, MG-13 ----------

async fn stored(t: &T, id: &str) -> (String, String) {
    let r = t.store().get("workitem", "p", id).await.unwrap().unwrap();
    (r["requestedby"].as_str().unwrap_or("").to_string(), r["createdby"].as_str().unwrap_or("").to_string())
}

/// A create by `who` with the X-Iter-Workid header `wid`.
async fn agent(t: &T, who: &str, wid: &str) -> String {
    let (c, v) = t.call_h(who, "POST", "/api/projects/p/workitems", Some(json!({"name": "child", "agent": "code", "request": "do"})), &[("x-iter-workid", wid)]).await;
    assert_eq!(c, 200, "{v}");
    v["id"].as_str().unwrap().to_string()
}

#[tokio::test]
async fn server_sets_provenance() {
    let t = world("p").await;
    t.served("p", "e2", "eng2").await;
    t.ok("adm", "PUT", "/api/projects/q", Some(json!({"state": "Running"}))).await;
    t.item("p", json!({"id": "run-e1", "state": "in-progress", "engine": "e1", "agent": "code"})).await;
    t.item("p", json!({"id": "queued-1", "state": "queued"})).await;
    t.item("p", json!({"id": "open-1"})).await;
    t.item("p", json!({"id": "closed-1", "state": "complete"})).await;
    t.item("q", json!({"id": "foreign-1"})).await;
    // REST, a person claiming to be an agent
    let r = t.ok("alice", "POST", "/api/projects/p/workitems", Some(json!({"name": "x", "agent": "code", "request": "do",
        "requestedby": "agent:code", "createdby": "iter_engine:bakugo"}))).await;
    assert_eq!(stored(&t, r["id"].as_str().unwrap()).await, ("user:alice".into(), "user:alice".into()));
    assert!(r["warnings"].to_string().contains("set by the server"), "{r}");
    // MCP, a person passing a fake calling item
    let (e, out) = tool(&t, "alice", "workitem_create", json!({"project": "p", "title": "y", "request": "do", "workid": "engine-fake-id"})).await;
    assert!(!e, "{out}");
    assert_eq!(stored(&t, out["id"].as_str().unwrap()).await, ("user:alice".into(), "user:alice".into()));
    assert!(out["warnings"].to_string().contains("engine-fake-id"), "{out}");
    // an agent: an engine session whose header names its own in-progress item
    assert_eq!(stored(&t, &agent(&t, "eng1", "run-e1").await).await, ("agent:code@e1".into(), "run-e1".into()));
    assert_eq!(stored(&t, &agent(&t, "eng2", "run-e1").await).await, ("engine:e2".into(), "engine:e2".into()), "another engine's item");
    assert_eq!(stored(&t, &agent(&t, "eng1", "queued-1").await).await, ("engine:e1".into(), "engine:e1".into()), "not in progress");
    // a person may name an open item of this project as the parent, nothing else
    for (parent, kept) in [("open-1", true), ("closed-1", false), ("foreign-1", false)] {
        let r = t.ok("alice", "POST", "/api/projects/p/workitems", Some(json!({"name": "c", "agent": "code", "request": "do", "createdby": parent}))).await;
        let want = if kept { parent.to_string() } else { "user:alice".to_string() };
        assert_eq!(stored(&t, r["id"].as_str().unwrap()).await.1, want, "{parent}");
    }
    // a PUT keeps who filed it
    let mut row = t.store().get("workitem", "p", "open-1").await.unwrap().unwrap();
    let before = (row["requestedby"].clone(), row["createdby"].clone());
    row["requestedby"] = json!("agent:forged");
    row["createdby"] = json!("someone");
    t.ok("alice", "PUT", "/api/projects/p/workitems/open-1?expect_version=1", Some(row)).await;
    let after = t.store().get("workitem", "p", "open-1").await.unwrap().unwrap();
    assert_eq!((after["requestedby"].clone(), after["createdby"].clone()), before);
    // the dedup "seen again by" row names the principal
    let rep = json!({"name": "flaky", "agent": "code", "request": "seen it", "requestedby": "agent:liar",
                     "tags": [{"text": "check:unit", "color": ""}, {"text": "container:core", "color": ""}]});
    let first = t.ok("alice", "POST", "/api/projects/p/workitems", Some(rep.clone())).await;
    t.ok("alice", "POST", "/api/projects/p/workitems", Some(rep)).await;
    let rows = t.ok("adm", "GET", &format!("/api/projects/p/workitems/{}/details", first["id"].as_str().unwrap()), None).await;
    let seen = rows.as_array().unwrap().iter().find(|r| r["key"] == "doc").unwrap().to_string();
    assert!(seen.contains("by user:alice") && !seen.contains("agent:liar"), "{seen}");
    t.done().await;
}

#[tokio::test]
async fn principal_band() {
    let t = world("p").await;
    let eng = t.ok("eng1", "POST", "/api/projects/p/workitems", Some(json!({"name": "e", "agent": "code", "request": "do", "requestedby": "user"}))).await;
    let usr = t.ok("alice", "POST", "/api/projects/p/workitems", Some(json!({"name": "u", "agent": "code", "request": "do"}))).await;
    let band = |v: &Value| v["priority"].as_i64().unwrap();
    assert!((50..=99).contains(&band(&eng)), "an engine: maintenance band: {eng}");
    assert!((40..=49).contains(&band(&usr)), "a person: human band: {usr}");
    t.done().await;
}

#[tokio::test]
async fn reject_names_the_real_actor() {
    let t = world("p").await;
    t.item("p", json!({"id": "by-alice", "state": "queued"})).await;
    t.item("p", json!({"id": "by-agent", "state": "in-progress", "engine": "e1", "agent": "code"})).await;
    let (e, out) = tool(&t, "alice", "workitem_reject", json!({"project": "p", "workid": "by-alice", "reason": "not real"})).await;
    assert!(!e, "{out}");
    let (e, out) = tool_h(&t, "eng1", "workitem_reject", json!({"project": "p", "reason": "duplicate"}), &[("x-iter-workid", "by-agent")]).await;
    assert!(!e, "{out}");
    for (id, who, by, by_workid, reason) in [("by-alice", "user:alice", "alice", None, "not real"), ("by-agent", "agent:code@e1", "iter_engine:e1", Some("by-agent"), "duplicate")] {
        let item = t.store().get("workitem", "p", id).await.unwrap().unwrap();
        assert_eq!((item["state"].as_str(), item["lasterror"].as_str()), (Some("parked"), Some(format!("rejected: {reason}").as_str())));
        let rows = t.ok("adm", "GET", &format!("/api/projects/p/workitems/{id}/details"), None).await;
        let doc = rows.as_array().unwrap().iter().find(|r| r["key"] == "doc").cloned().unwrap();
        assert_eq!(doc["value"], json!(format!("rejected by {who}: {reason}")));
        assert_eq!(doc["by"], by);
        assert_eq!(doc.get("by_workid").and_then(|w| w.as_str()), by_workid);
    }
    // a closed item is refused
    t.item("p", json!({"id": "done", "state": "complete"})).await;
    assert_eq!(t.call("alice", "POST", "/api/projects/p/workitems/done/reject", Some(json!({"reason": "x"}))).await.0, 403);
    t.done().await;
}

// ---------- MG-5, MG-6 ----------

fn chunks_for(text: &str) -> Value {
    json!({"chapters": [{"idx": 0, "title": "t"}], "chunks": [{"text": text, "chapter": 0, "vec_raw": vec![0.01f64; crate::rag::embed::DIM]}], "model": "m"})
}

/// Play the engine: claim the upload and store its chunks.
async fn ingest(app: Router, tok: String, project: &str, id: &str) {
    let call = |method: &str, path: String, body: Value| {
        let req = Request::builder().method(method).uri(path).header("authorization", format!("Bearer {tok}")).header("content-type", "application/json")
            .body(Body::from(serde_json::to_vec(&body).unwrap())).unwrap();
        app.clone().oneshot(req)
    };
    let resp = call("POST", format!("/api/projects/{project}/rag/work/claim"), json!({"engine": "e1", "kinds": ["ingest"]})).await.unwrap();
    let job: Value = serde_json::from_slice(&axum::body::to_bytes(resp.into_body(), 1 << 26).await.unwrap()).unwrap();
    assert_eq!(job["doc"]["id"], id, "{job}");
    let mut body = chunks_for("the phrase");
    body["engine"] = json!("e1");
    body["job_id"] = job["job_id"].clone();
    let resp = call("PUT", format!("/api/projects/{project}/rag/docs/{id}/chunks"), body).await.unwrap();
    assert!(resp.status().is_success());
}

#[tokio::test]
async fn upload_reports_searchable_and_engines() {
    let t = world("p").await;
    let (e, out) = tool(&t, "alice", "rag_add_document", json!({"project": "p", "filename": "notes.md", "text": "the phrase", "store": false})).await;
    assert!(!e, "{out}");
    assert_eq!((out["searchable"].as_bool(), out["live_engines"].as_u64(), out["state"].as_str()), (Some(false), Some(0), Some("queued")));
    assert!(out["message"].as_str().unwrap().contains("stays queued until one checks in"), "{out}");
    let id = out["id"].as_str().unwrap().to_string();
    let (_, d) = tool(&t, "alice", "rag_doc", json!({"project": "p", "id": id})).await;
    assert_eq!(d["searchable"], false);
    let started = std::time::Instant::now();
    let (_, out) = tool(&t, "alice", "rag_add_document", json!({"project": "p", "filename": "other.md", "text": "x", "store": false, "wait_sec": 2})).await;
    assert!(started.elapsed().as_secs_f64() < 3.0 && out["searchable"] == false && out["waited_sec"].as_u64() >= Some(1), "{out}");
    ingest(t.app.clone(), t.tok("adm").await, "p", &id).await;
    let (_, d) = tool(&t, "alice", "rag_doc", json!({"project": "p", "id": id})).await;
    assert_eq!(d["searchable"], true, "{d}");
    let (_, docs) = tool(&t, "alice", "rag_docs", json!({"project": "p"})).await;
    let row = docs["documents"].as_array().unwrap().iter().find(|x| x["id"] == id.as_str()).cloned().unwrap();
    assert_eq!(row["searchable"], true);
    // a live engine is counted
    seen(&t, "e1", 0).await;
    let (_, out) = tool(&t, "alice", "rag_add_document", json!({"project": "p", "filename": "third.md", "text": "x", "store": false})).await;
    assert_eq!(out["live_engines"], 1, "{out}");
    t.done().await;
}

#[tokio::test]
async fn wait_returns_when_searchable() {
    let t = world("p").await;
    seen(&t, "e1", 0).await;
    let (app, tok) = (t.app.clone(), t.tok("adm").await);
    let st = t.st.clone();
    let engine = tokio::spawn(async move {
        // the engine: wait for the upload to appear, then ingest it after ~1 s
        let a = st.store.arango().unwrap();
        for _ in 0..50 {
            let q = a.aql("FOR d IN rag_doc FILTER d.project == 'p' AND d.state == 'queued' RETURN d._key", json!({})).await.unwrap();
            if let Some(id) = q.first().and_then(|v| v.as_str()) {
                tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                ingest(app, tok, "p", id).await;
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
        panic!("no upload to ingest");
    });
    let started = std::time::Instant::now();
    let (e, out) = tool(&t, "alice", "rag_add_document", json!({"project": "p", "filename": "w.md", "text": "the phrase", "store": false, "wait_sec": 10})).await;
    assert!(!e, "{out}");
    assert!(started.elapsed().as_secs_f64() < 3.0, "{:?}", started.elapsed());
    assert_eq!((out["searchable"].as_bool(), out["message"].as_str()), (Some(true), Some("searchable now")), "{out}");
    engine.await.unwrap();
    t.done().await;
}

#[tokio::test]
async fn node_files_are_not_uploads() {
    let t = world("p").await;
    let (e, out) = tool(&t, "alice", "rag_add_document", json!({"project": "p", "filename": "x.code.iter.md", "text": "# x"})).await;
    assert!(e && out.to_string().contains("graph_node_update"), "{out}");
    use base64::Engine as _;
    let (c, v) = t.call("alice", "POST", "/api/projects/p/rag/docs", Some(json!({"filename": "x.code.iter.md", "content_b64": base64::engine::general_purpose::STANDARD.encode("# x")}))).await;
    assert!(c == 400 && v.to_string().contains("graph_node_update"), "the API refuses it too: {v}");
    let (e, out) = tool(&t, "alice", "rag_add_document", json!({"project": "p", "filename": "notes.md", "text": "# notes", "store": false})).await;
    assert!(!e, "{out}");
    t.done().await;
}

// ---------- MG-16, MG-17 ----------

/// A store whose work-item reads fail with a database error naming internals.
struct Failing(Arc<dyn Storage>);

#[async_trait::async_trait]
impl Storage for Failing {
    async fn get(&self, t: &str, pk: &str, sk: &str) -> Result<Option<Value>, StorageError> { self.0.get(t, pk, sk).await }
    async fn put(&self, t: &str, pk: &str, sk: &str, b: &Value) -> Result<(), StorageError> { self.0.put(t, pk, sk, b).await }
    async fn delete(&self, t: &str, pk: &str, sk: &str) -> Result<bool, StorageError> { self.0.delete(t, pk, sk).await }
    async fn query(&self, t: &str, pk: &str) -> Result<Vec<Value>, StorageError> {
        if t == "workitem" {
            return Err(StorageError::Backend("arango 400 (errorNum 17): AQL: secret /data/iter/x".into()));
        }
        self.0.query(t, pk).await
    }
    async fn scan(&self, t: &str) -> Result<Vec<Value>, StorageError> { self.0.scan(t).await }
    async fn put_versioned(&self, t: &str, pk: &str, sk: &str, b: &Value, v: u64) -> Result<(), StorageError> { self.0.put_versioned(t, pk, sk, b, v).await }
    async fn acquire_lock(&self, t: &str, pk: &str, sk: &str, b: &Value, now: &str, w: &str) -> Result<(), StorageError> { self.0.acquire_lock(t, pk, sk, b, now, w).await }
    async fn bump_seq(&self, p: &str, t: &str) -> Result<u64, StorageError> { self.0.bump_seq(p, t).await }
    async fn get_versions(&self, p: &str) -> Result<Vec<iter_core::VersionRow>, StorageError> { self.0.get_versions(p).await }
    fn arango(&self) -> Option<&crate::arango::ArangoBackend> { self.0.arango() }
    fn backend_name(&self) -> &'static str { "failing" }
}

fn no_internals(v: &Value) {
    let s = v.to_string();
    for bad in ["arango", "AQL", "errorNum", "/data/", "/api/", "Invalid", "HS256", "Algorithm"] {
        assert!(!s.contains(bad), "{bad} shown: {s}");
    }
}

#[tokio::test]
async fn internal_errors_are_generic() {
    let t = world("p").await;
    let failing = Arc::new(AppState::for_tests(Arc::new(Failing(t.st.store.clone())), b"settings-test"));
    let app = crate::weblayers::apply(crate::api::router(failing.clone()).merge(crate::mcp::routes(failing)), crate::weblayers::WebCfg::open(false));
    let tok = t.tok("adm").await;
    let send = |path: &str, body: Option<Value>| {
        let mut b = Request::builder().uri(path).header("authorization", format!("Bearer {tok}"));
        b = if body.is_some() { b.method("POST").header("content-type", "application/json") } else { b.method("GET") };
        let req = b.body(body.map(|v| Body::from(serde_json::to_vec(&v).unwrap())).unwrap_or_else(Body::empty)).unwrap();
        app.clone().oneshot(req)
    };
    let resp = send("/api/projects/p/workitems", None).await.unwrap();
    assert_eq!(resp.status().as_u16(), 500);
    let v: Value = serde_json::from_slice(&axum::body::to_bytes(resp.into_body(), 1 << 20).await.unwrap()).unwrap();
    no_internals(&v);
    let id = v["error_id"].as_str().unwrap_or("").to_string();
    assert!(id.starts_with("e-") && id.len() == 14, "{v}");
    let logged = crate::errid::ring().lock().unwrap().iter().find(|l| l.starts_with(&id)).cloned().unwrap();
    assert!(logged.contains("GET /api/projects/p/workitems") && logged.contains("errorNum 17") && logged.contains("/data/iter/x"), "{logged}");
    // through MCP: no route, no detail, the id
    let msg = json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": {"name": "workitem_list", "arguments": {"project": "p"}}});
    let resp = send("/mcp", Some(msg)).await.unwrap();
    let v: Value = serde_json::from_slice(&axum::body::to_bytes(resp.into_body(), 1 << 20).await.unwrap()).unwrap();
    assert_eq!(v["result"]["isError"], true);
    no_internals(&v["result"]);
    assert!(v["result"]["structuredContent"]["error"].as_str().unwrap().contains("(error_id e-"), "{v}");
    // tokens: none algorithm, wrong key, truncated
    use base64::Engine as _;
    let b64 = |s: &str| base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(s);
    let none = format!("{}.{}.", b64(r#"{"alg":"none","typ":"JWT"}"#), b64(r#"{"sub":"adm","role":"admin","tokenver":1,"exp":9999999999}"#));
    let wrong = crate::auth::mint_token(b"another-key", "adm", "admin", 1, 3600).unwrap();
    let good = t.tok("adm").await;
    for bad in [none, wrong, good[..good.len() - 5].to_string()] {
        let (c, v) = super::as_bearer(&t, &bad, "GET", "/api/projects", None).await;
        assert_eq!((c, v["error"].as_str()), (401, Some("invalid or expired token")), "{v}");
        assert!(v["error_id"].as_str().unwrap_or("").starts_with("e-"), "{v}");
        no_internals(&v);
    }
    // the model's directory: admins only
    let user = t.ok("alice", "GET", "/api/projects/p/rag", None).await;
    assert!(user["model"].get("dir").is_none() && user["model"].get("error").is_none(), "{}", user["model"]);
    let admin = t.ok("adm", "GET", "/api/projects/p/rag", None).await;
    assert!(admin["model"].get("dir").is_some() || admin["model"].get("error").is_some(), "{}", admin["model"]);
    t.done().await;
}

#[tokio::test]
async fn bearer_scheme_any_case_and_authz_still_runs() {
    let t = world("p").await;
    let alice = t.tok("alice").await;
    let bob = t.tok("bob").await; // not a member of p
    let raw = |h: String| {
        let req = Request::builder().uri("/api/projects/p/workitems").header("authorization", h).body(Body::empty()).unwrap();
        t.app.clone().oneshot(req)
    };
    for h in [format!("Bearer {alice}"), format!("bearer {alice}"), format!("BEARER {alice}"), format!("Bearer\t{alice}"), format!("Bearer  {alice}")] {
        assert_eq!(raw(h.clone()).await.unwrap().status().as_u16(), 200, "{h:.12}");
    }
    for h in [format!("Basic {alice}"), alice.clone()] {
        assert_eq!(raw(h.clone()).await.unwrap().status().as_u16(), 401, "{h:.12}");
    }
    for scheme in ["bearer", "BEARER"] {
        assert_eq!(raw(format!("{scheme} {bob}")).await.unwrap().status().as_u16(), 403, "{scheme}: the authz layer ran");
    }
    t.done().await;
}

// ---------- MG-19 ----------

#[tokio::test]
async fn protocol_version_and_batching() {
    let t = world("p").await;
    let one = json!({"jsonrpc": "2.0", "id": 1, "method": "ping"});
    let batch = json!([one.clone(), {"jsonrpc": "2.0", "id": 2, "method": "ping"}]);
    let (c, v) = t.call("alice", "POST", "/mcp", Some(batch.clone())).await;
    assert!(c == 200 && v.as_array().map(|a| a.len()) == Some(2), "no header: {c} {v}");
    let (c, v) = t.call_h("alice", "POST", "/mcp", Some(batch.clone()), &[("mcp-protocol-version", "2025-03-26")]).await;
    assert!(c == 200 && v.is_array(), "{c} {v}");
    let (c, v) = t.call_h("alice", "POST", "/mcp", Some(batch.clone()), &[("mcp-protocol-version", "2025-06-18")]).await;
    assert_eq!((c, v["error"]["code"].as_i64()), (400, Some(-32600)), "{v}");
    assert!(v["error"]["message"].as_str().unwrap().contains("batching is not supported in protocol version 2025-06-18") && v.get("id").map(|i| i.is_null()).unwrap_or(true), "{v}");
    for unsupported in ["2025-11-25", "1999-01-01"] {
        let (c, v) = t.call_h("alice", "POST", "/mcp", Some(one.clone()), &[("mcp-protocol-version", unsupported)]).await;
        assert_eq!(c, 400, "{unsupported}: {v}");
    }
    let (c, v) = t.call_h("alice", "POST", "/mcp", Some(one), &[("mcp-protocol-version", "2025-06-18")]).await;
    assert_eq!((c, v["result"].clone()), (200, json!({})));
    t.done().await;
}

// ---------- MG-18, MG-20 ----------

/// A raw request to `app` with headers; (status, response headers, body).
async fn raw(app: &Router, method: &str, path: &str, headers: &[(&str, &str)], body: Option<Value>) -> (u16, axum::http::HeaderMap, Value) {
    let mut b = Request::builder().method(method).uri(path);
    for (k, v) in headers {
        b = b.header(*k, *v);
    }
    let req = match body {
        Some(v) => b.header("content-type", "application/json").body(Body::from(serde_json::to_vec(&v).unwrap())).unwrap(),
        None => b.body(Body::empty()).unwrap(),
    };
    let resp = app.clone().oneshot(req).await.unwrap();
    let (code, h) = (resp.status().as_u16(), resp.headers().clone());
    let bytes = axum::body::to_bytes(resp.into_body(), 1 << 20).await.unwrap();
    (code, h, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
}

#[tokio::test]
async fn origin_guard_and_cors() {
    let t = world("p").await;
    let bearer = format!("Bearer {}", t.tok("alice").await);
    let get = |origin: Option<&'static str>| {
        let (app, bearer) = (t.app.clone(), bearer.clone());
        async move {
            let mut h = vec![("authorization", bearer.as_str()), ("host", "iter.local:8400")];
            if let Some(o) = origin {
                h.push(("origin", o));
            }
            raw(&app, "GET", "/api/projects/p/workitems", &h, None).await
        }
    };
    let acao = |h: &axum::http::HeaderMap| h.get("access-control-allow-origin").and_then(|v| v.to_str().ok()).map(String::from);
    // the list is empty: everyone, CORS echoed
    assert_eq!(get(None).await.0, 200);
    let (c, h, _) = get(Some("https://evil.example")).await;
    assert_eq!((c, acao(&h).as_deref()), (200, Some("https://evil.example")));
    assert_eq!(get(Some("https://iter.local:8400")).await.0, 200, "same origin");
    // admins only, well-formed entries only
    assert_eq!(t.call("alice", "PUT", "/api/server/origins", Some(json!({"allowed_origins": ["https://ok.example"]}))).await.0, 403);
    assert_eq!(t.call("adm", "PUT", "/api/server/origins", Some(json!({"allowed_origins": ["https://ok.example/path"]}))).await.0, 400);
    t.ok("adm", "PUT", "/api/server/origins", Some(json!({"allowed_origins": ["https://ok.example"]}))).await;
    assert_eq!(t.ok("adm", "GET", "/api/server/origins", None).await["allowed_origins"], json!(["https://ok.example"]));
    let (c, h, _) = get(Some("https://ok.example")).await;
    assert_eq!((c, acao(&h).as_deref()), (200, Some("https://ok.example")));
    assert!(h.get_all("vary").iter().any(|v| v.to_str().unwrap_or("").to_lowercase().contains("origin")), "{h:?}");
    assert!(h.get("access-control-allow-credentials").is_none());
    let (c, _, v) = get(Some("https://evil.example")).await;
    assert_eq!((c, v["error"].as_str()), (403, Some("origin not allowed")));
    assert_eq!(get(Some("null")).await.0, 403);
    assert_eq!(get(None).await.0, 200, "agents are never affected");
    assert_eq!(get(Some("https://iter.local:8400")).await.0, 200, "the webui on its own server");
    // /mcp: a JSON-RPC error without an id
    let ping = json!({"jsonrpc": "2.0", "id": 7, "method": "ping"});
    let (c, _, v) = raw(&t.app, "POST", "/mcp", &[("authorization", bearer.as_str()), ("origin", "https://evil.example")], Some(ping)).await;
    assert_eq!((c, v["error"]["message"].as_str(), v["id"].is_null()), (403, Some("origin not allowed"), true), "{v}");
    // a preflight from the listed origin names the MCP and iter headers
    let (c, h, _) = raw(&t.app, "OPTIONS", "/mcp", &[("origin", "https://ok.example"), ("access-control-request-method", "POST"),
        ("access-control-request-headers", "mcp-protocol-version, x-iter-workid")], None).await;
    let allowed = h.get("access-control-allow-headers").and_then(|v| v.to_str().ok()).unwrap_or("").to_lowercase();
    assert!(c < 300 && allowed.contains("mcp-protocol-version") && allowed.contains("x-iter-workid"), "{c} {h:?}");
    assert_eq!(h.get("access-control-max-age").and_then(|v| v.to_str().ok()), Some("600"));
    t.ok("adm", "PUT", "/api/server/origins", Some(json!({"allowed_origins": []}))).await;
    assert_eq!(get(Some("https://evil.example")).await.0, 200, "cleared: open again");
    t.done().await;
}

#[tokio::test]
async fn hsts_on_tls_only() {
    let t = world("p").await;
    let bearer = format!("Bearer {}", t.tok("alice").await);
    for tls_on in [true, false] {
        let app = crate::weblayers::apply(crate::api::router(t.st.clone()).merge(crate::mcp::routes(t.st.clone())), crate::weblayers::WebCfg::open(tls_on));
        for (method, path) in [("GET", "/health"), ("GET", "/api/projects/p/workitems"), ("GET", "/api/projects/nope/workitems"), ("GET", "/no/such/route")] {
            let (_, h, _) = raw(&app, method, path, &[("authorization", bearer.as_str())], None).await;
            let hsts = h.get("strict-transport-security").and_then(|v| v.to_str().ok());
            assert_eq!(hsts, if tls_on { Some("max-age=31536000") } else { None }, "{path} tls={tls_on}");
        }
    }
    t.done().await;
}

// ---------- MG-1 ----------

/// A JSON Schema check of `type`, `required`, `properties` and `items`.
fn conforms(v: &Value, s: &Value, at: &str) -> Result<(), String> {
    let types: Vec<&str> = match &s["type"] {
        Value::String(t) => vec![t.as_str()],
        Value::Array(a) => a.iter().filter_map(|t| t.as_str()).collect(),
        _ => vec![],
    };
    let is = |t: &str| match t {
        "object" => v.is_object(),
        "array" => v.is_array(),
        "string" => v.is_string(),
        "integer" => v.is_i64() || v.is_u64(),
        "number" => v.is_number(),
        "boolean" => v.is_boolean(),
        "null" => v.is_null(),
        _ => true,
    };
    if !types.is_empty() && !types.iter().any(|t| is(t)) {
        return Err(format!("{at}: {v} is not {types:?}"));
    }
    if let (Some(req), Some(o)) = (s["required"].as_array(), v.as_object()) {
        for k in req.iter().filter_map(|k| k.as_str()) {
            if !o.contains_key(k) {
                return Err(format!("{at}: missing required {k}"));
            }
        }
    }
    if let (Some(props), Some(o)) = (s["properties"].as_object(), v.as_object()) {
        for (k, ps) in props {
            if let Some(x) = o.get(k) {
                conforms(x, ps, &format!("{at}.{k}"))?;
            }
        }
    }
    if let (Some(items), Some(a)) = (s.get("items"), v.as_array()) {
        for (i, e) in a.iter().enumerate() {
            conforms(e, items, &format!("{at}[{i}]"))?;
        }
    }
    Ok(())
}

/// Replace every string equal to a placeholder of the examples.
fn substitute(v: &Value, map: &[(String, String)]) -> Value {
    match v {
        Value::String(s) => json!(map.iter().find(|(k, _)| k == s).map(|(_, r)| r.clone()).unwrap_or_else(|| s.clone())),
        Value::Array(a) => Value::Array(a.iter().map(|x| substitute(x, map)).collect()),
        Value::Object(o) => Value::Object(o.iter().map(|(k, x)| (k.clone(), substitute(x, map))).collect()),
        x => x.clone(),
    }
}

/// A project the examples run against: the work items, map nodes, requirement
/// and document they name; returns placeholder → real id.
async fn seed_examples(t: &T, project: &str) -> Vec<(String, String)> {
    use crate::mcp_docs::*;
    t.served(project, "e1", "eng1").await;
    t.ok("adm", "POST", "/api/settings/edges", Some(json!({"from": "user:alice", "to": format!("project:{project}")}))).await;
    t.item(project, json!({"id": EX_ITEM, "name": "export retry", "state": "in-progress", "engine": "e1"})).await;
    t.item(project, json!({"id": EX_ITEM2, "name": "gateway client"})).await;
    // the people-only tools' items
    t.item(project, json!({"id": EX_QUESTION, "name": "retry on 502?", "state": "question", "needs_approval": true})).await;
    let form = json!({"title": "Retry on 502 too?", "summary": "", "detail": "The gateway also answers 502.", "fields": [{"key": "answer", "label": "Answer", "type": "text", "value": ""}]});
    t.ok("alice", "POST", &format!("/api/projects/{project}/workitems/{EX_QUESTION}/details"), Some(json!({"key": "question", "valuetype": "json", "value": form}))).await;
    t.item(project, json!({"id": EX_FAILED, "name": "export run", "state": "failed", "lasterror": "gateway 401"})).await;
    t.item(project, json!({"id": EX_DUP, "name": "gateway client again"})).await;
    t.item(project, json!({"id": EX_OLD, "name": "obsolete cleanup", "state": "parked"})).await;
    t.store().put("agent_tooling", "aws", NOSK, &json!({"name": "aws", "kind": "capability", "desc": "AWS access", "body": "use the deployer role"})).await.unwrap();
    let node = |b: Value| {
        let mut b = b;
        b["project"] = json!(project);
        async move {
            let (e, v) = tool(t, "alice", "graph_node_create", b).await;
            assert!(!e, "{v}");
            v["node"].clone()
        }
    };
    let billing = node(json!({"nodetype": "code", "level": "context", "name": "billing"})).await;
    let gateway = node(json!({"nodetype": "code", "level": "context", "name": "gateway"})).await;
    let uc = node(json!({"nodetype": "usecase", "name": "checkout"})).await;
    let (e, r) = tool(t, "alice", "req_create", json!({"project": project, "node": billing["id"], "type": "techreq", "title": "Exports are retried"})).await;
    assert!(!e, "{r}");
    let (e, d) = tool(t, "alice", "rag_add_document", json!({"project": project, "filename": "notes.md", "text": "# notes", "store": false})).await;
    assert!(!e, "{d}");
    let s = |v: &Value| v.as_str().unwrap().to_string();
    vec![
        (EX_NODE.into(), s(&billing["id"])), (EX_NODE2.into(), s(&gateway["id"])), (EX_USECASE.into(), s(&uc["id"])),
        (EX_REQFILE.into(), s(&r["file"]["id"])), (EX_REQ.into(), s(&r["req"]["id"])), (EX_NODE_PATH.into(), s(&billing["path"])),
        (EX_DOC.into(), s(&d["id"])), ("pdy-dev".into(), project.into()),
    ]
}

fn example_of(doc: &crate::mcp_docs::ToolDoc, map: &[(String, String)]) -> Value {
    substitute(&doc.example, map)
}

#[tokio::test]
async fn examples_run_and_match_output_schema() {
    let t = T::new().await;
    let read_map = seed_examples(&t, "pdy-dev").await;
    let scratch_map = seed_examples(&t, "scratch").await;
    let docs = crate::mcp_docs::docs();
    let run = |name: &'static str, args: Value| {
        let t = &t;
        async move {
            let (e, out) = tool(t, "alice", name, args.clone()).await;
            assert!(!e, "{name} {args}: {out}");
            let doc = crate::mcp_docs::docs().into_iter().find(|d| d.name == name).unwrap();
            if let Err(why) = conforms(&out, &doc.output, name) {
                panic!("{name}: the result does not match its outputSchema: {why}\n{out}");
            }
        }
    };
    // the read-only tools against the seeded project (rag_search: its own test, it needs the model)
    for d in docs.iter().filter(|d| d.read_only && d.name != "rag_search") {
        run(d.name, example_of(d, &read_map)).await;
    }
    // the write tools on the scratch project, in dependency order
    let order = ["workitem_stop", "workitem_create", "workitem_doc", "workitem_wait", "workitem_block", "workitem_ask", "workitem_reject",
                 "workitem_answer", "workitem_set_state", "workitem_priority", "workitem_edit", "workitem_explain", "workitem_clone",
                 "workitem_duplicate_of", "workitem_reopen", "workitem_delete",
                 "graph_node_create", "graph_node_update", "graph_edge_add", "graph_edge_remove",
                 "req_create", "req_update", "req_move", "req_delete", "rag_add_document", "rag_link_document",
                 "rag_resummarize_document", "rag_delete_document"];
    let writes: Vec<&str> = docs.iter().filter(|d| !d.read_only).map(|d| d.name).collect();
    let mut sorted = writes.clone();
    sorted.sort();
    let mut o = order.to_vec();
    o.sort();
    assert_eq!(sorted, o, "every write tool is in the run order");
    for name in order {
        let d = docs.iter().find(|d| d.name == name).unwrap();
        run(d.name, example_of(d, &scratch_map)).await;
    }
    t.done().await;
}

#[tokio::test]
async fn rag_search_example_runs() {
    if crate::rag::embed::find_model_dir().is_none() {
        panic!("harness: embedding model unavailable — set ITER_EMBED_MODEL to an {} directory (installers/fetch_model.sh)", crate::rag::embed::MODEL_NAME);
    }
    let t = T::new().await;
    let map = seed_examples(&t, "pdy-dev").await;
    let d = crate::mcp_docs::docs().into_iter().find(|d| d.name == "rag_search").unwrap();
    let (e, out) = tool(&t, "alice", "rag_search", example_of(&d, &map)).await;
    assert!(!e, "{out}");
    conforms(&out, &d.output, "rag_search").unwrap();
    t.done().await;
}
