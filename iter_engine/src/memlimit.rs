//! Memory limits for agent sessions and test scripts (Linux cgroup v2,
//! 2026-10-27).
//!
//! Why: on 2026-10-27 a cargo-mutants mutant turned a loop into an endless
//! `Vec` push; its test binary, started from an agent's Bash tool, reached
//! 13 GB in seconds, the kernel's OOM killer took it, and systemd (the
//! unit's default `OOMPolicy=stop`) stopped the whole iter-engine service,
//! killing every other agent session with it — hourly, as the item re-ran.
//!
//! What: with the engine's unit cgroup delegated to it (`Delegate=yes` in
//! iter-engine.service), the engine builds this tree under that cgroup:
//!
//! ```text
//! <unit>/engine                     the engine process itself (moved here at startup)
//! <unit>/agent-<n>-<label>/         one per agent session: memory.max, memory.swap.max,
//!                                   memory.oom.group=1, subtree_control +memory
//! <unit>/agent-<n>-<label>/session  the `claude` process and everything it starts
//! <unit>/agent-<n>-<label>/test-…   one per test script `iter runtests` starts inside
//!                                   the session: its own memory.max / memory.swap.max
//! <unit>/exec-<n>-<label>/…         the same for an exec (shell) item
//! ```
//!
//! cgroup v2 lets only a cgroup without processes of its own give its
//! children limits ("no internal processes"), which is why the engine moves
//! out of the unit's own cgroup and why a session's processes sit in a
//! `session` leaf beside its test cgroups instead of in the agent cgroup.
//! A session's limit covers its test scripts too (they are inside it).
//!
//! A process is placed before it runs anything: `pre_exec` writes "0" (the
//! calling process) to the target's `cgroup.procs` through a descriptor
//! opened before the fork, so no grandchild can start outside the limit.
//!
//! The agent cgroup has `memory.oom.group=1`: when the session reaches its
//! limit the kernel kills the whole session (claude and all it started), so
//! the engine sees a dead session and says why, instead of a session limping
//! on with one tool process missing.  A test cgroup keeps `oom.group=0`:
//! there the kernel kills only the largest process (the runaway test
//! binary), so the script — cargo-mutants, a cargo test run — sees one
//! failed child and carries on.
//!
//! Anything missing (not Linux, cgroup v1, no delegation, no memory
//! controller, other processes sharing the engine's cgroup) turns the whole
//! feature off with one plain line at startup; spawning never depends on it.

// the parsing and reporting helpers are used only by the Linux half
#![cfg_attr(not(target_os = "linux"), allow(dead_code))]

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};

/// The agent (or exec) cgroup a session runs in; `iter runtests` makes each
/// script's cgroup under it.
pub const ENV_CGROUP: &str = "ITER_MEM_CGROUP";
/// Bytes one test script may use ("off": no limit of its own).
pub const ENV_TEST_MAX: &str = "ITER_MEM_TEST_MAX";
/// Swap one test script may use: bytes, "max", or "off" (not set).
pub const ENV_TEST_SWAP_MAX: &str = "ITER_MEM_TEST_SWAP_MAX";

/// Defaults (see docs/iter_guide.md, "How much memory may an agent session use?").
pub const DEFAULT_SESSION_MAX: &str = "40%";
pub const DEFAULT_SESSION_SWAP_MAX: &str = "1G";
pub const DEFAULT_TEST_MAX: &str = "30%";
pub const DEFAULT_TEST_SWAP_MAX: &str = "512M";

/// One configured limit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Size {
    /// not limited here (memory) / left as inherited (swap)
    Off,
    /// explicitly unlimited ("max")
    Max,
    Bytes(u64),
}

impl Size {
    fn bytes(self) -> Option<u64> {
        match self {
            Size::Bytes(b) => Some(b),
            _ => None,
        }
    }

    /// The value written to a `memory.*max` file, None = write nothing.
    fn file_value(self) -> Option<String> {
        match self {
            Size::Off => None,
            Size::Max => Some("max".into()),
            Size::Bytes(b) => Some(b.to_string()),
        }
    }

    /// For the environment a session hands to `iter runtests`.
    fn env_value(self) -> String {
        self.file_value().unwrap_or_else(|| "off".into())
    }

    fn words(self) -> String {
        match self {
            Size::Off => "no limit".into(),
            Size::Max => "unlimited".into(),
            Size::Bytes(0) => "none".into(),
            Size::Bytes(b) => human(b),
        }
    }
}

/// "6G", "512M", "1.5GiB", "40%" (of `ram`), "0"/"off"/"none" = off,
/// "max"/"infinity" = unlimited.  `zero_is_off`: memory limits treat 0 as
/// off; a swap limit of 0 means no swap at all.
pub fn parse_size(text: &str, ram: Option<u64>, zero_is_off: bool) -> Result<Size, String> {
    let t = text.trim().to_ascii_lowercase();
    match t.as_str() {
        "off" | "none" | "no" | "false" => return Ok(Size::Off),
        "max" | "infinity" | "unlimited" => return Ok(Size::Max),
        "" => return Err("empty size".into()),
        _ => {}
    }
    if let Some(p) = t.strip_suffix('%') {
        let pct: f64 = p.trim().parse().map_err(|_| format!("{text:?} is not a percentage"))?;
        if !(0.0..=100.0).contains(&pct) {
            return Err(format!("{text:?}: a percentage of the machine's memory is 0-100"));
        }
        let ram = ram.ok_or_else(|| format!("{text:?}: this machine's memory size is unknown, give a size like 6G"))?;
        let b = (ram as f64 * pct / 100.0) as u64;
        return Ok(if b == 0 && zero_is_off { Size::Off } else { Size::Bytes(b) });
    }
    let num_end = t.find(|c: char| !(c.is_ascii_digit() || c == '.')).unwrap_or(t.len());
    let (num, unit) = t.split_at(num_end);
    let n: f64 = num.parse().map_err(|_| format!("{text:?} is not a size (like 6G, 512M, 40% or off)"))?;
    let mult: u64 = match unit.trim().trim_end_matches('b').trim_end_matches('i') {
        "" => 1,
        "k" => 1 << 10,
        "m" => 1 << 20,
        "g" => 1 << 30,
        "t" => 1 << 40,
        _ => return Err(format!("{text:?}: unknown unit {unit:?} (K, M, G or T)")),
    };
    let b = (n * mult as f64) as u64;
    Ok(if b == 0 && zero_is_off { Size::Off } else { Size::Bytes(b) })
}

