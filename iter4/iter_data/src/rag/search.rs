//! GraphRAG search (2026-09-29): three rankings of a project's chunks, fused.
//!
//! - **keyword**: ArangoSearch view `rag_view` over each chunk's text, its AI
//!   summary, its heading and its document title (`text_en` analyzer: words
//!   stemmed, identifiers like `rag_waiting` kept whole), scored by BM25 —
//!   finds exact names, error codes and rare terms that vectors blur;
//! - **raw**: cosine similarity of the question to each chunk's raw-text vector;
//! - **summary**: cosine similarity to each chunk's summary vector (covers
//!   what the passage is about, in plain words).
//!
//! Each ranking contributes its top CANDIDATES. The two vector rankings are
//! first merged into one **meaning** ranking (a chunk's better similarity of
//! the two) — they measure the same thing and, fused separately, outvoted
//! keyword search (measured 2026-09-29: keyword's rank-1 answers fell out of
//! the top 5). Reciprocal rank fusion (RRF, score = Σ 1 / (60 + rank)) then
//! merges meaning and keyword on equal terms. `mode` picks rankings:
//! `hybrid` (default), `vector` (meaning only), `keyword`. At most
//! `per_doc` chunks of one document are returned (default 2), so one long
//! document cannot fill every slot.
//! Vectors are exact cosine in AQL, or ArangoDB vector indexes (arangod
//! `--vector-index`) once a project is large, re-scored exactly.
//!
//! Every hit returns the FULL chunk, its chapter and document summaries, and
//! its place in the map: a node file's neighbours and the documents it links
//! (`children.documents`), or the nodes that link an uploaded document.

use super::*;
use std::collections::{HashMap, HashSet};

/// Candidates each ranking contributes before fusion.
const CANDIDATES: usize = 50;
/// The RRF constant (60 is the published default: flattens the top ranks' dominance).
const RRF_K: f64 = 60.0;
/// Chunks in a project before the vector indexes are built.
const VECTOR_INDEX_MIN: u64 = 1024;

// ---------- the keyword view ----------

pub async fn ensure_view(a: &ArangoBackend) -> Result<(), String> {
    let text = json!({"analyzers": ["text_en"]});
    let ident = json!({"analyzers": ["identity"]});
    let links = json!({CHUNK_COLL: {"includeAllFields": false, "fields": {
        "text": text, "summary": text, "heading": text, "title": text,
        "project": ident, "kind": ident, "nodetype": ident, "doc": ident}}});
    match a.dbcall("POST", "/_api/view", Some(&json!({"name": VIEW, "type": "arangosearch", "links": links, "commitIntervalMsec": 500}))).await {
        Ok(_) => Ok(()),
        // exists: bring its links up to date (additive, idempotent)
        Err(e) if e.num == 1207 => a
            .dbcall("PATCH", &format!("/_api/view/{VIEW}/properties"), Some(&json!({"links": links})))
            .await
            .map(|_| ())
            .map_err(|e| e.to_string()),
        Err(e) => Err(e.to_string()),
    }
}

// ---------- the request ----------

#[derive(serde::Deserialize, Clone)]
pub struct SearchReq {
    pub query: String,
    #[serde(default = "default_k")]
    pub k: usize,
    /// file | node (empty = both)
    #[serde(default)]
    pub kinds: Vec<String>,
    /// node documents of these nodetypes only (code, bizreq, techreq, tests, usecase, interface…)
    #[serde(default)]
    pub nodetypes: Vec<String>,
    /// restrict to one document
    #[serde(default)]
    pub doc: String,
    /// hybrid (default) | vector | keyword
    #[serde(default)]
    pub mode: String,
    /// which vectors the vector rankings use: both (default) | raw | summary
    #[serde(default)]
    pub vectors: String,
    /// also search the built-in iter4 user guide (default true)
    #[serde(default = "yes")]
    pub include_guide: bool,
    /// most chunks of one document in the results (default 2; 0 = no limit)
    #[serde(default = "default_per_doc")]
    pub per_doc: usize,
    /// attach each hit's place in the map (default true)
    #[serde(default = "yes")]
    pub graph: bool,
    /// also rank whole documents by their summary vector
    #[serde(default)]
    pub docs: bool,
}
fn default_k() -> usize {
    8
}
fn default_per_doc() -> usize {
    2
}
fn yes() -> bool {
    true
}

