//! Pause and repair a checkout whose git keeps failing (built 2026-11-18:
//! on bakugo one stale ref lock failed ~155 work items in a row while the
//! engine kept starting more).
//!
//! Every work item's close reports its outcome here ([`record`]): a failure
//! of one of the engine's own git commands, with the [`Class`] its output
//! shows (crate::gitsafe::classify), or anything else.  When [`STREAK`]
//! items in a row fail with the same class, the checkout is **paused**: the
//! engine starts nothing new there ([`hold_reason`], the dispatch hold,
//! logged once and shown on every waiting item).  Each dispatch tick then
//! drives the incident ([`drive_with`]):
//!
//! 1. one **repair** agent session is started in the checkout, with the
//!    prompt `gitrepair_prompt.md` (or the `_git_repair` tooling row's body,
//!    when an admin defined one), told the class, the failing command and
//!    its output, and never to discard uncommitted work;
//! 2. when it ends, the engine **verifies** the checkout itself ([`verify`]:
//!    a forced stale-lock sweep, no unfinished merge or rebase, `git rev-parse
//!    --verify HEAD`, `git status`, and `git fetch` when there is a remote);
//! 3. verified: work is released and every item that failed with that class
//!    during the incident is queued again (attempts reset);
//! 4. not verified: the checkout stays paused and exactly one P0 work item is
//!    filed with the failing command and its output.  The check is re-run
//!    every [`RECHECK`], so a person's repair is taken up without a restart;
//!    no second repair session is ever started for the incident.
//!
//! A new incident within [`NO_REPAIR_AGAIN_WITHIN`] of the last release gets
//! no repair session at all (the last one evidently did not hold): it is
//! verified once and, still broken, goes straight to the P0.  So the engine
//! can never loop starting repair agents.
//!
//! State is per repository (crate::gitsafe::key) and in memory: an engine
//! restart starts clean, and the pre-pull lock sweep (crate::gitsafe) runs
//! before the first item anyway.

use crate::gitsafe::{self, Class, Failure};
use std::collections::HashMap;
use std::path::Path;
use std::sync::{Mutex, OnceLock};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

/// Items in a row failing with one git error class that pause the checkout.
pub const STREAK: usize = 3;
/// How often a paused checkout whose repair failed is checked again.
pub const RECHECK: Duration = Duration::from_secs(300);
/// No repair session for an incident this soon after the last release.
pub const NO_REPAIR_AGAIN_WITHIN: Duration = Duration::from_secs(3600);
/// The repair prompt shipped with the engine.
pub const DEFAULT_PROMPT: &str = include_str!("gitrepair_prompt.md");

/// How one work item's run ended, as far as git is concerned.
#[derive(Debug, Clone)]
pub enum Outcome {
    /// one of the engine's own git commands failed with a known class
    Git(Failure),
    /// anything else: success, a non-git failure, a gate hold
    Other,
}

/// What the engine does for an incident (the engine's implementation talks
/// to iter_data and the provider; tests record calls).
pub trait Hooks {
    /// Can a repair session start now (an account is usable)?
    fn ready(&self) -> bool {
        true
    }
    /// Start the repair session on a thread of its own: Ok(its final text).
    fn repair(&self, prompt: String) -> JoinHandle<Result<String, String>>;
    /// Queue these items again (each only if it is still failed or queued);
    /// returns how many were requeued.
    fn requeue(&self, ids: &[String], note: &str) -> usize;
    /// File the P0 work item; Ok(its id).
    fn file_p0(&self, name: &str, request: &str) -> Result<String, String>;
}

enum Phase {
    /// waiting for the first tick (or for a usable account) to start the repair
    Pending,
    Repairing(JoinHandle<Result<String, String>>),
    /// the repair (or the check) failed: paused, P0 filed, checked every RECHECK
    Stuck { checked: Instant, p0: Option<String> },
}

struct Incident {
    class: Class,
    since: String,
    items: Vec<String>,
    failure: Failure,
    phase: Phase,
    /// no repair session for this incident (one was released too recently)
    no_repair: bool,
}

