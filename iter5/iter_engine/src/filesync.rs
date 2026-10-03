//! Node-file sync, the engine half (iter5 spec §3). The engine is the only
//! writer to the checkout; iter_data never touches a repo.
//!
//! Every tick, per served project ([`tick`]):
//! 1. **filescan** — the node files (`*.iter.md` whose name says a node type;
//!    agent memory excluded) under the project node's `scandirs` (default: the
//!    whole topdir), git-ignore aware, never entering `.git`, `target`,
//!    `node_modules`, `.iter`, `.claude`. A tick only `stat`s: known files
//!    whose `(mtime, size)` moved are re-read, known directories whose mtime
//!    moved are re-listed (that is how new files and folders are found); a
//!    full walk runs on the first tick and every [`FULL_EVERY`]. A file
//!    whose bytes did not change (touched only) is not re-sent.
//! 2. **conform** — `nodefile::conform_against` (the semantic hash last
//!    synced is passed, so a hand edit bumps `timestamps.last_modified`); a
//!    changed text is written back unless the file sits inside a live work-item
//!    lock (then it is written once the lock is gone — the node gets the
//!    conformed text straight away) or the checkout is read-only.
//! 3. **sync** — `POST /api/projects/{p}/files/sync {engine, full, files:[{path,
//!    text, hash, base_version}], deleted:[path]}`; `base_version` = the
//!    `node_version` last applied / acked for that id (0 = unknown). `full` is
//!    true only for the first sync of a checkout with no saved state (every
//!    node file is in it). The reply's `applied` versions are remembered,
//!    `rewrite` texts are written (lock-aware), `conflicts` are logged.
//!    Conform write-backs and rewrites are committed together, scoped to those
//!    files: `iter: conform <n> node files`.
//! 4. **pending** (when the heartbeat said `files_waiting`) — `GET files/pending`,
//!    write / delete / move each file (paths leaving the topdir are refused; a
//!    path inside a live lock waits for a later tick), commit only the touched
//!    files (`iter: graph edit — …`), push when a remote exists, then
//!    `POST files/ack {engine, acks:[{id, node_version, path, hash, commit}]}`.
//!
//! State: per file `{id, hash, sem, version, stat}` in memory and in
//! `<topdir>/.iter/filesync.json` (keyed by server + project), so a restart
//! sends only what changed while the engine was down. Read-only checkouts are
//! scanned and synced but never written (no write-back, no pending writes, no
//! state file).
//!
//! [`build`] (designer push, §3.4) creates the repo and writes every pending
//! file in one commit `iter: build from design`.

use crate::client::Api;
use iter_core::nodefile::{self, NodeType};
use iter_local::walk::{self, Stat};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

/// A full walk of the scan roots at least this often.
pub const FULL_EVERY: Duration = Duration::from_secs(300);
/// GraphRAG re-index after a sync round changed something, at most this often.
pub const RAG_EVERY: Duration = Duration::from_secs(60);
/// Where the per-checkout state lives (inside the engine's `.iter/`).
pub const STATE_FILE: &str = ".iter/filesync.json";

/// What the engine knows about one node file.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct FileMeta {
    #[serde(default)]
    pub id: String,
    /// content hash of the file as last seen on disk
    #[serde(default)]
    pub hash: String,
    /// semantic hash of the node last synced (conform's "did it change")
    #[serde(default)]
    pub sem: String,
    /// node_version last applied / acked = the file_version
    #[serde(default)]
    pub version: u64,
    #[serde(default)]
    pub stat: Stat,
}

#[derive(Serialize, Deserialize, Default)]
struct Persisted {
    #[serde(default)]
    version: u32,
    #[serde(default)]
    server: String,
    #[serde(default)]
    project: String,
    #[serde(default)]
    scandirs: Vec<String>,
    #[serde(default)]
    project_dir: Option<String>,
    #[serde(default)]
    files: BTreeMap<String, FileMeta>,
}

/// Counters for logs and tests: what the last tick did.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct TickStats {
    pub full: bool,
    pub read: usize,
    pub sent: usize,
    pub deleted: usize,
    pub conformed: usize,
    pub rewritten: usize,
    pub pending_applied: usize,
    pub pending_waiting: usize,
    pub commits: Vec<String>,
}

/// Per served project, held by the engine between ticks.
#[derive(Default)]
pub struct FileSyncState {
    loaded: bool,
    /// no saved state: the first sync is `full`
    fresh: bool,
    /// key = path relative to the topdir
    files: BTreeMap<String, FileMeta>,
    dirs: HashMap<PathBuf, Stat>,
    last_full: Option<Instant>,
    ignored: iter_local::GitIgnored,
    scandirs: Vec<String>,
    project_dir: Option<String>,
    /// files to re-process next tick (a failed sync, a lock)
    retry: BTreeSet<String>,
    /// writes waiting for a lock: rel → (text, node_version to record —
    /// None keeps the known one, disk hash the text was made from — None =
    /// a server rewrite, written whatever the file holds now)
    deferred: BTreeMap<String, (String, Option<u64>, Option<String>)>,
    dirty: bool,
    last_rag: Option<Instant>,
    rag: Option<std::thread::JoinHandle<()>>,
    pub last: TickStats,
}

