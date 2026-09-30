//! The iter4 user guide as a GraphRAG document (2026-09-29): `docs/iter4_guide.md`
//! is compiled into iter_data (build.rs; empty when the file is absent) and
//! ingested at startup as one product-wide document — kind `guide`, scope
//! (project) `_iter` — re-ingested only when its text or the index version
//! changes. Every project's search includes its chunks unless the request
//! sets `include_guide: false`, so "how do I create a new project?" is
//! answered in any project. Its summaries are ordinary Summary agent work:
//! any engine claims `_iter` jobs when its own projects have none.

use super::*;

include!(concat!(env!("OUT_DIR"), "/guide.rs"));

pub const SCOPE: &str = "_iter";
pub const DOC_ID: &str = "iter-guide";
pub const TITLE: &str = "iter4 guide";
pub const PATH: &str = "iter4/docs/iter4_guide.md";
/// Part of the guide's hash, like the engine's INDEX_VERSION for node files.
const GUIDE_INDEX_VERSION: &str = "v2";

/// Ingest (or drop) the guide in the background; never blocks startup.
pub fn ingest_at_startup(store: std::sync::Arc<dyn Storage>) {
    tokio::spawn(async move {
        let Some(a) = store.arango() else { return };
        match ingest(a).await {
            Ok(msg) => println!("[iter_data] GraphRAG guide: {msg}"),
            Err(e) => eprintln!("[iter_data] GraphRAG guide not ingested: {e}"),
        }
    });
}

async fn ingest(a: &ArangoBackend) -> Result<String, String> {
    let text = GUIDE.trim();
    if text.is_empty() {
        remove_docs(a, SCOPE, &[DOC_ID.to_string()]).await.map_err(|_| "could not remove the old guide".to_string())?;
        return Ok("no docs/iter4_guide.md compiled in — nothing to index".into());
    }
    let hash = sha(&format!("{GUIDE_INDEX_VERSION}\n{text}"));
    let stored = a
        .aql("RETURN DOCUMENT(CONCAT('rag_doc/', @k)).hash", json!({"k": DOC_ID}))
        .await
        .map_err(|e| e.to_string())?
        .into_iter()
        .next()
        .and_then(|v| v.as_str().map(String::from));
    if stored.as_deref() == Some(hash.as_str()) {
        return Ok("unchanged".into());
    }
    let owned = text.to_string();
    let prepared = tokio::task::spawn_blocking(move || iter_rag::prepare(embed::get()?, TITLE, &owned))
        .await
        .map_err(|e| e.to_string())??;
    let mut p: pipeline::Prepared = serde_json::from_value(prepared).map_err(|e| e.to_string())?;
    p.format = "markdown".into();
    let meta = pipeline::DocMeta {
        project: SCOPE, id: DOC_ID.to_string(), kind: "guide", title: TITLE.to_string(), path: PATH.to_string(),
        hash, extra: json!({"description": "How to use iter4: projects, engines, work items, agents, the map, GraphRAG and MCP."}), by: "iter_data",
    };
    let n = p.chunks.len();
    pipeline::store_prepared(a, meta, &p).await.map_err(|e| match e {
        ApiError::Status(_, m) => m,
        _ => "store failed".into(),
    })?;
    Ok(format!("indexed {n} chunks (summaries follow from the Summary agent)"))
}
