//! GraphRAG (iter4, 2026-09-29): retrieval over a project's documents, tied
//! to its architecture map.
//!
//! Two kinds of document, both per project:
//! - **file**: an arbitrary upload (pdf, docx, pptx, html, md, txt…). iter_data
//!   keeps the bytes until an engine ingests them, and hands the original to
//!   an engine through datasync (`store_doc`) to be written — and committed
//!   unless the docs directory is git-ignored — under the project's docs
//!   directory (setting `docs_dir`, default `{topdir}/docs/`);
//! - **node**: a `*.iter.md` file of the map (code, bizreq, techreq, tests,
//!   usecase, actor…), pushed by the engine's `iter rag sync`.
//!
//! Map nodes name the uploaded documents that describe them in
//! `children.documents`; a hit on either side carries the other (the "graph"
//! in GraphRAG: node hits bring their neighbours and linked documents, file
//! hits bring the nodes that link them).
//!
//! Who does what (decided 2026-09-29): the **engine** does the heavy work —
//! extraction (OCR of scanned PDFs through Claude), chunking in the model's
//! own tokens, every chunk and summary vector, and the Summary agent — on its
//! native hardware. **iter_data** stores, orchestrates, and embeds only
//! search questions (same model; every vector carries the model's stamp).
//!
//! Pipeline, per document (engine jobs, claimed through `…/rag/work/claim`
//! when the heartbeat reply's `rag_waiting` says there is work):
//! 1. **ingest** (uploads): the engine extracts, chunks, embeds the raw text
//!    and PUTs the chunks (`…/rag/docs/{id}/chunks`); node files arrive
//!    already chunked and embedded through `PUT …/rag/nodes`;
//! 2. **chunks**: the Summary agent summarises a batch; the engine embeds each
//!    summary as the chunk's second vector;
//! 3. **rollup**: chapter + document summaries, the document summary embedded.
//!
//! GraphRAG needs a live engine with an LLM account (Stephen, 2026-09-29).
//! Search (search.rs) fuses three rankings: keyword (ArangoSearch BM25 over
//! chunk text, summary, heading and title), raw-text vectors and summary
//! vectors, and returns FULL chunks.
//!
//! Collections: `rag_doc`, `rag_chunk`, `rag_blob`, `rag_setting`; view `rag_view`.

pub mod guide;
pub mod pipeline;
pub mod search;

pub use iter_rag::{embed, extract};

use crate::api::{ApiError, AppState, AuthUser};
use crate::arango::ArangoBackend;
use crate::storage::Storage;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::routing::{get, post, put};
use axum::{Json, Router};
use iter_core::now_utc;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::sync::Arc;

pub(crate) type Ctx = State<Arc<AppState>>;

pub const DOC_COLL: &str = "rag_doc";
pub const CHUNK_COLL: &str = "rag_chunk";
pub const BLOB_COLL: &str = "rag_blob";
pub const SETTING_COLL: &str = "rag_setting";
pub const VIEW: &str = "rag_view";
/// Largest upload accepted.
pub const MAX_UPLOAD: usize = 25 * 1024 * 1024;
pub const DEFAULT_DOCS_DIR: &str = "{topdir}/docs/";
/// The scheduled RAG change sweep's tag (one open schedule per project).
pub const SWEEP_TAG: &str = "rag-sweep";
/// File types the engine can read.
pub const ACCEPTED: &[&str] = &["pdf", "docx", "pptx", "html", "htm", "md", "markdown", "txt", "csv", "json", "yaml", "yml", "rst", "adoc", "log", "xml", "toml"];

pub fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/api/projects/{name}/rag", get(status))
        .route("/api/projects/{name}/rag/settings", get(settings_get).put(settings_put))
        .route("/api/projects/{name}/rag/docs", get(docs_list).post(doc_upload))
        .route("/api/projects/{name}/rag/docs/{id}", get(doc_get).delete(doc_delete))
        .route("/api/projects/{name}/rag/docs/{id}/resummarize", post(doc_resummarize))
        .route("/api/projects/{name}/rag/docs/{id}/links", post(doc_link))
        .route("/api/projects/{name}/rag/docs/{id}/chunks", put(pipeline::doc_chunks_put))
        .route("/api/projects/{name}/rag/nodes", put(pipeline::nodes_put))
        .route("/api/projects/{name}/rag/nodes/hashes", get(pipeline::node_hashes))
        .route("/api/projects/{name}/rag/files", put(pipeline::files_put))
        .route("/api/projects/{name}/rag/files/hashes", get(pipeline::file_hashes))
        .route("/api/projects/{name}/rag/search", post(search::search))
        .route("/api/projects/{name}/rag/work/claim", post(pipeline::work_claim))
        .route("/api/projects/{name}/rag/work/done", post(pipeline::work_done))
        .route("/api/projects/{name}/rag/schedule", get(schedule_get).post(schedule_create))
        .route("/api/rag/model", get(model_status))
        // base64 uploads (25 MB → ~34 MB) and a large map's chunk vectors
        .layer(axum::extract::DefaultBodyLimit::max(128 * 1024 * 1024))
}

