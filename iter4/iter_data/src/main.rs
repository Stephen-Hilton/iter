//! iter_data — the central API server + persistence for iter4.
//! Storage is ArangoDB only (graph + documents in one engine, usually in the
//! same container; decided 2026-09-29 — no SQLite, no DynamoDB serving).
//! DynamoDB is read, never written, as the iter3 migration source
//! (`--migrate-from dynamodb`).

mod api;
mod arango;
#[cfg(test)]
mod contract_tests;
mod auth;
mod datasync;
mod ddb;
mod graph;
mod graph_view;
mod mcp;
mod migrate_ddb;
mod rag;
mod storage;
#[cfg(test)]
mod test_db;

/// The static musl build (the container) uses mimalloc: musl's allocator made
/// GraphRAG embedding ~5x slower than the native build (2026-09-29).
#[cfg(target_env = "musl")]
#[global_allocator]
static GLOBAL_ALLOC: mimalloc::MiMalloc = mimalloc::MiMalloc;

use api::{AppState, GLOBAL};
use clap::Parser;
use serde_json::json;
use std::sync::Arc;

#[derive(Parser, Debug)]
#[command(name = "iter_data", about = "iter4 central data/API server")]
struct Args {
    /// storage backend: arango (the only one; kept so scripts may say so)
    #[arg(long, default_value = "arango")]
    backend: String,
    /// ArangoDB server URL (ARANGO_URL env wins over the default)
    #[arg(long, default_value = "")]
    arango_url: String,
    /// ArangoDB database name (created if missing)
    #[arg(long, default_value = "iter4")]
    arango_db: String,
    /// dynamodb region of the --migrate-from source (falls back to AWS_DEFAULT_REGION)
    #[arg(long, default_value = "")]
    region: String,
    /// listen address
    #[arg(long, default_value = "127.0.0.1:8300")]
    listen: String,
    /// static webui directory ('' disables)
    #[arg(long, default_value = "")]
    webui_dir: String,
    /// JWT secret sidecar file (ITER_JWT_SECRET env wins)
    #[arg(long, default_value = "./iter_data.secret")]
    secret_file: String,
    /// .env file to load (KEY=VALUE lines; malformed lines skipped)
    #[arg(long, default_value = "./.env")]
    env_file: String,

    /// count and report only; write nothing (--migrate-from)
    #[arg(long, default_value_t = false)]
    migrate_dry_run: bool,
    /// replace rows that already exist (default: skip them)
    #[arg(long, default_value_t = false)]
    migrate_overwrite: bool,

    /// one-shot copy of every table from another backend into this one (then
    /// exit). Only "dynamodb" is supported: reads `<migrate-prefix>*` tables,
    /// never writes them. Honors --migrate-dry-run and --migrate-overwrite.
    #[arg(long, default_value = "")]
    migrate_from: String,
    /// source DynamoDB table prefix for --migrate-from
    #[arg(long, default_value = "iter3_")]
    migrate_prefix: String,
    /// comma-separated logical tables for --migrate-from (default: all)
    #[arg(long, default_value = "")]
    migrate_tables: String,

    /// GraphRAG embedding model directory (all-MiniLM-L6-v2: config.json,
    /// tokenizer.json, model.safetensors); ITER_EMBED_MODEL env, then
    /// models/all-MiniLM-L6-v2 near the binary, when empty
    #[arg(long, default_value = "")]
    embed_model: String,
}