pub async fn search(_u: AuthUser, State(st): Ctx, Path(name): Path<String>, Json(req): Json<SearchReq>) -> Result<Json<Value>, ApiError> {
    let a = arango(st.store.as_ref())?;
    Ok(Json(run_search(a, &name, &req).await?))
}

/// AQL filter on chunk `c` for the request's kinds/nodetypes/doc (FILTER form).
fn filter_clause(req: &SearchReq) -> String {
    let mut f = String::from("c.project IN @ps");
    if !req.kinds.is_empty() {
        f.push_str(" AND c.kind IN @kinds");
    }
    if !req.nodetypes.is_empty() {
        f.push_str(" AND c.nodetype IN @nts");
    }
    if !req.doc.is_empty() {
        f.push_str(" AND c.doc == @doc");
    }
    f
}

fn filter_bind(req: &SearchReq, project: &str) -> serde_json::Map<String, Value> {
    let mut b = serde_json::Map::new();
    b.insert("ps".into(), json!(scopes(req, project)));
    if !req.kinds.is_empty() {
        b.insert("kinds".into(), json!(req.kinds));
    }
    if !req.nodetypes.is_empty() {
        b.insert("nts".into(), json!(req.nodetypes));
    }
    if !req.doc.is_empty() {
        b.insert("doc".into(), json!(req.doc));
    }
    b
}

/// The projects a search reads: the project, plus the product-wide guide
/// unless excluded (by `include_guide: false`, or kinds that leave it out).
fn scopes(req: &SearchReq, project: &str) -> Vec<String> {
    let mut v = vec![project.to_string()];
    let kinds_allow = req.kinds.is_empty() || req.kinds.iter().any(|k| k == "guide");
    if req.include_guide && kinds_allow && req.nodetypes.is_empty() && project != guide::SCOPE {
        v.push(guide::SCOPE.to_string());
    }
    v
}

/// Top candidates by one vector field: (chunk key, cosine), best first.
async fn vector_ranking(a: &ArangoBackend, project: &str, req: &SearchReq, field: &str, qv: &[f32], use_index: bool) -> Result<Vec<(String, f64)>, ApiError> {
    let filters = filter_clause(req);
    let mut bind = filter_bind(req, project);
    bind.insert("q".into(), json!(qv));
    bind.insert("n".into(), json!(CANDIDATES));
    if use_index {
        // approximate candidates from the index, filtered, re-scored exactly
        let mut b = bind.clone();
        b.insert("wide".into(), json!(CANDIDATES * 20));
        let q = format!(
            "FOR key IN (FOR c IN rag_chunk SORT APPROX_NEAR_COSINE(c.{field}, @q) DESC LIMIT @wide RETURN c._key)
               LET c = DOCUMENT(CONCAT('rag_chunk/', key))
               FILTER {filters} AND c.{field} != null
               LET s = COSINE_SIMILARITY(c.{field}, @q) SORT s DESC LIMIT @n RETURN [c._key, s]"
        );
        if let Ok(r) = a.aql(&q, Value::Object(b)).await {
            if r.len() >= CANDIDATES.min(req.k) {
                return Ok(pairs(r));
            }
        }
    }
    let q = format!("FOR c IN rag_chunk FILTER {filters} AND c.{field} != null LET s = COSINE_SIMILARITY(c.{field}, @q) SORT s DESC LIMIT @n RETURN [c._key, s]");
    Ok(pairs(a.aql(&q, Value::Object(bind)).await.map_err(backend)?))
}

/// Top candidates by BM25 over text, summary, heading and title.
async fn keyword_ranking(a: &ArangoBackend, project: &str, req: &SearchReq) -> Result<Vec<(String, f64)>, ApiError> {
    let mut search = String::from("c.project IN @ps");
    if !req.kinds.is_empty() {
        search.push_str(" AND c.kind IN @kinds");
    }
    if !req.nodetypes.is_empty() {
        search.push_str(" AND c.nodetype IN @nts");
    }
    if !req.doc.is_empty() {
        search.push_str(" AND c.doc == @doc");
    }
    let q = format!(
        "LET toks = TOKENS(@q, 'text_en')
         FOR c IN {VIEW} SEARCH {search} AND ANALYZER(
             BOOST(c.text IN toks, 1.0) OR BOOST(c.summary IN toks, 1.2) OR BOOST(c.heading IN toks, 1.5) OR BOOST(c.title IN toks, 1.5),
             'text_en')
         LET s = BM25(c) SORT s DESC LIMIT @n RETURN [c._key, s]"
    );
    let mut bind = filter_bind(req, project);
    bind.insert("q".into(), json!(req.query.trim()));
    bind.insert("n".into(), json!(CANDIDATES));
    Ok(pairs(a.aql(&q, Value::Object(bind)).await.map_err(backend)?))
}

