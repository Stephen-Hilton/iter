//! The GraphRAG pipeline's server half: storing what engines prepare, and the
//! job protocol engines claim work through (iter_engine/src/rag.rs is the
//! other half).
//!
//! Jobs, in the order `work_claim` hands them out:
//! 1. **ingest** — an uploaded file waiting (`state: queued`): the engine gets
//!    the bytes, extracts (OCR through Claude for scans), chunks in model
//!    tokens, embeds, and PUTs `…/rag/docs/{id}/chunks`;
//! 2. **chunks** — up to JOB_MAX_CHUNKS chunk texts (any documents) for the
//!    Summary agent; the engine returns each summary with its vector;
//! 3. **rollup** — up to ROLLUP_MAX_DOCS documents whose chunks are all
//!    summarised: chapter + document summaries (document vector included).
//! Every claim is a filtered write, so of two racing engines one wins; a claim
//! lapses after CLAIM_SEC, so a dead engine strands nothing.

use super::*;
use std::collections::{HashMap, HashSet};

/// A claim older than this is free again (the claiming engine died).
pub const CLAIM_SEC: i64 = 600;
/// A chunk (or rollup, or ingest) that failed this many times stops retrying.
pub const MAX_ATTEMPTS: u64 = 3;
/// Most chunks / characters in one summary job (one LLM call).
pub const JOB_MAX_CHUNKS: usize = 8;
pub const JOB_MAX_CHARS: usize = 12_000;
/// A chapter's chunk summaries sent to a rollup at most (evenly sampled beyond it).
pub const CHAPTER_ROLLUP_CHARS: usize = 8_000;

/// Evenly spaced summaries whose text fits `budget` characters (all when they fit).
pub fn sample_to(items: Vec<Value>, budget: usize) -> Value {
    let len = |v: &Value| v["summary"].as_str().map(str::len).unwrap_or(0) + v["heading"].as_str().map(str::len).unwrap_or(0) + 8;
    let total: usize = items.iter().map(len).sum();
    if total <= budget || items.is_empty() {
        return Value::Array(items);
    }
    let keep = ((items.len() * budget) / total).max(1);
    let step = items.len() as f64 / keep as f64;
    Value::Array((0..keep).map(|i| items[((i as f64) * step) as usize].clone()).collect())
}

/// Most documents in one rollup job.
pub const ROLLUP_MAX_DOCS: usize = 4;

const CLAIMABLE_CHUNK: &str = "(c.sum_state == 'pending' OR (c.sum_state == 'claimed' AND c.sum_expires < @now))";
const CLAIMABLE_ROLLUP: &str = "d.state == 'rollup' AND (d.rollup_state == 'pending' OR (d.rollup_state == 'claimed' AND d.rollup_expires < @now))";
const CLAIMABLE_INGEST: &str = "(d.state == 'queued' OR (d.state == 'ingesting' AND d.ingest_expires < @now))";

// ---------- prepared documents (chunked + embedded by an engine) ----------

/// What an engine sends for one document: its chapters and chunks, each chunk
/// with its raw-text vector, and the stamp of the model that made them.
#[derive(serde::Deserialize, Default, Clone)]
pub struct Prepared {
    #[serde(default)]
    pub chapters: Vec<Value>,
    #[serde(default)]
    pub chunks: Vec<Value>,
    /// the embedder's stamp (`all-MiniLM-L6-v2@53aa51172d14`)
    #[serde(default)]
    pub model: String,
    #[serde(default)]
    pub format: String,
    #[serde(default)]
    pub chars: u64,
    #[serde(default)]
    pub pages: u64,
    /// the text came from OCR (a scanned PDF)
    #[serde(default)]
    pub ocr: bool,
}

fn vec_ok(v: &Value) -> bool {
    v.as_array().map(|a| a.len() == embed::DIM && a.iter().all(|x| x.is_number())).unwrap_or(false)
}

impl Prepared {
    fn check(&self) -> Result<(), String> {
        if self.chunks.is_empty() {
            return Err("no chunks: nothing to index".into());
        }
        for (i, c) in self.chunks.iter().enumerate() {
            if s(c, "text").trim().is_empty() {
                return Err(format!("chunk {i} has no text"));
            }
            if !vec_ok(&c["vec_raw"]) {
                return Err(format!("chunk {i}: vec_raw must be {} numbers", embed::DIM));
            }
        }
        Ok(())
    }
}

/// The document's identity and metadata (everything not in `Prepared`).
pub struct DocMeta<'a> {
    pub project: &'a str,
    pub id: String,
    pub kind: &'a str, // file | node
    pub title: String,
    pub path: String,
    pub hash: String,
    pub extra: Value, // node_id, nodetype, name, description / filename, size…
    pub by: &'a str,
}

