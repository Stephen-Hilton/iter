//! The engine's view of its env_file, hot-reloaded (spec: account hot reload,
//! 2026-09-11).  Tokens used to come from the process environment, which was
//! filled from the file exactly once at startup, so an account added while the
//! engine ran had no token until a restart — and nothing said so.
//!
//! One process-wide map behind a lock.  `init` seeds it from the real process
//! environment and overlays the env_file; `reload_if_changed` re-reads the
//! file when it moved and applies the diff to the keys the file owns.  Token
//! reads (`get`) go through the map, never `std::env`, because the engine is
//! multi-threaded and mutating the process environment under running threads
//! is undefined behaviour in edition 2024.
//!
//! Rules, all deliberate:
//!  * a key the process environment supplied at startup wins over the file
//!    and is never added, updated or removed by a reload (e2e exports
//!    `ACCT_A_TOKEN` and never writes it to the file);
//!  * within the file's keys only `*_TOKEN` names, plus every `token_envar`
//!    some served project names, are refreshed — an AWS credential in the
//!    same file is not re-read behind a running agent's back;
//!  * a key the caller pins (envstore::init's `pinned`) is never refreshed:
//!    a value captured at startup must not seem to change in the map;
//!  * values are never logged, at any level, in any message.
//!
//! Secret sources (2026-10-17), highest first:
//!  1. the process environment at startup (systemd `Environment=`, a
//!     container's env, AWS-injected variables);
//!  2. the secrets command (`--secrets-command`, `$ITER_SECRETS_COMMAND`),
//!     when one is configured: a shell command the engine runs at startup and
//!     every `--secrets-every` minutes, whose stdout is `KEY=value` lines
//!     (e.g. `aws ssm get-parameters-by-path … --with-decryption`, `op read`,
//!     a Vault or Keychain call). Its output stays in this map, never on disk
//!     and never in the process environment, so a token rotated in the store
//!     reaches the engine within minutes, with no restart. A failed run keeps
//!     the values of the last good one;
//!  3. the env_file (`<iter dir>/iter_engine/secrets/.env`), hot-reloaded as above.
//!
//! Account tokens are never handed to a child process except the one a
//! Claude session bills to (as `CLAUDE_CODE_OAUTH_TOKEN`): `scrub` removes
//! every account's `token_envar` from a child's environment, wherever it came
//! from (`set_account_vars` names them).

use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::sync::{OnceLock, RwLock};
use std::time::{Duration, Instant, SystemTime};

static STORE: OnceLock<RwLock<Store>> = OnceLock::new();

/// A file whose mtime is this close to the moment it was read may still be
/// edited within the same mtime tick on a coarse-granularity filesystem, so
/// the next reload re-reads it regardless of the stat (the racy-git rule).
const RACY_WINDOW: Duration = Duration::from_secs(2);

#[derive(Debug, Clone, PartialEq, Eq)]
struct Stamp {
    mtime: SystemTime,
    len: u64,
    hash: u64,
    /// the file was modified within RACY_WINDOW of being read
    racy: bool,
}

pub(crate) struct Store {
    vals: HashMap<String, String>,
    /// keys the env_file supplied (absent from the process environment at
    /// startup): the only keys a reload may touch
    file_owned: HashSet<String>,
    /// never refreshed even though it comes from the file
    pinned: HashSet<String>,
    stamp: Option<Stamp>,
    env_file: String,
    /// how many times the file was read (tests: an unchanged file is not re-read)
    reads: u64,
    /// the secrets command ("" = none) and what its last good run printed
    command: String,
    command_every: Duration,
    command_vals: HashMap<String, String>,
    command_ran: Option<Instant>,
    command_busy: bool,
    /// every account `token_envar` this engine knows of: never passed to a child
    account_vars: HashSet<String>,
}

/// What one reload changed — key names only, never values.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Changes {
    pub added: Vec<String>,
    pub updated: Vec<String>,
    pub removed: Vec<String>,
    /// keys the env_file defines after the reload
    pub total: usize,
}

impl Changes {
    fn is_empty(&self) -> bool {
        self.added.is_empty() && self.updated.is_empty() && self.removed.is_empty()
    }

