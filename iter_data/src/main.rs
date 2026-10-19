//! iter_data — the central API server + persistence for iter.
//! Storage is ArangoDB only (graph + documents in one engine, usually in the
//! same container; decided 2026-09-29 — no SQLite, no DynamoDB).

mod api;
mod arango;
mod authz;
#[cfg(test)]
mod contract_tests;
mod auth;
mod datasync;
mod defaults;
mod filesync;
mod graph;
mod keys;
mod graph_view;
#[cfg(test)]
mod graph_tests;
mod mcp;
mod names;
mod next;
mod nodes;
mod rag;
mod reqs;
mod settings;
mod storage;
mod switches;
mod testlogs;
mod tls;
pub mod sync_hooks;
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
#[command(name = "iter_data", about = "iter central data/API server")]
struct Args {
    /// storage backend: arango (the only one; kept so scripts may say so)
    #[arg(long, default_value = "arango")]
    backend: String,
    /// ArangoDB server URL (ARANGO_URL env wins over the default)
    #[arg(long, global = true, default_value = "")]
    arango_url: String,
    /// ArangoDB database name (ARANGO_DB env wins; created if missing)
    #[arg(long, global = true, default_value = arango::DEFAULT_DB)]
    arango_db: String,
    /// listen address (ITER_PORT, when set, replaces the default port 8400)
    #[arg(long, default_value = "")]
    listen: String,
    /// static webui directory ('' disables)
    #[arg(long, default_value = "")]
    webui_dir: String,
    /// the user sign-in secret file (ITER_JWT_SECRET env wins); the engine
    /// session secret (engine_session.secret) and the TLS certificate
    /// (--tls auto) live beside it
    #[arg(long, default_value = "./iter_data.secret")]
    secret_file: String,
    /// TLS: auto = a certificate made on first start and signed by a local CA
    /// of iter_data's own (name-constrained to this server; browsers trust it
    /// once the installers add the CA), both kept beside the secret file,
    /// pinned by engines; off = plain HTTP (a loopback dev loop, or behind a
    /// TLS proxy). ITER_TLS env wins over the default.
    #[arg(long, default_value = "")]
    tls: String,
    /// serve this certificate (PEM; a real one for a domain) instead of the
    /// one iter_data makes; needs --tls-key
    #[arg(long, default_value = "")]
    tls_cert: String,
    /// the private key (PEM) for --tls-cert
    #[arg(long, default_value = "")]
    tls_key: String,
    /// an extra name (DNS or IP) on the certificate iter_data makes (and its
    /// CA's name constraints); read when they are first made; repeatable
    #[arg(long)]
    tls_name: Vec<String>,
    /// .env file to load (KEY=VALUE lines; malformed lines skipped)
    #[arg(long, global = true, default_value = "./.env")]
    env_file: String,

    /// GraphRAG embedding model directory (all-MiniLM-L6-v2: config.json,
    /// tokenizer.json, model.safetensors); ITER_EMBED_MODEL env, then
    /// models/all-MiniLM-L6-v2 near the binary, when empty
    #[arg(long, default_value = "")]
    embed_model: String,

    /// none: serve the API and webui
    #[command(subcommand)]
    cmd: Option<Cmd>,
}

#[derive(clap::Subcommand, Debug)]
enum Cmd {
    /// engine keys, managed on this machine (no server is started)
    #[command(name = "engine-key", subcommand)]
    EngineKey(EngineKeyCmd),
    /// the default agents and tooling (iter_data/defaults/), managed on this
    /// machine (no server is started)
    #[command(subcommand)]
    Defaults(DefaultsCmd),
}

#[derive(clap::Subcommand, Debug)]
enum DefaultsCmd {
    /// add every default agent and tooling row this database lacks (one an
    /// admin deleted comes back; existing ones are never changed). The first
    /// start of a new database does this by itself, once
    Add,
}

#[derive(clap::Subcommand, Debug)]
enum EngineKeyCmd {
    /// register the public key of an engine on this same machine, as an admin
    /// would in the webui's Engine keys screen. Run as the service's user with
    /// its env file (the engine installer does:
    /// sudo -u iter iter_data engine-key add --env-file <iter dir>/iter_data/config/data.env …);
    /// the same key for the same engine again is a no-op
    Add {
        /// the engine's public key (`iter_engine key show`, the iter_… line)
        #[arg(long)]
        public_key: String,
        /// the engine's name
        #[arg(long)]
        engine: String,
    },
}

