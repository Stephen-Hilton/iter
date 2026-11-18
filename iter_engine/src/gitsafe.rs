//! Every git command the engine runs in a checkout goes through here (built
//! 2026-11-18, after a night on which ~155 work items failed in a row on
//! bakugo: a `git fetch` killed at 00:13 left
//! `.git/refs/remotes/origin/main.lock` behind with no git alive, every
//! item's pull refused that lock, and the engine kept starting items).
//!
//! Three things happen here:
//!
//! 1. **One git command at a time per repository.**  [`guard`] holds an
//!    in-process mutex for the repository (keyed by its common git dir, so
//!    every worktree of one repository shares it) and an exclusive file lock
//!    (`flock`) on `<common git dir>/iter-engine-git.mutex`, which the engine's
//!    own `iter` verbs (run by agents: `iter sync`, …) take too.  The guard is
//!    re-entrant on one thread, so a step that already holds it (a pull
//!    running its merge commit) never waits on itself.  Agents' own `git`
//!    commands in their Bash tool do not come through here (see the module
//!    doc of `loop.code.iter.md`, "git serialization").
//! 2. **Stale lock files are cleared.**  [`clear_stale_locks`] finds every
//!    `*.lock` under the common git dir (worktrees' own dirs under
//!    `worktrees/*/` included; the object store is skipped) and deletes one
//!    only when it is older than [`LOCK_STALE`] AND no live git process uses
//!    that repository: a process whose `/proc/<pid>/comm` is `git` (or
//!    `git-…`) with its working directory inside one of the repository's
//!    work trees or its git dir, or with `GIT_DIR` / `GIT_COMMON_DIR` /
//!    `GIT_WORK_TREE` pointing there.  Processes are judged by what they ARE,
//!    never by words in their command lines (agents run as `claude -p
//!    <prompt>` and their prompts mention git).  Every deletion is logged
//!    with its path and age.  Off Linux, any running `git` at all keeps every
//!    lock.
//! 3. **A command that fails on a lock is retried** ([`RETRIES`] times with
//!    a doubling pause, a forced lock sweep before each), and a command that
//!    still fails records its error class on the thread (`last_failure`) for
//!    the repair loop (crate::gitrepair).

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, OnceLock};
use std::time::{Duration, Instant, SystemTime};

/// A lock file this old, with no live git using its repository, was left
/// behind by a git that ended without removing it.
pub const LOCK_STALE: Duration = Duration::from_secs(120);
/// A command that failed on a lock is tried this many more times.
pub const RETRIES: u32 = 5;
/// The first pause before a retry; it doubles each time (0.5+1+2+4+8 s).
const FIRST_PAUSE: Duration = Duration::from_millis(500);
/// Unforced lock sweeps run at most this often per repository.
const SWEEP_EVERY: Duration = Duration::from_secs(30);
/// The engine's cross-process lock file in the common git dir.  It does not
/// end in `.lock`, so neither git nor the sweep ever treats it as git's.
pub const MUTEX_FILE: &str = "iter-engine-git.mutex";
/// Longest wait for another process's hold on [`MUTEX_FILE`]; after that the
/// command runs anyway (logged), so a hung holder can never stop the engine.
const FILE_LOCK_WAIT: Duration = Duration::from_secs(600);

// ---------------------------------------------------------------- the repository

/// Where a checkout's git data lives.
#[derive(Debug, Clone, PartialEq)]
pub struct GitDirs {
    /// the checkout's own git dir (`.git`, or `<common>/worktrees/<name>`)
    pub git_dir: PathBuf,
    /// the repository's shared git dir (refs, objects, config)
    pub common: PathBuf,
    /// every work tree of the repository: the main one and each linked worktree
    pub work_trees: Vec<PathBuf>,
}

fn canon(p: &Path) -> PathBuf {
    iter_core::platform::canonicalize(p).unwrap_or_else(|_| p.to_path_buf())
}