/// Replace a document's chunks with a prepared set: summaries (and summary
/// vectors) of chunks whose text is unchanged are kept, so a one-line edit to
/// a 40-chunk file re-summarises one chunk, not forty. The document then waits
/// for the Summary agent (or goes straight to rollup when nothing changed).
pub async fn store_prepared(a: &ArangoBackend, d: DocMeta<'_>, p: &Prepared) -> Result<Value, ApiError> {
    p.check().map_err(bad)?;
    let old = a
        .aql(
            "FOR c IN rag_chunk FILTER c.project == @p AND c.doc == @d AND c.sum_state == 'done'
             RETURN {h: c.text_hash, summary: c.summary, vec_sum: c.vec_sum, sum_model: c.sum_model}",
            json!({"p": d.project, "d": d.id}),
        )
        .await
        .map_err(backend)?;
    let reuse: HashMap<String, Value> = old.into_iter().map(|o| (s(&o, "h").to_string(), o)).collect();
    let prev = a
        .aql("RETURN DOCUMENT(CONCAT('rag_doc/', @k))", json!({"k": d.id}))
        .await
        .map_err(backend)?
        .into_iter()
        .next()
        .filter(|v| !v.is_null());
    let now = now_utc();
    let nodetype = d.extra.get("nodetype").cloned().unwrap_or(Value::Null);
    let mut rows = Vec::with_capacity(p.chunks.len());
    let mut pending = 0;
    for (i, c) in p.chunks.iter().enumerate() {
        let text = s(c, "text").trim().to_string();
        let h = sha(&text);
        let kept = reuse.get(&h);
        if kept.is_none() {
            pending += 1;
        }
        rows.push(json!({
            "_key": chunk_key(&d.id, i), "project": d.project, "doc": d.id, "kind": d.kind, "nodetype": nodetype,
            "path": d.path, "title": d.title, "idx": i, "chapter": c["chapter"].as_u64().unwrap_or(0), "heading": s(c, "heading"),
            "text": text, "chars": text.len(), "tokens": c.get("tokens").cloned().unwrap_or(Value::Null), "text_hash": h,
            "vec_raw": c["vec_raw"], "vec_model": p.model, "created": now,
            "summary": kept.map(|k| k["summary"].clone()).unwrap_or(json!("")),
            // no vec_sum until a summary exists: the sparse vector index skips a
            // missing attribute but refuses a null one
            "vec_sum": kept.map(|k| k["vec_sum"].clone()).unwrap_or(Value::Null),
            "sum_model": kept.map(|k| k["sum_model"].clone()).unwrap_or(Value::Null),
            "sum_state": if kept.is_some() { "done" } else { "pending" },
            "sum_engine": "", "sum_expires": "", "sum_attempts": 0, "sum_error": "",
        }));
    }
    let state = if pending > 0 { "summarizing" } else { "rollup" };
    let chapters: Vec<Value> = p
        .chapters
        .iter()
        .map(|c| {
            let idx = c["idx"].as_u64().unwrap_or(0);
            json!({"idx": idx, "title": s(c, "title"), "chunks": p.chunks.iter().filter(|k| k["chapter"].as_u64().unwrap_or(0) == idx).count(), "summary": ""})
        })
        .collect();
    let mut doc = json!({
        "_key": d.id, "project": d.project, "kind": d.kind, "title": d.title, "path": d.path,
        "format": if p.format.is_empty() { "markdown".to_string() } else { p.format.clone() },
        "hash": d.hash, "chars": p.chars, "pages": p.pages, "ocr": p.ocr, "chunks": p.chunks.len(), "chapters": chapters,
        "state": state, "summary": "", "vec_sum": null, "vec_model": p.model,
        "rollup_state": if state == "rollup" { "pending" } else { "" }, "rollup_engine": "", "rollup_expires": "", "rollup_attempts": 0,
        "error": "", "updated": now, "by": d.by,
        "created": prev.as_ref().map(|x| s(x, "created").to_string()).filter(|c| !c.is_empty()).unwrap_or_else(|| now.clone()),
    });
    // an upload's own fields (title the person gave, store state, file facts) survive
    if let Some(prev) = &prev {
        for k in ["store", "filename", "size", "file_sha", "by"] {
            if let Some(v) = prev.get(k) {
                doc[k] = v.clone();
            }
        }
    }
    if let (Some(o), Some(x)) = (doc.as_object_mut(), d.extra.as_object()) {
        for (k, v) in x {
            o.insert(k.clone(), v.clone());
        }
    }
    let trx = a.begin(&[DOC_COLL, CHUNK_COLL]).await.map_err(backend)?;
    let res = async {
        a.aql_in(&trx, "FOR c IN rag_chunk FILTER c.project == @p AND c.doc == @d REMOVE c IN rag_chunk", json!({"p": d.project, "d": d.id})).await?;
        a.aql_in(&trx, "FOR r IN @rows INSERT r INTO rag_chunk OPTIONS {keepNull: false}", json!({"rows": rows})).await?;
        a.aql_in(&trx, "UPSERT {_key: @k} INSERT @doc REPLACE @doc IN rag_doc", json!({"k": d.id, "doc": doc})).await
    }
    .await;
    match res {
        Ok(_) => a.commit(&trx).await.map_err(backend)?,
        Err(e) => {
            a.abort(&trx).await;
            return Err(backend(e));
        }
    }
    maybe_single_chunk_rollup(a, d.project, &d.id).await?;
    let _ = search::ensure_vector_indexes(a).await;
    Ok(clean(doc))
}