/// The webui, embedded so a container or Lambda deploy needs no filesystem:
/// the page, the Project graph viewer and Intro tab, and the vendored
/// Cytoscape libraries (MIT, licences in webui/vendor/).
const WEBUI_INDEX: &str = include_str!("../../webui/index.html");
const WEBUI_FILES: &[(&str, &str, &[u8])] = &[
    ("/graph.js", "application/javascript", include_bytes!("../../webui/graph.js")),
    ("/graph.css", "text/css", include_bytes!("../../webui/graph.css")),
    ("/graphedit.js", "application/javascript", include_bytes!("../../webui/graphedit.js")),
    ("/intro.js", "application/javascript", include_bytes!("../../webui/intro.js")),
    ("/intro.css", "text/css", include_bytes!("../../webui/intro.css")),
    ("/rag.js", "application/javascript", include_bytes!("../../webui/rag.js")),
    ("/rag.css", "text/css", include_bytes!("../../webui/rag.css")),
    ("/vendor/cytoscape.min.js", "application/javascript", include_bytes!("../../webui/vendor/cytoscape.min.js")),
    ("/vendor/layout-base.js", "application/javascript", include_bytes!("../../webui/vendor/layout-base.js")),
    ("/vendor/cose-base.js", "application/javascript", include_bytes!("../../webui/vendor/cose-base.js")),
    ("/vendor/cytoscape-fcose.js", "application/javascript", include_bytes!("../../webui/vendor/cytoscape-fcose.js")),
    ("/vendor/cytoscape-dagre.js", "application/javascript", include_bytes!("../../webui/vendor/cytoscape-dagre.js")),
];

async fn embedded_index(uri: axum::http::Uri) -> axum::response::Response {
    use axum::response::IntoResponse;
    let path = uri.path();
    if path == "/" || path == "/index.html" {
        return axum::response::Html(WEBUI_INDEX).into_response();
    }
    match WEBUI_FILES.iter().find(|(p, _, _)| *p == path) {
        Some((_, ctype, body)) => ([(axum::http::header::CONTENT_TYPE, *ctype)], *body).into_response(),
        None => (axum::http::StatusCode::NOT_FOUND, "not found").into_response(),
    }
}

/// Minimal .env loader: KEY=VALUE lines only, no shell semantics — the user's
/// .env contains lines a shell would choke on, so never `source` it.
fn load_env_file(path: &str) {
    let Ok(content) = std::fs::read_to_string(path) else { return };
    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some((k, v)) = line.split_once('=') {
            let k = k.trim();
            let v = v.trim().trim_matches('"').trim_matches('\'');
            if !k.is_empty() && !k.contains(char::is_whitespace) && std::env::var(k).is_err() {
                unsafe { std::env::set_var(k, v) };
            }
        }
    }
}

#[tokio::main]
async fn main() {
    let args = Args::parse();
    load_env_file(&args.env_file);
    if args.backend != "arango" {
        eprintln!("unknown backend '{}': iter4 stores everything in ArangoDB (--backend arango)", args.backend);
        std::process::exit(2);
    }
    let store: Arc<dyn storage::Storage> = Arc::new(open_arango(&args).await);

    if !args.migrate_from.is_empty() {
        return migrate_ddb::run_cli(store.as_ref(), &args.migrate_from, &args.migrate_prefix, &args.region,
            &args.migrate_tables, args.migrate_dry_run, args.migrate_overwrite).await;
    }

    bootstrap_admin(store.as_ref()).await;
    bootstrap_summary_agent(store.as_ref()).await;

    rag::guide::ingest_at_startup(store.clone());
    let secret = auth::load_secret(&args.secret_file);
    let state = Arc::new(AppState { store, secret });

    rag::embed::set_model_dir(&args.embed_model);
    match rag::embed::find_model_dir() {
        Some(d) => println!("[iter_data] GraphRAG embedding model: {} ({})", rag::embed::MODEL_NAME, d.display()),
        None => eprintln!("[iter_data] GraphRAG embedding model {} not found — ingestion and search will refuse (tools/fetch_model.sh)", rag::embed::MODEL_NAME),
    }

    let mut app = api::router(state.clone()).merge(mcp::routes(state)).layer(
        tower_http::cors::CorsLayer::new()
            .allow_origin(tower_http::cors::Any)
            .allow_methods(tower_http::cors::Any)
            .allow_headers(tower_http::cors::Any),
    );
    if !args.webui_dir.is_empty() {
        app = app.fallback_service(tower_http::services::ServeDir::new(&args.webui_dir));
    } else {
        app = app.fallback(embedded_index);
    }

    let listener = tokio::net::TcpListener::bind(&args.listen).await.expect("bind");
    println!("[iter_data] listening on {} (backend: arango)", args.listen);
    axum::serve(listener, app).await.expect("serve");
}

