//! `iter_engine key join | new | show | rotate | trust | status | session`
//! (2026-10-11, rotate/show/trust 2026-10-17): the engine machine's own
//! Ed25519 key, the way it proves who it is to iter_data (shapes and the
//! sign-in: iter_core::enginekey), and the server certificate it trusts
//! (trust.rs). The private key is written here, owner-only, and never
//! leaves the machine: no command prints it; only the public key is shown
//! and pasted into the webui's Engine keys screen.
//!
//! `new` writes the key file (`<iter dir>/iter_engine/secrets/engine.key`) when there is none, else
//! the next key beside it (`engine.key.next`), which a running engine signs
//! in with once an admin rotates it in the Engine keys screen (client.rs
//! `adopt_next`). `rotate` does the same with no admin step: it makes a new
//! key and sends iter_data a replacement signed by the current key; a
//! revoked key is refused (an admin sets that engine up again). `show`
//! prints the public key; `trust` pins iter_data's certificate (or checks it
//! against a fingerprint given, or switches to certificate-authority checks
//! for a server behind a TLS proxy); `status` signs in with each key and
//! shows the trusted certificate; `session` prints a session token for a
//! script's own API calls.
//!
//! `join <code>` (2026-10-23) sets a machine up from a join code made in the
//! webui (iter_core::enginekey::JoinCode): it trusts iter_data as the code
//! says, makes the key when there is none, registers it with the code's
//! one-time secret and signs in once, so nothing is pasted into the webui.

use crate::client::{Api, load_key, next_key_file};
use ed25519_dalek::SigningKey;
use ed25519_dalek::pkcs8::EncodePrivateKey;
use iter_core::enginekey;
use std::path::{Path, PathBuf};

#[derive(clap::Args, Debug)]
pub struct KeyArgs {
    #[command(subcommand)]
    action: Action,
}

#[derive(clap::Subcommand, Debug)]
enum Action {
    /// set this machine up from a join code (the webui's Engine keys → Add
    /// engine): trust iter_data as the code says, make the key when there is
    /// none, register it, sign in. Prints `engine=<name>` last
    Join {
        /// the join code (iterjoin1_…)
        code: String,
        /// this engine's name, when the code names none [this machine's short hostname]
        #[arg(long)]
        name: Option<String>,
        /// key file [<iter dir>/iter_engine/secrets/engine.key]
        #[arg(long)]
        key_file: Option<String>,
    },
    /// make this machine's engine key (the next key when one exists) and
    /// print its public key, to add in the webui's Engine keys screen
    New {
        /// key file [<iter dir>/iter_engine/secrets/engine.key]
        #[arg(long)]
        key_file: Option<String>,
        /// the engine name to add it under [this machine's short hostname]
        #[arg(long)]
        name: Option<String>,
    },
    /// sign in with the key and print the session token, for a script's own
    /// API calls (the installers/iter_engine scripts, deploy.ps1); exit 1 when iter_data refuses
    Session {
        /// key file [<iter dir>/iter_engine/secrets/engine.key]
        #[arg(long)]
        key_file: Option<String>,
        /// iter_data URL [$ITER_DATA_URL]
        #[arg(long)]
        data_url: Option<String>,
    },
    /// print this machine's public key and its id (never the private key)
    Show {
        /// key file [<iter dir>/iter_engine/secrets/engine.key]
        #[arg(long)]
        key_file: Option<String>,
    },
    /// make a new key and swap it in: iter_data accepts the replacement
    /// because the current key signs it; the old key keeps working for an
    /// hour for agents still running. A revoked key cannot rotate.
    Rotate {
        /// key file [<iter dir>/iter_engine/secrets/engine.key]
        #[arg(long)]
        key_file: Option<String>,
        /// iter_data URL [$ITER_DATA_URL]
        #[arg(long)]
        data_url: Option<String>,
    },
    /// trust iter_data's TLS certificate: with --fingerprint, only if it is
    /// that one (shown in the webui's Engine keys screen); without, the one it
    /// presents now (printed, to compare); --ca for a server behind a TLS proxy
    /// with a real certificate; --forget to drop the trust
    Trust {
        /// iter_data URL [$ITER_DATA_URL]
        #[arg(long)]
        data_url: Option<String>,
        /// the expected certificate fingerprint (sha256:…, colons optional)
        #[arg(long)]
        fingerprint: Option<String>,
        /// check the certificate with certificate authorities instead of pinning
        #[arg(long, default_value_t = false)]
        ca: bool,
        /// forget the trust entry for this server
        #[arg(long, default_value_t = false)]
        forget: bool,
    },
    /// fetch iter_data's local certificate authority (the one its own
    /// certificate is signed with, `--tls auto`) over the trusted connection,
    /// for the installers to add to this machine's trust stores so a browser
    /// here shows the webui as secure; exit 1 when the server has none or is
    /// not trusted yet
    Ca {
        /// iter_data URL [$ITER_DATA_URL]
        #[arg(long)]
        data_url: Option<String>,
        /// write the PEM here instead of printing it
        #[arg(long)]
        out: Option<String>,
    },
    /// sign in with each key and say whether iter_data accepts it; exit 0
    /// when the current key is accepted
    Status {
        /// key file [<iter dir>/iter_engine/secrets/engine.key]
        #[arg(long)]
        key_file: Option<String>,
        /// iter_data URL [$ITER_DATA_URL]
        #[arg(long)]
        data_url: Option<String>,
    },
}