/// The git dirs of the checkout at `top`; None when it is not a repository.
pub fn dirs(top: &Path) -> Option<GitDirs> {
    let dot = top.join(".git");
    let git_dir = if dot.is_dir() {
        dot
    } else if dot.is_file() {
        // a linked worktree: ".git" holds "gitdir: <path>"
        let text = std::fs::read_to_string(&dot).ok()?;
        let p = PathBuf::from(text.trim().strip_prefix("gitdir:")?.trim());
        if p.is_absolute() { p } else { top.join(p) }
    } else {
        return None;
    };
    let git_dir = canon(&git_dir);
    let common = match std::fs::read_to_string(git_dir.join("commondir")) {
        Ok(c) => {
            let p = PathBuf::from(c.trim());
            canon(&if p.is_absolute() { p } else { git_dir.join(p) })
        }
        Err(_) => git_dir.clone(),
    };
    let mut work_trees = vec![canon(top)];
    // the main work tree is the common dir's parent when that dir is ".git"
    if common.file_name().is_some_and(|n| n == ".git") {
        if let Some(parent) = common.parent() {
            work_trees.push(parent.to_path_buf());
        }
    }
    if let Ok(rd) = std::fs::read_dir(common.join("worktrees")) {
        for e in rd.flatten() {
            // worktrees/<name>/gitdir names "<work tree>/.git"
            if let Ok(g) = std::fs::read_to_string(e.path().join("gitdir")) {
                if let Some(wt) = Path::new(g.trim()).parent() {
                    work_trees.push(canon(wt));
                }
            }
        }
    }
    work_trees.sort();
    work_trees.dedup();
    Some(GitDirs { git_dir, common, work_trees })
}

/// The key every lock of `top` is taken under: its repository's common git
/// dir (so all worktrees of one repository share one lock), else the path.
pub fn key(top: &Path) -> String {
    match dirs(top) {
        Some(d) => d.common.to_string_lossy().into_owned(),
        None => canon(top).to_string_lossy().trim_end_matches('/').to_string(),
    }
}

// ---------------------------------------------------------------- one command at a time

thread_local! {
    /// repositories this thread holds the guard for, with the nesting depth
    static HELD: RefCell<HashMap<String, u32>> = RefCell::new(HashMap::new());
    /// the last engine git command on this thread that failed for good
    static LAST_FAILURE: RefCell<Option<Failure>> = const { RefCell::new(None) };
}

fn mutex_for(key: &str) -> &'static Mutex<()> {
    static MAP: OnceLock<Mutex<HashMap<String, &'static Mutex<()>>>> = OnceLock::new();
    let mut m = MAP.get_or_init(Default::default).lock().unwrap_or_else(|p| p.into_inner());
    m.entry(key.to_string()).or_insert_with(|| Box::leak(Box::new(Mutex::new(()))))
}

/// Held while one engine git step runs in a repository: the in-process mutex
/// and the cross-process file lock.  Dropping it releases both.
pub struct GitGuard {
    key: String,
    // field order: the file lock is released before the mutex
    _file: Option<std::fs::File>,
    _mutex: Option<MutexGuard<'static, ()>>,
}

impl Drop for GitGuard {
    fn drop(&mut self) {
        HELD.with(|h| {
            let mut h = h.borrow_mut();
            if let Some(n) = h.get_mut(&self.key) {
                *n -= 1;
                if *n == 0 {
                    h.remove(&self.key);
                }
            }
        });
    }
}

/// Take the repository's git lock for this thread (re-entrant).
pub fn guard(top: &str) -> GitGuard {
    let top = Path::new(top);
    let key = key(top);
    let nested = HELD.with(|h| {
        let mut h = h.borrow_mut();
        let n = h.entry(key.clone()).or_insert(0);
        *n += 1;
        *n > 1
    });
    if nested {
        return GitGuard { key, _file: None, _mutex: None };
    }
    let mutex = mutex_for(&key).lock().unwrap_or_else(|p| p.into_inner());
    let file = dirs(top).and_then(|d| file_lock(&d.common.join(MUTEX_FILE)));
    GitGuard { key, _file: file, _mutex: Some(mutex) }
}

/// Is this thread holding the guard for `top`'s repository?
#[cfg(test)]
pub fn held(top: &str) -> bool {
    let key = key(Path::new(top));
    HELD.with(|h| h.borrow().get(&key).copied().unwrap_or(0) > 0)
}