// ---------- helpers ----------

pub(crate) fn bad(msg: impl Into<String>) -> ApiError {
    ApiError::Status(StatusCode::BAD_REQUEST, msg.into())
}
pub(crate) fn not_found(msg: impl Into<String>) -> ApiError {
    ApiError::Status(StatusCode::NOT_FOUND, msg.into())
}
pub(crate) fn backend(e: impl std::fmt::Display) -> ApiError {
    ApiError::Status(StatusCode::INTERNAL_SERVER_ERROR, e.to_string())
}
pub(crate) fn unavailable(e: impl std::fmt::Display) -> ApiError {
    ApiError::Status(StatusCode::SERVICE_UNAVAILABLE, e.to_string())
}
pub(crate) fn conflict(msg: impl Into<String>) -> ApiError {
    ApiError::Status(StatusCode::CONFLICT, msg.into())
}
pub(crate) fn s<'a>(v: &'a Value, k: &str) -> &'a str {
    v.get(k).and_then(|x| x.as_str()).unwrap_or("")
}
pub(crate) fn arango(store: &dyn Storage) -> Result<&ArangoBackend, ApiError> {
    store.arango().ok_or_else(|| backend("GraphRAG needs the ArangoDB store"))
}
pub(crate) fn iso_in(sec: i64) -> String {
    (chrono::Utc::now() + chrono::Duration::seconds(sec)).format("%Y-%m-%dT%H:%M:%SZ").to_string()
}
pub fn sha(text: &str) -> String {
    sha_bytes(text.as_bytes())
}
pub fn sha_bytes(b: &[u8]) -> String {
    Sha256::digest(b).iter().map(|x| format!("{x:02x}")).collect()
}
/// A node document's key: stable per (project, path), within Arango's key rules.
pub(crate) fn node_doc_id(project: &str, path: &str) -> String {
    format!("n{}", &sha(&format!("{project}\u{0}{path}"))[..31])
}
pub(crate) fn chunk_key(doc: &str, idx: usize) -> String {
    format!("{doc}_{idx:05}")
}
/// Storage attributes and vectors never go over the wire.
pub(crate) fn clean(mut v: Value) -> Value {
    if let Some(o) = v.as_object_mut() {
        for k in ["_id", "_rev", "vec_raw", "vec_sum"] {
            o.remove(k);
        }
        if let Some(k) = o.remove("_key") {
            o.insert("id".into(), k);
        }
    }
    v
}
/// Embed search questions (or a straggler's summary) off the async runtime.
pub async fn embed_async(texts: Vec<String>) -> Result<Vec<Vec<f32>>, String> {
    if texts.is_empty() {
        return Ok(vec![]);
    }
    tokio::task::spawn_blocking(move || embed::get()?.embed(&texts)).await.map_err(|e| format!("embed task: {e}"))?
}

/// Collections, plain indexes and the keyword view (called from ArangoBackend::ensure_schema).
pub async fn ensure_schema(a: &ArangoBackend) -> Result<(), String> {
    for c in [DOC_COLL, CHUNK_COLL, BLOB_COLL, SETTING_COLL] {
        match a.dbcall("POST", "/_api/collection", Some(&json!({"name": c, "type": 2}))).await {
            Ok(_) => {}
            Err(e) if e.num == 1207 => {}
            Err(e) => return Err(e.to_string()),
        }
    }
    for (coll, fields, name) in [
        (DOC_COLL, json!(["project", "kind"]), "project_kind"),
        (DOC_COLL, json!(["project", "path"]), "project_path"),
        (DOC_COLL, json!(["project", "state"]), "project_state"),
        (CHUNK_COLL, json!(["project", "doc"]), "project_doc"),
        (CHUNK_COLL, json!(["project", "sum_state"]), "project_sum_state"),
        // a file hit finds the map nodes that name it (children.documents)
        (crate::arango::NODE_COLL, json!(["project", "documents[*]"]), "project_documents"),
    ] {
        a.dbcall("POST", &format!("/_api/index?collection={coll}"), Some(&json!({"type": "persistent", "fields": fields, "name": name})))
            .await
            .map_err(|e| e.to_string())?;
    }
    // chunks written before 2026-09-30 carry vec_sum: null while they wait for
    // a summary; the sparse vector index needs the attribute absent instead
    a.aql("FOR c IN rag_chunk FILTER HAS(c, 'vec_sum') AND c.vec_sum == null UPDATE c WITH {vec_sum: null} IN rag_chunk OPTIONS {keepNull: false}", json!({}))
        .await
        .map_err(|e| e.to_string())?;
    search::ensure_view(a).await
}

// ---------- settings ----------