#[allow(dead_code)] // used by tests and kept for callers
impl FileSyncState {
    /// Make the next tick walk everything.
    pub fn force_full(&mut self) {
        self.last_full = None;
    }
    pub fn known(&self, rel: &str) -> Option<&FileMeta> {
        self.files.get(rel)
    }
}

/// Files `build` wrote, handed to the next `tick` of the same checkout
/// (the engine's tick state for it may already be loaded).
static BUILT: std::sync::Mutex<Vec<(PathBuf, String, FileMeta)>> = std::sync::Mutex::new(Vec::new());

fn stored(rel: &str) -> String {
    format!("{{topdir}}/{rel}")
}

fn rel_of(stored: &str) -> &str {
    stored.strip_prefix("{topdir}/").unwrap_or(stored)
}

fn canon(topdir: &Path) -> PathBuf {
    topdir.canonicalize().unwrap_or_else(|_| topdir.to_path_buf())
}

// ---------------------------------------------------------------- git

/// One git command in `top`; a locked index (an agent's own git) is waited
/// out for up to ~10 s.
pub fn git(top: &Path, args: &[&str]) -> Result<String, String> {
    for attempt in 0.. {
        let out = Command::new("git")
            .args(args)
            .current_dir(top)
            .env("GIT_TERMINAL_PROMPT", "0")
            .output()
            .map_err(|e| format!("git {}: {e}", args.join(" ")))?;
        if out.status.success() {
            return Ok(String::from_utf8_lossy(&out.stdout).trim().to_string());
        }
        let err = String::from_utf8_lossy(&out.stderr).trim().to_string();
        if err.contains("index.lock") && attempt < 20 {
            std::thread::sleep(Duration::from_millis(500));
            continue;
        }
        return Err(format!("git {} failed: {err}", args.join(" ")));
    }
    unreachable!()
}

fn is_repo(top: &Path) -> bool {
    top.join(".git").exists()
}

/// A commit needs an author: when git has none configured, set a local one.
fn ensure_identity(top: &Path) {
    if git(top, &["config", "user.email"]).map(|s| s.is_empty()).unwrap_or(true) {
        let _ = git(top, &["config", "user.email", "iter-engine@localhost"]);
    }
    if git(top, &["config", "user.name"]).map(|s| s.is_empty()).unwrap_or(true) {
        let _ = git(top, &["config", "user.name", "iter engine"]);
    }
}

/// Commit exactly `rels` (relative paths; written, created or deleted) with
/// `msg`, under the checkout's git lock; push in the background when a
/// remote exists. Ok(None) = nothing to commit / not a repository.
pub fn commit_paths(top: &Path, rels: &[String], msg: &str) -> Result<Option<String>, String> {
    if rels.is_empty() || !is_repo(top) {
        return Ok(None);
    }
    let lock = crate::work::git_lock(&top.to_string_lossy());
    let _g = lock.lock().unwrap_or_else(|p| p.into_inner());
    // a path git ignores can never be committed; a deleted path git never
    // tracked has nothing to commit
    let present: Vec<PathBuf> = rels.iter().map(|r| top.join(r)).filter(|p| p.exists()).collect();
    let present: BTreeSet<String> = iter_local::drop_git_ignored(top, present)
        .into_iter()
        .filter_map(|p| p.strip_prefix(top).ok().map(|r| r.to_string_lossy().into_owned()))
        .collect();
    let mut paths: Vec<String> = Vec::new();
    for r in rels {
        if present.contains(r) || (!top.join(r).exists() && git(top, &["ls-files", "--", r]).map(|o| !o.is_empty()).unwrap_or(false)) {
            if !paths.contains(r) {
                paths.push(r.clone());
            }
        }
    }
    if paths.is_empty() {
        return Ok(None);
    }
    let mut add = vec!["add", "-A", "--"];
    add.extend(paths.iter().map(String::as_str));
    git(top, &add)?;
    let mut diff = vec!["diff", "--cached", "--quiet", "--"];
    diff.extend(paths.iter().map(String::as_str));
    if git(top, &diff).is_ok() {
        return Ok(None); // staged == HEAD for these paths
    }
    ensure_identity(top);
    let mut ci = vec!["commit", "-q", "--no-verify", "-m", msg, "--only", "--"];
    ci.extend(paths.iter().map(String::as_str));
    git(top, &ci)?;
    let sha = git(top, &["rev-parse", "HEAD"])?;
    push_async(top);
    Ok(Some(sha))
}