/// The engine's own folder (`<iter dir>/iter_engine`, `/opt/iter/iter_engine`
/// by default): `bin/`, `config/` (engine.conf, server_trust.json),
/// `secrets/` (engine.key, .env), `models/`, `logs/`. `$ITER_HOME` names it
/// (the installers' services set it); else the folder above this binary's
/// `bin/` when it holds `config/` or `secrets/`; else the old `~/.iter`.
pub fn iter_home() -> PathBuf {
    if let Some(h) = std::env::var_os("ITER_HOME").filter(|h| !h.is_empty()) {
        return PathBuf::from(h);
    }
    if let Some(h) = std::env::current_exe().ok().and_then(|e| e.canonicalize().ok()).and_then(|e| home_of_exe(&e)) {
        return h;
    }
    iter_core::platform::home_dir().unwrap_or_else(|| PathBuf::from(".")).join(".iter")
}

/// `<home>/bin/iter_engine` -> `<home>`, when `<home>` is laid out as the
/// engine's folder (has `config/` or `secrets/`).
fn home_of_exe(exe: &Path) -> Option<PathBuf> {
    let bin = exe.parent()?;
    if bin.file_name()? != "bin" {
        return None;
    }
    let home = bin.parent()?;
    (home.join("config").is_dir() || home.join("secrets").is_dir()).then(|| home.to_path_buf())
}

/// `<home>/<sub>/<name>`, or the old flat `<home>/<name>` while the home is
/// still flat (an install from before 2026-10-18 that has not been moved
/// yet): its own files, and new ones too, so it stays one layout.
pub fn home_file(sub: &str, name: &str) -> PathBuf {
    home_file_in(&iter_home(), sub, name)
}

fn home_file_in(home: &Path, sub: &str, name: &str) -> PathBuf {
    let new = home.join(sub).join(name);
    let old = home.join(name);
    if new.exists() {
        return new;
    }
    if old.exists() {
        return old;
    }
    let laid_out = home.join("config").is_dir() || home.join("secrets").is_dir();
    let flat = ["engine.key", ".env", "server_trust.json", "engine.conf"].iter().any(|f| home.join(f).exists());
    if flat && !laid_out { old } else { new }
}

pub fn default_key_file() -> PathBuf {
    home_file("secrets", "engine.key")
}