pub async fn settings(a: &ArangoBackend, project: &str) -> Value {
    let row = a
        .aql("RETURN DOCUMENT(CONCAT('rag_setting/', @k))", json!({"k": crate::arango::doc_key(project, "-")}))
        .await
        .ok()
        .and_then(|r| r.into_iter().next())
        .filter(|v| !v.is_null());
    let docs_dir = row.as_ref().map(|r| s(r, "docs_dir").to_string()).filter(|d| !d.is_empty()).unwrap_or_else(|| DEFAULT_DOCS_DIR.to_string());
    let gitignore = row.as_ref().and_then(|r| r.get("docs_gitignore")).and_then(|b| b.as_bool()).unwrap_or(false);
    let list = |k: &str| -> Value { row.as_ref().and_then(|r| r.get(k)).filter(|v| v.is_array()).cloned().unwrap_or(json!([])) };
    // node_types: which node files the change sweep indexes (empty = all);
    // repo_globs: other checkout files it indexes where they live (never copied)
    json!({"docs_dir": docs_dir, "docs_gitignore": gitignore, "node_types": list("node_types"), "repo_globs": list("repo_globs")})
}

async fn settings_get(_u: AuthUser, State(st): Ctx, Path(name): Path<String>) -> Result<Json<Value>, ApiError> {
    Ok(Json(settings(arango(st.store.as_ref())?, &name).await))
}

/// `{topdir}/…` form, trailing slash, never leaving the checkout.
pub fn norm_docs_dir(d: &str) -> Result<String, String> {
    let r = d.trim().trim_start_matches("{topdir}").trim_matches('/');
    if r.split('/').any(|seg| seg == ".." || seg == ".") {
        return Err(format!("docs_dir must stay inside the checkout: {d}"));
    }
    if r.split('/').any(|seg| seg == ".git") {
        return Err("docs_dir may not be inside .git".into());
    }
    Ok(if r.is_empty() { "{topdir}/".into() } else { format!("{{topdir}}/{r}/") })
}

/// `{docs_dir, docs_gitignore}`, either optional. Turning `docs_gitignore` on
/// (or moving the directory while it is on) queues a `gitignore_path` edit so
/// the first engine adds the directory to the project's .gitignore; off
/// removes the line. The directory's files are still written; an engine never
/// commits an ignored path.
async fn settings_put(u: AuthUser, State(st): Ctx, Path(name): Path<String>, Json(body): Json<Value>) -> Result<Json<Value>, ApiError> {
    u.require_writer()?;
    let a = arango(st.store.as_ref())?;
    let before = settings(a, &name).await;
    let dir = match body.get("docs_dir").and_then(|d| d.as_str()) {
        Some(d) => norm_docs_dir(d).map_err(bad)?,
        None => s(&before, "docs_dir").to_string(),
    };
    let gi = body.get("docs_gitignore").and_then(|b| b.as_bool()).unwrap_or(before["docs_gitignore"].as_bool().unwrap_or(false));
    let strings = |k: &str| -> Result<Vec<String>, ApiError> {
        match body.get(k) {
            None => Ok(before[k].as_array().cloned().unwrap_or_default().iter().filter_map(|x| x.as_str().map(String::from)).collect()),
            Some(Value::Array(a)) => Ok(a.iter().filter_map(|x| x.as_str().map(|s| s.trim().to_string())).filter(|s| !s.is_empty()).collect()),
            Some(_) => Err(bad(format!("{k} must be a list of strings"))),
        }
    };
    let node_types = strings("node_types")?;
    let repo_globs = strings("repo_globs")?;
    for g in &repo_globs {
        let r = g.trim_start_matches("{topdir}").trim_start_matches('/');
        if r.split('/').any(|seg| seg == "..") || g.starts_with('/') && !g.starts_with("{topdir}") {
            return Err(bad(format!("repo_globs stay inside the checkout: {g}")));
        }
    }
    if dir == "{topdir}/" && gi {
        return Err(bad("the docs directory is the whole checkout: it cannot be git-ignored"));
    }
    a.aql_retry(
        "UPSERT {_key: @k} INSERT {_key: @k, project: @p, docs_dir: @d, docs_gitignore: @g, node_types: @nt, repo_globs: @rg, updated: @now, by: @by}
         UPDATE {docs_dir: @d, docs_gitignore: @g, node_types: @nt, repo_globs: @rg, updated: @now, by: @by} IN rag_setting",
        json!({"k": crate::arango::doc_key(&name, "-"), "p": name, "d": dir, "g": gi, "nt": node_types, "rg": repo_globs, "now": now_utc(), "by": u.sub}),
    )
    .await
    .map_err(backend)?;
    let was_gi = before["docs_gitignore"].as_bool().unwrap_or(false);
    let mut queued = Vec::new();
    if gi && (!was_gi || s(&before, "docs_dir") != dir) {
        queued.push(json!({"op": "gitignore_path", "path": dir, "ignore": true, "reason": format!("GraphRAG setting docs_gitignore, set by {}", u.sub)}));
    }
    if !gi && was_gi {
        queued.push(json!({"op": "gitignore_path", "path": s(&before, "docs_dir"), "ignore": false, "reason": format!("GraphRAG setting docs_gitignore, cleared by {}", u.sub)}));
    }
    let mut rows = Vec::new();
    for op in queued {
        rows.push(crate::datasync::create(st.store.as_ref(), &name, &op, &u.sub).await?["id"].clone());
    }
    st.store.bump_seq(&name, "rag").await?;
    Ok(Json(json!({"docs_dir": dir, "docs_gitignore": gi, "node_types": node_types, "repo_globs": repo_globs, "datasync": rows})))
}