/// The webui, embedded so a container or Lambda deploy needs no filesystem:
/// the page, the Project graph viewer and Intro tab, and the vendored
/// Cytoscape libraries (MIT, licences in iter_data/webui/vendor/).
const WEBUI_INDEX: &str = include_str!("../webui/index.html");
const WEBUI_FILES: &[(&str, &str, &[u8])] = &[
    ("/graph.js", "application/javascript", include_bytes!("../webui/graph.js")),
    ("/graph.css", "text/css", include_bytes!("../webui/graph.css")),
    ("/graphedit.js", "application/javascript", include_bytes!("../webui/graphedit.js")),
    ("/intro.js", "application/javascript", include_bytes!("../webui/intro.js")),
    ("/intro.css", "text/css", include_bytes!("../webui/intro.css")),
    ("/rag.js", "application/javascript", include_bytes!("../webui/rag.js")),
    ("/rag.css", "text/css", include_bytes!("../webui/rag.css")),
    ("/vendor/cytoscape.min.js", "application/javascript", include_bytes!("../webui/vendor/cytoscape.min.js")),
    ("/vendor/layout-base.js", "application/javascript", include_bytes!("../webui/vendor/layout-base.js")),
    ("/vendor/cose-base.js", "application/javascript", include_bytes!("../webui/vendor/cose-base.js")),
    ("/vendor/cytoscape-fcose.js", "application/javascript", include_bytes!("../webui/vendor/cytoscape-fcose.js")),
    ("/vendor/cytoscape-dagre.js", "application/javascript", include_bytes!("../webui/vendor/cytoscape-dagre.js")),
    ("/vendor/elk.bundled.js", "application/javascript", include_bytes!("../webui/vendor/elk.bundled.js")),
    ("/kit.js", "application/javascript", include_bytes!("../webui/kit.js")),
    ("/kit.css", "text/css", include_bytes!("../webui/kit.css")),
    ("/settings.js", "application/javascript", include_bytes!("../webui/settings.js")),
];

