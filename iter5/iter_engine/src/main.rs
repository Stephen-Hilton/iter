//! iter_engine — the local engine binary for iter5: ONE engine per machine
//! serving every project the settings graph assigns to it (spec §4).
//!
//! Startup: `iter_engine --data-url <url> --env-file <path> [--name <name>]`.
//! The env file holds `ITER_ENGINE_TOKEN` (the iter_data credential) plus the
//! LLM tokens (`token_envar`s) and other secrets; nothing else is read from
//! disk — projects, checkouts and accounts come from
//! `GET /api/engines/{name}/assignments`.

mod assign;
mod client;
mod datasync;
mod cli;
mod dedup;
mod engine;
mod envstore;
mod filesync;
mod gate;
mod init;
mod prompt;
mod provider;
mod rag;
mod sweep;
mod sync;
mod usage;
mod work;

use base64::Engine as _;
use clap::Parser;
use client::Api;
use serde_json::json;

#[derive(clap::Subcommand, Debug)]
enum Cmd {
    /// agent-facing verbs (`iter add|ask|reject|block|doc|critreview|capability|status`); the
    /// engine installs `{topdir}/.iter/bin/iter` as a shim to this
    Cli(cli::CliArgs),
}

#[derive(Parser, Debug)]
#[command(name = "iter_engine", about = "iter5 engine: one per machine, serves every project assigned to it")]
struct Args {
    #[command(subcommand)]
    cmd: Option<Cmd>,
    /// iter_data URL (else $ITER_DATA_URL)
    #[arg(long)]
    data_url: Option<String>,
    /// env file: ITER_ENGINE_TOKEN, the accounts' token_envars, other secrets
    #[arg(long, default_value = ".env")]
    env_file: String,
    /// engine name (default: this machine's short hostname)
    #[arg(long)]
    name: Option<String>,
    /// run N ticks then drain and exit (0 = forever); used by tests
    #[arg(long, default_value_t = 0)]
    ticks: u64,
    /// create a user keypair + register the pubkey, then exit
    #[arg(long)]
    adduser: Option<String>,
    /// sign an approval for a workitem id (or unique id prefix), then exit
    #[arg(long)]
    approve: Option<String>,
    /// private key path for --approve (else ITER_APPROVE_KEYPATH env)
    #[arg(long)]
    pvtkeypath: Option<String>,
    /// approving user name for --approve (default: key file stem)
    #[arg(long)]
    user: Option<String>,
    /// print the assigned accounts' envars and whether each is set, then exit
    #[arg(long, default_value_t = false)]
    accounts: bool,
    /// like --accounts, but also ask each account's provider for its live
    /// 5h/7d usage% and write the snapshots
    #[arg(long, default_value_t = false)]
    probe: bool,
    /// validate a question-widget json file, then exit
    #[arg(long)]
    question_widget: Option<String>,
    /// append a "doc" detail row to a workitem (id or unique prefix; works on
    /// closed items too), then exit; text from --text or --file
    #[arg(long)]
    doc: Option<String>,
    /// doc text for --doc
    #[arg(long)]
    text: Option<String>,
    /// read the doc text from a file ("-" = stdin) for --doc
    #[arg(long)]
    file: Option<String>,
}

/// The engine's own credential, read from the env file.
pub const ENGINE_TOKEN_VAR: &str = "ITER_ENGINE_TOKEN";

