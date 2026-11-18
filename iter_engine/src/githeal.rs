//! Git self-heal for a served checkout, run when the engine starts (built
//! 2026-10-29: a power failure left pdy-dev's HEAD commit a 0-byte loose
//! object, and every work item then failed in seconds on "could not parse
//! HEAD", burning its retries until a human repaired the clone).
//!
//! `heal` runs once per checkout per engine process, on its own thread, before
//! anything else of the engine touches that checkout (file sync, replayed
//! closes, ghost repair, dispatch: `Healer::ready`). It repairs only git's own
//! bookkeeping under `.git`; work-tree files are never touched, so whatever an
//! interrupted run wrote stays on disk as uncommitted changes. In order:
//!
//! 1. lock files (`index.lock`, `HEAD.lock`, `refs/**.lock`, …) written before
//!    this boot, or older than `LOCK_STALE_SECS`, are removed;
//! 2. zero-length loose objects and pack files are removed (git never leaves
//!    one finished; a crash between creating and writing the file does);
//! 3. an empty or garbled HEAD is pointed back at its branch (its reflog, else
//!    the remote's default branch), and a garbled loose ref is removed, a branch
//!    getting back the last commit of its reflog;
//! 4. when anything was removed or HEAD's commit cannot be read, the remote is
//!    fetched: a commit that was already pushed comes back whole;
//! 5. a branch whose commit still cannot be read is moved to the newest
//!    readable commit of its reflog, else to its remote-tracking branch
//!    (`update-ref`: the work tree stays, so the lost commit's changes show as
//!    uncommitted and the next end-of-run commit takes them up);
//! 6. a corrupt index is set aside (`index.corrupt-<unix>`) and rebuilt from
//!    HEAD (`git reset -q`, which leaves the work tree alone);
//! 7. `git fsck --connectivity-only` must pass; a corrupt object it names is
//!    moved to `.git/iter-recovery/` and the remote fetched once more.
//!
//! A checkout still broken after that is held: `.git/iter-git-broken` records
//! why, the engine starts nothing there instead of failing every item, and the
//! check runs again every `RECHECK_SECS`, so a human's repair is taken up
//! without a restart. A checkout whose config names no fsync policy also gets
//! `core.fsync=committed` (git < 2.36: `core.fsyncObjectFiles`): git's default
//! leaves loose objects unsynced, which is how a commit becomes a 0-byte file.

use crate::work::{git_lock, run_shell};
use serde_json::json;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::thread::JoinHandle;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// A lock this old is taken as left behind by a git that is no longer running.
const LOCK_STALE_SECS: u64 = 120;
/// How often a held checkout is checked again.
const RECHECK_SECS: u64 = 300;
const FETCH_SECS: u64 = 300;
const FSCK_SECS: u64 = 900;
const FLAG_FILE: &str = "iter-git-broken";

/// What one check found: the repairs made, and why the checkout is still
/// unusable (None: it is fine).
#[derive(Debug, Default)]
pub struct Report {
    pub fixed: Vec<String>,
    pub broken: Option<String>,
}

enum State {
    Checking(JoinHandle<Report>),
    Ready,
    Held { since: Instant },
}

/// Each served checkout's check, keyed by its expanded topdir.
#[derive(Default)]
pub struct Healer {
    states: HashMap<String, State>,
}

impl Healer {
    /// Whether the engine may use `topdir` this tick: starts its check the
    /// first time it is asked, and takes up a finished one (logged under
    /// `label`, the project name).
    pub fn ready(&mut self, topdir: &str, label: &str) -> bool {
        let state = self.states.remove(topdir);
        let next = match state {
            None => Self::start(topdir),
            Some(State::Held { since, .. }) if since.elapsed() >= Duration::from_secs(RECHECK_SECS) => Self::start(topdir),
            Some(State::Checking(h)) if h.is_finished() => {
                let report = h.join().unwrap_or_else(|_| Report { broken: Some("the git check panicked".into()), ..Default::default() });
                for f in &report.fixed {
                    println!("[engine] {label}: git repair: {f}");
                }
                match report.broken {
                    None => State::Ready,
                    Some(why) => {
                        eprintln!("[engine] {label}: git checkout '{topdir}' is broken and could not be repaired: {why}");
                        println!("[engine] {label}: holding — no work starts in '{topdir}' until git is repaired (checked again every {} min)", RECHECK_SECS / 60);
                        State::Held { since: Instant::now() }
                    }
                }
            }
            Some(s) => s,
        };
        let ok = matches!(next, State::Ready);
        self.states.insert(topdir.to_string(), next);
        ok
    }