/// A one-chunk document's summary is its chunk's summary: finish the rollup here.
async fn maybe_single_chunk_rollup(a: &ArangoBackend, project: &str, doc: &str) -> Result<(), ApiError> {
    a.aql_retry(
        "LET d = DOCUMENT(CONCAT('rag_doc/', @d))
         FILTER d != null AND d.chunks == 1 AND d.state == 'rollup'
         LET c = FIRST(FOR c IN rag_chunk FILTER c.project == @p AND c.doc == @d RETURN c)
         FILTER c != null AND c.sum_state == 'done'
         UPDATE d WITH {state: 'ready', rollup_state: 'done', summary: c.summary, vec_sum: c.vec_sum,
                        chapters: [MERGE(d.chapters[0], {summary: c.summary})], updated: @now} IN rag_doc",
        json!({"p": project, "d": doc, "now": now_utc()}),
    )
    .await
    .map_err(backend)?;
    Ok(())
}

#[derive(serde::Deserialize)]
pub struct ChunksPutReq {
    engine: String,
    job_id: String,
    #[serde(flatten)]
    prepared: Prepared,
}

/// An engine's ingest result for an uploaded file it claimed.
pub async fn doc_chunks_put(u: AuthUser, State(st): Ctx, Path((name, id)): Path<(String, String)>, Json(req): Json<ChunksPutReq>) -> Result<Json<Value>, ApiError> {
    u.require_writer()?;
    let a = arango(st.store.as_ref())?;
    let doc = a
        .aql(
            "LET d = DOCUMENT(CONCAT('rag_doc/', @k)) FILTER d != null AND d.project == @p AND d.state == 'ingesting'
               AND d.ingest_engine == @e AND d.ingest_job == @j RETURN d",
            json!({"k": id, "p": name, "e": req.engine, "j": req.job_id}),
        )
        .await
        .map_err(backend)?
        .into_iter()
        .next()
        .ok_or_else(|| conflict(format!("document {id} is not being ingested by {} (job {})", req.engine, req.job_id)))?;
    let text_hash = req.prepared.chunks.iter().map(|c| sha(s(c, "text"))).collect::<Vec<_>>().join("");
    let meta = DocMeta {
        project: &name, id: id.clone(), kind: "file", title: s(&doc, "title").to_string(), path: s(&doc, "path").to_string(),
        hash: sha(&text_hash), extra: json!({}), by: s(&doc, "by"),
    };
    let out = store_prepared(a, meta, &req.prepared).await?;
    a.aql_retry("REMOVE {_key: @k} IN rag_blob OPTIONS {ignoreErrors: true}", json!({"k": id})).await.map_err(backend)?;
    st.store.bump_seq(&name, "rag").await?;
    Ok(Json(out))
}

// ---------- node documents (engine: `iter rag sync`) ----------

/// {path: hash} of every node document, so the engine sends only changed files.
pub async fn node_hashes(_u: AuthUser, State(st): Ctx, Path(name): Path<String>) -> Result<Json<Value>, ApiError> {
    let a = arango(st.store.as_ref())?;
    let rows = a
        .aql("FOR d IN rag_doc FILTER d.project == @p AND d.kind == 'node' RETURN [d.path, d.hash]", json!({"p": name}))
        .await
        .map_err(backend)?;
    let m: serde_json::Map<String, Value> = rows
        .into_iter()
        .filter_map(|r| Some((r.get(0)?.as_str()?.to_string(), r.get(1).cloned().unwrap_or(Value::Null))))
        .collect();
    Ok(Json(Value::Object(m)))
}

#[derive(serde::Deserialize)]
pub struct NodesReq {
    /// changed or new node files, chunked and embedded by the engine:
    /// {path, node_id, nodetype, name, description, hash, chapters, chunks, model, chars}
    #[serde(default)]
    nodes: Vec<Value>,
    /// when present: every node path in the checkout now; node documents not
    /// listed are removed (the file was deleted or renamed)
    #[serde(default)]
    keep: Option<Vec<String>>,
}

pub async fn nodes_put(u: AuthUser, State(st): Ctx, Path(name): Path<String>, Json(req): Json<NodesReq>) -> Result<Json<Value>, ApiError> {
    u.require_writer()?;
    let a = arango(st.store.as_ref())?;
    let existing: HashMap<String, String> = a
        .aql("FOR d IN rag_doc FILTER d.project == @p AND d.kind == 'node' RETURN [d.path, d.hash]", json!({"p": name}))
        .await
        .map_err(backend)?
        .into_iter()
        .filter_map(|r| Some((r.get(0)?.as_str()?.to_string(), r.get(1)?.as_str().unwrap_or("").to_string())))
        .collect();
    let (mut added, mut changed, mut unchanged, mut failed) = (0, 0, 0, Vec::<Value>::new());
    for n in &req.nodes {
        let path = s(n, "path").to_string();
        if !path.ends_with(".iter.md") {
            failed.push(json!({"path": path, "error": "not a *.iter.md node file"}));
            continue;
        }
        let hash = s(n, "hash").to_string();
        match existing.get(&path) {
            Some(h) if *h == hash && !hash.is_empty() => {
                unchanged += 1;
                continue;
            }
            Some(_) => changed += 1,
            None => added += 1,
        }
        let prepared: Prepared = match serde_json::from_value(n.clone()) {
            Ok(p) => p,
            Err(e) => {
                failed.push(json!({"path": path, "error": format!("prepared chunks: {e}")}));
                continue;
            }
        };
        let nodetype = s(n, "nodetype");
        let title = if s(n, "name").is_empty() { path.rsplit('/').next().unwrap_or(&path).to_string() } else { s(n, "name").to_string() };
        let extra = json!({"node_id": s(n, "node_id"), "nodetype": nodetype, "name": s(n, "name"), "description": s(n, "description")});
        let meta = DocMeta { project: &name, id: node_doc_id(&name, &path), kind: "node", title, path: path.clone(), hash, extra, by: &u.sub };
        if let Err(e) = store_prepared(a, meta, &prepared).await {
            let msg = match e {
                ApiError::Status(_, m) => m,
                _ => "ingest failed".into(),
            };
            failed.push(json!({"path": path, "error": msg}));
        }
    }
    let mut removed = 0;
    if let Some(keep) = &req.keep {
        let keep: HashSet<&str> = keep.iter().map(String::as_str).collect();
        let gone: Vec<String> = existing.keys().filter(|p| !keep.contains(p.as_str())).map(|p| node_doc_id(&name, p)).collect();
        removed = gone.len();
        remove_docs(a, &name, &gone).await?;
    }
    if added + changed + removed > 0 {
        st.store.bump_seq(&name, "rag").await?;
    }
    Ok(Json(json!({"added": added, "changed": changed, "unchanged": unchanged, "removed": removed, "failed": failed})))
}