// ---------- status ----------

async fn model_status(_u: AuthUser) -> Json<Value> {
    Json(embed::status())
}

/// Engines that serve `project` and were seen in the last 2 minutes.
async fn live_engines(store: &dyn Storage, project: &str) -> Vec<Value> {
    let cutoff = (chrono::Utc::now() - chrono::Duration::seconds(120)).format("%Y-%m-%dT%H:%M:%SZ").to_string();
    store
        .scan("engine")
        .await
        .unwrap_or_default()
        .into_iter()
        .filter(|e| e.pointer(&format!("/projects/{}", project.replace('~', "~0").replace('/', "~1"))).is_some())
        .filter(|e| s(e, "last_seen") >= cutoff.as_str())
        .map(|e| json!({"name": s(&e, "name"), "account": s(&e, "account"), "hold": s(&e, "hold"), "last_seen": s(&e, "last_seen")}))
        .collect()
}

async fn status(_u: AuthUser, State(st): Ctx, Path(name): Path<String>) -> Result<Json<Value>, ApiError> {
    let a = arango(st.store.as_ref())?;
    let now = now_utc();
    let docs = a
        .aql(
            "FOR d IN rag_doc FILTER d.project == @p COLLECT kind = d.kind, state = d.state WITH COUNT INTO n RETURN {kind, state, n}",
            json!({"p": name}),
        )
        .await
        .map_err(backend)?;
    let chunks = a
        .aql(
            "FOR c IN rag_chunk FILTER c.project == @p
             COLLECT state = (c.sum_state == 'claimed' AND c.sum_expires < @now) ? 'pending' : c.sum_state WITH COUNT INTO n
             RETURN {state, n}",
            json!({"p": name, "now": now}),
        )
        .await
        .map_err(backend)?;
    // vectors made by other weights than the ones that embed questions here
    let stamp = embed::disk_stamp().unwrap_or_default();
    let mismatch = a
        .aql(
            "RETURN LENGTH(FOR c IN rag_chunk FILTER c.project == @p AND c.vec_model != null AND c.vec_model != @m RETURN 1)",
            json!({"p": name, "m": stamp}),
        )
        .await
        .ok()
        .and_then(|r| r.first().and_then(|v| v.as_u64()))
        .unwrap_or(0);
    let schedule = find_schedule(st.store.as_ref(), &name).await?;
    let engines = live_engines(st.store.as_ref(), &name).await;
    Ok(Json(json!({
        "project": name,
        "docs": docs,
        "chunks": chunks,
        "waiting": pipeline::waiting_one(a, &name).await.unwrap_or(0),
        "settings": settings(a, &name).await,
        "model": embed::status(),
        "model_mismatch": mismatch,
        "vector_index": search::vector_index_state(a).await,
        "engines": engines,
        "schedule": schedule,
        "accepted": ACCEPTED,
    })))
}

// ---------- uploads ----------

/// A file name safe for the docs directory ("Q3 plan (v2).PDF" → "Q3-plan-v2.pdf").
pub fn safe_filename(name: &str) -> String {
    let base = name.rsplit(['/', '\\']).next().unwrap_or(name);
    let (stem, ext) = base.rsplit_once('.').unwrap_or((base, ""));
    let clean = |t: &str| -> String {
        let mut o = String::new();
        for c in t.chars() {
            if c.is_ascii_alphanumeric() || c == '_' || c == '.' {
                o.push(c);
            } else if !o.ends_with('-') {
                o.push('-');
            }
        }
        o.trim_matches(['-', '.']).to_string()
    };
    let st = clean(stem);
    let st = if st.is_empty() { "document".to_string() } else { st };
    let ex = clean(ext).to_ascii_lowercase();
    if ex.is_empty() { st } else { format!("{st}.{ex}") }
}

#[derive(serde::Deserialize)]
struct UploadReq {
    filename: String,
    content_b64: String,
    #[serde(default)]
    title: String,
    /// false = index only, do not store the original in the repo
    #[serde(default = "yes")]
    store: bool,
}
fn yes() -> bool {
    true
}