    /// `env_file reloaded: +DEV4_TOKEN -DEV2_TOKEN ~DEV1_TOKEN (14 keys)`
    pub fn log_line(&self) -> String {
        let mut parts: Vec<String> = Vec::new();
        parts.extend(self.added.iter().map(|k| format!("+{k}")));
        parts.extend(self.removed.iter().map(|k| format!("-{k}")));
        parts.extend(self.updated.iter().map(|k| format!("~{k}")));
        format!("env_file reloaded: {} ({} keys)", parts.join(" "), self.total)
    }
}

/// `KEY=value` lines; blank lines and `#` comments skipped; surrounding
/// quotes stripped (the same grammar `load_env_file` used since V3 day one).
fn parse(content: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some((k, v)) = line.split_once('=') {
            let k = k.trim();
            let v = v.trim().trim_matches('"').trim_matches('\'');
            if !k.is_empty() && !k.contains(char::is_whitespace) {
                out.push((k.to_string(), v.to_string()));
            }
        }
    }
    out
}

fn hash_of(bytes: &[u8]) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    bytes.hash(&mut h);
    h.finish()
}

impl Store {
    /// Seed from `process_env` (wins, immutable for the life of the process),
    /// then overlay the env_file.  Returns the file's (key, value) pairs that
    /// were adopted, so the caller can export them once at startup.
    pub(crate) fn seed(
        process_env: impl IntoIterator<Item = (String, String)>,
        env_file: &str,
        pinned: &[&str],
    ) -> (Self, Vec<(String, String)>) {
        let mut store = Store {
            vals: process_env.into_iter().collect(),
            file_owned: HashSet::new(),
            pinned: pinned.iter().map(|s| s.to_string()).collect(),
            stamp: None,
            env_file: env_file.to_string(),
            reads: 0,
            command: String::new(),
            command_every: Duration::from_secs(300),
            command_vals: HashMap::new(),
            command_ran: None,
            command_busy: false,
            account_vars: HashSet::new(),
        };
        let mut adopted = Vec::new();
        if let Some((content, stamp)) = store.read_file() {
            for (k, v) in parse(&content) {
                if store.vals.contains_key(&k) {
                    continue; // the process environment (or an earlier line) wins
                }
                store.file_owned.insert(k.clone());
                store.vals.insert(k.clone(), v.clone());
                adopted.push((k, v));
            }
            store.stamp = Some(stamp);
        }
        (store, adopted)
    }

    fn read_file(&mut self) -> Option<(String, Stamp)> {
        let meta = std::fs::metadata(&self.env_file).ok()?;
        let content = std::fs::read(&self.env_file).ok()?;
        self.reads += 1;
        let mtime = meta.modified().unwrap_or(SystemTime::UNIX_EPOCH);
        let racy = SystemTime::now().duration_since(mtime).map(|d| d < RACY_WINDOW).unwrap_or(true);
        let stamp = Stamp { mtime, len: content.len() as u64, hash: hash_of(&content), racy };
        Some((String::from_utf8_lossy(&content).into_owned(), stamp))
    }

    /// True when the file looks unchanged since the last read: same mtime and
    /// length, and the last read was not inside the racy window.
    fn stat_unchanged(&self) -> bool {
        let Some(prev) = &self.stamp else { return false };
        if prev.racy {
            return false;
        }
        match std::fs::metadata(&self.env_file) {
            Ok(m) => m.modified().unwrap_or(SystemTime::UNIX_EPOCH) == prev.mtime && m.len() == prev.len,
            Err(_) => false,
        }
    }

    fn refreshable(&self, key: &str, extra: &HashSet<String>) -> bool {
        self.file_owned.contains(key) && !self.pinned.contains(key) && (key.ends_with("_TOKEN") || extra.contains(key))
    }