#[derive(Default)]
struct Repo {
    streak: Vec<String>,
    streak_class: Option<Class>,
    incident: Option<Incident>,
    last_release: Option<Instant>,
    /// repair sessions started for this repository (this engine process)
    repairs_started: u32,
}

fn repos() -> std::sync::MutexGuard<'static, HashMap<String, Repo>> {
    static R: OnceLock<Mutex<HashMap<String, Repo>>> = OnceLock::new();
    R.get_or_init(Default::default).lock().unwrap_or_else(|p| p.into_inner())
}

/// One closed run's outcome for the checkout at `top`.
pub fn record(top: &str, workid: &str, outcome: Outcome) {
    let key = gitsafe::key(Path::new(top));
    let mut all = repos();
    let r = all.entry(key).or_default();
    if let Some(inc) = r.incident.as_mut() {
        // paused: items that were already running when it began still close
        if let Outcome::Git(f) = &outcome {
            if f.class == inc.class && !inc.items.iter().any(|i| i == workid) {
                inc.items.push(workid.to_string());
            }
        }
        return;
    }
    let Outcome::Git(f) = outcome else {
        r.streak.clear();
        r.streak_class = None;
        return;
    };
    if r.streak_class != Some(f.class) {
        r.streak.clear();
        r.streak_class = Some(f.class);
    }
    if !r.streak.iter().any(|i| i == workid) {
        r.streak.push(workid.to_string());
    }
    if r.streak.len() < STREAK {
        return;
    }
    let no_repair = r.last_release.is_some_and(|t| t.elapsed() < NO_REPAIR_AGAIN_WITHIN);
    println!(
        "[engine] {top}: {} work items in a row failed because {} — PAUSED: no new items start here{}",
        r.streak.len(),
        f.class.label(),
        if no_repair { "; the last repair was released under an hour ago, so no repair session this time" } else { "; a git repair session starts next" }
    );
    r.incident = Some(Incident {
        class: f.class,
        since: iter_core::now_utc(),
        items: std::mem::take(&mut r.streak),
        failure: f,
        phase: Phase::Pending,
        no_repair,
    });
    r.streak_class = None;
}

/// Why the checkout is paused (the dispatch hold); None when it is not.
pub fn hold_reason(top: &str) -> Option<String> {
    let key = gitsafe::key(Path::new(top));
    let all = repos();
    let inc = all.get(&key)?.incident.as_ref()?;
    let what = match &inc.phase {
        Phase::Pending => "a git repair session starts next".to_string(),
        Phase::Repairing(_) => "a git repair session is running".to_string(),
        Phase::Stuck { p0, .. } => format!(
            "the repair did not fix it; {}; checked again every {} min",
            match p0.as_deref() {
                Some("") => "a P0 item was filed".to_string(),
                Some(id) => format!("P0 item {} was filed", &id[id.len().saturating_sub(12)..]),
                None => "the P0 item could not be filed".to_string(),
            },
            RECHECK.as_secs() / 60
        ),
    };
    Some(format!("git paused since {}: {} items failed because {} — {what}", inc.since, inc.items.len(), inc.class.label()))
}

/// Is the checkout paused?
pub fn paused(top: &str) -> bool {
    hold_reason(top).is_some()
}

/// Repair sessions started for `top`'s repository by this process (tests).
#[cfg(test)]
pub fn repairs_started(top: &str) -> u32 {
    repos().get(&gitsafe::key(Path::new(top))).map(|r| r.repairs_started).unwrap_or(0)
}

/// The prompt for one repair session, from `template` (the default or the
/// `_git_repair` tooling row's body).
pub fn prompt(template: &str, top: &str, class: Class, items: &[String], failure: &Failure) -> String {
    let gitdir = gitsafe::dirs(Path::new(top)).map(|d| d.common.to_string_lossy().into_owned()).unwrap_or_else(|| format!("{top}/.git"));
    template
        .replace("{topdir}", top)
        .replace("{gitdir}", &gitdir)
        .replace("{class}", class.label())
        .replace("{count}", &items.len().to_string())
        .replace("{command}", &failure.command)
        .replace("{output}", failure.output.trim())
        .replace("{items}", &items.join(", "))
        .replace("{stamp}", &chrono::Utc::now().format("%Y%m%dT%H%M%SZ").to_string())
}