fn file_lock(path: &Path) -> Option<std::fs::File> {
    let f = std::fs::OpenOptions::new().create(true).truncate(false).write(true).open(path).ok()?;
    let started = Instant::now();
    loop {
        match f.try_lock() {
            Ok(()) => return Some(f),
            Err(std::fs::TryLockError::WouldBlock) if started.elapsed() < FILE_LOCK_WAIT => {
                std::thread::sleep(Duration::from_millis(100));
            }
            Err(std::fs::TryLockError::WouldBlock) => {
                println!("[engine] git: {} held by another process for over {}s — running without it", path.display(), FILE_LOCK_WAIT.as_secs());
                return None;
            }
            Err(std::fs::TryLockError::Error(e)) => {
                println!("[engine] git: cannot lock {}: {e} — running without it", path.display());
                return None;
            }
        }
    }
}

// ---------------------------------------------------------------- stale locks

/// One lock file the sweep deleted.
#[derive(Debug, Clone, PartialEq)]
pub struct Cleared {
    pub path: PathBuf,
    pub age_secs: u64,
}

/// Every `*.lock` file under the common git dir (the object store is
/// skipped except `objects/info`, where commit-graph writes lock).
pub fn lock_files(d: &GitDirs) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![d.common.clone()];
    if d.git_dir != d.common && !d.git_dir.starts_with(&d.common) {
        stack.push(d.git_dir.clone());
    }
    while let Some(dir) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&dir) else { continue };
        for e in rd.flatten() {
            let Ok(ft) = e.file_type() else { continue };
            let p = e.path();
            if ft.is_dir() {
                if p == d.common.join("objects") {
                    stack.push(p.join("info"));
                } else {
                    stack.push(p);
                }
            } else if ft.is_file() && p.extension().is_some_and(|x| x == "lock") {
                out.push(p);
            }
        }
    }
    out.sort();
    out
}

fn age_of(p: &Path) -> Option<Duration> {
    let m = std::fs::metadata(p).ok()?.modified().ok()?;
    Some(SystemTime::now().duration_since(m).unwrap_or_default())
}

/// The live git processes using the repository (pid, what tied it), as the
/// operating system reports them.  Off Linux: every running `git` (no way
/// to tell its directory without /proc), or one entry when `ps` fails.
pub fn live_git(d: &GitDirs) -> Vec<(u32, String)> {
    #[cfg(target_os = "linux")]
    {
        live_git_proc(d, Path::new("/proc"))
    }
    #[cfg(not(target_os = "linux"))]
    {
        let Ok(out) = std::process::Command::new("ps").args(["-A", "-o", "pid=,comm="]).output() else {
            return vec![(0, "ps failed: cannot tell whether git is running".into())];
        };
        let _ = d;
        String::from_utf8_lossy(&out.stdout)
            .lines()
            .filter_map(|l| {
                let mut it = l.split_whitespace();
                let pid = it.next()?.parse().ok()?;
                let comm = it.collect::<Vec<_>>().join(" ");
                let base = Path::new(&comm).file_name()?.to_string_lossy().into_owned();
                is_git_program(&base).then(|| (pid, format!("{base} is running")))
            })
            .collect()
    }
}

/// `git`, or one of git's own helper programs (`git-remote-https`, …).
fn is_git_program(comm: &str) -> bool {
    comm == "git" || comm.starts_with("git-")
}

/// [`live_git`] over a /proc tree (Linux).  A git process whose directory
/// cannot be read (another user's) counts as using the repository: the
/// sweep must never delete a lock a live git may hold.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub fn live_git_proc(d: &GitDirs, proc_root: &Path) -> Vec<(u32, String)> {
    let me = std::process::id();
    let inside = |p: &Path| -> bool { p.starts_with(&d.common) || p.starts_with(&d.git_dir) || d.work_trees.iter().any(|w| p.starts_with(w)) };
    let mut out = Vec::new();
    let Ok(rd) = std::fs::read_dir(proc_root) else { return vec![(0, format!("cannot read {}", proc_root.display()))] };
    for e in rd.flatten() {
        let Some(pid) = e.file_name().to_str().and_then(|s| s.parse::<u32>().ok()) else { continue };
        if pid == me {
            continue;
        }
        let dir = e.path();
        let Ok(comm) = std::fs::read_to_string(dir.join("comm")) else { continue };
        let comm = comm.trim();
        if !is_git_program(comm) {
            continue;
        }
        let Ok(cwd) = std::fs::read_link(dir.join("cwd")) else {
            out.push((pid, format!("{comm} whose working directory cannot be read")));
            continue;
        };
        if inside(&cwd) {
            out.push((pid, format!("{comm} in {}", cwd.display())));
            continue;
        }
        let env = std::fs::read(dir.join("environ")).unwrap_or_default();
        for kv in env.split(|b| *b == 0) {
            let kv = String::from_utf8_lossy(kv);
            for var in ["GIT_DIR=", "GIT_COMMON_DIR=", "GIT_WORK_TREE="] {
                if let Some(v) = kv.strip_prefix(var) {
                    let p = Path::new(v);
                    let p = canon(&if p.is_absolute() { p.to_path_buf() } else { cwd.join(p) });
                    if inside(&p) {
                        out.push((pid, format!("{comm} with {var}{v}")));
                    }
                }
            }
        }
    }
    out
}