fn main() {
    let args = Args::parse();
    if let Some(Cmd::Cli(cli_args)) = args.cmd {
        return cli::run(cli_args);
    }
    // helpers that need no iter_data
    if let Some(path) = &args.question_widget {
        return question_widget(path);
    }
    // seed the env store (and, once, the process environment) from the env
    // file; token reads from here on go through envstore::get, and the file
    // is re-read while running (account hot reload)
    envstore::init(&args.env_file, &[ENGINE_TOKEN_VAR]);
    let token = envstore::get(ENGINE_TOKEN_VAR).unwrap_or_default();
    let Some(data_url) = resolve_data_url(args.data_url.as_deref(), std::env::var("ITER_DATA_URL").ok().as_deref()) else {
        eprintln!("usage: iter_engine --data-url <iter_data url> --env-file <path> [--name <engine name>]");
        eprintln!("(no --data-url given and $ITER_DATA_URL is not set)");
        std::process::exit(2);
    };
    let api = Api::new(&data_url, &token);
    let name = args.name.clone().map(|n| n.trim().to_string()).filter(|n| !n.is_empty()).unwrap_or_else(short_hostname);

    if let Some(user) = &args.adduser {
        return adduser(&api, user);
    }
    if let Some(workid) = &args.approve {
        return approve(&api, workid, args.pvtkeypath.as_deref(), args.user.as_deref());
    }
    if args.accounts || args.probe {
        return accounts(&api, &name, args.probe);
    }
    if let Some(prefix) = &args.doc {
        return doc(&api, prefix, args.text.as_deref(), args.file.as_deref());
    }

    if token.is_empty() {
        eprintln!("no engine token: set {ENGINE_TOKEN_VAR} in {} (mint via POST /api/users/<engine-user>/token as admin)", args.env_file);
        std::process::exit(2);
    }
    let backend = api.get("/health").ok().and_then(|h| h["backend"].as_str().map(String::from)).unwrap_or_else(|| "unreachable".into());
    let mut rt = engine::EngineRuntime::new(api, name.clone(), args.env_file.clone());
    if args.ticks > 0 {
        rt.max_ticks = Some(args.ticks);
    }
    println!("[engine] {name} starting against {data_url} (iter_data backend: {backend}, env file {})", args.env_file);
    rt.run();
}

/// `--data-url` beats `$ITER_DATA_URL`; blank values do not count.
fn resolve_data_url(flag: Option<&str>, env: Option<&str>) -> Option<String> {
    [flag, env].into_iter().flatten().map(str::trim).find(|u| !u.is_empty()).map(String::from)
}

/// "mbp.local" -> "mbp"; "" -> "engine".
fn short_host(raw: &str) -> String {
    let h = raw.trim().split('.').next().unwrap_or("").trim().to_string();
    if h.is_empty() { "engine".into() } else { h }
}

/// This machine's short hostname: the default engine name (spec §4.1).
fn short_hostname() -> String {
    let raw = std::process::Command::new("hostname")
        .output()
        .ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).to_string())
        .filter(|h| !h.trim().is_empty())
        .or_else(|| std::env::var("HOSTNAME").ok())
        .unwrap_or_default();
    short_host(&raw)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn data_url_precedence() {
        assert_eq!(resolve_data_url(Some("http://f"), Some("http://e")).as_deref(), Some("http://f"));
        assert_eq!(resolve_data_url(None, Some("http://e")).as_deref(), Some("http://e"));
        assert_eq!(resolve_data_url(Some(" "), Some("")), None);
    }

    #[test]
    fn default_name_is_the_short_hostname() {
        assert_eq!(short_host("mbp.local\n"), "mbp");
        assert_eq!(short_host("fhserver.lan.example.com"), "fhserver");
        assert_eq!(short_host("  "), "engine");
        assert!(!short_hostname().contains('.'));
    }

    /// Startup args (§4.1): --data-url, --env-file, --name; --config is gone.
    #[test]
    fn startup_args_parse_and_config_is_gone() {
        let a = Args::try_parse_from(["iter_engine", "--data-url", "http://x:8400", "--env-file", "/e/.env", "--name", "mbp", "--ticks", "3"]).unwrap();
        assert_eq!((a.data_url.as_deref(), a.env_file.as_str(), a.name.as_deref(), a.ticks), (Some("http://x:8400"), "/e/.env", Some("mbp"), 3));
        let d = Args::try_parse_from(["iter_engine"]).unwrap();
        assert_eq!((d.data_url, d.env_file.as_str(), d.name), (None, ".env", None));
        assert!(Args::try_parse_from(["iter_engine", "--config", ".iter/config.json"]).is_err(), "--config was removed");
        // the agent shim's subcommand still parses without any engine args
        assert!(matches!(Args::try_parse_from(["iter_engine", "cli", "status"]).map(|a| a.cmd.is_some()), Ok(true)));
    }
}

fn question_widget(path: &str) {
    let content = if path == "-" {
        use std::io::Read;
        let mut s = String::new();
        std::io::stdin().read_to_string(&mut s).ok();
        s
    } else {
        std::fs::read_to_string(path).unwrap_or_default()
    };
    match serde_json::from_str::<serde_json::Value>(&content) {
        Ok(v) => {
            let errs = iter_core::widget::validate(&v);
            if errs.is_empty() {
                println!("OK: widget is valid");
            } else {
                for e in &errs {
                    println!("INVALID: {e}");
                }
                std::process::exit(1);
            }
        }
        Err(e) => {
            println!("INVALID: not json: {e}");
            std::process::exit(1);
        }
    }
}