    fn start(topdir: &str) -> State {
        let t = topdir.to_string();
        State::Checking(std::thread::spawn(move || heal(&t)))
    }
}

/// Where a checkout keeps its git data: `git` holds HEAD, the index and the
/// per-worktree files; `common` the objects, refs and config (the same folder
/// except in a linked worktree).
struct Dirs {
    git: PathBuf,
    common: PathBuf,
}

fn dirs(top: &Path) -> Option<Dirs> {
    let dot = top.join(".git");
    let git = if dot.is_dir() {
        dot
    } else {
        let text = std::fs::read_to_string(&dot).ok()?;
        let p = PathBuf::from(text.trim().strip_prefix("gitdir:")?.trim());
        if p.is_absolute() { p } else { top.join(p) }
    };
    let common = std::fs::read_to_string(git.join("commondir"))
        .ok()
        .map(|c| {
            let p = PathBuf::from(c.trim());
            if p.is_absolute() { p } else { git.join(p) }
        })
        .unwrap_or_else(|| git.clone());
    Some(Dirs { git, common })
}

fn q(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

fn git(top: &str, args: &str, secs: u64) -> Result<String, String> {
    run_shell(top, &format!("GIT_TERMINAL_PROMPT=0 git {args}"), secs)
}

fn is_oid(s: &str) -> bool {
    (s.len() == 40 || s.len() == 64) && s.bytes().all(|b| b.is_ascii_hexdigit())
}

fn short(oid: &str) -> &str {
    &oid[..9.min(oid.len())]
}

fn unix_now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// When this machine booted (Linux `/proc/stat`, macOS `kern.boottime`).
fn boot_time() -> Option<SystemTime> {
    if let Ok(stat) = std::fs::read_to_string("/proc/stat") {
        let secs = stat.lines().find_map(|l| l.strip_prefix("btime "))?.trim().parse::<u64>().ok()?;
        return Some(UNIX_EPOCH + Duration::from_secs(secs));
    }
    // "{ sec = 1793000000, usec = 0 } Thu Oct 29 …"
    let out = run_shell("/", "sysctl -n kern.boottime", 5).ok()?;
    let secs = out.split("sec = ").nth(1)?.split(|c: char| !c.is_ascii_digit()).next()?.parse::<u64>().ok()?;
    Some(UNIX_EPOCH + Duration::from_secs(secs))
}

/// Every file under `dir` (not following links), skipping the loose-object
/// folders and submodules' repositories.
fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let Ok(ft) = e.file_type() else { continue };
        let name = e.file_name().to_string_lossy().into_owned();
        if ft.is_dir() {
            let loose = name.len() == 2 && name.bytes().all(|b| b.is_ascii_hexdigit()) && dir.ends_with("objects");
            if !loose && name != "modules" {
                walk(&e.path(), out);
            }
        } else if ft.is_file() {
            out.push(e.path());
        }
    }
}

/// Step 1: lock files no running git holds any more.
fn remove_stale_locks(d: &Dirs, fixed: &mut Vec<String>) {
    let boot = boot_time();
    let now = SystemTime::now();
    let mut files = Vec::new();
    walk(&d.git, &mut files);
    if d.common != d.git {
        walk(&d.common, &mut files);
    }
    for f in files.iter().filter(|f| f.extension().is_some_and(|e| e == "lock")) {
        let Some(mtime) = std::fs::metadata(f).and_then(|m| m.modified()).ok() else { continue };
        let before_boot = boot.is_some_and(|b| mtime < b);
        let old = now.duration_since(mtime).is_ok_and(|a| a.as_secs() >= LOCK_STALE_SECS);
        if (before_boot || old) && std::fs::remove_file(f).is_ok() {
            let why = if before_boot { "left from before this boot" } else { "no git has held it for 2 min" };
            fixed.push(format!("removed the stale lock {} ({why})", f.strip_prefix(&d.git).or_else(|_| f.strip_prefix(&d.common)).unwrap_or(f).display()));
        }
    }
}