// ---------- repo files (engine: `iter rag sync`, setting repo_globs) ----------

/// A repo file's document key: stable per (project, path).
pub(crate) fn repo_doc_id(project: &str, path: &str) -> String {
    format!("r{}", &sha(&format!("{project}\u{0}repo\u{0}{path}"))[..31])
}

/// {path: hash} of every repo-file document.
pub async fn file_hashes(_u: AuthUser, State(st): Ctx, Path(name): Path<String>) -> Result<Json<Value>, ApiError> {
    let a = arango(st.store.as_ref())?;
    let rows = a
        .aql("FOR d IN rag_doc FILTER d.project == @p AND d.kind == 'file' AND d.source == 'repo' RETURN [d.path, d.hash]", json!({"p": name}))
        .await
        .map_err(backend)?;
    Ok(Json(Value::Object(rows.into_iter().filter_map(|r| Some((r.get(0)?.as_str()?.to_string(), r.get(1).cloned().unwrap_or(Value::Null)))).collect())))
}

#[derive(serde::Deserialize)]
pub struct FilesReq {
    /// changed or new files, chunked and embedded by the engine:
    /// {path, title, hash, format, chapters, chunks, model, chars, pages}
    #[serde(default)]
    files: Vec<Value>,
    /// when present: every path the repo_globs match now; repo-file documents
    /// not listed are removed
    #[serde(default)]
    keep: Option<Vec<String>>,
}

/// Files of the checkout indexed where they live (setting `repo_globs`):
/// kind `file`, `source: "repo"` — searchable like uploads, but never copied,
/// stored or committed anywhere.
pub async fn files_put(u: AuthUser, State(st): Ctx, Path(name): Path<String>, Json(req): Json<FilesReq>) -> Result<Json<Value>, ApiError> {
    u.require_writer()?;
    let a = arango(st.store.as_ref())?;
    let existing: HashMap<String, String> = a
        .aql("FOR d IN rag_doc FILTER d.project == @p AND d.kind == 'file' AND d.source == 'repo' RETURN [d.path, d.hash]", json!({"p": name}))
        .await
        .map_err(backend)?
        .into_iter()
        .filter_map(|r| Some((r.get(0)?.as_str()?.to_string(), r.get(1)?.as_str().unwrap_or("").to_string())))
        .collect();
    let (mut added, mut changed, mut unchanged, mut failed) = (0, 0, 0, Vec::<Value>::new());
    for f in &req.files {
        let path = s(f, "path").to_string();
        if !path.starts_with("{topdir}/") || path.split('/').any(|x| x == "..") {
            failed.push(json!({"path": path, "error": "a repo file's path is {topdir}/…"}));
            continue;
        }
        let hash = s(f, "hash").to_string();
        match existing.get(&path) {
            Some(h) if *h == hash && !hash.is_empty() => {
                unchanged += 1;
                continue;
            }
            Some(_) => changed += 1,
            None => added += 1,
        }
        let prepared: Prepared = match serde_json::from_value(f.clone()) {
            Ok(p) => p,
            Err(e) => {
                failed.push(json!({"path": path, "error": format!("prepared chunks: {e}")}));
                continue;
            }
        };
        let title = if s(f, "title").is_empty() { path.rsplit('/').next().unwrap_or(&path).to_string() } else { s(f, "title").to_string() };
        let extra = json!({"source": "repo", "filename": path.rsplit('/').next().unwrap_or("")});
        let meta = DocMeta { project: &name, id: repo_doc_id(&name, &path), kind: "file", title, path: path.clone(), hash, extra, by: &u.sub };
        if let Err(e) = store_prepared(a, meta, &prepared).await {
            failed.push(json!({"path": path, "error": match e { ApiError::Status(_, m) => m, _ => "ingest failed".into() }}));
        }
    }
    let mut removed = 0;
    if let Some(keep) = &req.keep {
        let keep: HashSet<&str> = keep.iter().map(String::as_str).collect();
        let gone: Vec<String> = existing.keys().filter(|p| !keep.contains(p.as_str())).map(|p| repo_doc_id(&name, p)).collect();
        removed = gone.len();
        remove_docs(a, &name, &gone).await?;
    }
    if added + changed + removed > 0 {
        st.store.bump_seq(&name, "rag").await?;
    }
    Ok(Json(json!({"added": added, "changed": changed, "unchanged": unchanged, "removed": removed, "failed": failed})))
}

