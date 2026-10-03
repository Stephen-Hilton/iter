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
    // the project graph store (nodes.rs): derived links are real edges of the
    // named graph, so AQL traversals follow them
    use iter_core::nodefile::{self as nf, EdgeKind, NodeDoc, NodeType};
    for (st, db) in backends().await {
        let Some(a) = st.arango() else { continue };
        let mut g = crate::nodes::Graph::load(st.as_ref(), "p").await.unwrap();
        let now = nf::now_ts();
        let mut mk = |t: NodeType, name: &str, path: &str| {
            let mut d = NodeDoc::new(t, name, "t", &now);
            d.path = path.into();
            let id = d.id.clone();
            g.insert_new(crate::nodes::canonical(&d, &now, "t"), "t", "");
            id
        };
        let p = mk(NodeType::Project, "p", "{topdir}/global/p.project.iter.md");
        let x = mk(NodeType::Code, "x", "{topdir}/src/x/x.code.iter.md");
        let y = mk(NodeType::Code, "y", "{topdir}/src/x/y/y.code.iter.md");
        g.add_edge(&p, &x, EdgeKind::Codenodes, "t").unwrap();
        g.add_edge(&x, &y, EdgeKind::Codenodes, "t").unwrap();
        g.save(st.as_ref()).await.unwrap();
        let n = a
            .aql("FOR v IN 1..5 OUTBOUND @s GRAPH 'iter_map' RETURN v.id", json!({"s": format!("node/{}", crate::arango::doc_key("p", &p))}))
            .await
            .unwrap();
        assert_eq!(n, vec![json!(x.clone()), json!(y.clone())]);
        // removing the edge removes the link
        let mut g = crate::nodes::Graph::load(st.as_ref(), "p").await.unwrap();
        g.remove_edge(&x, &y, EdgeKind::Codenodes, "t", "test").unwrap();
        g.save(st.as_ref()).await.unwrap();
        let links = a.aql("FOR l IN link FILTER l.project == 'p' RETURN l", json!({})).await.unwrap();
        assert_eq!(links.len(), 1);
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