/// `git push` when the checkout has a remote, on its own thread (a slow or
/// unreachable remote never delays a tick; a failure is logged).
fn push_async(top: &Path) {
    let Ok(remotes) = git(top, &["remote"]) else { return };
    let Some(remote) = remotes.lines().find(|r| *r == "origin").or_else(|| remotes.lines().next()).map(String::from) else { return };
    let top = top.to_path_buf();
    std::thread::spawn(move || {
        if let Err(e) = git(&top, &["push", "-q", &remote, "HEAD"]) {
            eprintln!("[engine] push from {} failed: {e}", top.display());
        }
    });
}

// ---------------------------------------------------------------- locks

/// Live work-item lock paths of the project (`{topdir}/…`). None = could not
/// ask (treated as "everything locked": writes wait).
fn live_locks(api: &Api, project: &str) -> Option<Vec<String>> {
    let rows = api.get(&format!("/api/projects/{project}/locks")).ok()?;
    let rows = rows.as_array().cloned().unwrap_or_default();
    Some(
        rows.iter()
            .filter(|r| r["kind"] == "lock" && r["holder_lease_live"] == true)
            .filter_map(|r| r["path"].as_str().map(String::from))
            .collect(),
    )
}

struct Locks<'a> {
    api: &'a Api,
    project: &'a str,
    rows: Option<Option<Vec<String>>>,
}

impl Locks<'_> {
    fn holds(&mut self, stored_path: &str) -> bool {
        if self.rows.is_none() {
            self.rows = Some(live_locks(self.api, self.project));
        }
        match self.rows.as_ref().unwrap() {
            None => true,
            Some(rows) => rows.iter().any(|l| iter_core::paths_overlap(l, stored_path) || l == "{topdir}" || l == "{topdir}/"),
        }
    }
}

// ---------------------------------------------------------------- state file

fn load_state(st: &mut FileSyncState, api: &Api, project: &str, top: &Path) {
    st.loaded = true;
    st.fresh = true;
    let Ok(text) = std::fs::read_to_string(top.join(STATE_FILE)) else { return };
    let Ok(p) = serde_json::from_str::<Persisted>(&text) else { return };
    if p.server != api.base || p.project != project {
        return; // another server / project: start over
    }
    st.files = p.files;
    st.scandirs = p.scandirs;
    st.project_dir = p.project_dir;
    st.fresh = false;
}

fn save_state(st: &FileSyncState, api: &Api, project: &str, top: &Path) {
    let p = Persisted { version: 1, server: api.base.clone(), project: project.to_string(), scandirs: st.scandirs.clone(), project_dir: st.project_dir.clone(), files: st.files.clone() };
    let path = top.join(STATE_FILE);
    if let Some(d) = path.parent() {
        let _ = std::fs::create_dir_all(d);
    }
    let tmp = path.with_extension(format!("json.tmp.{}", std::process::id()));
    if std::fs::write(&tmp, serde_json::to_string(&p).unwrap_or_default()).is_ok() {
        let _ = std::fs::rename(&tmp, &path);
    }
}

// ---------------------------------------------------------------- scan

fn synced_node(path: &Path) -> bool {
    nodefile::type_of(&path.to_string_lossy()).is_some_and(nodefile::is_synced)
}

fn roots(st: &FileSyncState, top: &Path) -> Vec<PathBuf> {
    let mut r = walk::scan_roots(top, &st.scandirs);
    // the project node's own folder is always watched
    let pd = st.project_dir.as_ref().map(|d| top.join(d)).unwrap_or_else(|| top.join("global"));
    if pd.is_dir() && !r.iter().any(|x| pd.starts_with(x)) {
        r.push(pd);
    }
    r
}