// ---------- the job protocol ----------

/// Work waiting for engines serving `project`: its own, plus the product-wide
/// guide's (any engine may take that).
pub async fn waiting_one(a: &ArangoBackend, project: &str) -> Result<u64, ApiError> {
    let own = waiting_scope(a, project).await?;
    Ok(own + if project == guide::SCOPE { 0 } else { waiting_scope(a, guide::SCOPE).await.unwrap_or(0) })
}

async fn waiting_scope(a: &ArangoBackend, project: &str) -> Result<u64, ApiError> {
    let q = format!(
        "RETURN LENGTH(FOR c IN rag_chunk FILTER c.project == @p AND {CLAIMABLE_CHUNK} RETURN 1)
              + LENGTH(FOR d IN rag_doc FILTER d.project == @p AND ({CLAIMABLE_ROLLUP} OR {CLAIMABLE_INGEST}) RETURN 1)"
    );
    let r = a.aql(&q, json!({"p": project, "now": now_utc()})).await.map_err(backend)?;
    Ok(r.first().and_then(|v| v.as_u64()).unwrap_or(0))
}

/// Work waiting per project (heartbeat reply `rag_waiting`).
pub async fn waiting(store: &dyn Storage, projects: &[String]) -> HashMap<String, u64> {
    let mut out = HashMap::new();
    let Some(a) = store.arango() else { return out };
    for p in projects {
        if let Ok(n) = waiting_one(a, p).await {
            if n > 0 {
                out.insert(p.clone(), n);
            }
        }
    }
    out
}

#[derive(serde::Deserialize)]
pub struct ClaimReq {
    engine: String,
    #[serde(default)]
    max_chunks: Option<usize>,
    /// which kinds this engine takes (default all): ingest, chunks, rollup
    #[serde(default)]
    kinds: Vec<String>,
}

async fn doc_brief(a: &ArangoBackend, id: &str) -> Result<Value, ApiError> {
    a.aql(
        "LET d = DOCUMENT(CONCAT('rag_doc/', @d)) RETURN {id: d._key, title: d.title, kind: d.kind, path: d.path, nodetype: d.nodetype,
                                                          chunks: d.chunks, chapters: LENGTH(d.chapters), filename: d.filename, format: d.format}",
        json!({"d": id}),
    )
    .await
    .map_err(backend)
    .map(|r| r.into_iter().next().unwrap_or(Value::Null))
}

/// One job for one engine: an ingest, else a batch of chunks to summarise,
/// else a batch of rollups, else `{"job": null}`.
pub async fn work_claim(u: AuthUser, State(st): Ctx, Path(name): Path<String>, Json(req): Json<ClaimReq>) -> Result<Json<Value>, ApiError> {
    u.require_writer()?;
    let a = arango(st.store.as_ref())?;
    let job = claim_in(a, &name, &req).await?;
    if !job["job"].is_null() || name == guide::SCOPE {
        return Ok(Json(job));
    }
    // nothing of the project's own: the product-wide guide's work, if any
    let mut job = claim_in(a, guide::SCOPE, &req).await?;
    if !job["job"].is_null() {
        job["scope"] = json!(guide::SCOPE);
    }
    Ok(Json(job))
}