    /// Re-read the file when it moved (or `force`) and apply the diff to the
    /// refreshable keys.  `None` when nothing changed.
    pub(crate) fn reload(&mut self, extra: &HashSet<String>, force: bool) -> Option<Changes> {
        if !force && self.stat_unchanged() {
            return None;
        }
        let (content, stamp) = match self.read_file() {
            Some(x) => x,
            None => {
                // the file is gone: every refreshable key it owned goes with it
                let removed: Vec<String> = {
                    let mut v: Vec<String> = self.file_owned.iter().filter(|k| self.refreshable(k, extra)).cloned().collect();
                    v.sort();
                    v
                };
                for k in &removed {
                    self.vals.remove(k);
                    self.file_owned.remove(k);
                }
                self.stamp = None;
                let ch = Changes { removed, total: self.file_owned.len(), ..Default::default() };
                return if ch.is_empty() { None } else { Some(ch) };
            }
        };
        let same_content = self.stamp.as_ref().map(|s| s.hash == stamp.hash).unwrap_or(false);
        self.stamp = Some(stamp);
        if same_content {
            return None;
        }
        let mut now: HashMap<String, String> = HashMap::new();
        for (k, v) in parse(&content) {
            now.entry(k).or_insert(v); // first definition wins, as at startup
        }
        let mut ch = Changes::default();
        // added: in the file, not process-owned, not yet in the map
        for (k, v) in &now {
            if self.vals.contains_key(k) {
                continue;
            }
            if !(k.ends_with("_TOKEN") || extra.contains(k)) || self.pinned.contains(k) {
                continue;
            }
            self.file_owned.insert(k.clone());
            self.vals.insert(k.clone(), v.clone());
            ch.added.push(k.clone());
        }
        // updated / removed: refreshable keys the file owned
        let owned: Vec<String> = self.file_owned.iter().cloned().collect();
        for k in owned {
            if !self.refreshable(&k, extra) {
                continue;
            }
            match now.get(&k) {
                Some(v) if self.vals.get(&k) != Some(v) => {
                    self.vals.insert(k.clone(), v.clone());
                    ch.updated.push(k);
                }
                Some(_) => {}
                None => {
                    self.vals.remove(&k);
                    self.file_owned.remove(&k);
                    ch.removed.push(k);
                }
            }
        }
        ch.added.sort();
        ch.updated.sort();
        ch.removed.sort();
        ch.total = self.file_owned.len();
        if ch.is_empty() { None } else { Some(ch) }
    }

    /// Trimmed; `None` when absent or empty. The process environment wins,
    /// then the secrets command, then the env_file.
    pub(crate) fn get(&self, key: &str) -> Option<String> {
        let clean = |v: &String| Some(v.trim().to_string()).filter(|v| !v.is_empty());
        if !self.file_owned.contains(key) {
            if let Some(v) = self.vals.get(key).and_then(clean) {
                return Some(v);
            }
        }
        self.command_vals.get(key).and_then(clean).or_else(|| self.vals.get(key).and_then(clean))
    }

    /// Where `key`'s value comes from: "env", "command", "file" or "" (unset).
    pub(crate) fn source_of(&self, key: &str) -> &'static str {
        let set = |v: Option<&String>| v.is_some_and(|v| !v.trim().is_empty());
        if !self.file_owned.contains(key) && set(self.vals.get(key)) {
            "env"
        } else if set(self.command_vals.get(key)) {
            "command"
        } else if set(self.vals.get(key)) {
            "file"
        } else {
            ""
        }
    }

    /// Take a secrets-command run's output: the new map and what changed.
    pub(crate) fn adopt_command(&mut self, pairs: Vec<(String, String)>) -> Changes {
        let mut now: HashMap<String, String> = HashMap::new();
        for (k, v) in pairs {
            now.entry(k).or_insert(v);
        }
        let mut ch = Changes::default();
        for (k, v) in &now {
            match self.command_vals.get(k) {
                None => ch.added.push(k.clone()),
                Some(old) if old != v => ch.updated.push(k.clone()),
                Some(_) => {}
            }
        }
        for k in self.command_vals.keys() {
            if !now.contains_key(k) {
                ch.removed.push(k.clone());
            }
        }
        ch.added.sort();
        ch.updated.sort();
        ch.removed.sort();
        ch.total = now.len();
        self.command_vals = now;
        ch
    }
}