fn pairs(rows: Vec<Value>) -> Vec<(String, f64)> {
    rows.into_iter()
        .filter_map(|r| Some((r.get(0)?.as_str()?.to_string(), r.get(1)?.as_f64().unwrap_or(0.0))))
        .collect()
}

/// One meaning ranking from the raw and summary vector rankings: each chunk
/// scored by the better of its two similarities.
pub fn merge_by_best(a: &[(String, f64)], b: &[(String, f64)]) -> Vec<(String, f64)> {
    let mut m: HashMap<&str, f64> = HashMap::new();
    for (k, s) in a.iter().chain(b.iter()) {
        let e = m.entry(k.as_str()).or_insert(f64::MIN);
        if *s > *e {
            *e = *s;
        }
    }
    let mut out: Vec<(String, f64)> = m.into_iter().map(|(k, s)| (k.to_string(), s)).collect();
    out.sort_by(|x, y| y.1.partial_cmp(&x.1).unwrap_or(std::cmp::Ordering::Equal).then(x.0.cmp(&y.0)));
    out
}

/// Reciprocal rank fusion: key → (fused score, [rank per ranking, 1-based]).
pub fn fuse(rankings: &[&[(String, f64)]]) -> Vec<(String, f64, Vec<Option<usize>>)> {
    let mut m: HashMap<&str, (f64, Vec<Option<usize>>)> = HashMap::new();
    for (ri, r) in rankings.iter().enumerate() {
        for (rank, (k, _)) in r.iter().enumerate() {
            let e = m.entry(k.as_str()).or_insert_with(|| (0.0, vec![None; rankings.len()]));
            e.0 += 1.0 / (RRF_K + (rank + 1) as f64);
            e.1[ri] = Some(rank + 1);
        }
    }
    let mut out: Vec<(String, f64, Vec<Option<usize>>)> = m.into_iter().map(|(k, (s, r))| (k.to_string(), s, r)).collect();
    out.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal).then(a.0.cmp(&b.0)));
    out
}