/// 6_442_450_944 -> "6 GiB", 6_657_000_000 -> "6.2 GiB", 536_870_912 -> "512 MiB".
pub fn human(b: u64) -> String {
    let units = [(1u64 << 40, "TiB"), (1 << 30, "GiB"), (1 << 20, "MiB"), (1 << 10, "KiB")];
    for (size, name) in units {
        if b >= size {
            let v = b as f64 / size as f64;
            let r = (v * 10.0).round() / 10.0;
            return if r.fract() == 0.0 { format!("{} {name}", r as u64) } else { format!("{r:.1} {name}") };
        }
    }
    format!("{b} bytes")
}

/// The four settings.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Limits {
    pub session: Size,
    pub session_swap: Size,
    pub test: Size,
    pub test_swap: Size,
}

impl Limits {
    fn any_on(&self) -> bool {
        self.session.bytes().is_some() || self.test.bytes().is_some()
    }

    /// From (flag, env value, default) per setting, in the order session,
    /// session swap, test, test swap.  A value that does not parse falls back
    /// to the default with a warning (a typo must not stop the engine).
    pub fn resolve(given: [(Option<&str>, Option<&str>); 4], ram: Option<u64>) -> (Limits, Vec<String>) {
        let names = ["--session-memory-max", "--session-swap-max", "--test-memory-max", "--test-swap-max"];
        let defaults = [DEFAULT_SESSION_MAX, DEFAULT_SESSION_SWAP_MAX, DEFAULT_TEST_MAX, DEFAULT_TEST_SWAP_MAX];
        let mut warnings = Vec::new();
        let mut out = [Size::Off; 4];
        for i in 0..4 {
            let zero_is_off = i % 2 == 0;
            let (flag, env) = given[i];
            let text = flag.or(env).map(str::trim).filter(|v| !v.is_empty()).unwrap_or(defaults[i]);
            out[i] = match parse_size(text, ram, zero_is_off) {
                Ok(s) => s,
                Err(e) => {
                    warnings.push(format!("{}: {e}; using the default {}", names[i], defaults[i]));
                    parse_size(defaults[i], ram, zero_is_off).unwrap_or(Size::Off)
                }
            };
        }
        (Limits { session: out[0], session_swap: out[1], test: out[2], test_swap: out[3] }, warnings)
    }
}

/// `MemTotal` from /proc/meminfo, in bytes.
pub fn ram_bytes() -> Option<u64> {
    let text = std::fs::read_to_string("/proc/meminfo").ok()?;
    parse_meminfo_total(&text)
}

pub fn parse_meminfo_total(text: &str) -> Option<u64> {
    let line = text.lines().find(|l| l.starts_with("MemTotal:"))?;
    let kb: u64 = line.split_whitespace().nth(1)?.parse().ok()?;
    Some(kb * 1024)
}

/// The cgroup v2 path in /proc/self/cgroup (the `0::<path>` line).
pub fn parse_proc_cgroup(text: &str) -> Option<String> {
    text.lines().find_map(|l| l.strip_prefix("0::")).map(|p| p.trim().to_string()).filter(|p| p.starts_with('/'))
}

/// Where the cgroup2 filesystem is mounted, from /proc/self/mountinfo.
pub fn parse_mountinfo_cgroup2(text: &str) -> Option<PathBuf> {
    text.lines().find_map(|l| {
        let (pre, post) = l.split_once(" - ")?;
        if post.split_whitespace().next()? != "cgroup2" {
            return None;
        }
        pre.split_whitespace().nth(4).map(PathBuf::from)
    })
}

/// Is `name` among the controllers listed (cgroup.controllers / subtree_control)?
pub fn has_controller(text: &str, name: &str) -> bool {
    text.split_whitespace().any(|c| c == name)
}

/// The counters of a `memory.events` (or `memory.events.local`) file.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Events {
    pub low: u64,
    pub high: u64,
    pub max: u64,
    pub oom: u64,
    pub oom_kill: u64,
    pub oom_group_kill: u64,
}

pub fn parse_events(text: &str) -> Events {
    let mut e = Events::default();
    for line in text.lines() {
        let mut it = line.split_whitespace();
        let (Some(k), Some(v)) = (it.next(), it.next().and_then(|v| v.parse::<u64>().ok())) else { continue };
        match k {
            "low" => e.low = v,
            "high" => e.high = v,
            "max" => e.max = v,
            "oom" => e.oom = v,
            "oom_kill" => e.oom_kill = v,
            "oom_group_kill" => e.oom_group_kill = v,
            _ => {}
        }
    }
    e
}

fn counters(e: &Events) -> String {
    format!("memory.events oom_kill={}, oom_group_kill={}, max={}", e.oom_kill, e.oom_group_kill, e.max)
}