/// Run the secrets command (`sh -c`, `cmd /C` on Windows) with a time limit;
/// its stdout's `KEY=value` lines. Stderr is reported only as its first line.
fn run_secrets_command(command: &str, timeout: Duration) -> Result<Vec<(String, String)>, String> {
    use std::io::Read;
    use std::process::{Command, Stdio};
    let mut c = if cfg!(windows) {
        let mut c = Command::new("cmd");
        c.args(["/C", command]);
        c
    } else {
        let mut c = Command::new("sh");
        c.args(["-c", command]);
        c
    };
    let mut child = c.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().map_err(|e| format!("could not start: {e}"))?;
    let mut out = child.stdout.take();
    let mut err = child.stderr.take();
    let reader = std::thread::spawn(move || {
        let mut s = String::new();
        if let Some(o) = out.as_mut() {
            let _ = o.read_to_string(&mut s);
        }
        s
    });
    let ereader = std::thread::spawn(move || {
        let mut s = String::new();
        if let Some(e) = err.as_mut() {
            let _ = e.read_to_string(&mut s);
        }
        s
    });
    let t0 = Instant::now();
    let status = loop {
        if let Some(st) = child.try_wait().map_err(|e| e.to_string())? {
            break st;
        }
        if t0.elapsed() > timeout {
            let _ = child.kill();
            let _ = child.wait();
            return Err(format!("still running after {}s: stopped", timeout.as_secs()));
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    let stdout = reader.join().unwrap_or_default();
    let stderr = ereader.join().unwrap_or_default();
    if !status.success() {
        let first = stderr.lines().find(|l| !l.trim().is_empty()).unwrap_or("").chars().take(200).collect::<String>();
        return Err(format!("exit {}{}", status.code().map(|c| c.to_string()).unwrap_or_else(|| "signal".into()), if first.is_empty() { String::new() } else { format!(": {first}") }));
    }
    Ok(parse(&stdout))
}

fn store() -> &'static RwLock<Store> {
    // a read before `init` (unit tests, helpers): the process environment alone
    STORE.get_or_init(|| RwLock::new(Store::seed(std::env::vars(), "", &[]).0))
}

/// Seed the store from the process environment plus `env_file`, and export
/// the file's keys to the process environment ONCE, here, while `main` is
/// still single-threaded: child processes (agents, exec scripts) inherit the
/// same environment they always did, minus every account token (`scrub`).
/// After this point the process environment is never written again; tokens
/// are read from the map. `pinned` names keys the file may define but a
/// reload must never touch.
pub fn init(env_file: &str, pinned: &[&str]) {
    let (store, adopted) = Store::seed(std::env::vars(), env_file, pinned);
    for (k, v) in &adopted {
        // SAFETY: called from main before any thread is spawned; this is the
        // same single write-at-startup the engine has always done
        unsafe { std::env::set_var(k, v) };
    }
    if STORE.set(RwLock::new(store)).is_err() {
        eprintln!("[engine] envstore::init called twice; the first call stands");
    }
}

/// How long one secrets-command run may take.
const COMMAND_TIMEOUT: Duration = Duration::from_secs(60);

/// Configure the secrets command and run it once now (startup: its tokens
/// are there for the first tick). `every` is how often it runs again.
pub fn set_command(command: &str, every: Duration) {
    let command = command.trim().to_string();
    if command.is_empty() {
        return;
    }
    if let Ok(mut s) = store().write() {
        s.command = command.clone();
        s.command_every = every.max(Duration::from_secs(30));
    }
    run_command_now();
}

/// Run the secrets command when it is due (from the engine's tick), in the
/// background so a slow secret store never holds a tick up.
pub fn refresh_command_if_due() {
    let due = store().write().ok().map(|mut s| {
        let due = !s.command.is_empty() && !s.command_busy && s.command_ran.is_none_or(|t| t.elapsed() >= s.command_every);
        if due {
            s.command_busy = true;
        }
        due
    });
    if due == Some(true) {
        std::thread::spawn(run_command_now);
    }
}

fn run_command_now() {
    let command = store().read().map(|s| s.command.clone()).unwrap_or_default();
    let result = run_secrets_command(&command, COMMAND_TIMEOUT);
    let Ok(mut s) = store().write() else { return };
    s.command_busy = false;
    s.command_ran = Some(Instant::now());
    match result {
        Ok(pairs) => {
            let first = s.command_vals.is_empty();
            let ch = s.adopt_command(pairs);
            if first || !ch.is_empty() {
                let mut parts: Vec<String> = ch.added.iter().map(|k| format!("+{k}")).collect();
                parts.extend(ch.removed.iter().map(|k| format!("-{k}")));
                parts.extend(ch.updated.iter().map(|k| format!("~{k}")));
                println!("[engine] secrets command: {} ({} keys)", parts.join(" "), ch.total);
            }
        }
        Err(e) => eprintln!("[engine] secrets command failed ({e}); keeping the values of its last good run"),
    }
}

/// Every account `token_envar` this engine serves (from its assignments).
pub fn set_account_vars(vars: &HashSet<String>) {
    if let Ok(mut s) = store().write() {
        s.account_vars = vars.clone();
    }
}

/// Remove every account token from a child's environment (the process may
/// hold some, from startup or the env_file). A Claude session then gets its
/// own token as `CLAUDE_CODE_OAUTH_TOKEN`, set after this.
pub fn scrub(cmd: &mut std::process::Command) {
    let vars = store().read().map(|s| s.account_vars.clone()).unwrap_or_default();
    for v in vars {
        cmd.env_remove(v);
    }
}

/// Where `key` comes from: "env", "command", "file" or "" when unset.
pub fn source_of(key: &str) -> &'static str {
    store().read().map(|s| s.source_of(key)).unwrap_or("")
}