async fn claim_in(a: &ArangoBackend, name: &str, req: &ClaimReq) -> Result<Value, ApiError> {
    let wants = |k: &str| req.kinds.is_empty() || req.kinds.iter().any(|x| x == k);
    let job = format!("j{}", &uuid::Uuid::new_v4().simple().to_string()[..16]);
    if wants("ingest") {
        let q = format!(
            "FOR d IN rag_doc FILTER d.project == @p AND {CLAIMABLE_INGEST} AND d.ingest_attempts < @max SORT d.updated LIMIT 1
             UPDATE d WITH {{state: 'ingesting', ingest_engine: @e, ingest_expires: @exp, ingest_job: @job, ingest_attempts: d.ingest_attempts + 1}} IN rag_doc
             RETURN NEW._key"
        );
        let bind = json!({"p": name, "now": now_utc(), "e": req.engine, "exp": iso_in(CLAIM_SEC), "job": job, "max": MAX_ATTEMPTS});
        if let Some(id) = a.aql_retry(&q, bind).await.map_err(backend)?.first().and_then(|d| d.as_str()).map(String::from) {
            let blob = a
                .aql("RETURN DOCUMENT(CONCAT('rag_blob/', @k))", json!({"k": id}))
                .await
                .map_err(backend)?
                .into_iter()
                .next()
                .filter(|b| !b.is_null());
            let Some(blob) = blob else {
                a.aql_retry("UPDATE {_key: @k} WITH {state: 'failed', error: 'the uploaded bytes are gone; upload the file again'} IN rag_doc", json!({"k": id}))
                    .await
                    .map_err(backend)?;
                return Ok(json!({"job": null}));
            };
            let doc = doc_brief(a, &id).await?;
            return Ok(json!({"job": "ingest", "job_id": job, "doc": doc, "filename": blob["filename"], "content_b64": blob["content_b64"]}));
        }
    }
    // rollups before more chunks: a document whose chunks are all summarised
    // becomes "ready" at once instead of after every other chunk in the queue
    // (2026-09-30: 211 documents sat at "rollup" behind 35k pending chunks)
    if wants("rollup") {
        let q = format!(
            "FOR d IN rag_doc FILTER d.project == @p AND {CLAIMABLE_ROLLUP} SORT d.updated LIMIT @n
             UPDATE d WITH {{rollup_state: 'claimed', rollup_engine: @e, rollup_job: @job, rollup_attempts: d.rollup_attempts + 1,
                             // a big document's rollup is many model calls (chapters in windows, then the whole): its claim lasts longer
                             rollup_expires: DATE_FORMAT(DATE_ADD(DATE_NOW(), @base + FLOOR(d.chunks / 2), 's'), '%yyyy-%mm-%ddT%hh:%ii:%ssZ')}} IN rag_doc
             RETURN NEW._key"
        );
        let bind = json!({"p": name, "now": now_utc(), "e": req.engine, "base": CLAIM_SEC, "job": job, "n": ROLLUP_MAX_DOCS});
        let ids: Vec<String> = a.aql_retry(&q, bind).await.map_err(backend)?.into_iter().filter_map(|d| d.as_str().map(String::from)).collect();
        if !ids.is_empty() {
            let mut docs = Vec::new();
            for id in &ids {
                let mut doc = doc_brief(a, id).await?;
                doc["chapters"] = Value::Array(
                    a.aql(
                        "LET d = DOCUMENT(CONCAT('rag_doc/', @d))
                         FOR ch IN d.chapters
                           RETURN {idx: ch.idx, title: ch.title,
                                   summaries: (FOR c IN rag_chunk FILTER c.project == d.project AND c.doc == d._key AND c.chapter == ch.idx
                                               SORT c.idx RETURN {heading: c.heading, summary: c.summary != '' ? c.summary : LEFT(c.text, 600)})}",
                        json!({"d": id}),
                    )
                    .await
                    .map_err(backend)?
                    .into_iter()
                    .map(|mut ch| {
                        ch["summaries"] = sample_to(ch["summaries"].as_array().cloned().unwrap_or_default(), CHAPTER_ROLLUP_CHARS);
                        ch
                    })
                    .collect(),
                );
                docs.push(doc);
            }
            return Ok(json!({"job": "rollup", "job_id": job, "docs": docs}));
        }
    }
    let now = now_utc();
    let max = req.max_chunks.unwrap_or(JOB_MAX_CHUNKS).clamp(1, 32);
    if wants("chunks") {
        // oldest first, in reading order, across documents, up to max chunks /
        // JOB_MAX_CHARS (the first chunk always fits) — one model call covers
        // several small node files
        // smallest documents first (fewest chunks still waiting), each in
        // reading order: the most documents reach "ready" soonest, and one huge
        // file no longer holds every small one behind it (2026-09-30: an
        // 11,530-chunk techreq held 864 small documents at 0/N for hours)
        let q = format!(
            "LET docs = (FOR c IN rag_chunk FILTER c.project == @p AND {CLAIMABLE_CHUNK}
                         COLLECT doc = c.doc WITH COUNT INTO n SORT n, doc LIMIT @max RETURN doc)
             LET picked = (FOR c IN rag_chunk FILTER c.project == @p AND c.doc IN docs AND {CLAIMABLE_CHUNK}
                           SORT POSITION(docs, c.doc, true), c.idx LIMIT @max RETURN {{k: c._key, n: c.chars}})
             FOR i IN 0..(LENGTH(picked) - 1)
               FILTER LENGTH(picked) > 0
               LET before = SUM(SLICE(picked, 0, i)[*].n)
               FILTER i == 0 OR before + picked[i].n <= @maxchars
               LET c = DOCUMENT(CONCAT('rag_chunk/', picked[i].k))
               UPDATE c WITH {{sum_state: 'claimed', sum_engine: @e, sum_expires: @exp, sum_job: @job, sum_attempts: c.sum_attempts + 1}} IN rag_chunk
               RETURN {{id: NEW._key, doc: NEW.doc, idx: NEW.idx, chapter: NEW.chapter, heading: NEW.heading, text: NEW.text,
                        title: NEW.title, path: NEW.path, kind: NEW.kind, nodetype: NEW.nodetype}}"
        );
        // ~1500 characters per requested chunk (a bigger batch = fewer model calls)
        let maxchars = JOB_MAX_CHARS.max(max * 1500);
        let bind = json!({"p": name, "now": now, "max": max, "maxchars": maxchars, "e": req.engine, "exp": iso_in(CLAIM_SEC), "job": job});
        let chunks = a.aql_retry(&q, bind).await.map_err(backend)?;
        if let Some(first) = chunks.first() {
            let doc = doc_brief(a, s(first, "doc")).await?;
            return Ok(json!({"job": "chunks", "job_id": job, "doc": doc, "chunks": chunks}));
        }
    }
    Ok(json!({"job": null}))
}

