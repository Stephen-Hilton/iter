//! Storage contract (iter4): the behaviour every caller relies on, checked
//! on ArangoDB — each test on its own throwaway database (test_db.rs).

use crate::storage::{Storage, StorageError};
use serde_json::json;
use std::sync::Arc;

async fn backends() -> Vec<(Arc<dyn Storage>, Option<String>)> {
    let (a, db) = crate::test_db::store().await;
    vec![(Arc::new(a), Some(db))]
}

async fn drop_db(db: &Option<String>) {
    if let Some(db) = db {
        crate::test_db::drop(db).await;
    }
}

#[tokio::test]
async fn contract_rows() {
    for (st, db) in backends().await {
        let n = st.backend_name();
        assert!(st.get("workitem", "p", "x").await.unwrap().is_none(), "{n}");
        let odd = "{topdir}/a b/ü:%/";
        st.put("lock", "my project", odd, &json!({"path": odd, "v": 1})).await.unwrap();
        assert_eq!(st.get("lock", "my project", odd).await.unwrap().unwrap()["v"], 1, "{n}");
        st.put("lock", "my project", odd, &json!({"path": odd, "v": 2})).await.unwrap();
        assert_eq!(st.get("lock", "my project", odd).await.unwrap().unwrap()["v"], 2, "{n}: put replaces");
        for sk in ["b", "a", "c"] {
            st.put("agent_tooling", "p", sk, &json!({"sk": sk})).await.unwrap();
        }
        st.put("agent_tooling", "q", "a", &json!({"sk": "qa"})).await.unwrap();
        let q: Vec<String> = st.query("agent_tooling", "p").await.unwrap().iter().map(|v| v["sk"].as_str().unwrap().to_string()).collect();
        assert_eq!(q, ["a", "b", "c"], "{n}: query ordered by sk, one pk only");
        assert_eq!(st.scan("agent_tooling").await.unwrap().len(), 4, "{n}");
        assert!(st.delete("agent_tooling", "p", "a").await.unwrap(), "{n}");
        assert!(!st.delete("agent_tooling", "p", "a").await.unwrap(), "{n}");
        // a long key still round-trips
        let long = "x".repeat(400);
        st.put("project", &long, "-", &json!({"n": 1})).await.unwrap();
        assert!(st.get("project", &long, "-").await.unwrap().is_some(), "{n}: long key");
        drop_db(&db).await;
    }
}

#[tokio::test]
async fn contract_versioned_writes() {
    for (st, db) in backends().await {
        let n = st.backend_name();
        st.put_versioned("workitem", "p", "w", &json!({"id": "w", "version": 1}), 0).await.unwrap();
        match st.put_versioned("workitem", "p", "w", &json!({"id": "w", "version": 1}), 0).await {
            Err(StorageError::Conflict(Some(cur))) => assert_eq!(cur["version"], 1, "{n}"),
            other => panic!("{n}: create twice must conflict, got {other:?}"),
        }
        st.put_versioned("workitem", "p", "w", &json!({"id": "w", "version": 2}), 1).await.unwrap();
        assert!(matches!(st.put_versioned("workitem", "p", "w", &json!({"version": 2}), 1).await, Err(StorageError::Conflict(_))), "{n}: stale expect");
        // a plain put that carries a version keeps the native version in step
        st.put("workitem", "p", "w", &json!({"id": "w", "version": 5})).await.unwrap();
        st.put_versioned("workitem", "p", "w", &json!({"id": "w", "version": 6}), 5).await.unwrap();
        // race: 16 writers expecting version 6 — exactly one wins
        let mut hs = Vec::new();
        for i in 0..16 {
            let st = st.clone();
            hs.push(tokio::spawn(async move {
                st.put_versioned("workitem", "p", "w", &json!({"id": "w", "version": 7, "who": i}), 6).await.is_ok()
            }));
        }
        let wins = futures_count(hs).await;
        assert_eq!(wins, 1, "{n}: exactly one versioned writer wins");
        drop_db(&db).await;
    }
}

#[tokio::test]
async fn contract_locks_and_seq() {
    for (st, db) in backends().await {
        let n = st.backend_name();
        let body = |w: &str, exp: &str| json!({"path": "{topdir}/src/", "workid": w, "expires": exp});
        st.acquire_lock("lock", "p", "{topdir}/src/", &body("A", "2099-01-01T00:00:00Z"), "2026-01-01T00:00:00Z", "A").await.unwrap();
        match st.acquire_lock("lock", "p", "{topdir}/src/", &body("B", "2099-01-01T00:00:00Z"), "2026-01-01T00:00:00Z", "B").await {
            Err(StorageError::Conflict(Some(cur))) => assert_eq!(cur["workid"], "A", "{n}: conflict carries the holder"),
            other => panic!("{n}: held lock must conflict, got {other:?}"),
        }
        st.acquire_lock("lock", "p", "{topdir}/src/", &body("A", "2099-01-02T00:00:00Z"), "2026-01-01T00:00:00Z", "A").await.unwrap();
        // after expiry anyone may take it
        st.acquire_lock("lock", "p", "{topdir}/src/", &body("B", "2100-01-01T00:00:00Z"), "2099-06-01T00:00:00Z", "B").await.unwrap();
        // race on a fresh path: exactly one holder
        let mut hs = Vec::new();
        for i in 0..16 {
            let st = st.clone();
            hs.push(tokio::spawn(async move {
                let w = format!("W{i}");
                st.acquire_lock("lock", "p", "{topdir}/race/", &json!({"workid": w, "expires": "2099-01-01T00:00:00Z"}), "2026-01-01T00:00:00Z", &w)
                    .await
                    .is_ok()
            }));
        }
        assert_eq!(futures_count(hs).await, 1, "{n}: exactly one lock holder");
        // seq: 40 concurrent bumps give 40 distinct values ending at 40
        let mut hs = Vec::new();
        for _ in 0..40 {
            let st = st.clone();
            hs.push(tokio::spawn(async move { st.bump_seq("p", "workitem").await.unwrap() }));
        }
        let mut seen = std::collections::HashSet::new();
        for h in hs {
            seen.insert(h.await.unwrap());
        }
        assert_eq!(seen.len(), 40, "{n}: distinct seq values");
        let v = st.get_versions("p").await.unwrap();
        assert_eq!(v.iter().find(|r| r.table == "workitem").unwrap().seq, 40, "{n}");
        drop_db(&db).await;
    }
}