/// What to tell a person after a session, from its cgroups' counters:
/// `all` = the agent cgroup's memory.events (it and everything under it),
/// `own` = its memory.events.local (`oom` there = its own limit was hit),
/// `leaf` = the `session` leaf's (the claude process tree).  None when the
/// memory limit killed nothing.
pub fn session_report(limit: Size, all: &Events, own: &Events, leaf: &Events) -> Option<String> {
    if leaf.oom_kill > 0 {
        if own.oom > 0 {
            return Some(format!(
                "agent session killed: its processes reached the {} memory limit for one agent session ({})",
                limit.words(),
                counters(all)
            ));
        }
        return Some(format!(
            "agent session killed by the kernel's out-of-memory killer while the engine's service (or the machine) was out of memory, not at this session's own {} limit ({})",
            limit.words(),
            counters(all)
        ));
    }
    if all.oom_kill > 0 {
        return Some(format!(
            "{} process(es) of this session's test scripts were killed at their memory limit; the session went on ({})",
            all.oom_kill,
            counters(all)
        ));
    }
    None
}

/// The same for one test script's cgroup (memory.events of a leaf).
pub fn script_report(limit: Size, ev: &Events) -> Option<String> {
    if ev.oom_kill == 0 {
        return None;
    }
    if ev.oom > 0 {
        return Some(format!(
            "killed by the memory limit: {} of the script's processes reached the {} limit for one test script ({})",
            ev.oom_kill,
            limit.words(),
            counters(ev)
        ));
    }
    Some(format!(
        "{} of the script's processes were killed by the kernel's out-of-memory killer while its agent session (or the engine's service) was at its limit ({})",
        ev.oom_kill,
        counters(ev)
    ))
}

/// Dir name piece: [A-Za-z0-9-], at most 24 chars.
fn safe_label(label: &str) -> String {
    let s: String = label.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' { c } else { '-' }).collect();
    let s = s.trim_matches('-');
    let tail = s.len().saturating_sub(24);
    let s = &s[tail..];
    if s.is_empty() { "x".into() } else { s.to_string() }
}

// ---------------------------------------------------------------- the tree

struct Tree {
    root: PathBuf,
    limits: Limits,
    seq: AtomicU64,
    /// session cgroups in use (the stale sweep leaves them alone)
    live: Mutex<HashSet<PathBuf>>,
}

static TREE: OnceLock<Tree> = OnceLock::new();

/// Turn the limits on for this engine process, or say why not.  Prints one
/// line either way; never fails.  Call once, early in the engine's startup.
pub fn init(limits: Limits) -> bool {
    match setup(&limits) {
        Ok(root) => {
            let level = |mem: Size, swap: Size| match mem.bytes() {
                Some(_) => format!("at most {} (swap {})", mem.words(), swap.words()),
                None => "no limit of its own".to_string(),
            };
            println!(
                "[engine] memory limits on: each agent session {}, each test script {}, in cgroup {}",
                level(limits.session, limits.session_swap),
                level(limits.test, limits.test_swap),
                root.display()
            );
            let _ = TREE.set(Tree { root, limits, seq: AtomicU64::new(0), live: Mutex::new(HashSet::new()) });
            true
        }
        Err(why) => {
            println!("[engine] memory limits off: {why}; agent sessions and test scripts run without them");
            false
        }
    }
}

fn setup(limits: &Limits) -> Result<PathBuf, String> {
    if !limits.any_on() {
        return Err("turned off in the configuration (--session-memory-max and --test-memory-max are both off)".into());
    }
    #[cfg(target_os = "linux")]
    {
        let rel = std::fs::read_to_string("/proc/self/cgroup")
            .ok()
            .and_then(|t| parse_proc_cgroup(&t))
            .ok_or("this machine uses cgroup v1 only (/proc/self/cgroup has no cgroup v2 line)")?;
        let mnt = std::fs::read_to_string("/proc/self/mountinfo")
            .ok()
            .and_then(|t| parse_mountinfo_cgroup2(&t))
            .ok_or("no cgroup2 filesystem is mounted")?;
        let root = mnt.join(rel.trim_start_matches('/'));
        linux::setup_at(&root)?;
        Ok(root)
    }
    #[cfg(not(target_os = "linux"))]
    {
        Err(format!("cgroups exist only on Linux (this is {})", std::env::consts::OS))
    }
}

/// A cgroup one command runs in: an agent / exec session, or one test script.
pub struct Scope {
    /// the cgroup carrying the limit (agent-…, exec-… or test-…)
    dir: PathBuf,
    /// the cgroup the process goes into (dir/session, or dir itself for a test)
    #[allow(dead_code)]
    leaf: PathBuf,
    #[allow(dead_code)]
    procs: std::fs::File,
    limit: Size,
    /// the session's limits for the test scripts inside it (None: a test scope)
    test_env: Option<(Size, Size)>,
}

/// A cgroup for one agent session (`kind` "agent") or exec item ("exec");
/// None when limits are off or the cgroup could not be made (logged).
pub fn session(kind: &str, label: &str) -> Option<Scope> {
    let tree = TREE.get()?;
    #[cfg(target_os = "linux")]
    {
        // sweep and reserve under one lock: a session being made on another
        // thread is empty until its child starts, and a sweep that did not
        // see it in `live` removed it mid-make (its limit then not applied)
        let dir = {
            let mut live = tree.live.lock().unwrap_or_else(|e| e.into_inner());
            linux::sweep_stale(&tree.root, &live);
            let n = tree.seq.fetch_add(1, Ordering::Relaxed) + 1;
            let dir = tree.root.join(format!("{kind}-{n}-{}", safe_label(label)));
            live.insert(dir.clone());
            dir
        };
        match linux::make_session(&dir, &tree.limits) {
            Ok(scope) => Some(scope),
            Err(e) => {
                println!("[engine] {label}: memory limit not applied to this session: {e}");
                linux::remove(&dir);
                if let Ok(mut l) = tree.live.lock() {
                    l.remove(&dir);
                }
                None
            }
        }
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (tree, kind, label, Ordering::Relaxed);
        None
    }
}