/// The engine's own check that the checkout's git works: Err((command, output))
/// names the first check that failed.
pub fn verify(top: &str) -> Result<(), (String, String)> {
    gitsafe::sweep(Path::new(top), true);
    let Some(d) = gitsafe::dirs(Path::new(top)) else {
        return Err(("test -e .git".into(), format!("{top} is not a git checkout")));
    };
    for f in ["MERGE_HEAD", "CHERRY_PICK_HEAD", "REVERT_HEAD", "rebase-merge", "rebase-apply"] {
        if d.git_dir.join(f).exists() {
            return Err((format!("test ! -e {}", d.git_dir.join(f).display()), format!("an unfinished operation is still recorded: {f} exists")));
        }
    }
    let run = |cmd: &str, secs: u64| gitsafe::run(top, cmd, secs).map_err(|e| (cmd.to_string(), e));
    run("git rev-parse --verify -q HEAD", 30)?;
    run("git status --porcelain", 120)?;
    let remotes = run("git remote", 15)?;
    if !remotes.trim().is_empty() {
        run("git fetch -q", 300)?;
    }
    Ok(())
}

/// Drive the incident of `top`'s repository one step (module doc); called
/// every dispatch tick while the checkout is paused.  `verify_fn` is
/// [`verify`] outside tests; `template` is the repair prompt's template.
pub fn drive_with(top: &str, hooks: &dyn Hooks, verify_fn: &dyn Fn(&str) -> Result<(), (String, String)>, template: &str) {
    let key = gitsafe::key(Path::new(top));
    // what to do is decided under the state lock; slow work runs outside it
    enum Step {
        Start(String),
        Check { repaired: Option<Result<String, String>> },
        Recheck,
    }
    let step = {
        let mut all = repos();
        let Some(r) = all.get_mut(&key) else { return };
        let Some(inc) = r.incident.as_mut() else { return };
        match &mut inc.phase {
            Phase::Pending if inc.no_repair => Step::Check { repaired: None },
            Phase::Pending => {
                if !hooks.ready() {
                    return;
                }
                Step::Start(prompt(template, top, inc.class, &inc.items, &inc.failure))
            }
            Phase::Repairing(h) if h.is_finished() => {
                let Phase::Repairing(h) = std::mem::replace(&mut inc.phase, Phase::Pending) else { unreachable!() };
                Step::Check { repaired: Some(h.join().unwrap_or_else(|_| Err("the repair session's thread panicked".into()))) }
            }
            Phase::Repairing(_) => return,
            Phase::Stuck { checked, .. } if checked.elapsed() >= RECHECK => Step::Recheck,
            Phase::Stuck { .. } => return,
        }
    };
    match step {
        Step::Start(p) => {
            println!("[engine] {top}: starting the git repair session");
            let h = hooks.repair(p);
            let mut all = repos();
            if let Some(r) = all.get_mut(&key) {
                r.repairs_started += 1;
                if let Some(inc) = r.incident.as_mut() {
                    inc.phase = Phase::Repairing(h);
                }
            }
        }
        Step::Check { repaired } => {
            match &repaired {
                Some(Ok(text)) => println!("[engine] {top}: git repair session ended: {}", last_line(text)),
                Some(Err(e)) => println!("[engine] {top}: git repair session failed: {}", last_line(e)),
                None => {}
            }
            match verify_fn(top) {
                Ok(()) => release(top, &key, hooks, "the engine's own checks (git status, rev-parse HEAD, fetch) pass after the repair session"),
                Err((cmd, out)) => stuck(top, &key, hooks, &cmd, &out, repaired.as_ref()),
            }
        }
        Step::Recheck => match verify_fn(top) {
            Ok(()) => release(top, &key, hooks, "the engine's periodic check passes again (repaired from outside)"),
            Err(_) => {
                let mut all = repos();
                if let Some(Incident { phase: Phase::Stuck { checked, .. }, .. }) = all.get_mut(&key).and_then(|r| r.incident.as_mut()) {
                    *checked = Instant::now();
                }
            }
        },
    }
}