/// Step 2: objects and packs a crash left empty. True when any went.
fn remove_empty_objects(d: &Dirs, fixed: &mut Vec<String>) -> bool {
    let objects = d.common.join("objects");
    let mut empty: Vec<PathBuf> = Vec::new();
    if let Ok(rd) = std::fs::read_dir(&objects) {
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            if name.len() != 2 || !name.bytes().all(|b| b.is_ascii_hexdigit()) {
                continue;
            }
            for o in std::fs::read_dir(e.path()).into_iter().flatten().flatten() {
                if o.metadata().is_ok_and(|m| m.is_file() && m.len() == 0) {
                    empty.push(o.path());
                }
            }
        }
    }
    // .keep and .promisor files are empty by design
    for p in std::fs::read_dir(objects.join("pack")).into_iter().flatten().flatten() {
        let path = p.path();
        let packish = path.extension().is_some_and(|e| ["pack", "idx", "rev", "bitmap", "mtimes"].iter().any(|x| e == *x));
        if packish && p.metadata().is_ok_and(|m| m.is_file() && m.len() == 0) {
            empty.push(path);
        }
    }
    let mut any = false;
    for path in empty {
        // git writes objects read-only
        if let Ok(m) = std::fs::metadata(&path) {
            let mut perm = m.permissions();
            #[allow(clippy::permissions_set_readonly_false)]
            perm.set_readonly(false);
            let _ = std::fs::set_permissions(&path, perm);
        }
        if std::fs::remove_file(&path).is_ok() {
            any = true;
            let rel = path.strip_prefix(&objects).unwrap_or(&path).to_string_lossy().into_owned();
            let what = if rel.starts_with("pack") { rel.clone() } else { format!("object {}", rel.replace(['/', '\\'], "")) };
            fixed.push(format!("removed the empty {what} (written when the machine went down)"));
        }
    }
    any
}

/// The branch HEAD names (`refs/heads/x`), None when detached or unreadable.
fn head_ref(d: &Dirs) -> Option<String> {
    let head = std::fs::read_to_string(d.git.join("HEAD")).ok()?;
    head.trim().strip_prefix("ref:").map(|r| r.trim().to_string())
}

/// (old, new) oids of a reflog, oldest first.
fn reflog(d: &Dirs, refname: &str) -> Vec<(String, String)> {
    let base = if refname == "HEAD" { &d.git } else { &d.common };
    std::fs::read_to_string(base.join("logs").join(refname))
        .unwrap_or_default()
        .lines()
        .filter_map(|l| {
            let mut w = l.split_whitespace();
            Some((w.next()?.to_string(), w.next()?.to_string()))
        })
        .collect()
}

fn ref_exists(d: &Dirs, refname: &str) -> bool {
    d.common.join(refname).is_file()
        || std::fs::read_to_string(d.common.join("packed-refs")).unwrap_or_default().lines().any(|l| l.split_whitespace().nth(1) == Some(refname))
}

/// The remote fetched from: `origin`, else the first one.
fn remote(top: &str) -> Option<String> {
    let remotes = git(top, "remote", 15).ok()?;
    let names: Vec<&str> = remotes.lines().map(str::trim).filter(|r| !r.is_empty()).collect();
    names.iter().find(|r| **r == "origin").or(names.first()).map(|r| r.to_string())
}