/// A cgroup for one test script, under the session named by
/// `$ITER_MEM_CGROUP` (set by the engine on every session it limits).  None
/// outside such a session; a session whose cgroup cannot take the script's
/// prints one line (once per process) and runs scripts without a limit.
pub fn test_scope(label: &str) -> Option<Scope> {
    let parent = std::env::var_os(ENV_CGROUP).filter(|p| !p.is_empty()).map(PathBuf::from)?;
    let limit = std::env::var(ENV_TEST_MAX).ok().and_then(|v| parse_size(&v, None, true).ok()).unwrap_or(Size::Off);
    let swap = std::env::var(ENV_TEST_SWAP_MAX).ok().and_then(|v| parse_size(&v, None, false).ok()).unwrap_or(Size::Off);
    limit.bytes()?;
    test_scope_in(&parent, limit, swap, label)
}

pub fn test_scope_in(parent: &Path, limit: Size, swap: Size, label: &str) -> Option<Scope> {
    #[cfg(target_os = "linux")]
    {
        static SEQ: AtomicU64 = AtomicU64::new(0);
        let n = SEQ.fetch_add(1, Ordering::Relaxed) + 1;
        let dir = parent.join(format!("test-{}-{n}-{}", std::process::id(), safe_label(label)));
        match linux::make_test(&dir, limit, swap) {
            Ok(s) => Some(s),
            Err(e) => {
                linux::remove(&dir);
                static ONCE: std::sync::Once = std::sync::Once::new();
                ONCE.call_once(|| eprintln!("iter runtests: test scripts run without a memory limit: {e}"));
                None
            }
        }
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (parent, limit, swap, label, Ordering::Relaxed);
        None
    }
}

impl Scope {
    /// Make `cmd` start inside this cgroup (before it runs anything), and
    /// for a session hand it the cgroup its test scripts go under.
    pub fn apply(&self, cmd: &mut Command) {
        if let Some((test, swap)) = self.test_env {
            cmd.env(ENV_CGROUP, &self.dir).env(ENV_TEST_MAX, test.env_value()).env(ENV_TEST_SWAP_MAX, swap.env_value());
        }
        #[cfg(target_os = "linux")]
        linux::place_on_exec(cmd, &self.procs);
    }

    /// Is `pid` (just spawned) in this scope?  True when it can no longer be
    /// told (it already exited).
    pub fn holds(&self, pid: u32) -> bool {
        let Ok(text) = std::fs::read_to_string(format!("/proc/{pid}/cgroup")) else { return true };
        let Some(rel) = parse_proc_cgroup(&text) else { return true };
        self.leaf.ends_with(rel.trim_start_matches('/'))
    }

    /// Kill every process in the scope now, however it was started
    /// (`cgroup.kill`, Linux 5.14+); best effort.
    pub fn kill(&self) {
        let _ = std::fs::write(self.dir.join("cgroup.kill"), "1");
    }

    /// After the command ended: what the memory limit killed, in plain words
    /// (None: nothing).  The cgroup is removed when this scope drops.
    pub fn report(&self) -> Option<String> {
        let read = |p: PathBuf| parse_events(&std::fs::read_to_string(p).unwrap_or_default());
        if self.test_env.is_some() {
            let all = read(self.dir.join("memory.events"));
            let own = read(self.dir.join("memory.events.local"));
            let leaf = read(self.leaf.join("memory.events"));
            session_report(self.limit, &all, &own, &leaf)
        } else {
            script_report(self.limit, &read(self.dir.join("memory.events")))
        }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }
}

impl Drop for Scope {
    fn drop(&mut self) {
        #[cfg(target_os = "linux")]
        linux::remove(&self.dir);
        if let Some(mut l) = TREE.get().and_then(|tree| tree.live.lock().ok()) {
            l.remove(&self.dir);
        }
    }
}

#[cfg(target_os = "linux")]
mod linux {
    use super::*;
    use std::os::unix::io::AsRawFd;

    unsafe extern "C" {
        fn write(fd: std::ffi::c_int, buf: *const std::ffi::c_void, count: usize) -> isize;
    }

    fn rd(p: &Path) -> Result<String, String> {
        std::fs::read_to_string(p).map_err(|e| format!("cannot read {}: {e}", p.display()))
    }

    fn wr(p: &Path, v: &str) -> Result<(), String> {
        std::fs::write(p, v).map_err(|e| format!("cannot write {v:?} to {}: {e}", p.display()))
    }