#[derive(serde::Deserialize)]
pub struct DoneReq {
    engine: String,
    job_id: String,
    /// ingest (failures only: a success is the chunks PUT) | chunks | rollup
    job: String,
    /// chunks: [{id, summary, vec_sum}] — a chunk missing here (or with an empty summary) failed
    #[serde(default)]
    results: Vec<Value>,
    /// rollup: one entry per document [{doc, doc_summary, vec_sum, chapters: [{idx, summary}]}]
    #[serde(default)]
    docs: Vec<Value>,
    /// ingest: the document; rollup of one document (older engines)
    #[serde(default)]
    doc: String,
    #[serde(default)]
    doc_summary: String,
    #[serde(default)]
    chapters: Vec<Value>,
    /// the whole job failed (extraction, model error, unparsable answer)
    #[serde(default)]
    error: String,
    /// the summarising model (haiku…)
    #[serde(default)]
    model: String,
    /// the embedder's stamp for the vectors sent
    #[serde(default)]
    embed_model: String,
    /// `_iter` for product-wide work (the user guide) claimed through a project
    #[serde(default)]
    scope: String,
}

pub async fn work_done(u: AuthUser, State(st): Ctx, Path(name): Path<String>, Json(req): Json<DoneReq>) -> Result<Json<Value>, ApiError> {
    u.require_writer()?;
    let a = arango(st.store.as_ref())?;
    let name = if req.scope == guide::SCOPE { guide::SCOPE.to_string() } else { name };
    let out = match req.job.as_str() {
        "ingest" => ingest_failed(a, &name, &req).await?,
        "chunks" => chunks_done(a, &name, &req).await?,
        "rollup" => rollup_done(a, &name, &req).await?,
        other => return Err(bad(format!("job must be ingest | chunks | rollup (got {other:?})"))),
    };
    st.store.bump_seq(&name, "rag").await?;
    Ok(Json(out))
}

async fn ingest_failed(a: &ArangoBackend, project: &str, req: &DoneReq) -> Result<Value, ApiError> {
    let r = a
        .aql_retry(
            "FOR d IN rag_doc FILTER d._key == @d AND d.project == @p AND d.state == 'ingesting' AND d.ingest_engine == @e AND d.ingest_job == @j
             LET give_up = d.ingest_attempts >= @max OR @permanent
             UPDATE d WITH {state: give_up ? 'failed' : 'queued', ingest_engine: '', ingest_expires: '', error: @err, updated: @now} IN rag_doc
             RETURN NEW.state",
            json!({"d": req.doc, "p": project, "e": req.engine, "j": req.job_id, "max": MAX_ATTEMPTS, "now": now_utc(),
                   "err": if req.error.is_empty() { "ingest failed" } else { &req.error },
                   // the file itself is unreadable: retrying cannot help
                   "permanent": req.error.contains("no text could be extracted") || req.error.contains("not a supported")}),
        )
        .await
        .map_err(backend)?;
    match r.first().and_then(|v| v.as_str()) {
        Some(st) => Ok(json!({"doc": req.doc, "state": st})),
        None => Err(conflict(format!("document {} is not being ingested by {} (job {})", req.doc, req.engine, req.job_id))),
    }
}

/// A vector from the engine, or (an engine too old to embed) one made here.
async fn vector_or_embed(given: &Value, text: String) -> Result<Value, ApiError> {
    if vec_ok(given) {
        return Ok(given.clone());
    }
    let v = embed_async(vec![text]).await.map_err(unavailable)?;
    Ok(json!(v.into_iter().next().unwrap_or_default()))
}

async fn chunks_done(a: &ArangoBackend, project: &str, req: &DoneReq) -> Result<Value, ApiError> {
    let mine = a
        .aql(
            "FOR c IN rag_chunk FILTER c.project == @p AND c.sum_job == @j AND c.sum_state == 'claimed' AND c.sum_engine == @e
             RETURN {id: c._key, doc: c.doc, title: c.title, heading: c.heading, attempts: c.sum_attempts}",
            json!({"p": project, "j": req.job_id, "e": req.engine}),
        )
        .await
        .map_err(backend)?;
    if mine.is_empty() {
        return Err(conflict(format!("job {} holds no chunks claimed by {} (lapsed and re-claimed?)", req.job_id, req.engine)));
    }
    let got: HashMap<&str, &Value> = req
        .results
        .iter()
        .filter_map(|r| Some((r.get("id")?.as_str()?, r)))
        .filter(|(_, r)| !s(r, "summary").trim().is_empty())
        .collect();
    let stamp = if req.embed_model.is_empty() { embed::disk_stamp().unwrap_or_default() } else { req.embed_model.clone() };
    let mut updates = Vec::new();
    for c in mine.iter().filter(|c| got.contains_key(s(c, "id"))) {
        let r = got[s(c, "id")];
        let sm = s(r, "summary").trim();
        let input = if s(c, "heading").is_empty() { format!("{} — {sm}", s(c, "title")) } else { format!("{} — {}: {sm}", s(c, "title"), s(c, "heading")) };
        let v = vector_or_embed(&r["vec_sum"], input).await?;
        updates.push(json!({"_key": s(c, "id"), "summary": sm, "vec_sum": v, "sum_state": "done", "sum_error": "", "sum_by": req.model,
                            "sum_model": stamp, "sum_expires": ""}));
    }
    let failed: Vec<Value> = mine
        .iter()
        .filter(|c| !got.contains_key(s(c, "id")))
        .map(|c| {
            let give_up = c["attempts"].as_u64().unwrap_or(0) >= MAX_ATTEMPTS;
            json!({"_key": s(c, "id"), "sum_state": if give_up { "failed" } else { "pending" }, "sum_expires": "", "sum_engine": "",
                   "sum_error": if req.error.is_empty() { "no summary returned for this chunk".to_string() } else { req.error.clone() }})
        })
        .collect();
    a.aql_retry("FOR u IN @rows UPDATE u IN rag_chunk", json!({"rows": updates.iter().chain(failed.iter()).collect::<Vec<_>>()}))
        .await
        .map_err(backend)?;
    // a document whose chunks are all settled moves on to its rollup
    let docs: HashSet<&str> = mine.iter().map(|c| s(c, "doc")).collect();
    for d in docs {
        a.aql_retry(
            "LET open = LENGTH(FOR c IN rag_chunk FILTER c.project == @p AND c.doc == @d AND c.sum_state IN ['pending', 'claimed'] RETURN 1)
             FOR d IN rag_doc FILTER d._key == @d AND d.state == 'summarizing' AND open == 0
             UPDATE d WITH {state: 'rollup', rollup_state: 'pending', updated: @now} IN rag_doc",
            json!({"p": project, "d": d, "now": now_utc()}),
        )
        .await
        .map_err(backend)?;
        maybe_single_chunk_rollup(a, project, d).await?;
    }
    Ok(json!({"summarized": updates.len(), "failed": failed.len()}))
}