/// Step 3: HEAD and loose refs that hold no ref at all.
fn repair_refs(top: &str, d: &Dirs, fixed: &mut Vec<String>) -> Result<(), String> {
    let head = std::fs::read_to_string(d.git.join("HEAD")).unwrap_or_default();
    let head = head.trim();
    let head_ok = head.strip_prefix("ref:").is_some_and(|r| r.trim().starts_with("refs/")) || is_oid(head);
    if !head_ok {
        // the branch last checked out, else the remote's default, else main / master
        let from_log = std::fs::read_to_string(d.git.join("logs/HEAD"))
            .unwrap_or_default()
            .lines()
            .rev()
            .find_map(|l| l.split_once("\tcheckout: moving from ").and_then(|(_, m)| m.rsplit(" to ").next()).map(|b| format!("refs/heads/{}", b.trim())))
            .filter(|r| ref_exists(d, r) || !reflog(d, r).is_empty());
        let from_remote = remote(top).and_then(|r| std::fs::read_to_string(d.common.join(format!("refs/remotes/{r}/HEAD"))).ok().map(|t| (r, t))).and_then(|(r, t)| {
            t.trim().strip_prefix(&format!("ref: refs/remotes/{r}/")).map(|b| format!("refs/heads/{b}"))
        });
        let fallback = ["refs/heads/main", "refs/heads/master"].into_iter().map(String::from).find(|r| ref_exists(d, r) || !reflog(d, r).is_empty());
        let Some(branch) = from_log.or(from_remote).or(fallback) else {
            return Err("HEAD is unreadable and no branch to point it at was found".into());
        };
        std::fs::write(d.git.join("HEAD"), format!("ref: {branch}\n")).map_err(|e| format!("cannot rewrite HEAD: {e}"))?;
        fixed.push(format!("HEAD was {}; pointed it back at {branch}", if head.is_empty() { "empty".to_string() } else { "garbled".to_string() }));
    }
    let mut files = Vec::new();
    walk(&d.common.join("refs"), &mut files);
    for f in files.iter().filter(|f| f.extension().is_none_or(|e| e != "lock")) {
        let text = std::fs::read_to_string(f).unwrap_or_default();
        let t = text.trim();
        if is_oid(t) || t.starts_with("ref: refs/") {
            continue;
        }
        let Ok(rel) = f.strip_prefix(&d.common) else { continue };
        let refname = rel.to_string_lossy().replace('\\', "/");
        if std::fs::remove_file(f).is_err() {
            continue;
        }
        let back = refname.starts_with("refs/heads/").then(|| reflog(d, &refname).last().map(|(_, new)| new.clone())).flatten().filter(|o| is_oid(o));
        match back {
            Some(oid) => {
                git(top, &format!("update-ref {} {oid}", q(&refname)), 30)?;
                fixed.push(format!("{refname} was garbled; set it back to {} from its reflog", short(&oid)));
            }
            None => fixed.push(format!("removed the garbled ref {refname}")),
        }
    }
    Ok(())
}

enum Head {
    Ok,
    /// a branch with no commit yet (a fresh repository)
    Unborn,
    Broken(String),
}

fn readable(top: &str, oid: &str) -> bool {
    git(top, &format!("cat-file -e {}", q(&format!("{oid}^{{commit}}"))), 30).is_ok() && git(top, &format!("cat-file -e {}", q(&format!("{oid}^{{tree}}"))), 30).is_ok()
}

fn head_state(top: &str, d: &Dirs) -> Head {
    if readable(top, "HEAD") {
        return Head::Ok;
    }
    if let Some(r) = head_ref(d)
        && !ref_exists(d, &r)
        && reflog(d, &r).is_empty()
    {
        return Head::Unborn;
    }
    let why = git(top, "rev-parse --verify HEAD^{commit}", 30).err().unwrap_or_else(|| "HEAD's tree cannot be read".into());
    Head::Broken(why.lines().find(|l| !l.trim().is_empty()).unwrap_or("").trim().to_string())
}

fn fetch(top: &str, fixed: &mut Vec<String>) -> bool {
    let Some(r) = remote(top) else { return false };
    match git(top, &format!("fetch -q {}", q(&r)), FETCH_SECS) {
        Ok(_) => {
            fixed.push(format!("fetched {r} to get back objects the crash lost"));
            true
        }
        Err(e) => {
            fixed.push(format!("could not fetch {r}: {}", e.lines().last().unwrap_or("").trim()));
            false
        }
    }
}