pub async fn run_search(a: &ArangoBackend, project: &str, req: &SearchReq) -> Result<Value, ApiError> {
    let query = req.query.trim();
    if query.is_empty() {
        return Err(bad("query is empty"));
    }
    let k = req.k.clamp(1, 50);
    let started = std::time::Instant::now();
    let mode = if req.mode.is_empty() { "hybrid" } else { req.mode.as_str() };
    let (use_kw, use_vec) = match mode {
        "keyword" => (true, false),
        "vector" => (false, true),
        "hybrid" => (true, true),
        other => return Err(bad(format!("mode must be hybrid | vector | keyword (got {other:?})"))),
    };
    let (use_raw, use_sum) = match req.vectors.as_str() {
        "raw" => (true, false),
        "summary" => (false, true),
        _ => (true, true),
    };
    let qv = if use_vec { embed_async(vec![query.to_string()]).await.map_err(unavailable)?.remove(0) } else { vec![] };
    let indexed = use_vec && has_vector_indexes(a).await;
    let raw = if use_vec && use_raw { vector_ranking(a, project, req, "vec_raw", &qv, indexed).await? } else { vec![] };
    let sum = if use_vec && use_sum { vector_ranking(a, project, req, "vec_sum", &qv, indexed).await? } else { vec![] };
    let kw = if use_kw { keyword_ranking(a, project, req).await? } else { vec![] };
    let meaning = merge_by_best(&raw, &sum);
    let fused = fuse(&[&meaning, &kw]);
    // chunk → document, for the per-document cap
    let cand_keys: Vec<&str> = fused.iter().map(|f| f.0.as_str()).collect();
    let doc_of: HashMap<String, String> = a
        .aql("FOR key IN @keys LET c = DOCUMENT(CONCAT('rag_chunk/', key)) RETURN [key, c.doc]", json!({"keys": cand_keys}))
        .await
        .map_err(backend)?
        .into_iter()
        .filter_map(|r| Some((r.get(0)?.as_str()?.to_string(), r.get(1)?.as_str()?.to_string())))
        .collect();
    let mut per: HashMap<&str, usize> = HashMap::new();
    let top: Vec<&(String, f64, Vec<Option<usize>>)> = fused
        .iter()
        .filter(|f| {
            if req.per_doc == 0 {
                return true;
            }
            let d = doc_of.get(&f.0).map(String::as_str).unwrap_or("");
            let n = per.entry(d).or_insert(0);
            *n += 1;
            *n <= req.per_doc
        })
        .take(k)
        .collect();
    let keys: Vec<&str> = top.iter().map(|t| t.0.as_str()).collect();
    let cos = |r: &[(String, f64)], key: &str| r.iter().find(|x| x.0 == key).map(|x| x.1);
    let rows = a
        .aql(
            "FOR key IN @keys LET c = DOCUMENT(CONCAT('rag_chunk/', key))
             RETURN {id: c._key, doc: c.doc, idx: c.idx, chapter: c.chapter, heading: c.heading, text: c.text, summary: c.summary,
                     sr: (@q == null OR c.vec_raw == null) ? null : COSINE_SIMILARITY(c.vec_raw, @q),
                     ss: (@q == null OR c.vec_sum == null) ? null : COSINE_SIMILARITY(c.vec_sum, @q)}",
            json!({"keys": keys, "q": if use_vec { json!(qv) } else { Value::Null }}),
        )
        .await
        .map_err(backend)?;
    let by_key: HashMap<String, Value> = rows.into_iter().map(|r| (s(&r, "id").to_string(), r)).collect();
    // documents, and the map around each hit
    let doc_ids: Vec<String> = by_key.values().map(|h| s(h, "doc").to_string()).collect::<HashSet<_>>().into_iter().collect();
    let docs: HashMap<String, Value> = a
        .aql(
            "FOR d IN rag_doc FILTER d._key IN @ids RETURN {id: d._key, title: d.title, kind: d.kind, path: d.path, nodetype: d.nodetype,
                                                             node_id: d.node_id, summary: d.summary, chapters: d.chapters, state: d.state}",
            json!({"ids": doc_ids}),
        )
        .await
        .map_err(backend)?
        .into_iter()
        .map(|d| (s(&d, "id").to_string(), d))
        .collect();
    let mut graph: HashMap<String, Value> = HashMap::new();
    if req.graph {
        let node_ids: Vec<String> = docs.values().filter(|d| !s(d, "node_id").is_empty()).map(|d| s(d, "node_id").to_string()).collect();
        if !node_ids.is_empty() {
            graph.extend(node_context(a, project, &node_ids).await?);
        }
        for d in docs.values().filter(|d| d["kind"] == "file") {
            graph.insert(format!("file:{}", s(d, "id")), json!({"linked_nodes": linked_nodes(a, project, s(d, "path")).await}));
        }
    }
    let mut results = Vec::new();
    for (key, score, ranks) in top {
        let Some(mut h) = by_key.get(key).cloned() else { continue };
        let d = docs.get(s(&h, "doc")).cloned().unwrap_or(Value::Null);
        let ch = h["chapter"].as_u64().unwrap_or(0) as usize;
        let chapter = d["chapters"].get(ch).cloned().unwrap_or(Value::Null);
        let sr = h["sr"].as_f64().or_else(|| cos(&raw, key));
        let ss = h["ss"].as_f64().or_else(|| cos(&sum, key));
        let rank_in = |r: &[(String, f64)]| r.iter().position(|x| x.0 == *key).map(|p| p + 1);
        let (rr, rs) = (rank_in(&raw), rank_in(&sum));
        let mut matched = Vec::new();
        if rr.is_some() {
            matched.push("raw");
        }
        if rs.is_some() {
            matched.push("summary");
        }
        if ranks[1].is_some() {
            matched.push("keyword");
        }
        h["score"] = json!(score);
        h["similarity"] = json!(sr.into_iter().chain(ss).fold(f64::NAN, f64::max));
        h["score_raw"] = json!(sr);
        h["score_summary"] = json!(ss);
        h["ranks"] = json!({"meaning": ranks[0], "raw": rr, "summary": rs, "keyword": ranks[1]});
        h["keyword_bm25"] = json!(cos(&kw, key));
        h["matched"] = json!(matched);
        if let Some(o) = h.as_object_mut() {
            o.remove("sr");
            o.remove("ss");
        }
        h["document"] = json!({"id": d["id"], "title": d["title"], "kind": d["kind"], "path": d["path"], "nodetype": d["nodetype"],
                               "node_id": d["node_id"], "summary": d["summary"], "state": d["state"]});
        h["chapter_title"] = chapter["title"].clone();
        h["chapter_summary"] = chapter["summary"].clone();
        let g = if d["kind"] == "file" { graph.get(&format!("file:{}", s(&d, "id"))) } else { graph.get(s(&d, "node_id")) };
        if let Some(g) = g {
            h["graph"] = g.clone();
        }
        results.push(h);
    }
    let mut out = json!({"query": query, "k": k, "mode": mode, "method": if indexed { "vector-index" } else { "exact" },
                         "vectors": req.vectors, "ms": 0, "results": results, "model": embed::disk_stamp().unwrap_or_default(),
                         "candidates": {"raw": raw.len(), "summary": sum.len(), "keyword": kw.len()}});
    if req.docs && use_vec {
        let mut bind = filter_bind(req, project);
        bind.remove("doc");
        bind.insert("q".into(), json!(qv));
        bind.insert("k".into(), json!(k));
        let mut f = String::from("d.project IN @ps AND d.vec_sum != null");
        if !req.kinds.is_empty() {
            f.push_str(" AND d.kind IN @kinds");
        }
        if !req.nodetypes.is_empty() {
            f.push_str(" AND d.nodetype IN @nts");
        }
        let q = format!(
            "FOR d IN rag_doc FILTER {f} LET score = COSINE_SIMILARITY(d.vec_sum, @q) SORT score DESC LIMIT @k
             RETURN {{id: d._key, title: d.title, kind: d.kind, path: d.path, nodetype: d.nodetype, summary: d.summary, score}}"
        );
        out["documents"] = Value::Array(a.aql(&q, Value::Object(bind)).await.map_err(backend)?);
    }
    out["ms"] = json!(started.elapsed().as_millis() as u64);
    Ok(out)
}