/// Delete the stale lock files of `top`'s repository (module doc, point 2);
/// returns what was deleted.  Each deletion is logged.
pub fn clear_stale_locks(top: &Path) -> Vec<Cleared> {
    clear_stale_locks_with(top, LOCK_STALE, &live_git)
}

/// [`clear_stale_locks`] with the age limit and the liveness test given (tests).
pub fn clear_stale_locks_with(top: &Path, min_age: Duration, live: &dyn Fn(&GitDirs) -> Vec<(u32, String)>) -> Vec<Cleared> {
    let Some(d) = dirs(top) else { return vec![] };
    let old: Vec<PathBuf> = lock_files(&d).into_iter().filter(|p| age_of(p).is_some_and(|a| a >= min_age)).collect();
    if old.is_empty() {
        return vec![];
    }
    let users = live(&d);
    if !users.is_empty() {
        let who: Vec<String> = users.iter().take(3).map(|(pid, why)| format!("pid {pid} ({why})")).collect();
        println!(
            "[engine] git: {} lock file(s) older than {}s in {} kept — a live git uses the repository: {}",
            old.len(), min_age.as_secs(), d.common.display(), who.join(", ")
        );
        return vec![];
    }
    let mut cleared = Vec::new();
    for p in old {
        // re-read the age: a git that started since may have re-made it
        let Some(age) = age_of(&p).filter(|a| *a >= min_age) else { continue };
        match std::fs::remove_file(&p) {
            Ok(()) => {
                println!("[engine] git: removed stale lock {} ({}s old, no live git uses the repository)", p.display(), age.as_secs());
                cleared.push(Cleared { path: p, age_secs: age.as_secs() });
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => println!("[engine] git: could not remove stale lock {}: {e}", p.display()),
        }
    }
    cleared
}

/// [`clear_stale_locks`], at most once per [`SWEEP_EVERY`] per repository
/// unless `force`.
pub fn sweep(top: &Path, force: bool) -> Vec<Cleared> {
    static LAST: OnceLock<Mutex<HashMap<String, Instant>>> = OnceLock::new();
    let k = key(top);
    {
        let mut m = LAST.get_or_init(Default::default).lock().unwrap_or_else(|p| p.into_inner());
        if !force && m.get(&k).is_some_and(|t| t.elapsed() < SWEEP_EVERY) {
            return vec![];
        }
        m.insert(k, Instant::now());
    }
    clear_stale_locks(top)
}

// ---------------------------------------------------------------- error classes

/// The kind of git trouble a failed command's output shows: three items in a
/// row failing with the same class pause the checkout (crate::gitrepair).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Class {
    /// `index.lock` or another non-ref lock file is in the way
    Lock,
    /// a ref (or `packed-refs`) could not be locked or moved
    RefLock,
    /// the index file is corrupt
    Index,
    /// objects or HEAD cannot be read (empty or corrupt objects, broken refs)
    Objects,
    /// a merge, rebase or cherry-pick was left unfinished
    Unfinished,
}

impl Class {
    pub fn label(self) -> &'static str {
        match self {
            Class::Lock => "a git lock file is in the way",
            Class::RefLock => "a git ref cannot be locked or moved",
            Class::Index => "the git index is corrupt",
            Class::Objects => "git objects or HEAD cannot be read",
            Class::Unfinished => "a merge or rebase was left unfinished",
        }
    }

    /// Worth retrying in a moment: another git may simply be finishing.
    pub fn retryable(self) -> bool {
        matches!(self, Class::Lock | Class::RefLock)
    }
}