pub fn run(args: KeyArgs) {
    match args.action {
        Action::Join { code, name, key_file } => {
            let file = key_file.map(PathBuf::from).unwrap_or_else(default_key_file);
            std::process::exit(match join(&code, name.as_deref(), &file) {
                Ok(()) => 0,
                Err(e) => {
                    eprintln!("{e}");
                    1
                }
            })
        }
        Action::New { key_file, name } => {
            let file = key_file.map(PathBuf::from).unwrap_or_else(default_key_file);
            std::process::exit(match new(&file, &name.unwrap_or_else(crate::short_hostname)) {
                Ok(()) => 0,
                Err(e) => {
                    eprintln!("{e}");
                    1
                }
            })
        }
        Action::Session { key_file, data_url } => {
            let file = key_file.map(PathBuf::from).unwrap_or_else(default_key_file);
            let Some(url) = crate::resolve_data_url(data_url.as_deref(), std::env::var("ITER_DATA_URL").ok().as_deref()) else {
                eprintln!("no --data-url given and $ITER_DATA_URL is not set");
                std::process::exit(2);
            };
            match load_key(&file).map_err(|e| e.to_string()).and_then(|k| Api::new(&url, "").login_with(&k).map_err(|e| e.to_string())) {
                Ok((token, _)) => println!("{token}"),
                Err(e) => {
                    eprintln!("{e}");
                    std::process::exit(1);
                }
            }
        }
        Action::Show { key_file } => {
            let file = key_file.map(PathBuf::from).unwrap_or_else(default_key_file);
            match load_key(&file) {
                Ok(k) => {
                    let (pubkey, id) = public(&k);
                    println!("{pubkey}\n(id {id}; the private key stays in {})", file.display());
                }
                Err(e) => {
                    eprintln!("{e} (iter_engine key new makes it)");
                    std::process::exit(1);
                }
            }
        }
        Action::Rotate { key_file, data_url } => {
            let file = key_file.map(PathBuf::from).unwrap_or_else(default_key_file);
            let Some(url) = crate::resolve_data_url(data_url.as_deref(), std::env::var("ITER_DATA_URL").ok().as_deref()) else {
                eprintln!("no --data-url given and $ITER_DATA_URL is not set");
                std::process::exit(2);
            };
            std::process::exit(match rotate(&file, &url) {
                Ok(()) => 0,
                Err(e) => {
                    eprintln!("{e}");
                    1
                }
            })
        }
        Action::Trust { data_url, fingerprint, ca, forget } => {
            let Some(url) = crate::resolve_data_url(data_url.as_deref(), std::env::var("ITER_DATA_URL").ok().as_deref()) else {
                eprintln!("no --data-url given and $ITER_DATA_URL is not set");
                std::process::exit(2);
            };
            std::process::exit(match trust(&url, fingerprint.as_deref(), ca, forget) {
                Ok(()) => 0,
                Err(e) => {
                    eprintln!("{e}");
                    1
                }
            })
        }
        Action::Ca { data_url, out } => {
            let Some(url) = crate::resolve_data_url(data_url.as_deref(), std::env::var("ITER_DATA_URL").ok().as_deref()) else {
                eprintln!("no --data-url given and $ITER_DATA_URL is not set");
                std::process::exit(2);
            };
            std::process::exit(match server_ca(&url, out.as_deref()) {
                Ok(()) => 0,
                Err(e) => {
                    eprintln!("{e}");
                    1
                }
            })
        }
        Action::Status { key_file, data_url } => {
            let file = key_file.map(PathBuf::from).unwrap_or_else(default_key_file);
            let url = crate::resolve_data_url(data_url.as_deref(), std::env::var("ITER_DATA_URL").ok().as_deref());
            std::process::exit(status(&file, url.as_deref()));
        }
    }
}

/// Write a fresh key to `path` (owner-only on unix).
fn write_key(path: &Path) -> Result<SigningKey, String> {
    let key = SigningKey::generate(&mut rand::rngs::OsRng);
    let pem = key.to_pkcs8_pem(Default::default()).map_err(|e| e.to_string())?;
    if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    std::fs::write(path, pem.as_bytes()).map_err(|e| format!("{}: {e}", path.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).map_err(|e| e.to_string())?;
    }
    Ok(key)
}

fn public(key: &SigningKey) -> (String, String) {
    let bytes = key.verifying_key().to_bytes();
    (enginekey::format_public(&bytes), enginekey::key_id(&bytes))
}

fn new(file: &Path, engine: &str) -> Result<(), String> {
    let current = file.exists().then(|| load_key(file)).transpose()?;
    let target = if current.is_some() { next_key_file(file) } else { file.to_path_buf() };
    let replaced_next = (current.is_some() && target.exists()).then(|| load_key(&target).ok()).flatten();
    let key = write_key(&target)?;
    let (pubkey, id) = public(&key);
    println!("Engine key for engine \"{engine}\" (id {id}). Its public key:\n\n  {pubkey}\n");
    println!("The private key stays in {} and never leaves this machine; only the public key above is pasted anywhere.\n", target.display());
    match &current {
        None => println!("Add it: in the webui, 🔑 Engine keys (header, admins) → Add: paste the public key, engine name \"{engine}\"."),
        Some(cur) => {
            let (_, cur_id) = public(cur);
            println!("This engine keeps signing in with its current key (id {cur_id}) until iter_data accepts the new one;");
            println!("a running engine then switches to it within a minute, by itself, and it becomes {}.", file.display());
            println!("Add it: in the webui, 🔑 Engine keys → the row of key {cur_id} → rotate…: paste the public key.");
        }
    }
    if let Some(old) = replaced_next {
        println!("\n(It replaced an earlier next key, id {}, that was never taken up.)", public(&old).1);
    }
    Ok(())
}