fn release(top: &str, key: &str, hooks: &dyn Hooks, why: &str) {
    let inc = {
        let mut all = repos();
        let Some(r) = all.get_mut(key) else { return };
        r.last_release = Some(Instant::now());
        r.streak.clear();
        r.streak_class = None;
        r.incident.take()
    };
    let Some(inc) = inc else { return };
    let note = format!("re-queued after the git repair of {top}: these items failed because {} between {} and {}; {why}", inc.class.label(), inc.since, iter_core::now_utc());
    let n = hooks.requeue(&inc.items, &note);
    println!("[engine] {top}: git verified — RELEASED; {n} of {} item(s) that failed during the pause queued again", inc.items.len());
}

fn stuck(top: &str, key: &str, hooks: &dyn Hooks, cmd: &str, out: &str, repaired: Option<&Result<String, String>>) {
    let (class, since, items, filed) = {
        let all = repos();
        let Some(inc) = all.get(key).and_then(|r| r.incident.as_ref()) else { return };
        let filed = matches!(&inc.phase, Phase::Stuck { p0: Some(_), .. });
        (inc.class, inc.since.clone(), inc.items.clone(), filed)
    };
    let mut p0 = None;
    if !filed {
        let name = format!("git broken in {top}: {} — engine paused", class.label());
        let session = match repaired {
            Some(Ok(t)) => format!("The repair session ended with:\n\n```\n{}\n```\n", tail(t, 3000)),
            Some(Err(e)) => format!("The repair session failed:\n\n```\n{}\n```\n", tail(e, 3000)),
            None => "No repair session ran: one was released less than an hour ago and the trouble came back.\n".to_string(),
        };
        let request = format!(
            "The engine paused work in `{top}` at {since}: {} work items in a row failed because {}. {session}\n\
             The engine's own check still fails. Failing command:\n\n```\n{cmd}\n```\n\nIts output:\n\n```\n{}\n```\n\n\
             Items that failed during the pause (they are queued again once the check passes): {}.\n\n\
             Fix the checkout's git state without discarding uncommitted work. The engine re-runs its check every {} minutes \
             and releases work by itself once it passes; no restart is needed.",
            items.len(), class.label(), tail(out, 4000), items.join(", "), RECHECK.as_secs() / 60
        );
        match hooks.file_p0(&name, &request) {
            Ok(id) => {
                println!("[engine] {top}: git still broken after the repair (`{cmd}`: {}) — stays PAUSED; filed P0 {}", last_line(out), &id[id.len().saturating_sub(12)..]);
                p0 = Some(id);
            }
            Err(e) => println!("[engine] {top}: git still broken after the repair (`{cmd}`: {}) — stays PAUSED; could not file the P0: {e}", last_line(out)),
        }
    }
    let mut all = repos();
    if let Some(inc) = all.get_mut(key).and_then(|r| r.incident.as_mut()) {
        // stuck() runs once per incident (the check after the repair), so
        // at most one P0 is filed; a filing that failed is not tried again
        inc.phase = Phase::Stuck { checked: Instant::now(), p0 };
    }
}

fn last_line(s: &str) -> String {
    let l = s.lines().rev().find(|l| !l.trim().is_empty()).unwrap_or("").trim();
    l.chars().take(300).collect()
}

fn tail(s: &str, n: usize) -> String {
    let c: Vec<char> = s.chars().collect();
    c[c.len().saturating_sub(n)..].iter().collect()
}

#[cfg(test)]
#[path = "gitrepair_tests.rs"]
mod tests;
