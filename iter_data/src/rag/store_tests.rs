//! GraphRAG writes against a real ArangoDB with trained vector indexes
//! (MCP/GraphRAG hardening, MG-3 / MG-4): no writer may store a null vector,
//! because a sparse vector index skips a missing attribute but refuses null
//! (`arango 400 (errorNum 17): AQL: array expected for vector attribute`).
//! The dev ArangoDB must run with `--vector-index`; without it these tests
//! fail with "harness: vector index unavailable" rather than skip.

use super::pipeline::{DocMeta, Prepared, store_prepared};
use super::*;

/// A deterministic unit-ish vector for seed `i`.
fn vector(i: u64) -> Vec<f64> {
    let mut x = i.wrapping_mul(2654435761).wrapping_add(12345);
    (0..embed::DIM)
        .map(|_| {
            x = x.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            ((x >> 33) as f64 / (1u64 << 31) as f64) - 0.5
        })
        .collect()
}

/// 64 chunks with both vectors, then both vector indexes trained on them (nLists 8).
async fn with_vector_indexes() -> (ArangoBackend, String) {
    let (a, db) = crate::test_db::store().await;
    let rows: Vec<Value> = (0..64u64)
        .map(|i| json!({"_key": format!("seed_{i:05}"), "project": "seed", "doc": "seed", "kind": "file", "idx": i, "text": format!("seed {i}"),
                        "vec_raw": vector(i), "vec_sum": vector(i + 1000), "sum_state": "done"}))
        .collect();
    a.aql("FOR r IN @rows INSERT r INTO rag_chunk", json!({"rows": rows})).await.unwrap();
    for field in ["vec_raw", "vec_sum"] {
        let body = json!({"type": "vector", "name": field, "fields": [field], "sparse": true,
                          "params": {"metric": "cosine", "dimension": embed::DIM, "nLists": 8}});
        if let Err(e) = a.dbcall("POST", &format!("/_api/index?collection={CHUNK_COLL}"), Some(&body)).await {
            crate::test_db::drop(&db).await;
            panic!("harness: vector index unavailable ({}): start the dev ArangoDB with --vector-index", e.msg);
        }
    }
    (a, db)
}

fn prepared(texts: &[&str]) -> Prepared {
    serde_json::from_value(json!({
        "chapters": [{"idx": 0, "title": "one"}],
        "chunks": texts.iter().enumerate().map(|(i, t)| json!({"text": t, "chapter": 0, "vec_raw": vector(500 + i as u64)})).collect::<Vec<_>>(),
        "model": "test-model", "format": "markdown", "chars": 10,
    }))
    .unwrap()
}

fn meta<'a>(project: &'a str, id: &str, kind: &'a str) -> DocMeta<'a> {
    DocMeta { project, id: id.into(), kind, title: "t".into(), path: format!("{{topdir}}/{id}.md"), hash: "h".into(), extra: json!({}), by: "tester" }
}

/// Every chunk or document holding a vector attribute whose value is null.
async fn null_vectors(a: &ArangoBackend) -> Vec<Value> {
    a.aql(
        "FOR x IN UNION((FOR c IN rag_chunk RETURN c), (FOR d IN rag_doc RETURN d))
           FILTER (HAS(x, 'vec_sum') AND x.vec_sum == null) OR (HAS(x, 'vec_raw') AND x.vec_raw == null)
           RETURN x._id",
        json!({}),
    )
    .await
    .unwrap()
}

#[tokio::test]
async fn chunk_without_summary_stores_no_null_vector() {
    let (a, db) = with_vector_indexes().await;
    let out = store_prepared(&a, meta("p", "fdoc1", "file"), &prepared(&["alpha", "beta", "gamma"])).await;
    assert!(out.is_ok(), "store_prepared of a new 3-chunk document: {:?}", out.err());
    assert_eq!(null_vectors(&a).await, Vec::<Value>::new());
    let pending = a.aql("FOR c IN rag_chunk FILTER c.doc == 'fdoc1' RETURN [c.sum_state, HAS(c, 'vec_sum')]", json!({})).await.unwrap();
    assert_eq!(pending, vec![json!(["pending", false]); 3]);
    crate::test_db::drop(&db).await;
}