/// Accept an upload: keep the bytes for the engine's ingest job, queue the
/// original for the docs directory. The document is searchable once an engine
/// has ingested it (seconds when one is live).
async fn doc_upload(u: AuthUser, State(st): Ctx, Path(name): Path<String>, Json(req): Json<UploadReq>) -> Result<Json<Value>, ApiError> {
    use base64::Engine as _;
    u.require_writer()?;
    let a = arango(st.store.as_ref())?;
    let b64 = req.content_b64.trim();
    let bytes = base64::engine::general_purpose::STANDARD.decode(b64).map_err(|e| bad(format!("content_b64: {e}")))?;
    if bytes.len() > MAX_UPLOAD {
        return Err(bad(format!("{} is {} MB; the limit is {} MB", req.filename, bytes.len() / 1_048_576, MAX_UPLOAD / 1_048_576)));
    }
    let fname = safe_filename(&req.filename);
    if fname.ends_with(".iter.md") {
        return Err(bad("*.iter.md files are map nodes: they are indexed from the checkout by `iter rag sync`, not uploaded"));
    }
    let ext = extract::ext_of(&fname);
    if ["doc", "ppt", "xls"].contains(&ext.as_str()) {
        return Err(bad(format!(".{ext} (the old binary Office format) is not supported; save it as .docx/.pptx/.pdf")));
    }
    if !ACCEPTED.contains(&ext.as_str()) && std::str::from_utf8(&bytes).is_err() {
        return Err(bad(format!("{fname}: not a supported document type ({})", ACCEPTED.join(", "))));
    }
    let settings = settings(a, &name).await;
    let path = format!("{}{}", s(&settings, "docs_dir"), fname);
    // same path = same document (a re-upload replaces it)
    let existing = a
        .aql("FOR d IN rag_doc FILTER d.project == @p AND d.path == @path AND d.kind == 'file' LIMIT 1 RETURN d", json!({"p": name, "path": path}))
        .await
        .map_err(backend)?
        .into_iter()
        .next();
    let id = existing.as_ref().map(|d| s(d, "_key").to_string()).unwrap_or_else(|| format!("f{}", uuid::Uuid::new_v4().simple()));
    let title = if req.title.trim().is_empty() { req.filename.clone() } else { req.title.trim().to_string() };
    let now = now_utc();
    a.aql_retry(
        "UPSERT {_key: @k} INSERT @b REPLACE @b IN rag_blob",
        json!({"k": id, "b": {"_key": id, "project": name, "filename": fname, "content_b64": b64, "created": now}}),
    )
    .await
    .map_err(backend)?;
    let mut doc = json!({
        "_key": id, "project": name, "kind": "file", "title": title, "path": path, "filename": req.filename, "size": bytes.len(),
        "file_sha": sha_bytes(&bytes), "format": ext, "state": "queued",
        "chunks": existing.as_ref().and_then(|d| d.get("chunks")).cloned().unwrap_or(json!(0)),
        "chapters": existing.as_ref().and_then(|d| d.get("chapters")).cloned().unwrap_or(json!([])),
        "summary": existing.as_ref().map(|d| s(d, "summary").to_string()).unwrap_or_default(),
        "ingest_engine": "", "ingest_expires": "", "ingest_attempts": 0, "error": "",
        "created": existing.as_ref().map(|d| s(d, "created").to_string()).filter(|c| !c.is_empty()).unwrap_or_else(|| now.clone()),
        "updated": now, "by": u.sub, "store": Value::Null,
    });
    if req.store {
        let op = json!({"op": "store_doc", "path": path, "content_b64": b64, "reason": format!("GraphRAG upload by {}", u.sub)});
        let row = crate::datasync::create(st.store.as_ref(), &name, &op, &u.sub).await?;
        doc["store"] = json!({"state": "pending", "datasync": row["id"], "requested": now_utc()});
    }
    a.aql_retry("UPSERT {_key: @k} INSERT @doc REPLACE @doc IN rag_doc", json!({"k": id, "doc": doc}))
        .await
        .map_err(backend)?;
    st.store.bump_seq(&name, "rag").await?;
    Ok(Json(clean(doc)))
}

// ---------- documents ----------

#[derive(serde::Deserialize, Default)]
struct DocsQ {
    #[serde(default)]
    kind: String,
    #[serde(default)]
    state: String,
}

/// The project's documents (no chunk text, no vectors), newest first; a
/// file's `store.state` follows its datasync row (pending → stored/failed).
async fn docs_list(_u: AuthUser, State(st): Ctx, Path(name): Path<String>, Query(q): Query<DocsQ>) -> Result<Json<Value>, ApiError> {
    let a = arango(st.store.as_ref())?;
    let rows = a
        .aql(
            "FOR d IN rag_doc FILTER d.project == @p AND (@k == '' OR d.kind == @k) AND (@s == '' OR d.state == @s)
             LET done = LENGTH(FOR c IN rag_chunk FILTER c.project == @p AND c.doc == d._key AND c.sum_state == 'done' RETURN 1)
             LET links = d.kind == 'file' ? LENGTH(FOR n IN node FILTER n.project == @p AND d.path IN n.documents[*] RETURN 1) : 0
             SORT d.updated DESC
             RETURN MERGE(UNSET(d, '_id', '_rev', 'vec_sum'), {summarized: done, linked: links})",
            json!({"p": name, "k": q.kind, "s": q.state}),
        )
        .await
        .map_err(backend)?;
    let checkouts = checkouts(st.store.as_ref(), &name).await;
    let mut out = Vec::with_capacity(rows.len());
    for r in rows {
        let mut d = with_store_state(st.store.as_ref(), &name, clean(r)).await;
        d["locations"] = locations(&checkouts, s(&d, "path"));
        out.push(d);
    }
    Ok(Json(Value::Array(out)))
}