/// Step 5: move HEAD's branch to the newest commit that can still be read.
fn rewind_head(top: &str, d: &Dirs, fixed: &mut Vec<String>) -> Result<(), String> {
    let branch = head_ref(d);
    let log_name = branch.clone().unwrap_or_else(|| "HEAD".into());
    let bad = git(top, "rev-parse -q --verify HEAD", 15).map(|s| s.trim().to_string()).unwrap_or_default();
    let mut candidates: Vec<(String, &str)> = Vec::new();
    for (old, new) in reflog(d, &log_name).into_iter().rev() {
        for oid in [new, old] {
            if is_oid(&oid) && oid.bytes().any(|b| b != b'0') && oid != bad && !candidates.iter().any(|(c, _)| *c == oid) {
                candidates.push((oid, "the newest readable commit of its reflog"));
            }
        }
    }
    if let (Some(b), Some(r)) = (branch.as_deref().and_then(|b| b.strip_prefix("refs/heads/")), remote(top))
        && let Ok(oid) = git(top, &format!("rev-parse -q --verify {}", q(&format!("refs/remotes/{r}/{b}"))), 15)
    {
        candidates.push((oid.trim().to_string(), "its remote-tracking branch"));
    }
    let Some((oid, from)) = candidates.into_iter().find(|(o, _)| readable(top, o)) else {
        return Err("HEAD's commit is lost and neither its reflog nor the remote holds a readable one".into());
    };
    let what = match &branch {
        Some(b) => {
            git(top, &format!("update-ref -m {} {} {oid}", q("iter: git repair after a crash"), q(b)), 30)?;
            b.trim_start_matches("refs/heads/").to_string()
        }
        None => {
            git(top, &format!("update-ref --no-deref -m {} HEAD {oid}", q("iter: git repair after a crash")), 30)?;
            "detached HEAD".to_string()
        }
    };
    let lost = if bad.is_empty() { "an unreadable commit".to_string() } else { format!("the unreadable commit {}", short(&bad)) };
    fixed.push(format!("{what} pointed at {lost}; moved it to {} ({from}); the work tree is unchanged, so that commit's changes show as uncommitted", short(&oid)));
    Ok(())
}

/// Step 6: an index git cannot read is set aside and rebuilt from HEAD.
fn repair_index(top: &str, d: &Dirs, unborn: bool, fixed: &mut Vec<String>) -> Result<(), String> {
    let index = d.git.join("index");
    let empty = std::fs::metadata(&index).is_ok_and(|m| m.len() == 0);
    let corrupt = empty
        || match git(top, "status --porcelain -uno", 120) {
            Ok(_) => false,
            Err(e) => {
                let e = e.to_lowercase();
                e.contains("index file") || e.contains("bad signature") || e.contains("index entry") || e.contains("index uses")
            }
        };
    if !corrupt {
        return Ok(());
    }
    let aside = d.git.join(format!("index.corrupt-{}", unix_now()));
    std::fs::rename(&index, &aside).map_err(|e| format!("cannot move the corrupt index aside: {e}"))?;
    if !unborn {
        git(top, "reset -q", 120).map_err(|e| format!("cannot rebuild the index: {e}"))?;
    }
    fixed.push(format!("the index was corrupt; set it aside as {} and rebuilt it from HEAD (staged changes are unstaged; the work tree is unchanged)", aside.file_name().unwrap_or_default().to_string_lossy()));
    Ok(())
}

/// Step 7: fsck's complaint, None when it passes. A timeout is let pass: a
/// huge repository is not held for being slow.
fn fsck(top: &str) -> Option<String> {
    match git(top, "fsck --connectivity-only --no-dangling --no-reflogs --no-progress", FSCK_SECS) {
        Ok(_) => None,
        Err(e) if e.starts_with("timed out") => None,
        Err(e) => Some(e),
    }
}