/// The retired engine setup script (iter_data/iter_engine_setup.sh): it now only
/// points at installers/iter_engine/, so an old link says where to go. Served in
/// both webui modes; it holds no secrets, so no sign-in.
const ENGINE_SETUP_SH: &[u8] = include_bytes!("../iter_engine_setup.sh");
async fn engine_setup_sh() -> axum::response::Response {
    use axum::response::IntoResponse;
    ([(axum::http::header::CONTENT_TYPE, "text/x-shellscript; charset=utf-8")], ENGINE_SETUP_SH).into_response()
}

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
        eprintln!("unknown backend '{}': iter stores everything in ArangoDB (--backend arango)", args.backend);
        std::process::exit(2);
    }
    let store: Arc<dyn storage::Storage> = Arc::new(open_arango(&args).await);
    if let Some(Cmd::EngineKey(EngineKeyCmd::Add { public_key, engine })) = &args.cmd {
        match keys::add_local(store.as_ref(), public_key.trim(), engine.trim()).await {
            Ok(msg) => println!("{msg}"),
            Err(e) => {
                eprintln!("iter_data engine-key add: {e}");
                std::process::exit(1);
            }
        }
        return;
    }
    if let Some(Cmd::Defaults(DefaultsCmd::Add)) = &args.cmd {
        let added = match settings::bootstrap(store.as_ref()).await {
            Ok(()) => defaults::add_missing(store.as_ref()).await,
            Err(e) => Err(e),
        };
        match added {
            Ok(a) if a.is_empty() => println!("every default agent and tooling row is already here"),
            Ok(a) => println!("added: {}", a.join(", ")),
            Err(e) => {
                eprintln!("iter_data defaults add: {e}");
                std::process::exit(1);
            }
        }
        return;
    }

    bootstrap_admin(store.as_ref()).await;
    // the settings graph (§7): placeholders, iter_data:self, providers,
    // workitem types
    if let Err(e) = settings::bootstrap(store.as_ref()).await {
        eprintln!("[iter_data] settings graph bootstrap failed: {e}");
    }
    // the default agents and tooling, once per database (defaults.rs)
    if let Err(e) = defaults::add_once(store.as_ref()).await {
        eprintln!("[iter_data] could not add the default agents: {e}");
    }

    rag::guide::ingest_at_startup(store.clone());
    let secret = auth::load_secret("ITER_JWT_SECRET", &args.secret_file);
    let dir = std::path::Path::new(&args.secret_file).parent().filter(|d| !d.as_os_str().is_empty()).unwrap_or(std::path::Path::new(".")).to_path_buf();
    let engine_secret = auth::load_secret("ITER_ENGINE_SESSION_SECRET", &dir.join("engine_session.secret").to_string_lossy());
    let tls_mode = if args.tls.trim().is_empty() { std::env::var("ITER_TLS").unwrap_or_else(|_| "auto".into()) } else { args.tls.clone() };
    let tls = match tls::TlsState::setup(&tls_mode, &args.tls_cert, &args.tls_key, &dir, &args.tls_name) {
        Ok(t) => Arc::new(t),
        Err(e) => {
            eprintln!("[iter_data] TLS: {e}");
            std::process::exit(2);
        }
    };
    let state = Arc::new(AppState { store, secret, engine_secret, tls: tls.clone(), state_dir: dir.clone() });

    rag::embed::set_model_dir(&args.embed_model);
    match rag::embed::find_model_dir() {
        Some(d) => println!("[iter_data] GraphRAG embedding model: {} ({})", rag::embed::MODEL_NAME, d.display()),
        None => eprintln!("[iter_data] GraphRAG embedding model {} not found — ingestion and search will refuse (installers/fetch_model.sh)", rag::embed::MODEL_NAME),
    }

    let mut app = api::router(state.clone())
        .merge(mcp::routes(state.clone()))
        .route("/iter_engine_setup.sh", axum::routing::get(engine_setup_sh))
        .layer(
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

    let listen = if !args.listen.is_empty() {
        args.listen.clone()
    } else {
        let port = std::env::var("ITER_PORT").ok().filter(|p| !p.trim().is_empty()).unwrap_or_else(|| "8400".into());
        format!("127.0.0.1:{}", port.trim())
    };
    let addr: std::net::SocketAddr = listen.parse().unwrap_or_else(|e| panic!("--listen {listen}: {e}"));
    // display names in paths resolve to ids before routing (names.rs)
    let app = tower::Layer::layer(&axum::middleware::from_fn_with_state(state, names::rewrite), app);
    // the peer address reaches handlers (login rate limits per address)
    let make = axum::ServiceExt::<axum::extract::Request>::into_make_service_with_connect_info::<std::net::SocketAddr>(app.clone());
    match tls.rustls_config().await {
        Err(e) => {
            eprintln!("[iter_data] TLS: {e}");
            std::process::exit(2);
        }
        Ok(None) => {
            println!("[iter_data] listening on http://{listen} (backend: arango, TLS off)");
            if !addr.ip().is_loopback() {
                eprintln!("[iter_data] WARNING: plain HTTP on {listen}: passwords, session tokens and data cross the network unencrypted (--tls auto, or a TLS proxy in front)");
            }
            let listener = tokio::net::TcpListener::bind(addr).await.expect("bind");
            axum::serve(listener, make).await.expect("serve");
        }
        Ok(Some(cfg)) => {
            println!("[iter_data] listening on https://{listen} (backend: arango, TLS {})", tls.mode.as_str());
            println!("[iter_data] TLS certificate fingerprint (engines pin it; also in the webui's Engine keys screen): sha256:{}", tls.fingerprint());
            // axum_server hands hyper's body; the router takes axum's
            let svc = tower::ServiceBuilder::new()
                .map_request(|r: axum::http::Request<hyper::body::Incoming>| r.map(axum::body::Body::new))
                .service(app);
            let make = axum::ServiceExt::<axum::http::Request<hyper::body::Incoming>>::into_make_service_with_connect_info::<std::net::SocketAddr>(svc);
            axum_server::bind_rustls(addr, cfg).serve(make).await.expect("serve");
        }
    }
}