/// `iter --adduser "stephen"` (decided 2026-09-01): keypair to
/// .iter/users/<name>.pem (add-only), gitignore the users dir, register the
/// pubkey with iter_data when a connection is available.
fn adduser(api: &Api, name: &str) {
    use ed25519_dalek::SigningKey;
    use ed25519_dalek::pkcs8::EncodePrivateKey;

    let users_dir = std::path::Path::new(".iter/users");
    std::fs::create_dir_all(users_dir).expect("create .iter/users");
    let keypath = users_dir.join(format!("{name}.pem"));
    if keypath.exists() {
        eprintln!("refusing: {} already exists (reset flow: delete it, re-run, have an admin paste the new pubkey)", keypath.display());
        std::process::exit(1);
    }

    let signing = SigningKey::generate(&mut rand::rngs::OsRng);
    let pem = signing.to_pkcs8_pem(Default::default()).expect("encode pem");
    std::fs::write(&keypath, pem.as_bytes()).expect("write pem");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&keypath, std::fs::Permissions::from_mode(0o600));
    }
    println!("wrote private key: {}", keypath.display());

    // .iter/.gitignore must ignore everything under users/
    let gi_path = std::path::Path::new(".iter/.gitignore");
    let existing = std::fs::read_to_string(gi_path).unwrap_or_default();
    if !existing.lines().any(|l| l.trim() == "users/") {
        let mut content = existing;
        if !content.is_empty() && !content.ends_with('\n') {
            content.push('\n');
        }
        content.push_str("users/\n");
        std::fs::write(gi_path, content).expect("write .iter/.gitignore");
        println!("ensured .iter/.gitignore ignores users/");
    }

    let pubkey_b64 =
        base64::engine::general_purpose::STANDARD.encode(signing.verifying_key().to_bytes());
    println!("public key (base64): {pubkey_b64}");

    match api.post(&format!("/api/users/{name}/pubkey"), &json!({"pubkey": pubkey_b64})) {
        Ok(_) => println!("registered pubkey for '{name}' in iter_data"),
        Err(e) => println!(
            "could not register with iter_data ({e}); have an admin paste the pubkey via the users page"
        ),
    }
}

fn approve(api: &Api, workid_prefix: &str, pvtkeypath: Option<&str>, user: Option<&str>) {
    use ed25519_dalek::SigningKey;
    use ed25519_dalek::pkcs8::DecodePrivateKey;
    use ed25519_dalek::Signer;

    let keypath = pvtkeypath
        .map(String::from)
        .or_else(|| std::env::var("ITER_APPROVE_KEYPATH").ok())
        .unwrap_or_default();
    if keypath.is_empty() {
        eprintln!("no key: pass --pvtkeypath or set ITER_APPROVE_KEYPATH in your .env");
        std::process::exit(2);
    }
    let pem = std::fs::read_to_string(&keypath).unwrap_or_else(|e| {
        eprintln!("cannot read {keypath}: {e}");
        std::process::exit(2);
    });
    let signing = SigningKey::from_pkcs8_pem(&pem).unwrap_or_else(|e| {
        eprintln!("cannot parse {keypath}: {e}");
        std::process::exit(2);
    });
    let username = user
        .map(String::from)
        .or_else(|| {
            std::path::Path::new(&keypath)
                .file_stem()
                .map(|s| s.to_string_lossy().to_string())
        })
        .unwrap_or_default();

    let (project, id) = find_workitem(api, workid_prefix);
    let sig = signing.sign(id.as_bytes());
    let sig_b64 = base64::engine::general_purpose::STANDARD.encode(sig.to_bytes());
    match api.post(
        &format!("/api/projects/{project}/workitems/{id}/approve"),
        &json!({"user": username, "signature": sig_b64}),
    ) {
        Ok(_) => println!("approved {id} in '{project}' as {username}"),
        Err(e) => {
            eprintln!("approval rejected: {e}");
            std::process::exit(1);
        }
    }
}