/// Corrupt objects fsck named, moved to `.git/iter-recovery/<unix>/`.
fn set_aside_corrupt(d: &Dirs, complaint: &str, fixed: &mut Vec<String>) -> bool {
    let dest = d.common.join("iter-recovery").join(unix_now().to_string());
    let mut any = false;
    for l in complaint.lines() {
        let Some(path) = l.split("(stored in ").nth(1).and_then(|r| r.split(')').next()) else { continue };
        let p = PathBuf::from(path.trim());
        let p = if p.is_absolute() { p } else { d.common.parent().map(|t| t.join(&p)).unwrap_or(p) };
        if !p.is_file() || std::fs::create_dir_all(&dest).is_err() {
            continue;
        }
        let name = p.file_name().unwrap_or_default().to_owned();
        if std::fs::rename(&p, dest.join(&name)).is_ok() {
            any = true;
            fixed.push(format!("moved the corrupt object {} to {}", p.display(), dest.display()));
        }
    }
    any
}

/// Set git to sync loose objects too, unless the checkout already chose.
fn harden(top: &str, fixed: &mut Vec<String>) {
    let set = |k: &str| git(top, &format!("config --get {k}"), 15).is_ok_and(|v| !v.trim().is_empty());
    if set("core.fsync") || set("core.fsyncObjectFiles") {
        return;
    }
    let version = git(top, "--version", 15).unwrap_or_default();
    let mut nums = version.split_whitespace().nth(2).unwrap_or("").split('.').map(|n| n.parse::<u32>().unwrap_or(0));
    let modern = (nums.next().unwrap_or(0), nums.next().unwrap_or(0)) >= (2, 36);
    let (k, v) = if modern { ("core.fsync", "committed") } else { ("core.fsyncObjectFiles", "true") };
    if git(top, &format!("config {k} {v}"), 15).is_ok() {
        fixed.push(format!("set {k}={v}, so a commit is on disk before git reports it done"));
    }
}

/// Check `topdir` and repair what an unclean shutdown left; see the module doc.
pub fn heal(topdir: &str) -> Report {
    let mut r = Report::default();
    let top = Path::new(topdir);
    let Some(d) = dirs(top) else { return r }; // not a git checkout
    let lock = git_lock(topdir);
    let _held = lock.lock().unwrap_or_else(|p| p.into_inner());
    let outcome = (|| -> Result<(), String> {
        remove_stale_locks(&d, &mut r.fixed);
        let mut removed = remove_empty_objects(&d, &mut r.fixed);
        let refs_before = r.fixed.len();
        repair_refs(topdir, &d, &mut r.fixed)?;
        removed |= r.fixed.len() > refs_before;
        let mut fetched = false;
        let mut head = head_state(topdir, &d);
        if removed || matches!(head, Head::Broken(_)) {
            fetched = fetch(topdir, &mut r.fixed);
            head = head_state(topdir, &d);
        }
        if let Head::Broken(_) = head {
            rewind_head(topdir, &d, &mut r.fixed)?;
            head = head_state(topdir, &d);
        }
        if let Head::Broken(why) = head {
            return Err(format!("HEAD still cannot be read: {why}"));
        }
        repair_index(topdir, &d, matches!(head, Head::Unborn), &mut r.fixed)?;
        if let Some(complaint) = fsck(topdir) {
            let moved = set_aside_corrupt(&d, &complaint, &mut r.fixed);
            if moved || !fetched {
                fetch(topdir, &mut r.fixed);
            }
            if let Some(still) = fsck(topdir) {
                return Err(format!("git fsck: {}", still.lines().filter(|l| !l.trim().is_empty()).take(3).collect::<Vec<_>>().join("; ")));
            }
        }
        Ok(())
    })();
    harden(topdir, &mut r.fixed);
    let flag = d.git.join(FLAG_FILE);
    match outcome {
        Ok(()) => {
            if std::fs::remove_file(&flag).is_ok() {
                r.fixed.push("the checkout is usable again".into());
            }
        }
        Err(why) => {
            let _ = std::fs::write(&flag, json!({"why": why, "ts": chrono::Utc::now().to_rfc3339(), "fixed": r.fixed}).to_string());
            r.broken = Some(why);
        }
    }
    r
}

#[cfg(test)]
#[path = "githeal_tests.rs"]
mod tests;