#[tokio::test]
async fn contract_graph_on_arango() {
    // the native graph path: replace + traversal through AQL
    for (st, db) in backends().await {
        let Some(a) = st.arango() else { continue };
        let v = |id: &str, t: &str| json!({"id": id, "nodetype": t, "name": id, "path": format!("{{topdir}}/{id}"), "vhash": id});
        let e = |f: &str, k: &str, t: &str| json!({"from": f, "kind": k, "to": t});
        let vs = vec![v("m", "main"), v("a", "code"), v("b", "code"), v("t", "testgroup")];
        let es = vec![e("m", "root", "a"), e("a", "codenodes", "b"), e("b", "testgroups", "t")];
        crate::graph::replace_pub(st.as_ref(), "p", &vs, &es).await.unwrap();
        let n = a
            .aql(
                "FOR x IN 1..5 OUTBOUND @s GRAPH 'iter_map' RETURN x.id",
                json!({"s": format!("node/{}", crate::arango::doc_key("p", "m"))}),
            )
            .await
            .unwrap();
        assert_eq!(n.len(), 3);
        // a re-sync without b drops b and its edges; t keeps its test result
        a.aql("UPDATE {_key: @k} WITH {test: {result: 'red'}} IN node", json!({"k": crate::arango::doc_key("p", "t")})).await.unwrap();
        crate::graph::replace_pub(st.as_ref(), "p", &[v("m", "main"), v("a", "code"), v("t", "testgroup")], &[e("m", "root", "a")]).await.unwrap();
        let t = a.aql("RETURN DOCUMENT(CONCAT('node/', @k))", json!({"k": crate::arango::doc_key("p", "t")})).await.unwrap();
        assert_eq!(t[0]["test"]["result"], "red");
        let links = a.aql("FOR l IN link FILTER l.project == 'p' RETURN l", json!({})).await.unwrap();
        assert_eq!(links.len(), 1);
        drop_db(&db).await;
    }
}

#[tokio::test]
async fn contract_usecase_tag_lookup_uses_the_array_index() {
    // a use case's picture is one lookup on node.usecases[*] (usecase_map.rs), not a traversal
    for (st, db) in backends().await {
        let Some(a) = st.arango() else { continue };
        let v = |id: &str, t: &str, key: &str| json!({"id": id, "nodetype": t, "name": id, "key": key, "dir": format!("{{topdir}}/{key}"),
            "path": format!("{{topdir}}/{key}/{id}.code.iter.md"), "vhash": id});
        let e = |f: &str, k: &str, t: &str| json!({"from": f, "kind": k, "to": t});
        let mut vs = vec![v("m", "main", "m"), v("ctx", "code", "ctx"), v("api", "code", "ctx/api"), v("db", "code", "ctx/db"),
            json!({"id": "u", "nodetype": "usecase", "ucid": "usecase:read", "name": "read", "path": "{topdir}/usecases/read/read.usecase.iter.md"})];
        let es = vec![e("m", "root", "ctx"), e("ctx", "codenodes", "api"), e("ctx", "codenodes", "db"), e("u", "codenodes", "api")];
        iter_local::usecase_map::usecase_map(&mut vs, &es);
        crate::graph::replace_pub(st.as_ref(), "p", &vs, &es).await.unwrap();
        let q = "FOR n IN node FILTER n.project == @p AND @u IN n.usecases[*] SORT n.key RETURN n.id";
        let bind = json!({"p": "p", "u": "usecase:read"});
        assert_eq!(a.aql(q, bind.clone()).await.unwrap(), vec![json!("ctx"), json!("api")]);
        let plan = a.dbcall("POST", "/_api/explain", Some(&json!({"query": q, "bindVars": bind}))).await.unwrap();
        let used: Vec<String> = plan["plan"]["nodes"].as_array().unwrap().iter()
            .flat_map(|n| n["indexes"].as_array().cloned().unwrap_or_default())
            .filter_map(|i| i["name"].as_str().map(String::from)).collect();
        assert!(used.contains(&"project_usecases".to_string()), "indexes used: {used:?}");
        drop_db(&db).await;
    }
}

async fn futures_count(hs: Vec<tokio::task::JoinHandle<bool>>) -> usize {
    let mut n = 0;
    for h in hs {
        if h.await.unwrap() {
            n += 1;
        }
    }
    n
}