/// A new key, swapped in by a replacement the current key signs; the new
/// key is written beside the old first, so a crash never loses both.
fn rotate(file: &Path, url: &str) -> Result<(), String> {
    let current = load_key(file)?;
    let next = next_key_file(file);
    let new = write_key(&next)?;
    let api = Api::new(url, "");
    if let Some(why) = api.refused() {
        return Err(why.to_string());
    }
    // sign in first: it makes (or checks) the trust in iter_data's certificate
    api.login_with(&current).map_err(|e| format!("the current key could not sign in, so it cannot rotate: {e}"))?;
    match api.rotate_key(&current, &new) {
        Ok((id, stops)) => {
            std::fs::rename(&next, file).map_err(|e| format!("iter_data took the new key ({id}) but it could not replace {}: {e}; move {} there by hand", file.display(), next.display()))?;
            println!("Rotated: this machine now signs in with key {id} ({}).", file.display());
            println!("The old key ({}) keeps working until {stops} for agents still running, then stops.", public(&current).1);
            println!("A running engine switches to the new key at its next sign-in, by itself.");
            Ok(())
        }
        Err(e) => {
            let _ = std::fs::remove_file(&next);
            Err(format!("iter_data refused the rotation: {e}"))
        }
    }
}

/// `key join`: trust, key, registration and a first sign-in from one code.
fn join(code: &str, name: Option<&str>, file: &Path) -> Result<(), String> {
    let c = enginekey::decode_join(code).ok_or("that is not a join code: copy the whole iterjoin1_… line from the webui (🔑 Engine keys → Add engine)")?;
    if c.expires < chrono::Utc::now().timestamp() {
        return Err("this join code has expired: make a new one in the webui (🔑 Engine keys → Add engine)".into());
    }
    let url = c.url.trim_end_matches('/').to_string();
    trust_from_code(&url, &c.pins)?;
    let engine = if !c.engine.is_empty() {
        if name.is_some_and(|n| n != c.engine) {
            println!("the join code is for engine \"{}\": that name is used", c.engine);
        }
        c.engine.clone()
    } else {
        name.map(String::from).unwrap_or_else(crate::short_hostname)
    };
    let api = Api::new(&url, "");
    let key = if file.exists() {
        let k = load_key(file)?;
        // set up already (an installer run again): the code is not needed
        if let Ok(v) = api.login_answer(&k) {
            let got = v["engine"].as_str().unwrap_or(&engine).to_string();
            println!("this machine's key (id {}) signs in to {url} already, as engine \"{got}\": the join code was not needed", public(&k).1);
            println!("engine={got}");
            return Ok(());
        }
        k
    } else {
        let k = write_key(file)?;
        println!("made this machine's engine key (id {}): {}", public(&k).1, file.display());
        k
    };
    let (got, already) = api.join(&c.secret, &key, &engine).map_err(|e| format!("iter_data refused the join code: {}", e.body.trim()))?;
    let id = public(&key).1;
    if already {
        println!("iter_data has this machine's key already (id {id}, engine \"{got}\"): the code was not needed and stays unused");
    } else {
        println!("iter_data registered this machine's key (id {id}) as engine \"{got}\"");
    }
    if got != engine {
        println!("(an engine named \"{engine}\" exists already, so this one is \"{got}\")");
    }
    api.login_with(&key).map_err(|e| format!("registered, but the first sign-in failed: {e}"))?;
    println!("signed in to {url}");
    println!("engine={got}");
    Ok(())
}