/// ArangoDB from the environment (which wins over the flags):
/// ARANGO_URL, ARANGO_DB, ARANGO_USER [iter_user] and ARANGO_PASSWORD;
/// ARANGO_ROOT_PASSWORD only to set that user and database up;
/// ARANGO_CERT_FINGERPRINT (sha256:…, comma-separated) or ARANGO_CA_CERT (a
/// PEM file) for HTTPS; ARANGO_ALLOW_PLAIN_HTTP=1 for http:// to another
/// machine on a private network.
async fn open_arango(args: &Args) -> arango::ArangoBackend {
    let env = |k: &str| std::env::var(k).ok().map(|v| v.trim().to_string()).filter(|v| !v.is_empty());
    let fail = |msg: String| -> ! {
        eprintln!("[iter_data] arango: {msg}");
        std::process::exit(2);
    };
    let mut pins = Vec::new();
    for p in env("ARANGO_CERT_FINGERPRINT").unwrap_or_default().split([',', ' ']).filter(|p| !p.is_empty()) {
        match iter_core::enginekey::normalize_fingerprint(p) {
            Some(fp) => pins.push(fp),
            None => fail(format!("ARANGO_CERT_FINGERPRINT {p:?} is not a sha256 fingerprint")),
        }
    }
    let ca_pem = env("ARANGO_CA_CERT").map(|path| std::fs::read(&path).unwrap_or_else(|e| fail(format!("ARANGO_CA_CERT {path}: {e}"))));
    let cfg = arango::ArangoConfig {
        url: env("ARANGO_URL").or_else(|| Some(args.arango_url.trim().to_string()).filter(|u| !u.is_empty())).unwrap_or_else(|| "http://127.0.0.1:8529".into()),
        db: env("ARANGO_DB").unwrap_or_else(|| args.arango_db.clone()),
        user: env("ARANGO_USER").unwrap_or_else(|| arango::DEFAULT_USER.into()),
        password: std::env::var("ARANGO_PASSWORD").unwrap_or_default(),
        root_password: std::env::var("ARANGO_ROOT_PASSWORD").unwrap_or_default(),
        allow_plain_http: env("ARANGO_ALLOW_PLAIN_HTTP").is_some_and(|v| matches!(v.to_ascii_lowercase().as_str(), "1" | "true" | "yes")),
        pins,
        ca_pem,
    };
    let tls = if !cfg.url.to_ascii_lowercase().starts_with("https://") {
        "plain http"
    } else if !cfg.pins.is_empty() {
        "https, pinned certificate"
    } else if cfg.ca_pem.is_some() {
        "https, ARANGO_CA_CERT"
    } else {
        "https, public CAs"
    };
    match arango::ArangoBackend::open(&cfg).await {
        Ok(b) => {
            println!("[iter_data] arango {} ({tls}) database '{}' as user '{}' ready", cfg.url, cfg.db, cfg.user);
            b
        }
        Err(e) => fail(format!("init failed: {e}")),
    }
}

/// The first admin's id, made on an empty database.
pub const BOOTSTRAP_ADMIN: &str = "admin";
/// Written beside the secret file once that admin replaced the first-boot
/// password; the installers then remove ITER_ADMIN_PASSWORD from the env
/// file (and SSM on AWS).
pub const ADMIN_PASSWORD_CHANGED: &str = "admin_password_changed";

/// The first admin, on an empty database: ITER_ADMIN_PASSWORD (or a
/// generated password printed once), which must be changed at the first
/// sign-in.
pub(crate) async fn bootstrap_admin(store: &dyn storage::Storage) {
    let users = store.scan("webui_user").await.unwrap_or_default();
    if !users.is_empty() {
        if std::env::var("ITER_ADMIN_PASSWORD").is_ok_and(|p| !p.trim().is_empty()) {
            println!("[iter_data] ITER_ADMIN_PASSWORD is set but only seeds an empty database; remove it from the env file once the admin has chosen their own");
        }
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
        "user": BOOTSTRAP_ADMIN, "email": "", "role": "admin", "pwhash": pwhash, "must_change_password": true,
        "tokenver": 1, "css": "", "pubkey": "", "settings": {}, "authz": {}
    });
    store.put("webui_user", BOOTSTRAP_ADMIN, "-", &row).await.expect("bootstrap admin");
    let _ = store.bump_seq(GLOBAL, "webui_user").await;
    if generated {
        println!("[iter_data] bootstrapped user 'admin' with password: {password}");
        println!("[iter_data] (set ITER_ADMIN_PASSWORD to control this); it must be changed at the first sign-in");
    } else {
        println!("[iter_data] bootstrapped user 'admin' (password from ITER_ADMIN_PASSWORD; it must be changed at the first sign-in)");
    }
}