/// For each node id: its neighbours in the map (both directions, named) and
/// the uploaded documents it names in `children.documents`.
async fn node_context(a: &ArangoBackend, project: &str, ids: &[String]) -> Result<HashMap<String, Value>, ApiError> {
    let rows = a
        .aql(
            "FOR id IN @ids
               LET me = FIRST(FOR v IN node FILTER v.project == @p AND v.id == id RETURN v)
               LET outs = (FOR l IN link FILTER l.project == @p AND l.from == id
                             LET n = FIRST(FOR v IN node FILTER v.project == @p AND v.id == l.to RETURN v)
                             RETURN {dir: 'out', kind: l.kind, id: l.to, name: n.name, nodetype: n.nodetype, path: n.path})
               LET ins = (FOR l IN link FILTER l.project == @p AND l.from != null AND l.to == id
                            LET n = FIRST(FOR v IN node FILTER v.project == @p AND v.id == l.from RETURN v)
                            RETURN {dir: 'in', kind: l.kind, id: l.from, name: n.name, nodetype: n.nodetype, path: n.path})
               LET docs = (FOR p IN (me.documents || [])
                             LET d = FIRST(FOR d IN rag_doc FILTER d.project == @p AND d.path == p RETURN d)
                             RETURN {path: p, id: d._key, title: d.title, summary: d.summary})
               RETURN {id: id, neighbours: APPEND(ins, outs), documents: docs}",
            json!({"p": project, "ids": ids}),
        )
        .await
        .map_err(backend)?;
    Ok(rows
        .into_iter()
        .map(|r| (s(&r, "id").to_string(), json!({"node_id": r["id"], "neighbours": r["neighbours"], "documents": r["documents"]})))
        .collect())
}

// ---------- vector indexes ----------

static VECTOR_NOTE: std::sync::Mutex<String> = std::sync::Mutex::new(String::new());
static VECTOR_RETRY_AT: std::sync::Mutex<Option<std::time::Instant>> = std::sync::Mutex::new(None);

pub async fn vector_index_state(a: &ArangoBackend) -> Value {
    let idx = a.dbcall("GET", &format!("/_api/index?collection={CHUNK_COLL}"), None).await.ok();
    let names: Vec<String> = idx
        .and_then(|v| v.get("indexes").and_then(|i| i.as_array()).cloned())
        .unwrap_or_default()
        .into_iter()
        .filter(|i| s(i, "type") == "vector")
        .map(|i| s(&i, "name").to_string())
        .collect();
    json!({"indexes": names, "note": VECTOR_NOTE.lock().map(|n| n.clone()).unwrap_or_default()})
}

async fn has_vector_indexes(a: &ArangoBackend) -> bool {
    vector_index_state(a).await["indexes"].as_array().map(|v| v.len() >= 2).unwrap_or(false)
}