/// Trust `url` as a join code says: one of `pins` (iter_data's own
/// certificates) is pinned; a certificate a public authority issued for the
/// host (a TLS front) means mode ca; anything else is refused and nothing
/// is saved.
fn trust_from_code(url: &str, pins: &[String]) -> Result<(), String> {
    use crate::trust;
    if let Some(why) = trust::refuse_reason(url) {
        return Err(why);
    }
    if !url.to_ascii_lowercase().starts_with("https://") {
        return Ok(());
    }
    let (seen, chain) = trust::probe(url)?;
    if pins.contains(&seen) {
        trust::save(url, &trust::Entry { mode: "pin".into(), pins: pins.to_vec() })?;
        println!("trusted: {url} presents iter_data's own certificate sha256:{seen}, as the join code says");
    } else if trust::publicly_valid(url, &chain) {
        trust::save(url, &trust::Entry { mode: "ca".into(), pins: Vec::new() })?;
        println!("trusted: {url} presents a certificate a public authority issued for {} (a TLS front before iter_data)", trust::host(url));
    } else {
        return Err(format!(
            "{url} presents certificate sha256:{seen}, which is neither one the join code names ({}) nor one a public certificate authority issued for {}: NOT trusted, nothing was registered. Something else answers at that URL, or the code is from another iter_data",
            if pins.is_empty() { "none".to_string() } else { pins.iter().map(|p| format!("sha256:{p}")).collect::<Vec<_>>().join(", ") },
            trust::host(url)
        ));
    }
    Ok(())
}

/// Make, check or drop the trust in iter_data's certificate.
/// `key ca`: iter_data's local CA, fetched over the trusted connection.
fn server_ca(url: &str, out: Option<&str>) -> Result<(), String> {
    let pem = Api::new(url, "").get_trusted_text("/tls/ca.crt").map_err(|e| match e.status {
        404 => format!("{url} has no local certificate authority (it serves a certificate you brought, or is older than v2026.10.19)"),
        _ => format!("{url}: {}", e.body),
    })?;
    if !pem.trim_start().starts_with("-----BEGIN CERTIFICATE-----") {
        return Err(format!("{url}/tls/ca.crt did not return a certificate"));
    }
    match out {
        Some(path) => {
            std::fs::write(path, pem.as_bytes()).map_err(|e| format!("{path}: {e}"))?;
            println!("iter_data's local certificate authority: {path}");
        }
        None => print!("{pem}"),
    }
    Ok(())
}

fn trust(url: &str, fingerprint: Option<&str>, ca: bool, forget: bool) -> Result<(), String> {
    use crate::trust;
    if let Some(why) = trust::refuse_reason(url) {
        return Err(why);
    }
    if forget {
        let gone = trust::forget(url)?;
        println!("{}", if gone { "forgotten: the next sign-in trusts iter_data's certificate on first use again" } else { "nothing was trusted for that server" });
        return Ok(());
    }
    if !url.trim().to_ascii_lowercase().starts_with("https://") {
        println!("{url} is plain http to this machine: there is no certificate to trust");
        return Ok(());
    }
    if ca {
        trust::save(url, &trust::Entry { mode: "ca".into(), pins: Vec::new() })?;
        let api = Api::new(url, "");
        api.seen_fingerprint().map_err(|e| format!("trusting certificate authorities for {url}, but the connection failed: {e}"))?;
        println!("{url}: its certificate is now checked against certificate authorities (for a server behind a TLS proxy with a real certificate)");
        return Ok(());
    }
    let want = match fingerprint {
        Some(f) => Some(iter_core::enginekey::normalize_fingerprint(f).ok_or_else(|| format!("{f:?} is not a certificate fingerprint (64 hex, sha256:… and colons allowed)"))?),
        None => None,
    };
    // a fresh connection with nothing pinned, to see what the server presents
    let (seen, chain) = trust::probe(url)?;
    if let Some(w) = &want {
        if *w != seen {
            return Err(format!(
                "{url} presents certificate sha256:{seen}, not the sha256:{w} you gave: NOT trusted (something may be in between){}",
                if trust::publicly_valid(url, &chain) {
                    format!(". It is a certificate a public authority issued for {}, so iter_data is probably behind a TLS proxy: run this again without --fingerprint (or use a join code)", trust::host(url))
                } else {
                    String::new()
                }
            ));
        }
    } else if trust::publicly_valid(url, &chain) {
        trust::save(url, &trust::Entry { mode: "ca".into(), pins: Vec::new() })?;
        println!("trusted: {url} presents a certificate a public authority issued for {}: checked against certificate authorities (it may be renewed)", trust::host(url));
        return Ok(());
    }
    trust::save(url, &trust::Entry { mode: "pin".into(), pins: vec![seen.clone()] })?;
    if want.is_some() {
        println!("trusted: {url} presents the certificate you gave (sha256:{seen})");
    } else {
        println!("trusted: {url} presents certificate sha256:{seen}");
        println!("Compare it with the fingerprint in the webui's Engine keys screen; if they differ, run this again with --forget and find out why.");
    }
    Ok(())
}