/// (candidates to read, deleted) — both relative paths.
fn scan(st: &mut FileSyncState, top: &Path, full: bool) -> (Vec<String>, Vec<String>) {
    let rel = |p: &Path| p.strip_prefix(top).ok().map(|r| r.to_string_lossy().replace('\\', "/"));
    let mut new_files: Vec<PathBuf> = Vec::new();
    let mut candidates: BTreeSet<String> = std::mem::take(&mut st.retry);
    let mut deleted = Vec::new();
    if full {
        st.ignored = iter_local::GitIgnored::load(top);
        let mut dirs = HashMap::new();
        let mut found: BTreeSet<String> = BTreeSet::new();
        for r in roots(st, top) {
            let w = walk::walk(&r, &st.ignored);
            dirs.extend(w.dirs);
            for f in w.files.into_iter().filter(|f| synced_node(f)) {
                if let Some(rp) = rel(&f) {
                    found.insert(rp);
                }
            }
        }
        st.dirs = dirs;
        for rp in &found {
            match st.files.get(rp) {
                None => new_files.push(top.join(rp)),
                Some(m) => {
                    if walk::stat(&top.join(rp)) != Some(m.stat) {
                        candidates.insert(rp.clone());
                    }
                }
            }
        }
        for k in st.files.keys() {
            if !found.contains(k) {
                deleted.push(k.clone());
            }
        }
    } else {
        let known_dirs: Vec<(PathBuf, Stat)> = st.dirs.iter().map(|(d, s)| (d.clone(), *s)).collect();
        for (d, old) in known_dirs {
            match walk::stat(&d) {
                None => {
                    st.dirs.retain(|k, _| !k.starts_with(&d));
                }
                Some(s) if s.mtime_ns != old.mtime_ns => {
                    st.dirs.insert(d.clone(), s);
                    let (subdirs, files) = walk::list_dir(&d);
                    for sd in subdirs {
                        if !st.dirs.contains_key(&sd) && !st.ignored.contains(&sd) {
                            let w = walk::walk(&sd, &st.ignored);
                            st.dirs.extend(w.dirs);
                            new_files.extend(w.files.into_iter().filter(|f| synced_node(f)));
                        }
                    }
                    for f in files.into_iter().filter(|f| synced_node(f)) {
                        if rel(&f).is_some_and(|rp| !st.files.contains_key(&rp)) {
                            new_files.push(f);
                        }
                    }
                }
                Some(_) => {}
            }
        }
        for (k, m) in &st.files {
            match walk::stat(&top.join(k)) {
                None => deleted.push(k.clone()),
                Some(s) if s != m.stat => {
                    candidates.insert(k.clone());
                }
                Some(_) => {}
            }
        }
    }
    // one git call for everything new (a folder git ignores that appeared
    // since the last full walk, e.g. a fresh build output copy)
    new_files.sort();
    new_files.dedup();
    for f in iter_local::drop_git_ignored(top, new_files) {
        if let Some(rp) = rel(&f) {
            candidates.insert(rp);
        }
    }
    candidates.retain(|c| !deleted.contains(c));
    (candidates.into_iter().collect(), deleted)
}

// ---------------------------------------------------------------- tick

struct Outgoing {
    rel: String,
    text: String,
    meta: FileMeta,
}