    fn mkdir(p: &Path) -> Result<(), String> {
        match std::fs::create_dir(p) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => Ok(()),
            Err(e) => Err(format!("cannot create cgroup {}: {e}", p.display())),
        }
    }

    /// Move this process into `<root>/engine` and enable the memory
    /// controller for `root`'s children.
    pub fn setup_at(root: &Path) -> Result<(), String> {
        let ctl = rd(&root.join("cgroup.controllers"))?;
        if !has_controller(&ctl, "memory") {
            return Err(format!(
                "the memory controller is not available in the engine's cgroup {} (cgroup.controllers: {:?}); its systemd service needs Delegate=yes",
                root.display(),
                ctl.trim()
            ));
        }
        let me = std::process::id();
        let others: Vec<String> = rd(&root.join("cgroup.procs"))?.lines().map(str::trim).filter(|p| !p.is_empty() && *p != me.to_string()).map(String::from).collect();
        if !others.is_empty() {
            return Err(format!(
                "the engine's cgroup {} also holds {} other process(es) (pid {}); the engine only builds its own cgroups when it is alone in one, as in its systemd service",
                root.display(),
                others.len(),
                others.iter().take(5).cloned().collect::<Vec<_>>().join(", ")
            ));
        }
        let eng = root.join("engine");
        mkdir(&eng).map_err(|e| format!("{e} (the engine's cgroup is not delegated to its user: its systemd service needs Delegate=yes)"))?;
        if let Err(e) = wr(&eng.join("cgroup.procs"), &me.to_string()) {
            let _ = std::fs::remove_dir(&eng);
            return Err(e);
        }
        if let Err(e) = wr(&root.join("cgroup.subtree_control"), "+memory") {
            let _ = wr(&root.join("cgroup.procs"), &me.to_string());
            let _ = std::fs::remove_dir(&eng);
            return Err(e);
        }
        sweep_stale(root, &HashSet::new());
        Ok(())
    }

    pub fn make_session(dir: &Path, l: &Limits) -> Result<Scope, String> {
        mkdir(dir)?;
        if let Some(v) = l.session.file_value() {
            wr(&dir.join("memory.max"), &v)?;
        }
        if let Some(v) = l.session_swap.file_value() {
            // absent without swap accounting: no swap limit then, not an error
            let p = dir.join("memory.swap.max");
            if p.exists() {
                wr(&p, &v)?;
            }
        }
        wr(&dir.join("memory.oom.group"), "1")?;
        wr(&dir.join("cgroup.subtree_control"), "+memory")?;
        let leaf = dir.join("session");
        mkdir(&leaf)?;
        let procs = std::fs::OpenOptions::new().write(true).open(leaf.join("cgroup.procs")).map_err(|e| format!("cannot open {}: {e}", leaf.join("cgroup.procs").display()))?;
        Ok(Scope { dir: dir.to_path_buf(), leaf, procs, limit: l.session, test_env: Some((l.test, l.test_swap)) })
    }

    pub fn make_test(dir: &Path, limit: Size, swap: Size) -> Result<Scope, String> {
        let parent = dir.parent().unwrap_or(dir);
        let sub = rd(&parent.join("cgroup.subtree_control"))?;
        if !has_controller(&sub, "memory") {
            return Err(format!("the session's cgroup {} gives its children no memory controller", parent.display()));
        }
        mkdir(dir)?;
        if let Some(v) = limit.file_value() {
            wr(&dir.join("memory.max"), &v)?;
        }
        if let Some(v) = swap.file_value() {
            let p = dir.join("memory.swap.max");
            if p.exists() {
                wr(&p, &v)?;
            }
        }
        let procs = std::fs::OpenOptions::new().write(true).open(dir.join("cgroup.procs")).map_err(|e| format!("cannot open {}: {e}", dir.join("cgroup.procs").display()))?;
        Ok(Scope { dir: dir.to_path_buf(), leaf: dir.to_path_buf(), procs, limit, test_env: None })
    }

    /// In the forked child, before exec: write "0" (= the calling process)
    /// to the open `cgroup.procs`.  Only `write(2)` runs there, which is
    /// async-signal-safe; the descriptor is close-on-exec, so the program
    /// never sees it.  A failed write leaves the child where it was (the
    /// caller checks with `Scope::holds` and says so).
    pub fn place_on_exec(cmd: &mut Command, procs: &std::fs::File) {
        use std::os::unix::process::CommandExt;
        let fd = procs.as_raw_fd();
        // SAFETY: the closure only calls write(2) on a descriptor that stays
        // open until spawn returns (the Scope owns it), allocates nothing and
        // takes no lock
        unsafe {
            cmd.pre_exec(move || {
                let _ = write(fd, b"0".as_ptr().cast(), 1);
                Ok(())
            });
        }
    }

    /// Remove `dir` and every cgroup under it that is empty (best effort;
    /// one still holding processes stays until a later sweep).
    pub fn remove(dir: &Path) {
        if let Ok(rd) = std::fs::read_dir(dir) {
            for e in rd.flatten() {
                if e.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                    remove(&e.path());
                }
            }
        }
        let _ = std::fs::remove_dir(dir);
    }

    /// Remove session cgroups left behind (a session whose background
    /// processes outlived it, an engine that died) once they are empty.
    pub fn sweep_stale(root: &Path, live: &HashSet<PathBuf>) {
        let Ok(rd) = std::fs::read_dir(root) else { return };
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            let p = e.path();
            if (name.starts_with("agent-") || name.starts_with("exec-")) && !live.contains(&p) {
                let populated = std::fs::read_to_string(p.join("cgroup.events")).map(|t| t.lines().any(|l| l.trim() == "populated 1")).unwrap_or(true);
                if !populated {
                    remove(&p);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_parse_with_units_percent_and_off() {
        let ram = Some(16u64 << 30);
        assert_eq!(parse_size("6G", ram, true), Ok(Size::Bytes(6 << 30)));
        assert_eq!(parse_size("6GiB", ram, true), Ok(Size::Bytes(6 << 30)));
        assert_eq!(parse_size(" 512m ", ram, true), Ok(Size::Bytes(512 << 20)));
        assert_eq!(parse_size("1.5G", ram, true), Ok(Size::Bytes(3 << 29)));
        assert_eq!(parse_size("1048576", ram, true), Ok(Size::Bytes(1 << 20)));
        assert_eq!(parse_size("25%", ram, true), Ok(Size::Bytes(4 << 30)));
        for off in ["off", "OFF", "none", "0", "0%"] {
            assert_eq!(parse_size(off, ram, true), Ok(Size::Off), "{off}");
        }
        assert_eq!(parse_size("max", ram, true), Ok(Size::Max));
        // a swap limit of 0 is "no swap", not "off"
        assert_eq!(parse_size("0", ram, false), Ok(Size::Bytes(0)));
        assert_eq!(parse_size("off", ram, false), Ok(Size::Off));
        for bad in ["", "6X", "lots", "120%", "-1G"] {
            assert!(parse_size(bad, ram, true).is_err(), "{bad:?} should not parse");
        }
        assert!(parse_size("40%", None, true).is_err(), "a percentage needs the machine's memory size");
    }

    #[test]
    fn human_sizes_read_plainly() {
        assert_eq!(human(6 << 30), "6 GiB");
        assert_eq!(human(512 << 20), "512 MiB");
        assert_eq!(human((16_293_044u64 * 1024) * 40 / 100), "6.2 GiB");
        assert_eq!(human(100), "100 bytes");
    }

    #[test]
    fn limits_resolve_flag_then_env_then_default_and_survive_typos() {
        let ram = Some(10u64 << 30);
        let (l, w) = Limits::resolve([(None, None); 4], ram);
        assert!(w.is_empty());
        assert_eq!(l, Limits { session: Size::Bytes(4 << 30), session_swap: Size::Bytes(1 << 30), test: Size::Bytes(3 << 30), test_swap: Size::Bytes(512 << 20) });
        let (l, w) = Limits::resolve([(Some("8G"), Some("2G")), (None, Some("0")), (None, Some("off")), (Some("bogus"), None)], ram);
        assert_eq!((l.session, l.session_swap, l.test, l.test_swap), (Size::Bytes(8 << 30), Size::Bytes(0), Size::Off, Size::Bytes(512 << 20)));
        assert_eq!(w.len(), 1);
        assert!(w[0].starts_with("--test-swap-max:") && w[0].contains("default 512M"), "{w:?}");
        let (l, _) = Limits::resolve([(Some("off"), None), (None, None), (Some("off"), None), (None, None)], ram);
        assert!(!l.any_on());
    }

    #[test]
    fn proc_cgroup_mountinfo_controllers_and_meminfo_parse() {
        assert_eq!(parse_proc_cgroup("0::/system.slice/iter-engine.service\n").as_deref(), Some("/system.slice/iter-engine.service"));
        // hybrid: v1 lines plus the unified one
        assert_eq!(parse_proc_cgroup("12:memory:/x\n1:name=systemd:/y\n0::/user.slice/a.scope\n").as_deref(), Some("/user.slice/a.scope"));
        assert_eq!(parse_proc_cgroup("12:memory:/x\n1:name=systemd:/y\n"), None, "v1 only");
        let mi = "24 1 0:22 / /sys rw,nosuid shared:7 - sysfs sysfs rw\n\
                  30 24 0:26 / /sys/fs/cgroup rw,nosuid,nodev,noexec shared:9 - cgroup2 cgroup2 rw,nsdelegate,memory_recursiveprot\n";
        assert_eq!(parse_mountinfo_cgroup2(mi), Some(PathBuf::from("/sys/fs/cgroup")));
        assert_eq!(parse_mountinfo_cgroup2("24 1 0:22 / /sys rw - sysfs sysfs rw\n"), None);
        assert!(has_controller("cpuset cpu io memory pids\n", "memory"));
        assert!(!has_controller("cpu io pids\n", "memory"));
        assert!(!has_controller("memoryx\n", "memory"));
        assert_eq!(parse_meminfo_total("MemTotal:       16293044 kB\nMemFree: 1 kB\n"), Some(16293044 * 1024));
    }

    #[test]
    fn memory_events_parse_and_reports_say_why() {
        let e = parse_events("low 0\nhigh 0\nmax 41\noom 1\noom_kill 3\noom_group_kill 1\n");
        assert_eq!(e, Events { low: 0, high: 0, max: 41, oom: 1, oom_kill: 3, oom_group_kill: 1 });
        let none = Events::default();
        assert_eq!(session_report(Size::Bytes(6 << 30), &none, &none, &none), None);
        let r = session_report(Size::Bytes(6 << 30), &e, &Events { oom: 1, ..none }, &Events { oom_kill: 2, ..none }).unwrap();
        assert_eq!(r, "agent session killed: its processes reached the 6 GiB memory limit for one agent session (memory.events oom_kill=3, oom_group_kill=1, max=41)");
        let r = session_report(Size::Bytes(6 << 30), &e, &none, &Events { oom_kill: 1, ..none }).unwrap();
        assert!(r.contains("out of memory, not at this session's own 6 GiB limit"), "{r}");
        let r = session_report(Size::Bytes(6 << 30), &Events { oom_kill: 1, ..none }, &none, &none).unwrap();
        assert!(r.contains("test scripts were killed") && r.contains("the session went on"), "{r}");
        assert_eq!(script_report(Size::Bytes(1 << 30), &none), None);
        let r = script_report(Size::Bytes(1 << 30), &Events { oom: 1, oom_kill: 1, max: 9, ..none }).unwrap();
        assert!(r.starts_with("killed by the memory limit: 1 of the script's processes reached the 1 GiB limit"), "{r}");
    }

    #[test]
    fn labels_are_safe_cgroup_names() {
        assert_eq!(safe_label("6ae9fe1c-1234"), "6ae9fe1c-1234");
        assert_eq!(safe_label("a/b c..d"), "a-b-c--d");
        assert_eq!(safe_label("../.."), "x");
        assert_eq!(safe_label(&"z".repeat(40)).len(), 24);
    }

    /// Off everywhere it cannot work: with nothing initialised (tests, the
    /// `iter` CLI) no scope is made and commands run as before.
    #[test]
    fn without_init_no_session_scope_is_made() {
        assert!(session("agent", "x").is_none());
    }

    /// The refusals, on a stand-in folder (no cgroup needed): no memory
    /// controller, a cgroup this user may not write (no Delegate=yes), and
    /// another process sharing the cgroup.
    #[cfg(target_os = "linux")]
    #[test]
    fn setup_refuses_plainly_without_memory_delegation_or_alone() {
        use std::os::unix::fs::PermissionsExt;
        let d = std::env::temp_dir().join(format!("iter-memlimit-fake-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(d.join("cgroup.procs"), format!("{}\n", std::process::id())).unwrap();
        std::fs::write(d.join("cgroup.controllers"), "cpu io pids\n").unwrap();
        let e = linux::setup_at(&d).unwrap_err();
        assert!(e.contains("memory controller is not available") && e.contains("Delegate=yes"), "{e}");
        std::fs::write(d.join("cgroup.controllers"), "cpu memory pids\n").unwrap();
        std::fs::set_permissions(&d, std::fs::Permissions::from_mode(0o555)).unwrap();
        let e = linux::setup_at(&d);
        std::fs::set_permissions(&d, std::fs::Permissions::from_mode(0o755)).unwrap();
        let e = e.unwrap_err();
        assert!(e.contains("cannot create cgroup") && e.contains("needs Delegate=yes"), "{e}");
        std::fs::write(d.join("cgroup.procs"), format!("{}\n1\n", std::process::id())).unwrap();
        let e = linux::setup_at(&d).unwrap_err();
        assert!(e.contains("also holds 1 other process(es) (pid 1)"), "{e}");
        let _ = std::fs::remove_dir_all(&d);
    }

    // ------------------------------------------------------ real cgroups
    //
    // `systemd-run --user --scope -p Delegate=yes` gives this test binary a
    // delegated cgroup v2 of its own, without root.  The outer test re-runs
    // this binary in one (just this test, ITER_MEMLIMIT_IN_SCOPE=1); where
    // no user systemd or no delegation exists it prints why and skips —
    // unless ITER_MEMLIMIT_REQUIRE=1, which makes a skip a failure.

    #[cfg(target_os = "linux")]
    const RUNAWAY_PY: &str = "import sys\nb=[]\nfor i in range(32):\n    b.append(bytearray(16<<20))\n    for j in range(0,16<<20,4096): b[-1][j]=1\nprint('runaway finished at', len(b)*16, 'MiB')\nsys.stdout.flush()\n";

    #[cfg(target_os = "linux")]
    fn skip_or_fail(why: &str) {
        if std::env::var("ITER_MEMLIMIT_REQUIRE").is_ok_and(|v| v == "1") {
            panic!("ITER_MEMLIMIT_REQUIRE=1 but the cgroup test cannot run: {why}");
        }
        println!("SKIP cgroup memory-limit test: {why}");
    }

    /// Outside the scope: re-run test `name` alone in a delegated user
    /// scope capped at `max` (no swap), check it passed, and return false.
    /// Inside it: true (the caller goes on).
    #[cfg(target_os = "linux")]
    fn in_scope(name: &str, max: &str) -> bool {
        if std::env::var("ITER_MEMLIMIT_IN_SCOPE").is_ok() {
            return true;
        }
        if Command::new("python3").arg("-c").arg("pass").status().map(|s| !s.success()).unwrap_or(true) {
            skip_or_fail("python3 (the runaway) is not installed");
            return false;
        }
        match Command::new("systemd-run").args(["--user", "--scope", "-q", "true"]).output() {
            Ok(o) if o.status.success() => {}
            Ok(o) => {
                skip_or_fail(&format!("systemd-run --user --scope does not work here: {}", String::from_utf8_lossy(&o.stderr).trim()));
                return false;
            }
            Err(e) => {
                skip_or_fail(&format!("no systemd-run: {e}"));
                return false;
            }
        }
        let exe = std::env::current_exe().unwrap();
        let out = Command::new("systemd-run")
            .args(["--user", "--scope", "-q", "-p", "Delegate=yes", "-p", &format!("MemoryMax={max}"), "-p", "MemorySwapMax=0", "--"])
            .arg(&exe)
            .args([name, "--exact", "--nocapture", "--test-threads=1"])
            .env("ITER_MEMLIMIT_IN_SCOPE", "1")
            .output()
            .unwrap();
        let text = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
        println!("--- inside the scope (MemoryMax={max}) ---\n{text}--- end ---");
        if let Some(l) = text.lines().find(|l| l.contains("SKIP-IN-SCOPE:")) {
            skip_or_fail(l);
            return false;
        }
        assert!(out.status.success() && text.contains("1 passed"), "the in-scope run failed");
        false
    }

    /// Sessions that each stay under their own limit but together pass the
    /// service's (the scope's 700 MiB): the kernel kills one whole session —
    /// not the engine, not both — and the report says the service was out
    /// of memory, not the session at its own limit.
    #[cfg(target_os = "linux")]
    #[test]
    fn cgroup_service_oom_kills_one_whole_session() {
        if !in_scope("memlimit::tests::cgroup_service_oom_kills_one_whole_session", "700M") {
            return;
        }
        let limits = Limits { session: Size::Bytes(600 << 20), session_swap: Size::Bytes(0), test: Size::Off, test_swap: Size::Off };
        if !init(limits) {
            println!("SKIP-IN-SCOPE: the scope's cgroup is not delegated to this user");
            return;
        }
        // each takes 400 MiB in 16 MiB steps, holds it 4 s, and exits
        let grow = "import time\nb=[]\nfor i in range(25):\n    b.append(bytearray(16<<20))\n    for j in range(0,16<<20,4096): b[-1][j]=1\n    time.sleep(0.05)\ntime.sleep(4)\nprint('held 400 MiB')\n";
        let run = move |label: &'static str| {
            std::thread::spawn(move || {
                let mut c = Command::new("python3");
                c.args(["-c", grow]).stdin(std::process::Stdio::null()).stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::piped());
                crate::work::wait_in_session(c, None, 60, &crate::provider::StopCheck::none(), "agent", label)
            })
        };
        let (a, b) = (run("one"), run("two"));
        let (a, b) = (a.join().unwrap(), b.join().unwrap());
        println!("session one -> {a:?}\nsession two -> {b:?}");
        let killed: Vec<&String> = [&a, &b].into_iter().filter_map(|r| r.as_ref().err()).collect();
        assert_eq!(killed.len(), 1, "exactly one session is killed: {a:?} {b:?}");
        assert!(killed[0].starts_with("agent session killed by the kernel's out-of-memory killer while the engine's service (or the machine) was out of memory, not at this session's own 600 MiB limit"), "{}", killed[0]);
        assert!([&a, &b].into_iter().any(|r| r.as_ref().is_ok_and(|o| o.contains("held 400 MiB"))), "the other session finished");
        println!("IN-SCOPE OK: the service-level out-of-memory kill took one whole session; the engine and the other session lived");
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn cgroup_limits_kill_the_runaway_only() {
        const NAME: &str = "memlimit::tests::cgroup_limits_kill_the_runaway_only";
        if !in_scope(NAME, "1G") {
            return;
        }
        // ---- inside the delegated scope: this process is alone in its cgroup
        let limits = Limits { session: Size::Bytes(160 << 20), session_swap: Size::Bytes(0), test: Size::Bytes(96 << 20), test_swap: Size::Bytes(0) };
        if !init(limits) {
            println!("SKIP-IN-SCOPE: the scope's cgroup is not delegated to this user");
            return;
        }
        let root = TREE.get().unwrap().root.clone();
        let me = std::fs::read_to_string("/proc/self/cgroup").unwrap();
        assert!(me.trim().ends_with("/engine"), "the engine moved itself into <unit>/engine: {me}");

        // A: a runaway session next to a well-behaved one, through the real spawn path
        let sibling = std::thread::spawn(|| {
            let mut c = Command::new("bash");
            c.args(["-c", "sleep 3; cat /proc/self/cgroup; echo sibling-ok"]).stdin(std::process::Stdio::null()).stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::piped());
            crate::work::wait_in_session(c, None, 60, &crate::provider::StopCheck::none(), "agent", "sibling")
        });
        let mut c = Command::new("python3");
        c.args(["-c", RUNAWAY_PY]).stdin(std::process::Stdio::null()).stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::piped());
        let runaway = crate::work::wait_in_session(c, None, 60, &crate::provider::StopCheck::none(), "agent", "runaway");
        println!("runaway session -> {runaway:?}");
        let err = runaway.expect_err("the runaway session must be killed, not finish");
        assert!(err.starts_with("agent session killed: its processes reached the 160 MiB memory limit for one agent session (memory.events oom_kill="), "{err}");
        assert!(err.contains("oom_group_kill=1") && !err.contains("runaway finished"), "{err}");
        let sib = sibling.join().unwrap();
        println!("sibling session -> {sib:?}");
        let sib = sib.expect("the sibling session survives");
        assert!(sib.contains("sibling-ok") && sib.contains("/agent-") && sib.trim_end().contains("/session"), "{sib}");

        // B: inside a session, `iter runtests`' run_script gives each script
        // its own cgroup: the runaway script's python is killed, the script
        // and the session go on, the next script passes
        let exe = std::env::current_exe().unwrap();
        let mut c = Command::new(&exe);
        c.args(["memlimit::tests::scripts_helper", "--exact", "--nocapture", "--test-threads=1"])
            .env("ITER_MEMLIMIT_HELPER", "1")
            .env_remove("ITER_MEMLIMIT_IN_SCOPE")
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());
        let helper = crate::work::wait_in_session(c, None, 120, &crate::provider::StopCheck::none(), "agent", "tests");
        println!("test-script session -> {helper:?}");
        let out = helper.expect("the session running the scripts survives");
        assert!(out.contains("RUNAWAY-SCRIPT memory=killed by the memory limit:") && out.contains("reached the 96 MiB limit for one test script"), "{out}");
        assert!(out.contains("RUNAWAY-SCRIPT stdout=after-python exit=137"), "the script went on after its python was killed: {out}");
        assert!(out.contains("GOOD-SCRIPT code=0 memory=<none>"), "{out}");
        assert!(out.lines().any(|l| l.starts_with("GOOD-SCRIPT stdout=0::") && l.contains("/agent-") && l.contains("/test-")), "the script ran in its own test cgroup under the session's: {out}");
        assert!(out.contains("1 passed"), "{out}");

        // nothing left behind but the engine's own cgroup
        let left: Vec<String> = std::fs::read_dir(&root).unwrap().flatten().filter(|e| e.path().is_dir()).map(|e| e.file_name().to_string_lossy().into_owned()).collect();
        assert_eq!(left, vec!["engine".to_string()], "session cgroups are removed after their sessions");
        println!("IN-SCOPE OK: runaway killed at its limit, sibling and parent alive, cgroups removed");
    }

    /// Run as the command of a limited session by the test above (does
    /// nothing otherwise).
    #[cfg(target_os = "linux")]
    #[test]
    fn scripts_helper() {
        if std::env::var("ITER_MEMLIMIT_HELPER").is_err() {
            return;
        }
        let dir = std::env::temp_dir().join(format!("iter-memlimit-scripts-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let bad = dir.join("runaway.sh");
        std::fs::write(&bad, format!("python3 -c \"{}\"\necho after-python exit=$?\n", RUNAWAY_PY.replace('"', "\\\""))).unwrap();
        let good = dir.join("good.sh");
        std::fs::write(&good, "cat /proc/self/cgroup\necho fine\n").unwrap();
        let r = crate::local::runtests::run_script(&bad, &dir, std::time::Duration::from_secs(60), None, &[]);
        println!("RUNAWAY-SCRIPT memory={}", r.memory);
        println!("RUNAWAY-SCRIPT stdout={}", r.stdout.trim());
        let g = crate::local::runtests::run_script(&good, &dir, std::time::Duration::from_secs(60), None, &[]);
        println!("GOOD-SCRIPT code={} memory={}", g.code, if g.memory.is_empty() { "<none>" } else { &g.memory });
        println!("GOOD-SCRIPT stdout={}", g.stdout.trim().replace('\n', " | "));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