/// The env_file path given to `init` ("" before it).
pub fn env_file() -> String {
    store().read().map(|s| s.env_file.clone()).unwrap_or_default()
}

/// Trimmed value of `key` from the map; `None` when unset or empty.
pub fn get(key: &str) -> Option<String> {
    store().read().ok().and_then(|s| s.get(key))
}

/// Stat the env_file and, when it moved (or `force`), re-read it and apply
/// the diff to the refreshable keys: `*_TOKEN` names plus `extra` (every
/// `token_envar` the served projects name).  `None` when nothing changed.
pub fn reload_if_changed(extra: &HashSet<String>, force: bool) -> Option<Changes> {
    store().write().ok().and_then(|mut s| s.reload(extra, force))
}

#[cfg(test)]
pub(crate) fn set_for_test(key: &str, value: &str) {
    if let Ok(mut s) = store().write() {
        s.vals.insert(key.to_string(), value.to_string());
    }
}

#[cfg(test)]
pub(crate) fn unset_for_test(key: &str) {
    if let Ok(mut s) = store().write() {
        s.vals.remove(key);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn sources_rank_env_then_command_then_file_and_a_failed_run_keeps_the_last_good_one() {
        let f = tmp("src", "A_TOKEN=from-file\nB_TOKEN=from-file\nC_TOKEN=from-file\n");
        let env = vec![("A_TOKEN".to_string(), "from-env".to_string())];
        let (mut s, _) = Store::seed(env, &f, &[]);
        assert_eq!((s.get("A_TOKEN").as_deref(), s.source_of("A_TOKEN")), (Some("from-env"), "env"));
        let ch = s.adopt_command(vec![("A_TOKEN".into(), "from-cmd".into()), ("B_TOKEN".into(), "from-cmd".into()), ("D_TOKEN".into(), "only-cmd".into())]);
        assert_eq!((ch.added.len(), ch.total), (3, 3));
        assert_eq!(s.get("A_TOKEN").as_deref(), Some("from-env"), "the process environment still wins");
        assert_eq!((s.get("B_TOKEN").as_deref(), s.source_of("B_TOKEN")), (Some("from-cmd"), "command"), "the command beats the file");
        assert_eq!((s.get("C_TOKEN").as_deref(), s.source_of("C_TOKEN")), (Some("from-file"), "file"));
        assert_eq!(s.get("D_TOKEN").as_deref(), Some("only-cmd"));
        assert_eq!(s.source_of("NOPE"), "");
        // a later run: changed, removed
        let ch = s.adopt_command(vec![("B_TOKEN".into(), "rotated".into())]);
        assert_eq!((ch.updated.clone(), ch.removed.clone()), (vec!["B_TOKEN".to_string()], vec!["A_TOKEN".to_string(), "D_TOKEN".to_string()]));
        assert_eq!(s.get("B_TOKEN").as_deref(), Some("rotated"));
        assert_eq!(s.get("D_TOKEN"), None);
    }

    #[cfg(unix)]
    #[test]
    fn the_secrets_command_prints_pairs_fails_loudly_and_is_stopped_when_slow() {
        let ok = run_secrets_command("printf 'X_TOKEN=abc\\n# c\\nY_TOKEN=\"q v\"\\n'", Duration::from_secs(5)).unwrap();
        assert_eq!(ok, vec![("X_TOKEN".into(), "abc".into()), ("Y_TOKEN".into(), "q v".into())]);
        let e = run_secrets_command("echo oops >&2; exit 3", Duration::from_secs(5)).unwrap_err();
        assert!(e.starts_with("exit 3") && e.contains("oops"), "{e}");
        let t = Instant::now();
        assert!(run_secrets_command("sleep 5", Duration::from_millis(300)).unwrap_err().contains("stopped"));
        assert!(t.elapsed() < Duration::from_secs(3));
    }

    #[test]
    fn scrub_removes_every_account_token_from_a_child() {
        let mut vars = HashSet::new();
        vars.insert("SCRUB_TEST_TOKEN".to_string());
        set_account_vars(&vars);
        let mut cmd = std::process::Command::new("true");
        cmd.env("SCRUB_TEST_TOKEN", "leak").env("OTHER", "kept");
        scrub(&mut cmd);
        let envs: Vec<(String, Option<String>)> = cmd.get_envs().map(|(k, v)| (k.to_string_lossy().into_owned(), v.map(|v| v.to_string_lossy().into_owned()))).collect();
        assert!(envs.contains(&("SCRUB_TEST_TOKEN".to_string(), None)), "removed: {envs:?}");
        assert!(envs.contains(&("OTHER".to_string(), Some("kept".to_string()))));
        set_account_vars(&HashSet::new());
    }

    fn tmp(name: &str, content: &str) -> String {
        let p = std::env::temp_dir().join(format!("iter-envstore-{}-{}-{}.env", std::process::id(), name, rand_suffix()));
        std::fs::write(&p, content).unwrap();
        age(&p.to_string_lossy(), 10);
        p.to_string_lossy().into_owned()
    }

    fn rand_suffix() -> u64 {
        use std::sync::atomic::{AtomicU64, Ordering};
        static N: AtomicU64 = AtomicU64::new(0);
        N.fetch_add(1, Ordering::SeqCst)
    }

    /// push the mtime `secs` into the past so the file is outside the racy
    /// window (a real env_file was last edited long before the engine started)
    fn age(path: &str, secs: u64) {
        let f = std::fs::OpenOptions::new().write(true).open(path).unwrap();
        f.set_modified(SystemTime::now() - Duration::from_secs(secs)).unwrap();
    }

    fn no_env() -> Vec<(String, String)> {
        vec![]
    }

    #[test]
    fn reload_picks_up_an_appended_token() {
        let f = tmp("t1", "DEV1_TOKEN=a\n");
        let (mut s, _) = Store::seed(no_env(), &f, &[]);
        assert_eq!(s.get("DEV1_TOKEN").as_deref(), Some("a"));
        let mut fh = std::fs::OpenOptions::new().append(true).open(&f).unwrap();
        writeln!(fh, "DEV9_TOKEN=xyz").unwrap();
        drop(fh);
        let ch = s.reload(&HashSet::new(), false).expect("a change");
        assert_eq!(ch.added, vec!["DEV9_TOKEN".to_string()]);
        assert!(ch.updated.is_empty() && ch.removed.is_empty());
        assert_eq!(ch.total, 2);
        assert_eq!(s.get("DEV9_TOKEN").as_deref(), Some("xyz"));
    }

    #[test]
    fn reload_removes_a_deleted_token() {
        let f = tmp("t2", "DEV1_TOKEN=a\nDEV9_TOKEN=xyz\n");
        let (mut s, _) = Store::seed(no_env(), &f, &[]);
        assert_eq!(s.get("DEV9_TOKEN").as_deref(), Some("xyz"));
        std::fs::write(&f, "DEV1_TOKEN=a\n").unwrap();
        let ch = s.reload(&HashSet::new(), false).expect("a change");
        assert_eq!(ch.removed, vec!["DEV9_TOKEN".to_string()]);
        assert_eq!(s.get("DEV9_TOKEN"), None);
        assert_eq!(s.get("DEV1_TOKEN").as_deref(), Some("a"));
    }

    #[test]
    fn unchanged_file_is_not_re_read() {
        let f = tmp("t3", "DEV1_TOKEN=a\n");
        let (mut s, _) = Store::seed(no_env(), &f, &[]);
        assert_eq!(s.reads, 1);
        assert!(s.reload(&HashSet::new(), false).is_none());
        assert!(s.reload(&HashSet::new(), false).is_none());
        assert_eq!(s.reads, 1, "an unchanged file must not be re-read");
        // force re-reads but reports nothing when the content is the same
        assert!(s.reload(&HashSet::new(), true).is_none());
        assert_eq!(s.reads, 2);
    }

    /// A coarse-mtime filesystem (HFS+, FAT, some NFS) stamps a file edited
    /// within a second of the engine's read with the SAME mtime; the length
    /// is the same too (a rotated token).  The store re-reads a file whose
    /// last read fell inside the racy window and diffs by content.
    #[test]
    fn a_length_preserving_edit_is_still_seen() {
        let f = tmp("t4", "DEV1_TOKEN=a\n");
        std::fs::write(&f, "DEV1_TOKEN=a\n").unwrap(); // fresh mtime: the engine reads it just after it was written
        let (mut s, _) = Store::seed(no_env(), &f, &[]);
        let before = std::fs::metadata(&f).unwrap().modified().unwrap();
        std::fs::write(&f, "DEV1_TOKEN=b\n").unwrap();
        // same length, same mtime as the last read
        std::fs::OpenOptions::new().write(true).open(&f).unwrap().set_modified(before).unwrap();
        let ch = s.reload(&HashSet::new(), false).expect("a change");
        assert_eq!(ch.updated, vec!["DEV1_TOKEN".to_string()]);
        assert_eq!(s.get("DEV1_TOKEN").as_deref(), Some("b"));
    }

    #[test]
    fn process_env_wins_and_is_never_removed() {
        let f = tmp("t5", "ACCT_A_TOKEN=file\nDEV1_TOKEN=a\n");
        let env = vec![("ACCT_A_TOKEN".to_string(), "proc".to_string())];
        let (mut s, adopted) = Store::seed(env, &f, &[]);
        assert_eq!(s.get("ACCT_A_TOKEN").as_deref(), Some("proc"));
        assert_eq!(adopted, vec![("DEV1_TOKEN".to_string(), "a".to_string())]);
        std::fs::write(&f, "ACCT_A_TOKEN=other\n").unwrap();
        let ch = s.reload(&HashSet::new(), true).expect("DEV1_TOKEN went away");
        assert_eq!(ch.removed, vec!["DEV1_TOKEN".to_string()]);
        assert_eq!(s.get("ACCT_A_TOKEN").as_deref(), Some("proc"));
        std::fs::write(&f, "").unwrap();
        assert!(s.reload(&HashSet::new(), true).is_none());
        assert_eq!(s.get("ACCT_A_TOKEN").as_deref(), Some("proc"));
    }

    #[test]
    fn the_reload_line_never_prints_a_value() {
        let f = tmp("t6", "DEV1_TOKEN=a\n");
        let (mut s, _) = Store::seed(no_env(), &f, &[]);
        std::fs::write(&f, "DEV1_TOKEN=a\nDEV9_TOKEN=supersecret\n").unwrap();
        let line = s.reload(&HashSet::new(), false).expect("a change").log_line();
        assert!(line.contains("+DEV9_TOKEN"), "{line}");
        assert!(!line.contains("supersecret"), "{line}");
        assert_eq!(line, "env_file reloaded: +DEV9_TOKEN (2 keys)");
    }

    #[test]
    fn only_token_names_and_declared_envars_refresh() {
        let f = tmp("t7", "AWS_SECRET=s1\nMY_CRED=c1\nDEV1_TOKEN=a\nITER_ENGINE_TOKEN=e1\n");
        let (mut s, _) = Store::seed(no_env(), &f, &["ITER_ENGINE_TOKEN"]);
        std::fs::write(&f, "AWS_SECRET=s2\nMY_CRED=c2\nDEV1_TOKEN=b\nITER_ENGINE_TOKEN=e2\nNEW_CRED=n\n").unwrap();
        let extra: HashSet<String> = ["MY_CRED".to_string(), "NEW_CRED".to_string()].into_iter().collect();
        let ch = s.reload(&extra, false).expect("a change");
        assert_eq!(ch.updated, vec!["DEV1_TOKEN".to_string(), "MY_CRED".to_string()]);
        assert_eq!(ch.added, vec!["NEW_CRED".to_string()]);
        assert_eq!(s.get("AWS_SECRET").as_deref(), Some("s1"), "a non-token key is not refreshed");
        assert_eq!(s.get("ITER_ENGINE_TOKEN").as_deref(), Some("e1"), "the pinned key is not refreshed");
        assert_eq!(s.get("MY_CRED").as_deref(), Some("c2"));
    }

    #[test]
    fn get_is_trimmed_and_empty_is_none() {
        let f = tmp("t8", "DEV1_TOKEN=\" a \"\nDEV2_TOKEN=\n");
        let (s, _) = Store::seed(no_env(), &f, &[]);
        assert_eq!(s.get("DEV1_TOKEN").as_deref(), Some("a"));
        assert_eq!(s.get("DEV2_TOKEN"), None);
        assert_eq!(s.get("NOPE"), None);
    }
}