/// The checkouts of `project`: (engine, topdir, seen in the last 2 minutes),
/// from every engine record that serves it.
pub(crate) async fn checkouts(store: &dyn Storage, project: &str) -> Vec<(String, String, bool)> {
    let cutoff = (chrono::Utc::now() - chrono::Duration::seconds(120)).format("%Y-%m-%dT%H:%M:%SZ").to_string();
    let key = project.replace('~', "~0").replace('/', "~1");
    let mut v: Vec<(String, String, bool)> = store
        .scan("engine")
        .await
        .unwrap_or_default()
        .into_iter()
        .filter_map(|e| {
            let top = e.pointer(&format!("/projects/{key}/dirs/topdir"))?.as_str()?.trim_end_matches('/').to_string();
            Some((s(&e, "name").to_string(), top, s(&e, "last_seen") >= cutoff.as_str()))
        })
        .collect();
    v.sort_by(|a, b| b.2.cmp(&a.2).then(a.0.cmp(&b.0)));
    v
}

/// Where a document's file sits on each engine's machine: `{topdir}/…`
/// resolved against every checkout of the project (live engines first).
pub(crate) fn locations(checkouts: &[(String, String, bool)], path: &str) -> Value {
    let Some(rel) = path.strip_prefix("{topdir}") else { return json!([]) };
    Value::Array(checkouts.iter().map(|(e, top, live)| json!({"engine": e, "path": format!("{top}{rel}"), "live": live})).collect())
}

pub(crate) async fn with_store_state(store: &dyn Storage, project: &str, mut doc: Value) -> Value {
    let dsid = doc.pointer("/store/datasync").and_then(|x| x.as_str()).unwrap_or("").to_string();
    if !dsid.is_empty() {
        if let Ok(Some(row)) = store.get("datasync", project, &dsid).await {
            // written but not committed: a git-ignored docs directory, or a
            // checkout that is not a git repository root
            let uncommitted = s(&row, "note").contains("git-ignored") || s(&row, "commit").is_empty();
            doc["store"]["state"] = json!(match s(&row, "state") {
                "applied" if uncommitted => "written",
                "applied" => "stored",
                "failed" => "failed",
                "claimed" => "storing",
                _ => "pending",
            });
            doc["store"]["commit"] = json!(s(&row, "commit"));
            doc["store"]["error"] = json!(s(&row, "error"));
        }
    }
    doc
}

/// Map nodes whose `children.documents` name `path`.
pub(crate) async fn linked_nodes(a: &ArangoBackend, project: &str, path: &str) -> Vec<Value> {
    a.aql(
        "FOR n IN node FILTER n.project == @p AND @path IN n.documents[*] SORT n.path
         RETURN {id: n.id, name: n.name, nodetype: n.nodetype, path: n.path}",
        json!({"p": project, "path": path}),
    )
    .await
    .unwrap_or_default()
}

async fn doc_get(_u: AuthUser, State(st): Ctx, Path((name, id)): Path<(String, String)>) -> Result<Json<Value>, ApiError> {
    let a = arango(st.store.as_ref())?;
    let doc = a
        .aql("LET d = DOCUMENT(CONCAT('rag_doc/', @k)) FILTER d != null AND d.project == @p RETURN d", json!({"k": id, "p": name}))
        .await
        .map_err(backend)?
        .into_iter()
        .next()
        .ok_or_else(|| not_found(format!("no GraphRAG document {id}")))?;
    let chunks = a
        .aql(
            "FOR c IN rag_chunk FILTER c.project == @p AND c.doc == @d SORT c.idx
             RETURN UNSET(c, '_id', '_rev', 'vec_raw', 'vec_sum')",
            json!({"p": name, "d": id}),
        )
        .await
        .map_err(backend)?;
    let mut doc = with_store_state(st.store.as_ref(), &name, clean(doc)).await;
    if doc["kind"] == "file" {
        doc["linked_nodes"] = Value::Array(linked_nodes(a, &name, s(&doc, "path")).await);
    }
    doc["chunk_list"] = Value::Array(chunks.into_iter().map(clean).collect());
    Ok(Json(doc))
}

#[derive(serde::Deserialize, Default)]
struct DeleteQ {
    /// also remove the stored original from the repo (file documents)
    #[serde(default)]
    remove_file: bool,
}

async fn doc_delete(u: AuthUser, State(st): Ctx, Path((name, id)): Path<(String, String)>, Query(q): Query<DeleteQ>) -> Result<Json<Value>, ApiError> {
    u.require_writer()?;
    let a = arango(st.store.as_ref())?;
    let doc = a
        .aql("LET d = DOCUMENT(CONCAT('rag_doc/', @k)) FILTER d != null AND d.project == @p RETURN d", json!({"k": id, "p": name}))
        .await
        .map_err(backend)?
        .into_iter()
        .next()
        .ok_or_else(|| not_found(format!("no GraphRAG document {id}")))?;
    remove_docs(a, &name, &[id.clone()]).await?;
    let mut removal = Value::Null;
    if q.remove_file && s(&doc, "kind") == "file" {
        let op = json!({"op": "remove_doc", "path": s(&doc, "path"), "reason": format!("GraphRAG document removed by {}", u.sub)});
        let row = crate::datasync::create(st.store.as_ref(), &name, &op, &u.sub).await?;
        removal = json!({"datasync": row["id"]});
    }
    st.store.bump_seq(&name, "rag").await?;
    Ok(Json(json!({"removed": id, "file_removal": removal})))
}