/// The real router over a database with vector indexes, called as an admin.
struct R {
    a: Arc<crate::arango::ArangoBackend>,
    app: axum::Router,
    tok: String,
    db: String,
}

impl R {
    async fn new() -> R {
        let (a, db) = with_vector_indexes().await;
        let a = Arc::new(a);
        let st = Arc::new(AppState::for_tests(a.clone(), b"rag-store-test"));
        st.store.put("webui_user", "adm", "-", &json!({"user": "adm", "role": "admin", "tokenver": 1, "pwhash": ""})).await.unwrap();
        st.store.put("project", "p", "-", &json!({"name": "p", "state": "Running"})).await.unwrap();
        let tok = crate::auth::mint_token(&st.secret, "adm", "admin", 1, 3600).unwrap();
        R { a, app: crate::api::router(st), tok, db }
    }
    async fn call(&self, method: &str, path: &str, body: Value) -> Value {
        use tower::ServiceExt;
        let req = axum::http::Request::builder()
            .method(method)
            .uri(path)
            .header("authorization", format!("Bearer {}", self.tok))
            .header("content-type", "application/json")
            .body(axum::body::Body::from(serde_json::to_vec(&body).unwrap()))
            .unwrap();
        let resp = self.app.clone().oneshot(req).await.unwrap();
        let code = resp.status();
        let v: Value = serde_json::from_slice(&axum::body::to_bytes(resp.into_body(), 64 << 20).await.unwrap()).unwrap_or(Value::Null);
        assert!(code.is_success(), "{method} {path}: {code} {v}");
        v
    }
    async fn assert_no_null(&self, after: &str) {
        assert_eq!(null_vectors(&self.a).await, Vec::<Value>::new(), "a null vector after {after}");
    }
}

fn chunk_json(text: &str, seed: u64) -> Value {
    json!({"text": text, "chapter": 0, "vec_raw": vector(seed)})
}

#[tokio::test]
async fn every_vector_writer_omits_null() {
    let r = R::new().await;
    // node files
    let node = json!({"path": "{topdir}/a.code.iter.md", "node_id": "n1", "nodetype": "code", "name": "a", "hash": "h1",
                      "chapters": [{"idx": 0, "title": "a"}], "chunks": [chunk_json("node text", 1), chunk_json("more node text", 2)], "model": "m"});
    let out = r.call("PUT", "/api/projects/p/rag/nodes", json!({"nodes": [node]})).await;
    assert_eq!(out["failed"], json!([]), "{out}");
    r.assert_no_null("nodes_put").await;
    // repo files
    let file = json!({"path": "{topdir}/README.md", "hash": "h2", "chapters": [{"idx": 0, "title": "r"}], "chunks": [chunk_json("readme", 3)], "model": "m"});
    let out = r.call("PUT", "/api/projects/p/rag/files", json!({"files": [file]})).await;
    assert_eq!(out["failed"], json!([]), "{out}");
    r.assert_no_null("files_put").await;
    // an upload, claimed and ingested by an engine
    use base64::Engine as _;
    let up = r.call("POST", "/api/projects/p/rag/docs", json!({"filename": "one.md", "content_b64": base64::engine::general_purpose::STANDARD.encode("one chunk"), "store": false})).await;
    let id = up["id"].as_str().unwrap().to_string();
    let job = r.call("POST", "/api/projects/p/rag/work/claim", json!({"engine": "e1", "kinds": ["ingest"]})).await;
    assert_eq!(job["job"], "ingest", "{job}");
    r.call("PUT", &format!("/api/projects/p/rag/docs/{id}/chunks"), json!({"engine": "e1", "job_id": job["job_id"],
        "chapters": [{"idx": 0, "title": "one"}], "chunks": [chunk_json("one chunk", 4)], "model": "m"})).await;
    r.assert_no_null("doc_chunks_put").await;
    // the guide goes through the same store_prepared
    let p = prepared(&["guide one", "guide two"]);
    let gm = DocMeta { project: guide::SCOPE, id: guide::DOC_ID.into(), kind: "guide", title: guide::TITLE.into(), path: guide::PATH.into(), hash: "g".into(), extra: json!({}), by: "iter_data" };
    store_prepared(&r.a, gm, &p).await.unwrap();
    r.assert_no_null("guide ingest").await;
    // a one-chunk document summarised: the rollup copies the chunk's vector
    let job = r.call("POST", "/api/projects/p/rag/work/claim", json!({"engine": "e1", "kinds": ["chunks"], "max_chunks": 32})).await;
    assert_eq!(job["job"], "chunks", "{job}");
    let results: Vec<Value> = job["chunks"].as_array().unwrap().iter().map(|c| json!({"id": c["id"], "summary": "a summary", "vec_sum": vector(9)})).collect();
    r.call("POST", "/api/projects/p/rag/work/done", json!({"engine": "e1", "job_id": job["job_id"], "job": "chunks", "results": results})).await;
    let d = r.call("GET", &format!("/api/projects/p/rag/docs/{id}"), json!(null)).await;
    assert_eq!(d["state"], "ready", "the one-chunk rollup ran: {d}");
    r.assert_no_null("single-chunk rollup").await;
    // resummarize clears both vectors without writing null
    r.call("POST", &format!("/api/projects/p/rag/docs/{id}/resummarize"), json!({})).await;
    r.assert_no_null("resummarize").await;
    crate::test_db::drop(&r.db).await;
}