/// Build the two vector indexes once the collection holds VECTOR_INDEX_MIN
/// chunks (an IVF index is trained on what is there: too few rows make a
/// poor one). Sparse, so chunks still waiting for a summary (vec_sum null)
/// are simply not in the summary index. A server started without
/// `--vector-index` refuses: noted once, retried every 10 minutes.
pub async fn ensure_vector_indexes(a: &ArangoBackend) -> Result<(), String> {
    if has_vector_indexes(a).await {
        return Ok(());
    }
    if let Ok(g) = VECTOR_RETRY_AT.lock() {
        if g.map(|t| t.elapsed() < std::time::Duration::from_secs(600)).unwrap_or(false) {
            return Ok(());
        }
    }
    let n = a
        .aql("RETURN LENGTH(rag_chunk)", json!({}))
        .await
        .map_err(|e| e.to_string())?
        .first()
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    if n < VECTOR_INDEX_MIN {
        return Ok(());
    }
    let n_lists = ((n as f64).sqrt() as u64).clamp(8, 1024);
    for field in ["vec_raw", "vec_sum"] {
        let body = json!({"type": "vector", "name": field, "fields": [field], "sparse": true, "inBackground": true,
                          "params": {"metric": "cosine", "dimension": embed::DIM, "nLists": n_lists}});
        if let Err(e) = a.dbcall("POST", &format!("/_api/index?collection={CHUNK_COLL}"), Some(&body)).await {
            let msg = format!("vector index {field} not built: {} — searches use exact cosine", e.msg);
            eprintln!("[iter_data] GraphRAG: {msg}");
            if let Ok(mut g) = VECTOR_NOTE.lock() {
                *g = msg.clone();
            }
            if let Ok(mut g) = VECTOR_RETRY_AT.lock() {
                *g = Some(std::time::Instant::now());
            }
            return Err(msg);
        }
    }
    if let Ok(mut g) = VECTOR_NOTE.lock() {
        g.clear();
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(keys: &[&str]) -> Vec<(String, f64)> {
        keys.iter().map(|k| (k.to_string(), 0.0)).collect()
    }

    #[test]
    fn the_guide_joins_every_search_unless_left_out() {
        let req = |kinds: &[&str], nts: &[&str], g: bool| SearchReq {
            query: "q".into(), k: 5, kinds: kinds.iter().map(|x| x.to_string()).collect(), nodetypes: nts.iter().map(|x| x.to_string()).collect(),
            doc: String::new(), mode: String::new(), vectors: String::new(), include_guide: g, per_doc: 2, graph: true, docs: false,
        };
        assert_eq!(scopes(&req(&[], &[], true), "p"), vec!["p", "_iter"]);
        assert_eq!(scopes(&req(&[], &[], false), "p"), vec!["p"]);
        assert_eq!(scopes(&req(&["file"], &[], true), "p"), vec!["p"]);
        assert_eq!(scopes(&req(&["guide"], &[], true), "p"), vec!["p", "_iter"]);
        assert_eq!(scopes(&req(&[], &["code"], true), "p"), vec!["p"]);
    }

    #[test]
    fn meaning_is_one_ranking_by_the_better_similarity() {
        let raw = vec![("a".to_string(), 0.5), ("b".to_string(), 0.4)];
        let sum = vec![("b".to_string(), 0.7), ("c".to_string(), 0.3)];
        let m = merge_by_best(&raw, &sum);
        assert_eq!(m.iter().map(|x| x.0.as_str()).collect::<Vec<_>>(), vec!["b", "a", "c"]);
        // keyword's first place is not outvoted by two correlated vector rankings
        let f = fuse(&[&m, &r(&["k", "c"])]);
        let pos = |key: &str| f.iter().position(|x| x.0 == key).unwrap();
        assert!(pos("k") < pos("a"), "{f:?}");
    }

    #[test]
    fn fusion_rewards_agreement_over_one_first_place() {
        // "b" is 2nd in all three; "a" is 1st in raw only
        let f = fuse(&[&r(&["a", "b"]), &r(&["c", "b"]), &r(&["d", "b"])]);
        assert_eq!(f[0].0, "b");
        assert_eq!(f[0].2, vec![Some(2), Some(2), Some(2)]);
        let a = f.iter().find(|x| x.0 == "a").unwrap();
        assert_eq!(a.2, vec![Some(1), None, None]);
        assert!(fuse(&[&[], &[], &[]]).is_empty());
    }
}