pub(crate) async fn remove_docs(a: &ArangoBackend, project: &str, ids: &[String]) -> Result<(), ApiError> {
    if ids.is_empty() {
        return Ok(());
    }
    for q in [
        "FOR c IN rag_chunk FILTER c.project == @p AND c.doc IN @ids REMOVE c IN rag_chunk",
        "FOR b IN rag_blob FILTER b.project == @p AND b._key IN @ids REMOVE b IN rag_blob",
        "FOR d IN rag_doc FILTER d.project == @p AND d._key IN @ids REMOVE d IN rag_doc",
    ] {
        a.aql_retry(q, json!({"p": project, "ids": ids})).await.map_err(backend)?;
    }
    Ok(())
}

/// Throw away every summary of one document and ask the agent again.
async fn doc_resummarize(u: AuthUser, State(st): Ctx, Path((name, id)): Path<(String, String)>) -> Result<Json<Value>, ApiError> {
    u.require_writer()?;
    let a = arango(st.store.as_ref())?;
    let n = a
        .aql_retry(
            "FOR c IN rag_chunk FILTER c.project == @p AND c.doc == @d
             UPDATE c WITH {sum_state: 'pending', summary: '', vec_sum: null, sum_attempts: 0, sum_engine: '', sum_expires: '', sum_error: ''} IN rag_chunk
             OPTIONS {keepNull: false}
             RETURN 1",
            json!({"p": name, "d": id}),
        )
        .await
        .map_err(backend)?
        .len();
    a.aql_retry(
        "FOR d IN rag_doc FILTER d._key == @d AND d.project == @p AND d.state NOT IN ['queued', 'ingesting']
         UPDATE d WITH {state: 'summarizing', summary: '', vec_sum: null, rollup_state: '', rollup_attempts: 0,
                        chapters: (FOR c IN d.chapters RETURN MERGE(c, {summary: ''})), updated: @now} IN rag_doc",
        json!({"p": name, "d": id, "now": now_utc()}),
    )
    .await
    .map_err(backend)?;
    st.store.bump_seq(&name, "rag").await?;
    Ok(Json(json!({"doc": id, "chunks_reset": n})))
}

#[derive(serde::Deserialize)]
struct LinkReq {
    /// the map node's file, `{topdir}/…/x.code.iter.md`, or its id
    node: String,
    #[serde(default)]
    unlink: bool,
}

/// Link an uploaded document to a map node (or unlink it): an edit of the
/// node's `children.documents` in the project graph (iter5: visible at once,
/// written to the node file by the next engine through files/pending).
/// `node` is the node's path (`{topdir}/…/x.code.iter.md`) or its id.
async fn doc_link(u: AuthUser, State(st): Ctx, Path((name, id)): Path<(String, String)>, Json(req): Json<LinkReq>) -> Result<Json<Value>, ApiError> {
    u.require_writer()?;
    let a = arango(st.store.as_ref())?;
    let doc = a
        .aql("LET d = DOCUMENT(CONCAT('rag_doc/', @k)) FILTER d != null AND d.project == @p RETURN d", json!({"k": id, "p": name}))
        .await
        .map_err(backend)?
        .into_iter()
        .next()
        .ok_or_else(|| not_found(format!("no GraphRAG document {id}")))?;
    if s(&doc, "kind") != "file" {
        return Err(bad("only uploaded documents are linked to nodes (node files are the map itself)"));
    }
    let _g = crate::nodes::lock(&name).await;
    let mut g = crate::nodes::Graph::load(st.store.as_ref(), &name).await?;
    let node = g
        .live()
        .find(|n| n.doc.path == req.node || n.doc.id == req.node)
        .cloned()
        .ok_or_else(|| not_found(format!("no node {} in {name} (give its path or id)", req.node)))?;
    let dpath = s(&doc, "path").to_string();
    let mut nd = node.doc.clone();
    let list = nd.children.extra.entry("documents".into()).or_default();
    let had = list.iter().any(|e| iter_core::nodefile::expand_entry(e, &node.doc.path, None) == dpath);
    if req.unlink {
        list.retain(|e| iter_core::nodefile::expand_entry(e, &node.doc.path, None) != dpath);
    } else if !had {
        list.push(dpath.clone());
    }
    if list.is_empty() {
        nd.children.extra.remove("documents");
    }
    let changed = nd != node.doc;
    if changed {
        let what = if req.unlink { "unlink" } else { "link" };
        g.commit_edit(&node.doc.id, nd, &u.sub, &format!("GraphRAG: {what} {dpath}"), true)?;
        g.save(st.store.as_ref()).await?;
    }
    Ok(Json(json!({"node": node.doc.id, "document": dpath, "linked": !req.unlink, "changed": changed})))
}

// ---------- the scheduled RAG change sweep ----------

