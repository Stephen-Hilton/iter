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
//!  * the engine's own credential (`ITER_ENGINE_TOKEN` per config) is pinned:
//!    the `Api` client captured it at startup, so refreshing the map would
//!    change nothing and only mislead a reader of the log;
//!  * values are never logged, at any level, in any message.

use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::sync::{OnceLock, RwLock};
use std::time::{Duration, SystemTime};

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

    /// Trimmed; `None` when absent or empty.
    pub(crate) fn get(&self, key: &str) -> Option<String> {
        self.vals.get(key).map(|v| v.trim().to_string()).filter(|v| !v.is_empty())
    }
}

fn store() -> &'static RwLock<Store> {
    // a read before `init` (unit tests, helpers): the process environment alone
    STORE.get_or_init(|| RwLock::new(Store::seed(std::env::vars(), "", &[]).0))
}

/// Seed the store from the process environment plus `env_file`, and export
/// the file's keys to the process environment ONCE, here, while `main` is
/// still single-threaded: child processes (agents, exec scripts) inherit the
/// same environment they always did.  After this point the process
/// environment is never written again; tokens are read from the map.
/// `pinned` names keys the file may define but a reload must never touch
/// (the engine's own `ITER_ENGINE_TOKEN`).
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

    fn tmp(name: &str, content: &str) -> String {
        let p = std::env::temp_dir().join(format!("iter3-envstore-{}-{}-{}.env", std::process::id(), name, rand_suffix()));
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
