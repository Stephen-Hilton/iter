//! `iter_engine key new | status` (2026-10-11): the engine machine's own
//! Ed25519 key, the way it proves who it is to iter_data (shapes and the
//! sign-in: iter_core::enginekey). The private key is written here and never
//! leaves the machine; only the public key is pasted into the webui's Engine
//! keys screen.
//!
//! `new` writes the key file (`~/.iter/engine.key`, owner-only) when there
//! is none, else the next key beside it (`engine.key.next`), which a running
//! engine signs in with as soon as iter_data accepts it (client.rs
//! `adopt_next`) — a rotation with no restart and no gap. `status` signs in
//! with each key and says whether iter_data accepts it; `session` prints a
//! session token for a script's own API calls.

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

/// `~/.iter`: the engine's own files (key, env file, binary, logs).
pub fn iter_home() -> PathBuf {
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

/// One line per key; exit 0 when the current key is accepted, 1 when it is
/// refused or missing, 2 when iter_data cannot be reached or no URL was given.
fn status(file: &Path, data_url: Option<&str>) -> i32 {
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