async fn rollup_done(a: &ArangoBackend, project: &str, req: &DoneReq) -> Result<Value, ApiError> {
    let mut entries: Vec<Value> = req.docs.clone();
    if entries.is_empty() && !req.doc.is_empty() {
        entries.push(json!({"doc": req.doc, "doc_summary": req.doc_summary, "chapters": req.chapters}));
    }
    let claimed: Vec<String> = a
        .aql(
            "FOR d IN rag_doc FILTER d.project == @p AND d.rollup_job == @j AND d.rollup_engine == @e AND d.rollup_state == 'claimed' RETURN d._key",
            json!({"p": project, "j": req.job_id, "e": req.engine}),
        )
        .await
        .map_err(backend)?
        .into_iter()
        .filter_map(|v| v.as_str().map(String::from))
        .collect();
    if claimed.is_empty() {
        return Err(conflict(format!("rollup {} holds no documents claimed by {}", req.job_id, req.engine)));
    }
    // documents of this job the answer left out fail like an empty summary
    for id in &claimed {
        if !entries.iter().any(|e| s(e, "doc") == id) {
            entries.push(json!({"doc": id, "doc_summary": ""}));
        }
    }
    let mut out = Vec::new();
    for e in entries.iter().filter(|e| claimed.iter().any(|c| c == s(e, "doc"))) {
        out.push(rollup_one(a, project, req, e).await?);
    }
    Ok(json!({"rollups": out}))
}

async fn rollup_one(a: &ArangoBackend, project: &str, req: &DoneReq, e: &Value) -> Result<Value, ApiError> {
    let doc_id = s(e, "doc");
    let summary = s(e, "doc_summary").trim();
    let doc = a
        .aql("LET d = DOCUMENT(CONCAT('rag_doc/', @d)) FILTER d != null AND d.project == @p RETURN d", json!({"d": doc_id, "p": project}))
        .await
        .map_err(backend)?
        .into_iter()
        .next()
        .unwrap_or(Value::Null);
    if summary.is_empty() {
        let give_up = doc["rollup_attempts"].as_u64().unwrap_or(0) >= MAX_ATTEMPTS;
        a.aql_retry(
            "UPDATE {_key: @d} WITH {rollup_state: @st, state: @ds, rollup_expires: '', rollup_engine: '', error: @err} IN rag_doc",
            json!({"d": doc_id, "st": if give_up { "failed" } else { "pending" }, "ds": if give_up { "ready" } else { "rollup" },
                   "err": if req.error.is_empty() { "no document summary returned" } else { &req.error }}),
        )
        .await
        .map_err(backend)?;
        return Ok(json!({"doc": doc_id, "rollup": "failed", "gave_up": give_up}));
    }
    let v = vector_or_embed(&e["vec_sum"], format!("{} — {summary}", s(&doc, "title"))).await?;
    let by_idx: HashMap<u64, &str> = e["chapters"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|c| Some((c.get("idx")?.as_u64()?, c.get("summary")?.as_str()?.trim())))
        .collect();
    let chapters: Vec<Value> = doc["chapters"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .map(|mut c| {
            if let Some(sm) = by_idx.get(&c["idx"].as_u64().unwrap_or(0)) {
                c["summary"] = json!(sm);
            }
            c
        })
        .collect();
    a.aql_retry(
        "UPDATE {_key: @d} WITH {state: 'ready', rollup_state: 'done', rollup_expires: '', summary: @s, vec_sum: @v, chapters: @ch,
                                 summary_model: @m, error: '', updated: @now} IN rag_doc OPTIONS {mergeObjects: false}",
        json!({"d": doc_id, "s": summary, "v": v, "ch": chapters, "m": req.model, "now": now_utc()}),
    )
    .await
    .map_err(backend)?;
    Ok(json!({"doc": doc_id, "rollup": "done"}))
}