/// One file-sync round for one project (see the module doc).
pub fn tick(
    api: &Api,
    engine: &str,
    project: &str,
    topdir: &Path,
    read_only: bool,
    files_waiting: bool,
    st: &mut FileSyncState,
) -> Result<(), String> {
    if !topdir.is_dir() {
        return Ok(()); // not built yet
    }
    let top = canon(topdir);
    if !st.loaded {
        load_state(st, api, project, &top);
    }
    // files a build just wrote
    {
        let mut built = BUILT.lock().unwrap_or_else(|p| p.into_inner());
        let mine: Vec<(PathBuf, String, FileMeta)> = built.iter().filter(|(t, _, _)| *t == top).cloned().collect();
        built.retain(|(t, _, _)| *t != top);
        for (_, rel, m) in mine {
            st.files.insert(rel, m);
            st.dirty = true;
        }
    }
    let mut stats = TickStats::default();
    let full = st.last_full.is_none_or(|t| t.elapsed() >= FULL_EVERY);
    stats.full = full;
    let (candidates, deleted) = scan(st, &top, full);
    if full {
        st.last_full = Some(Instant::now());
    }
    let mut locks = Locks { api, project, rows: None };
    let now = nodefile::now_ts();
    let creator = format!("engine.{engine}");
    let mut outgoing: Vec<Outgoing> = Vec::new();
    let mut written: Vec<String> = Vec::new(); // conform write-backs (relative)
    let mut touch_only: Vec<(String, Stat)> = Vec::new();
    for rp in &candidates {
        let path = top.join(rp);
        let Ok(text) = std::fs::read_to_string(&path) else { continue };
        stats.read += 1;
        let disk_hash = nodefile::content_hash(&text);
        let prev = st.files.get(rp).cloned();
        if let Some(m) = &prev {
            if m.hash == disk_hash {
                if let Some(s) = walk::stat(&path) {
                    touch_only.push((rp.clone(), s));
                }
                continue;
            }
        }
        let sp = stored(rp);
        // a hand edit is stamped with the file's own mtime, not the scan time:
        // a file edited while the engine was down must not look newer than a
        // server edit made after it (conflicts go to the newer last_modified)
        let edited = file_mtime_ts(&path).unwrap_or_else(|| now.clone());
        let c = nodefile::conform_against(&sp, &text, &edited, &creator, prev.as_ref().map(|m| m.sem.as_str()).filter(|s| !s.is_empty()));
        let Some(doc) = c.doc else { continue };
        let mut meta = FileMeta {
            id: doc.id.clone(),
            hash: disk_hash,
            sem: nodefile::semantic_hash(&doc),
            version: prev.as_ref().filter(|m| m.id == doc.id).map(|m| m.version).unwrap_or(0),
            stat: walk::stat(&path).unwrap_or_default(),
        };
        if c.changed && !read_only {
            if locks.holds(&sp) {
                // a running item is editing here: the node gets the conformed
                // text now, the file once the lock is gone
                st.deferred.insert(rp.clone(), (c.text.clone(), None, Some(meta.hash.clone())));
            } else if std::fs::write(&path, &c.text).is_ok() {
                meta.hash = nodefile::content_hash(&c.text);
                meta.stat = walk::stat(&path).unwrap_or_default();
                written.push(rp.clone());
            }
        }
        if doc.nodetype == NodeType::Project {
            let sd: Vec<String> = doc.front.get("scandirs").and_then(|v| v.as_array()).map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect()).unwrap_or_default();
            st.project_dir = Some(rp.rsplit_once('/').map(|(d, _)| d.to_string()).unwrap_or_default());
            if walk::scan_roots(&top, &sd) != walk::scan_roots(&top, &st.scandirs) {
                st.last_full = None; // walk the new roots next tick
            }
            st.scandirs = sd;
        }
        outgoing.push(Outgoing { rel: rp.clone(), text: c.text, meta });
    }
    for (rp, s) in touch_only {
        if let Some(m) = st.files.get_mut(&rp) {
            m.stat = s;
            st.dirty = true;
        }
    }
    stats.conformed = written.len();

    // ---- sync
    let mut changed_something = false;
    let mut rewritten: Vec<String> = Vec::new();
    if !outgoing.is_empty() || !deleted.is_empty() {
        let by_id: HashMap<String, u64> = st.files.values().map(|m| (m.id.clone(), m.version)).collect();
        let files: Vec<Value> = outgoing
            .iter()
            .map(|o| {
                let base = if o.meta.version > 0 { o.meta.version } else { by_id.get(&o.meta.id).copied().unwrap_or(0) };
                json!({"path": stored(&o.rel), "text": o.text, "hash": o.meta.hash, "base_version": base})
            })
            .collect();
        let send_full = full && st.fresh;
        let body = json!({
            "engine": engine,
            "full": send_full,
            "files": files,
            "deleted": deleted.iter().map(|d| stored(d)).collect::<Vec<_>>(),
        });
        match api.post(&format!("/api/projects/{project}/files/sync"), &body) {
            Ok(reply) => {
                st.fresh = false;
                stats.sent = outgoing.len();
                stats.deleted = deleted.len();
                changed_something = true;
                for d in &deleted {
                    st.files.remove(d);
                }
                for o in outgoing {
                    st.files.insert(o.rel, o.meta);
                }
                for a in reply["applied"].as_array().into_iter().flatten() {
                    let (Some(id), Some(v)) = (a["id"].as_str(), a["node_version"].as_u64()) else { continue };
                    let path = a["path"].as_str().map(rel_of).map(String::from);
                    for (k, m) in st.files.iter_mut() {
                        if m.id == id && path.as_deref().is_none_or(|p| p == k) {
                            m.version = v;
                        }
                    }
                }
                for r in reply["rewrite"].as_array().into_iter().flatten() {
                    let (Some(p), Some(text)) = (r["path"].as_str(), r["text"].as_str()) else { continue };
                    let v = r["node_version"].as_u64().unwrap_or(0);
                    st.deferred.insert(rel_of(p).to_string(), (text.to_string(), Some(v), None));
                }
                for c in reply["conflicts"].as_array().into_iter().flatten() {
                    println!("[engine] {project}: file sync conflict {}", c);
                }
                for id in reply["removed"].as_array().into_iter().flatten().filter_map(|x| x.as_str()) {
                    println!("[engine] {project}: node {id} removed from the graph");
                }
                st.dirty = true;
            }
            Err(e) => {
                // nothing recorded: every candidate is read again next tick
                for o in &outgoing {
                    st.retry.insert(o.rel.clone());
                    st.deferred.remove(&o.rel);
                }
                commit_conform(&top, &written, &mut stats, project, read_only);
                st.last = stats;
                return Err(format!("files/sync: {e}"));
            }
        }
    }

    // ---- server rewrites (lock-aware)
    if !read_only && !st.deferred.is_empty() {
        let deferred = std::mem::take(&mut st.deferred);
        for (rp, (text, v, expect)) in deferred {
            let sp = stored(&rp);
            let Ok(path) = walk::real_path(&top, &sp) else {
                eprintln!("[engine] {project}: refused a rewrite outside the checkout: {sp}");
                continue;
            };
            if let Some(h) = &expect {
                // made from a text the file no longer holds: the scan sees the
                // newer edit and conforms that instead
                let now_hash = std::fs::read_to_string(&path).map(|t| nodefile::content_hash(&t)).unwrap_or_default();
                if &now_hash != h {
                    continue;
                }
            }
            if locks.holds(&sp) {
                st.deferred.insert(rp, (text, v, expect));
                continue;
            }
            if let Some(d) = path.parent() {
                let _ = std::fs::create_dir_all(d);
            }
            if std::fs::write(&path, &text).is_err() {
                st.deferred.insert(rp, (text, v, expect));
                continue;
            }
            let doc = nodefile::parse_tolerant(&sp, &text).ok().map(|(d, _)| d);
            st.files.insert(rp.clone(), FileMeta {
                id: doc.as_ref().map(|d| d.id.clone()).unwrap_or_default(),
                hash: nodefile::content_hash(&text),
                sem: doc.as_ref().map(nodefile::semantic_hash).unwrap_or_default(),
                version: v.or_else(|| st.files.get(&rp).map(|m| m.version)).unwrap_or(0),
                stat: walk::stat(&path).unwrap_or_default(),
            });
            rewritten.push(rp);
            st.dirty = true;
        }
    }
    stats.rewritten = rewritten.len();
    let mut to_commit = written.clone();
    to_commit.extend(rewritten.iter().cloned());
    to_commit.sort();
    to_commit.dedup();
    commit_conform(&top, &to_commit, &mut stats, project, read_only);

    // ---- pending graph edits
    if files_waiting && !read_only {
        match apply_pending(api, engine, project, &top, st, &mut locks, false) {
            Ok((applied, waiting, commit)) => {
                stats.pending_applied = applied;
                stats.pending_waiting = waiting;
                if let Some(c) = commit {
                    stats.commits.push(c);
                }
                if applied > 0 {
                    changed_something = true;
                }
            }
            Err(e) => {
                st.last = stats;
                return Err(e);
            }
        }
    }

    if st.dirty && !read_only {
        save_state(st, api, project, &top);
        st.dirty = false;
    }
    if changed_something {
        if stats.sent + stats.deleted + stats.pending_applied > 0 {
            println!(
                "[engine] {project}: file sync — {} sent, {} deleted, {} conformed, {} rewritten, {} graph edit(s) written{}",
                stats.sent, stats.deleted, stats.conformed, stats.rewritten, stats.pending_applied,
                if stats.full { " (full scan)" } else { "" }
            );
        }
        maybe_rag(api, project, &top, st);
    }
    st.last = stats;
    Ok(())
}

