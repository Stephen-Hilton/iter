//! `iter_engine key new | show | rotate | trust | status | session`
//! (2026-10-11, rotate/show/trust 2026-10-17): the engine machine's own
//! Ed25519 key, the way it proves who it is to iter_data (shapes and the
//! sign-in: iter_core::enginekey), and the server certificate it trusts
//! (trust.rs). The private key is written here, owner-only, and never
//! leaves the machine: no command prints it; only the public key is shown
//! and pasted into the webui's Engine keys screen.
//!
//! `new` writes the key file (`~/.iter/engine.key`) when there is none, else
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
    /// make this machine's engine key (the next key when one exists) and
    /// print its public key, to add in the webui's Engine keys screen
    New {
        /// key file [~/.iter/engine.key]
        #[arg(long)]
        key_file: Option<String>,
        /// the engine name to add it under [this machine's short hostname]
        #[arg(long)]
        name: Option<String>,
    },
    /// sign in with the key and print the session token, for a script's own
    /// API calls (the installers/iter_engine scripts, deploy.ps1); exit 1 when iter_data refuses
    Session {
        /// key file [~/.iter/engine.key]
        #[arg(long)]
        key_file: Option<String>,
        /// iter_data URL [$ITER_DATA_URL]
        #[arg(long)]
        data_url: Option<String>,
    },
    /// print this machine's public key and its id (never the private key)
    Show {
        /// key file [~/.iter/engine.key]
        #[arg(long)]
        key_file: Option<String>,
    },
    /// make a new key and swap it in: iter_data accepts the replacement
    /// because the current key signs it; the old key keeps working for an
    /// hour for agents still running. A revoked key cannot rotate.
    Rotate {
        /// key file [~/.iter/engine.key]
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
    /// sign in with each key and say whether iter_data accepts it; exit 0
    /// when the current key is accepted
    Status {
        /// key file [~/.iter/engine.key]
        #[arg(long)]
        key_file: Option<String>,
        /// iter_data URL [$ITER_DATA_URL]
        #[arg(long)]
        data_url: Option<String>,
    },
}

/// `~/.iter`: the engine's own files (key, env file, trusted server
/// certificates, binary, logs). `$ITER_HOME` moves it (tests, containers).
pub fn iter_home() -> PathBuf {
    if let Some(h) = std::env::var_os("ITER_HOME").filter(|h| !h.is_empty()) {
        return PathBuf::from(h);
    }
    iter_core::platform::home_dir().unwrap_or_else(|| PathBuf::from(".")).join(".iter")
}

pub fn default_key_file() -> PathBuf {
    iter_home().join("engine.key")
}

pub fn run(args: KeyArgs) {
    match args.action {
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

/// Make, check or drop the trust in iter_data's certificate.
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
    trust::forget(url)?;
    let api = Api::new(url, "");
    let seen = api.seen_fingerprint().map_err(|e| format!("could not reach {url}: {e}"))?;
    if let Some(w) = &want {
        if *w != seen {
            return Err(format!("{url} presents certificate sha256:{seen}, not the sha256:{w} you gave: NOT trusted (something may be in between)"));
        }
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
}