/// Find one workitem by id or unique prefix across visible projects; exits
/// on zero or ambiguous matches.
fn find_workitem(api: &Api, workid_prefix: &str) -> (String, String) {
    let projects = api.get("/api/projects").ok().and_then(|v| v.as_array().cloned()).unwrap_or_default();
    let mut matches: Vec<(String, String)> = Vec::new();
    for p in &projects {
        let pname = iter_core::settings::record_id(p);
        if let Ok(items) = api.get(&format!("/api/projects/{pname}/workitems")) {
            for i in items.as_array().cloned().unwrap_or_default() {
                let id = i.get("id").and_then(|x| x.as_str()).unwrap_or("").to_string();
                if id.starts_with(workid_prefix) || id.replace('-', "").starts_with(workid_prefix) {
                    matches.push((pname.clone(), id));
                }
            }
        }
    }
    match matches.len() {
        0 => {
            eprintln!("no workitem matches '{workid_prefix}'");
            std::process::exit(1);
        }
        1 => matches.remove(0),
        n => {
            eprintln!("'{workid_prefix}' is ambiguous ({n} matches); use more of the id");
            std::process::exit(1);
        }
    }
}

/// `iter --doc <id> --text "..."` / `--file notes.md` (decided 2026-09-03):
/// append a "doc" detail row.  This is the one write allowed on a closed
/// item, so an agent can leave a closeout note on a finished prerequisite.
fn doc(api: &Api, workid_prefix: &str, text: Option<&str>, file: Option<&str>) {
    let body = match (text, file) {
        (Some(t), _) => t.to_string(),
        (None, Some("-")) => {
            use std::io::Read;
            let mut s = String::new();
            std::io::stdin().read_to_string(&mut s).ok();
            s
        }
        (None, Some(path)) => std::fs::read_to_string(path).unwrap_or_else(|e| {
            eprintln!("cannot read {path}: {e}");
            std::process::exit(2);
        }),
        (None, None) => {
            eprintln!("--doc needs --text \"...\" or --file <path|->");
            std::process::exit(2);
        }
    };
    if body.trim().is_empty() {
        eprintln!("doc text is empty");
        std::process::exit(2);
    }
    let (project, id) = find_workitem(api, workid_prefix);
    match api.post(
        &format!("/api/projects/{project}/workitems/{id}/details"),
        &json!({"key": "doc", "valuetype": "text", "value": body.trim_end()}),
    ) {
        Ok(row) => println!(
            "doc #{} appended to {id} in '{project}'",
            row.get("order").and_then(|o| o.as_i64()).unwrap_or(-1)
        ),
        Err(e) => {
            eprintln!("doc rejected: {e}");
            std::process::exit(1);
        }
    }
}

/// `--accounts` / `--probe`: every account this engine's assignments name,
/// per project, whether its token is set in the env file, and (probe) its
/// provider's live usage written to the snapshot.
fn accounts(api: &Api, engine: &str, probe: bool) {
    let asg = match api.get(&format!("/api/engines/{engine}/assignments")).map_err(|e| e.to_string()).and_then(|v| assign::Assignments::parse(&v)) {
        Ok(a) => a,
        Err(e) => {
            println!("cannot read the assignments of engine '{engine}': {e}");
            std::process::exit(1);
        }
    };
    for p in &asg.projects {
        println!("project: {} (topdir {}{})", p.project, p.topdir, if p.read_only { ", read-only" } else { "" });
        for a in asg.accounts_of(p) {
            let tok = envstore::get(&a.token_envar);
            println!(
                "  {} [{}]: {} = {}",
                a.name, a.provider, a.token_envar,
                if tok.is_some() { "SET" } else { "NOT SET (add to your env file)" }
            );
            if probe && tok.is_some() {
                match provider::record_usage(&a.provider, &a.name, tok.as_deref(), None) {
                    Some(u) => println!(
                        "      usage: 5h {:.0}% 7d {:.0}% ({}{}) -> {}",
                        u.five_hour_pct, u.seven_day_pct, u.status,
                        if u.is_using_overage { ", OVERAGE" } else { "" },
                        usage::snapshot_path(&a.name).display()
                    ),
                    None => println!("      probe FAILED (no usage from provider {})", a.provider),
                }
            }
        }
    }
    if asg.projects.is_empty() {
        println!("no projects assigned to engine '{engine}' (connect it with a serves edge in the settings graph)");
    }
}