fn commit_conform(top: &Path, rels: &[String], stats: &mut TickStats, project: &str, read_only: bool) {
    if rels.is_empty() || read_only {
        return;
    }
    let msg = format!("iter: conform {} node file{}", rels.len(), if rels.len() == 1 { "" } else { "s" });
    match commit_paths(top, rels, &msg) {
        Ok(Some(sha)) => stats.commits.push(sha),
        Ok(None) => {}
        Err(e) => eprintln!("[engine] {project}: conform commit failed: {e}"),
    }
}

/// GraphRAG re-index after a sync round changed something: one background
/// run per project at a time, at most once per [`RAG_EVERY`].
fn maybe_rag(api: &Api, project: &str, top: &Path, st: &mut FileSyncState) {
    if std::env::var("ITER_NO_RAG_SYNC").is_ok() || cfg!(test) {
        return;
    }
    if st.rag.as_ref().is_some_and(|h| !h.is_finished()) {
        return;
    }
    if st.last_rag.is_some_and(|t| t.elapsed() < RAG_EVERY) {
        return;
    }
    st.last_rag = Some(Instant::now());
    let (api, project, top) = (api.clone(), project.to_string(), top.to_path_buf());
    st.rag = Some(std::thread::spawn(move || match crate::rag::sync(&api, &project, &top, false, false) {
        Ok(r) if r.sent > 0 || r.removed > 0 => println!(
            "[engine] {project}: GraphRAG re-indexed after a file sync — {} node file(s) sent (+{} ~{} -{})",
            r.sent, r.added, r.changed, r.removed
        ),
        Ok(_) => {}
        Err(e) => eprintln!("[engine] {project}: GraphRAG re-index skipped: {e}"),
    }));
}

// ---------------------------------------------------------------- pending

fn pending_rows(api: &Api, project: &str) -> Result<Vec<Value>, String> {
    let v = api.get(&format!("/api/projects/{project}/files/pending")).map_err(|e| format!("files/pending: {e}"))?;
    Ok(v.as_array().cloned().or_else(|| v["pending"].as_array().cloned()).unwrap_or_default())
}

fn file_name(stored_path: &str) -> &str {
    stored_path.rsplit('/').next().unwrap_or(stored_path)
}