#[tokio::test]
async fn kept_summary_with_missing_vector_is_pending() {
    let (a, db) = with_vector_indexes().await;
    store_prepared(&a, meta("p", "fkeep", "file"), &prepared(&["same text"])).await.unwrap();
    // an older write: marked done with a summary but no vector
    a.aql("FOR c IN rag_chunk FILTER c.doc == 'fkeep' UPDATE c WITH {sum_state: 'done', summary: 'old'} IN rag_chunk", json!({})).await.unwrap();
    store_prepared(&a, meta("p", "fkeep", "file"), &prepared(&["same text"])).await.unwrap();
    let c = a.aql("FOR c IN rag_chunk FILTER c.doc == 'fkeep' RETURN [c.sum_state, HAS(c, 'vec_sum'), c.summary]", json!({})).await.unwrap();
    assert_eq!(c, vec![json!(["pending", false, ""])]);
    crate::test_db::drop(&db).await;
}

#[tokio::test]
async fn repair_requeues_vector_failures() {
    let (a, db) = crate::test_db::store().await;
    let err17 = "arango 400 (errorNum 17): AQL: array expected for vector attribute for document \"x\"";
    let docs = json!([
        {"_key": "fhit", "project": "p", "kind": "file", "state": "failed", "error": err17, "ingest_attempts": 3},
        {"_key": "fother", "project": "p", "kind": "file", "state": "failed", "error": "no text could be extracted", "ingest_attempts": 3},
        {"_key": "fnoblob", "project": "p", "kind": "file", "state": "failed", "error": err17, "ingest_attempts": 3},
        {"_key": "fnull", "project": "p", "kind": "file", "state": "ready", "vec_sum": null},
    ]);
    a.aql("FOR d IN @d INSERT d INTO rag_doc", json!({"d": docs})).await.unwrap();
    a.aql("FOR k IN ['fhit', 'fother'] INSERT {_key: k, project: 'p', filename: 'x.md', content_b64: ''} INTO rag_blob", json!({})).await.unwrap();
    a.aql("INSERT {_key: 'cnull', project: 'p', doc: 'fnull', vec_raw: null, vec_sum: null} INTO rag_chunk", json!({})).await.unwrap();
    let state = |a: &crate::arango::ArangoBackend| {
        let a = a.clone();
        async move { a.aql("FOR d IN rag_doc SORT d._key RETURN [d._key, d.state, d.ingest_attempts, HAS(d, 'vec_sum')]", json!({})).await.unwrap() }
    };
    repair_null_vectors(&a).await.unwrap();
    let first = state(&a).await;
    assert_eq!(first, vec![
        json!(["fhit", "queued", 0, false]),
        json!(["fnoblob", "failed", 3, false]),
        json!(["fnull", "ready", null, false]),
        json!(["fother", "failed", 3, false]),
    ]);
    assert_eq!(null_vectors(&a).await, Vec::<Value>::new());
    repair_null_vectors(&a).await.unwrap();
    assert_eq!(state(&a).await, first, "a second run changes nothing");
    crate::test_db::drop(&db).await;
}