/// One line per key; exit 0 when the current key is accepted, 1 when it is
/// refused or missing, 2 when iter_data cannot be reached or no URL was given.
fn status(file: &Path, data_url: Option<&str>) -> i32 {
    if let Some(url) = data_url {
        match crate::trust::load(url) {
            Some(e) if e.mode == "ca" => println!("server    {url}: certificate checked against certificate authorities"),
            Some(e) => println!("server    {url}: trusts certificate {}", e.pins.iter().map(|p| format!("sha256:{p}")).collect::<Vec<_>>().join(", ")),
            None if url.to_ascii_lowercase().starts_with("https://") => println!("server    {url}: no certificate trusted yet (the first sign-in trusts it; or `iter_engine key trust`)"),
            None => println!("server    {url}: plain http to this machine"),
        }
    }
    let mut code = 2;
    for (path, current) in [(file.to_path_buf(), true), (next_key_file(file), false)] {
        if !path.exists() {
            if current {
                println!("key       not found: {} (iter_engine key new makes it)", path.display());
                code = 1;
            }
            continue;
        }
        let label = if current { "key" } else { "next key" };
        let key = match load_key(&path) {
            Ok(k) => k,
            Err(e) => {
                println!("{label:<9} {e}");
                if current {
                    code = 1;
                }
                continue;
            }
        };
        let (_, id) = public(&key);
        let verdict = match data_url {
            None => "(no --data-url: not checked)".to_string(),
            Some(url) => match Api::new(url, "").login_with(&key) {
                Ok(_) => {
                    if current {
                        code = 0;
                    }
                    "accepted by iter_data".into()
                }
                Err(e) if e.status == 0 => format!("iter_data not reachable: {}", e.body),
                Err(e) => {
                    if current {
                        code = 1;
                    }
                    format!("refused (HTTP {}): {}", e.status, e.body.trim())
                }
            },
        };
        println!("{label:<9} id {id:<13} {verdict}");
    }
    code
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_first_key_is_current_and_a_later_one_is_next() {
        let dir = std::env::temp_dir().join(format!("iter-key-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let file = dir.join("engine.key");
        new(&file, "lap1").unwrap();
        let first = public(&load_key(&file).unwrap()).1;
        assert!(!next_key_file(&file).exists());
        new(&file, "lap1").unwrap();
        assert_eq!(public(&load_key(&file).unwrap()).1, first, "the current key is kept");
        assert_ne!(public(&load_key(&next_key_file(&file)).unwrap()).1, first);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(&file).unwrap().permissions().mode() & 0o777, 0o600);
        }
        assert_eq!(status(&file, None), 2);
        assert_eq!(status(&dir.join("missing.key"), None), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_home_is_the_folder_above_bin_once_it_is_laid_out() {
        let dir = std::env::temp_dir().join(format!("iter-home-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let home = dir.join("iter").join("iter_engine");
        std::fs::create_dir_all(home.join("bin")).unwrap();
        let exe = home.join("bin").join("iter_engine");
        assert_eq!(home_of_exe(&exe), None, "no config/ or secrets/ yet");
        std::fs::create_dir_all(home.join("secrets")).unwrap();
        assert_eq!(home_of_exe(&exe), Some(home.clone()));
        assert_eq!(home_of_exe(&dir.join("target").join("release").join("iter_engine")), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_flat_home_stays_flat_and_a_new_one_is_laid_out() {
        let dir = std::env::temp_dir().join(format!("iter-homefile-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let (flat, fresh) = (dir.join("flat"), dir.join("fresh"));
        std::fs::create_dir_all(&flat).unwrap();
        std::fs::create_dir_all(&fresh).unwrap();
        std::fs::write(flat.join("engine.key"), "k").unwrap();
        assert_eq!(home_file_in(&flat, "secrets", "engine.key"), flat.join("engine.key"));
        assert_eq!(home_file_in(&flat, "config", "server_trust.json"), flat.join("server_trust.json"), "a new file in a flat home stays flat");
        assert_eq!(home_file_in(&fresh, "config", "server_trust.json"), fresh.join("config").join("server_trust.json"));
        std::fs::create_dir_all(flat.join("secrets")).unwrap();
        assert_eq!(home_file_in(&flat, "config", "server_trust.json"), flat.join("config").join("server_trust.json"), "laid out: the new place");
        assert_eq!(home_file_in(&flat, "secrets", "engine.key"), flat.join("engine.key"), "until it moves, the old key is used");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