/// Apply the pending graph edits: (applied, waiting on a lock, commit).
/// `build` = the designer build (commit message and no lock waits on a
/// fresh tree are the caller's business; `extra` paths join the commit).
fn apply_pending(
    api: &Api,
    engine: &str,
    project: &str,
    top: &Path,
    st: &mut FileSyncState,
    locks: &mut Locks,
    build: bool,
) -> Result<(usize, usize, Option<String>), String> {
    let rows = pending_rows(api, project)?;
    if rows.is_empty() && !build {
        return Ok((0, 0, None));
    }
    let mut touched: Vec<String> = Vec::new();
    let mut acks: Vec<Value> = Vec::new();
    let mut summary: Vec<String> = Vec::new();
    let mut waiting = 0usize;
    for row in &rows {
        let id = row["id"].as_str().unwrap_or("").to_string();
        let op = row["op"].as_str().unwrap_or("write");
        let sp = row["path"].as_str().unwrap_or("").to_string();
        let old = row["old_path"].as_str().map(String::from);
        let text = row["text"].as_str().unwrap_or("").to_string();
        let version = row["node_version"].as_u64().unwrap_or(0);
        let path = match walk::real_path(top, &sp) {
            Ok(p) => p,
            Err(e) => {
                eprintln!("[engine] {project}: refused pending {op} of node {id}: {e}");
                continue;
            }
        };
        let old_path = match old.as_deref().filter(|_| op == "move") {
            Some(o) => match walk::real_path(top, o) {
                Ok(p) => Some(p),
                Err(e) => {
                    eprintln!("[engine] {project}: refused pending move of node {id}: {e}");
                    continue;
                }
            },
            None => None,
        };
        if locks.holds(&sp) || old.as_deref().filter(|_| op == "move").is_some_and(|o| locks.holds(o)) {
            println!("[engine] {project}: graph edit of {} waits: a running work item holds a lock on it", file_name(&sp));
            waiting += 1;
            continue;
        }
        let rel = rel_of(&sp).to_string();
        let hash = match op {
            "delete" => {
                if path.exists() {
                    if let Err(e) = std::fs::remove_file(&path) {
                        eprintln!("[engine] {project}: cannot delete {sp}: {e}");
                        continue;
                    }
                }
                st.files.remove(&rel);
                summary.push(format!("delete {}", file_name(&sp)));
                String::new()
            }
            "write" | "move" => {
                if let Some(d) = path.parent() {
                    if let Err(e) = std::fs::create_dir_all(d) {
                        eprintln!("[engine] {project}: cannot create {}: {e}", d.display());
                        continue;
                    }
                }
                if let Err(e) = std::fs::write(&path, &text) {
                    eprintln!("[engine] {project}: cannot write {sp}: {e}");
                    continue;
                }
                if let (Some(op_old), Some(o)) = (&old_path, &old) {
                    if op_old != &path && op_old.exists() {
                        let _ = std::fs::remove_file(op_old);
                    }
                    let orel = rel_of(o).to_string();
                    st.files.remove(&orel);
                    touched.push(orel);
                    summary.push(format!("move {} → {}", file_name(o), file_name(&sp)));
                } else {
                    summary.push(format!("write {}", file_name(&sp)));
                }
                let doc = nodefile::parse_tolerant(&sp, &text).ok().map(|(d, _)| d);
                let h = nodefile::content_hash(&text);
                if nodefile::type_of(&sp).is_some_and(nodefile::is_synced) {
                    st.files.insert(rel.clone(), FileMeta {
                        id: if id.is_empty() { doc.as_ref().map(|d| d.id.clone()).unwrap_or_default() } else { id.clone() },
                        hash: h.clone(),
                        sem: doc.as_ref().map(nodefile::semantic_hash).unwrap_or_default(),
                        version,
                        stat: walk::stat(&path).unwrap_or_default(),
                    });
                }
                h
            }
            other => {
                eprintln!("[engine] {project}: unknown pending op {other:?} for node {id}; skipped");
                continue;
            }
        };
        // the server's own one-line summary of the edit, when it gives one
        if let Some(sm) = row["summary"].as_str().map(str::trim).filter(|x| !x.is_empty()) {
            if let Some(last) = summary.last_mut() {
                *last = sm.to_string();
            }
        }
        touched.push(rel);
        st.dirty = true;
        acks.push(json!({"id": id, "node_version": version, "path": sp, "hash": hash, "commit": ""}));
    }
    if acks.is_empty() && !build {
        return Ok((0, waiting, None));
    }
    let commit = if build {
        touched.push(".gitignore".into());
        commit_paths(top, &touched, "iter: build from design")?
    } else {
        let mut msg = format!("iter: graph edit — {}", summary.iter().take(3).cloned().collect::<Vec<_>>().join(", "));
        if summary.len() > 3 {
            msg.push_str(&format!(" and {} more", summary.len() - 3));
        }
        match commit_paths(top, &touched, &msg) {
            Ok(c) => c,
            Err(e) => {
                eprintln!("[engine] {project}: graph edit commit failed: {e}");
                None
            }
        }
    };
    let sha = commit.clone().unwrap_or_default();
    for a in acks.iter_mut() {
        a["commit"] = json!(sha);
    }
    let n = acks.len();
    if !acks.is_empty() {
        api.post(&format!("/api/projects/{project}/files/ack"), &json!({"engine": engine, "acks": acks}))
            .map_err(|e| format!("files/ack: {e}"))?;
    }
    Ok((n, waiting, commit))
}