async fn find_schedule(store: &dyn Storage, project: &str) -> Result<Value, ApiError> {
    let items = store.query("workitem", project).await?;
    Ok(items
        .into_iter()
        .find(|i| {
            s(i, "state") == "scheduled"
                && i.get("tags").and_then(|t| t.as_array()).map(|t| t.iter().any(|x| s(x, "text") == SWEEP_TAG)).unwrap_or(false)
        })
        .map(|i| json!({"id": s(&i, "id"), "name": s(&i, "name"), "sched": i.get("sched").cloned().unwrap_or(Value::Null), "exec_shell": s(&i, "exec_shell")}))
        .unwrap_or(Value::Null))
}

async fn schedule_get(_u: AuthUser, State(st): Ctx, Path(name): Path<String>) -> Result<Json<Value>, ApiError> {
    Ok(Json(json!({"schedule": find_schedule(st.store.as_ref(), &name).await?})))
}

#[derive(serde::Deserialize)]
struct ScheduleReq {
    /// minutes between sweeps (default 60)
    #[serde(default)]
    every_min: Option<u64>,
}

/// "Create a scheduled RAG change sweep": a scheduled exec work item running
/// `iter rag sync`, which re-reads every node file of the map and re-indexes
/// the ones whose text changed. One per project (an existing one is returned).
/// Engines also re-index after every map push; the sweep is the backstop.
async fn schedule_create(u: AuthUser, st: Ctx, Path(name): Path<String>, Json(req): Json<ScheduleReq>) -> Result<Json<Value>, ApiError> {
    u.require_writer()?;
    let existing = find_schedule(st.0.store.as_ref(), &name).await?;
    if !existing.is_null() {
        return Ok(Json(json!({"schedule": existing, "created": false})));
    }
    let mins = req.every_min.unwrap_or(60).clamp(5, 7 * 24 * 60);
    let every = if mins % 60 == 0 { format!("{}h", mins / 60) } else { format!("{mins}m") };
    let body = json!({
        "name": format!("GraphRAG change sweep, every {every}: re-index changed *.iter.md node files"),
        "agent": "test", "state": "scheduled", "exec_shell": "iter rag sync",
        // upkeep: the maintenance band (50+), behind human and use-case work
        "priority": iter_core::PRIO_BAND_MAINT.0,
        "sched": {"kind": "every", "every_min": mins},
        "lockdirs": [], "blockedby": [], "context": [], "tags": [{"text": SWEEP_TAG, "color": ""}],
        "requestedby": "user", "prework": [], "postwork": [],
        "request": "Scheduled GraphRAG change sweep: `iter rag sync` reads every node file (*.iter.md) the project map lists, chunks and embeds the ones whose text changed since the last sweep and sends them to iter_data (unchanged chunks keep their summaries; the Summary agent summarises the rest), and removes the documents of node files that no longer exist.",
    });
    let created = crate::api::workitem_create(u, st.clone(), Path(name.clone()), Json(body)).await?;
    Ok(Json(json!({"schedule": {"id": created.0["id"], "name": created.0["name"], "sched": created.0["sched"]}, "created": true})))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rollup_samples_fit_their_budget() {
        let items: Vec<Value> = (0..100).map(|i| json!({"heading": "", "summary": "x".repeat(92), "i": i})).collect();
        let out = pipeline::sample_to(items.clone(), 1000);
        let a = out.as_array().unwrap();
        assert_eq!(a.len(), 10);
        assert_eq!((a[0]["i"].as_u64(), a[9]["i"].as_u64()), (Some(0), Some(90)), "evenly spaced");
        assert_eq!(pipeline::sample_to(items[..3].to_vec(), 1000).as_array().unwrap().len(), 3);
    }

    #[test]
    fn locations_resolve_topdir_per_engine() {
        let c = vec![("mac".to_string(), "/Users/s/dev/p".to_string(), true), ("srv".to_string(), "~/p".to_string(), false)];
        let l = locations(&c, "{topdir}/docs/a.pdf");
        assert_eq!(l[0]["path"], "/Users/s/dev/p/docs/a.pdf");
        assert_eq!((l[1]["engine"].as_str(), l[1]["path"].as_str(), l[1]["live"].as_bool()), (Some("srv"), Some("~/p/docs/a.pdf"), Some(false)));
        assert_eq!(locations(&c, "iter4/docs/iter4_guide.md"), json!([]));
    }

    #[test]
    fn filenames_and_docs_dirs() {
        assert_eq!(safe_filename("Q3 plan (v2).PDF"), "Q3-plan-v2.pdf");
        assert_eq!(safe_filename("../../etc/passwd"), "passwd");
        assert_eq!(safe_filename("..."), "document");
        assert_eq!(norm_docs_dir("docs").unwrap(), "{topdir}/docs/");
        assert_eq!(norm_docs_dir("{topdir}/reference/pdfs/").unwrap(), "{topdir}/reference/pdfs/");
        assert!(norm_docs_dir("../outside").is_err());
        assert!(norm_docs_dir(".git/x").is_err());
    }
}