/// The class of a failed git command's output; None for trouble a repair
/// of the checkout cannot fix (network, authentication, a real conflict,
/// a hook's refusal, …).
pub fn classify(err: &str) -> Option<Class> {
    let e = err.to_ascii_lowercase();
    let lock_file = e.contains("unable to create '") && e.contains(".lock': file exists");
    if e.contains("cannot lock ref") || e.contains("packed-refs.lock") || (lock_file && e.contains("/refs/")) || e.contains("unable to update local ref") {
        return Some(Class::RefLock);
    }
    if lock_file || e.contains("index.lock") || e.contains("another git process seems to be running") {
        return Some(Class::Lock);
    }
    if e.contains("index file corrupt") || e.contains("bad index file") || e.contains("index file smaller than expected") || e.contains("bad signature 0x") || e.contains("index uses ") && e.contains("extension, which we do not understand") {
        return Some(Class::Index);
    }
    if e.contains("is empty") && e.contains("object file")
        || e.contains("loose object") && e.contains("is corrupt")
        || e.contains("could not parse head")
        || e.contains("bad object")
        || e.contains("unable to read tree")
        || e.contains("invalid sha1 pointer")
        || e.contains("not a valid object name head")
        || e.contains("bad ref for")
        || e.contains("packfile") && e.contains("cannot be accessed")
    {
        return Some(Class::Objects);
    }
    if e.contains("you have not concluded your merge")
        || e.contains("merge_head exists")
        || e.contains("pulling is not possible because you have unmerged files")
        || e.contains("committing is not possible because you have unmerged files")
        || e.contains("there is already a rebase-merge directory")
        || e.contains("it seems that there is already a rebase")
        || e.contains("you have not concluded your cherry-pick")
    {
        return Some(Class::Unfinished);
    }
    None
}

/// An engine git command that failed for good on this thread.
#[derive(Debug, Clone, PartialEq)]
pub struct Failure {
    pub class: Class,
    pub command: String,
    pub output: String,
}

/// Forget this thread's last failure (a work item's run starts clean).
pub fn reset_failure() {
    LAST_FAILURE.with(|f| *f.borrow_mut() = None);
}

/// This thread's last engine git failure with a [`Class`] since [`reset_failure`].
pub fn last_failure() -> Option<Failure> {
    LAST_FAILURE.with(|f| f.borrow().clone())
}

fn note_failure(command: &str, output: &str) {
    if let Some(class) = classify(output) {
        LAST_FAILURE.with(|f| *f.borrow_mut() = Some(Failure { class, command: command.to_string(), output: output.chars().take(4000).collect() }));
    }
}

// ---------------------------------------------------------------- running

/// Run one git step under the repository's guard: a throttled stale-lock
/// sweep first, then `step`; a lock failure is retried after a forced
/// sweep, up to [`RETRIES`] times with a doubling pause.  `what` names the
/// command in logs and in the recorded failure.
pub fn retrying<T>(top: &str, what: &str, mut step: impl FnMut() -> Result<T, String>) -> Result<T, String> {
    let _g = guard(top);
    sweep(Path::new(top), false);
    let mut pause = FIRST_PAUSE;
    for attempt in 0..=RETRIES {
        match step() {
            Ok(v) => return Ok(v),
            Err(e) => {
                let class = classify(&e);
                if attempt < RETRIES && class.is_some_and(Class::retryable) {
                    let cleared = sweep(Path::new(top), true);
                    println!(
                        "[engine] git: `{}` failed on a lock ({}); {} stale lock(s) removed, retry {}/{} in {}ms",
                        clip(what, 120), first_line(&e), cleared.len(), attempt + 1, RETRIES, pause.as_millis()
                    );
                    std::thread::sleep(pause);
                    pause *= 2;
                    continue;
                }
                note_failure(what, &e);
                return Err(e);
            }
        }
    }
    unreachable!()
}

/// A git shell script (`git …`, run by bash in `top`) through [`retrying`].
pub fn run(top: &str, script: &str, timeout_sec: u64) -> Result<String, String> {
    retrying(top, script, || crate::work::run_shell(top, script, timeout_sec))
}

fn first_line(s: &str) -> String {
    clip(s.lines().find(|l| !l.trim().is_empty()).unwrap_or(""), 200)
}

fn clip(s: &str, n: usize) -> String {
    if s.chars().count() <= n { s.to_string() } else { format!("{}…", s.chars().take(n).collect::<String>()) }
}

#[cfg(test)]
#[path = "gitsafe_tests.rs"]
mod tests;