// ---------------------------------------------------------------- build

/// The project node's `gitrepo` (from its pending text, else the graph).
fn gitrepo_of(api: &Api, project: &str) -> String {
    let from_text = |sp: &str, text: &str| -> Option<String> {
        if nodefile::type_of(sp) != Some(NodeType::Project) {
            return None;
        }
        let (d, _) = nodefile::parse_tolerant(sp, text).ok()?;
        d.front.get("gitrepo").and_then(|v| v.as_str()).map(String::from)
    };
    if let Ok(rows) = pending_rows(api, project) {
        for r in &rows {
            if let Some(g) = from_text(r["path"].as_str().unwrap_or(""), r["text"].as_str().unwrap_or("")) {
                return g.trim().to_string();
            }
        }
    }
    if let Ok(g) = api.get(&format!("/api/projects/{project}/graph")) {
        for n in g["nodes"].as_array().into_iter().flatten() {
            if n["nodetype"] == "project" {
                return n["front"]["gitrepo"].as_str().unwrap_or("").trim().to_string();
            }
        }
    }
    String::new()
}

/// The designer build (§3.4): `mkdir -p`, `git init` (+ `origin` from the
/// project node's `gitrepo`), `.gitignore` with `.iter/`, every pending file
/// written, one commit `iter: build from design`, acks, `POST build/done`.
pub fn build(api: &Api, engine: &str, project: &str, topdir: &Path) -> Result<(), String> {
    std::fs::create_dir_all(topdir).map_err(|e| format!("cannot create {}: {e}", topdir.display()))?;
    let top = canon(topdir);
    if !is_repo(&top) {
        git(&top, &["init", "-q"])?;
    }
    let repo = gitrepo_of(api, project);
    if !repo.is_empty() {
        let remotes = git(&top, &["remote"]).unwrap_or_default();
        if !remotes.lines().any(|r| r == "origin") {
            git(&top, &["remote", "add", "origin", &repo])?;
        }
    }
    let gi = top.join(".gitignore");
    let cur = std::fs::read_to_string(&gi).unwrap_or_default();
    if !cur.lines().any(|l| matches!(l.trim(), ".iter/" | ".iter" | "/.iter/" | "/.iter")) {
        let mut s = cur.clone();
        if !s.is_empty() && !s.ends_with('\n') {
            s.push('\n');
        }
        s.push_str(".iter/\n");
        std::fs::write(&gi, s).map_err(|e| format!("cannot write .gitignore: {e}"))?;
    }
    let mut st = FileSyncState::default();
    load_state(&mut st, api, project, &top);
    let mut locks = Locks { api, project, rows: None };
    let (n, waiting, commit) = apply_pending(api, engine, project, &top, &mut st, &mut locks, true)?;
    save_state(&st, api, project, &top);
    {
        let mut built = BUILT.lock().unwrap_or_else(|p| p.into_inner());
        for (rel, m) in &st.files {
            built.push((top.clone(), rel.clone(), m.clone()));
        }
    }
    let commit = commit.unwrap_or_else(|| git(&top, &["rev-parse", "HEAD"]).unwrap_or_default());
    api.post(&format!("/api/projects/{project}/build/done"), &json!({"engine": engine, "commit": commit}))
        .map_err(|e| format!("build/done: {e}"))?;
    println!(
        "[engine] {project}: built {} from the design — {n} file(s), commit {}{}",
        top.display(),
        &commit[..commit.len().min(12)],
        if waiting > 0 { format!(", {waiting} waiting on locks") } else { String::new() }
    );
    Ok(())
}

#[cfg(test)]
#[path = "filesync_tests.rs"]
mod tests;

/// A file's modification time in the node-file timestamp format, capped at now.
fn file_mtime_ts(path: &Path) -> Option<String> {
    let modified = std::fs::metadata(path).ok()?.modified().ok()?;
    let t: chrono::DateTime<chrono::Utc> = modified.into();
    Some(t.min(chrono::Utc::now()).format("%Y-%m-%d %H:%M:%SZ").to_string())
}