async fn open_arango(args: &Args) -> arango::ArangoBackend {
    let url = std::env::var("ARANGO_URL")
        .ok()
        .filter(|u| !u.trim().is_empty())
        .or_else(|| Some(args.arango_url.clone()).filter(|u| !u.trim().is_empty()))
        .unwrap_or_else(|| "http://127.0.0.1:8529".into());
    let db = std::env::var("ARANGO_DB").ok().filter(|d| !d.trim().is_empty()).unwrap_or_else(|| args.arango_db.clone());
    let user = std::env::var("ARANGO_USER").ok().filter(|u| !u.is_empty()).unwrap_or_else(|| "root".into());
    let password = std::env::var("ARANGO_PASSWORD").unwrap_or_default();
    match arango::ArangoBackend::new(&url, &db, &user, &password).await {
        Ok(b) => {
            println!("[iter_data] arango {url} database '{db}' ready");
            b
        }
        Err(e) => {
            eprintln!("[iter_data] arango init failed: {e}");
            std::process::exit(2);
        }
    }
}

/// First-run bootstrap: if no users exist, create "admin".
/// Password from ITER_ADMIN_PASSWORD, else generated and printed ONCE.
/// GraphRAG (2026-09-29): the `summary` agent record, created once so its
/// model, timeout and prompt are editable in the webui like any agent's. It
/// never runs as a work item: the engine's Summary worker reads it (rag.rs).
async fn bootstrap_summary_agent(store: &dyn storage::Storage) {
    if store.get("agent", "summary", "-").await.ok().flatten().is_some() {
        return;
    }
    let row = json!({
        "name": "summary", "model": "haiku", "timeoutsec": 300, "max": 2, "flags": "", "childstate": "queued",
        "closegate": null, "lockshape": {"none": true},
        "desc": "GraphRAG Summary agent: writes the retrieval summaries of document chunks, chapters and whole documents (Haiku by default). Runs on the engine outside the agent cap and the queue whenever the heartbeat reply says summaries are waiting; never a work item. Its output is embedded as each chunk's second vector.",
        "promptbody": "# Agent Definition: summary\n\nYou are iter's Summary agent. You write short retrieval summaries for a semantic search index\n(GraphRAG): a developer or another agent will later search the project's documents in plain words, and your summary is what\nlets them find the right passage. Write for that search:\n- say what the passage is about and what it states, in plain declarative sentences — never \"this chunk\", \"this section\" or \"the text\";\n- name the concrete things it contains (components, functions, fields, commands, error codes, numbers, people, decisions) exactly as written;\n- no opinions, no advice, nothing that is not in the passage.\n"
    });
    match store.put("agent", "summary", "-", &row).await {
        Ok(()) => {
            let _ = store.bump_seq(GLOBAL, "agent").await;
            println!("[iter_data] created the GraphRAG `summary` agent record (haiku)");
        }
        Err(e) => eprintln!("[iter_data] could not create the summary agent record: {e}"),
    }
}

async fn bootstrap_admin(store: &dyn storage::Storage) {
    let users = store.scan("webui_user").await.unwrap_or_default();
    if !users.is_empty() {
        return;
    }
    let (password, generated) = match std::env::var("ITER_ADMIN_PASSWORD") {
        Ok(p) if !p.trim().is_empty() => (p.trim().to_string(), false),
        _ => {
            use rand::Rng;
            let p: String = rand::thread_rng()
                .sample_iter(&rand::distributions::Alphanumeric)
                .take(20)
                .map(char::from)
                .collect();
            (p, true)
        }
    };
    let pwhash = auth::hash_password(&password).expect("hash admin password");
    let row = json!({
        "user": "admin", "email": "", "role": "admin", "pwhash": pwhash,
        "tokenver": 1, "css": "", "pubkey": "", "settings": {}, "authz": {}
    });
    store.put("webui_user", "admin", "-", &row).await.expect("bootstrap admin");
    let _ = store.bump_seq(GLOBAL, "webui_user").await;
    if generated {
        println!("[iter_data] bootstrapped user 'admin' with password: {password}");
        println!("[iter_data] (set ITER_ADMIN_PASSWORD to control this; change via the users API)");
    } else {
        println!("[iter_data] bootstrapped user 'admin' (password from ITER_ADMIN_PASSWORD)");
    }
}
