use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use crate::testgroups::{self, TestGroup};
use crate::workitems;

/// Default wall-clock budget for one testgroup's scripts (shared across the
/// group). Compiled-in default — override per invocation with
/// `iter runtests --timeout-min` / `iter testsweep --group-timeout-min` (the
/// "Test Loop" scheduled workitem carries the flag visibly in its command).
pub const DEFAULT_GROUP_TIMEOUT_MIN: u64 = 20;

/// The whole engine⇄script contract, from the exit code alone (features/TDD.md
/// "Test Contract"): 0 = ran, all as expected; 1 = ran, something unexpected;
/// anything else (crash, missing dep, timeout kill) = the script itself broke —
/// a distinct state so infrastructure problems never masquerade as red tests.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    Green,
    Red,
    Error,
}

impl Outcome {
    pub fn as_str(&self) -> &'static str {
        match self {
            Outcome::Green => "passed",
            Outcome::Red => "failed",
            Outcome::Error => "error",
        }
    }
}

#[derive(Debug)]
pub struct TestRunResult {
    pub id: String,
    pub name: String,
    pub shell: String,
    pub outcome: Outcome,
    pub exit_code: i32,
    pub pass: u64,
    pub total: u64,
    /// the full run log (header + stdout + stderr) — kept in memory and
    /// reported to the work item, never written under the tree (2026-09-08:
    /// `<test_dir>/runs/` is gone)
    pub log: String,
    /// Human-readable note for `error` outcomes (timeout, spawn failure, …).
    pub detail: String,
    /// false = a `gates:false` entry: logged and counted on its own line, but
    /// left out of the group's outcome and pass/total (F20).
    pub gates: bool,
}

#[derive(Debug)]
pub struct GroupRunResult {
    pub label: String,
    pub tg_file: PathBuf,
    pub test_dir: PathBuf,
    pub outcome: Outcome,
    pub pass: u64,
    pub total: u64,
    pub runs: Vec<TestRunResult>,
    /// True when every registered test ran (no --test filter): only then do the
    /// group's lastrun/result/counts get updated — a filtered run proves nothing
    /// about the group.
    pub full_run: bool,
}

/// Find the testgroup `label` across every testgroup.iter.md under `code_root`.
/// Duplicate labels are reported so they get rationalized. Among duplicates a
/// copy outside any hidden directory wins (F14): `.claude/worktrees/…` sorts
/// first, and taking the first match used to grade a worktree's stale copy.
pub fn locate_group(code_root: &Path, label: &str) -> Result<(PathBuf, TestGroup), String> {
    let mut found: Vec<(PathBuf, TestGroup)> = Vec::new();
    for file in testgroups::find_files(code_root) {
        let Ok(content) = std::fs::read_to_string(&file) else { continue };
        for g in testgroups::parse(&content) {
            if g.label == label {
                found.push((file.clone(), g));
            }
        }
    }
    match found.len() {
        0 => Err(format!("testgroup \"{}\" not found under {}", label, code_root.display())),
        1 => Ok(found.into_iter().next().unwrap()),
        n => {
            let hidden = |p: &Path| {
                p.strip_prefix(code_root)
                    .unwrap_or(p)
                    .components()
                    .any(|c| c.as_os_str().to_str().is_some_and(|s| s.starts_with('.') && s != "." && s != ".."))
            };
            let pick = found.iter().position(|(p, _)| !hidden(p)).unwrap_or(0);
            eprintln!(
                "warning: testgroup \"{}\" defined in {} files; using {}",
                label,
                n,
                found[pick].0.display()
            );
            Ok(found.swap_remove(pick))
        }
    }
}

/// Run a testgroup's shell scripts (optionally narrowed to one test id/script by
/// `filter`), capture each run's log in memory (reported to the work item;
/// nothing is written under the tree since 2026-09-08), and on a
/// full run update the group's JSONL block (lastrun/result/counts) in place.
/// The group shares one wall-clock budget of `timeout_min` minutes
/// (`DEFAULT_GROUP_TIMEOUT_MIN` unless a flag overrides it); scripts that would
/// start past the deadline are recorded as `error` without running.
pub fn run_group(
    tg_file: &Path,
    label: &str,
    filter: Option<&str>,
    timeout_min: u64,
) -> Result<GroupRunResult, String> {
    run_group_stamped(tg_file, label, filter, timeout_min, true)
}

/// `run_group` with the registry stamp optional: the test sweep passes
/// `false` (2026-09-30) — it records results on the map, and a run that
/// writes into the checkout would leave files no commit owns.
pub fn run_group_stamped(
    tg_file: &Path,
    label: &str,
    filter: Option<&str>,
    timeout_min: u64,
    stamp: bool,
) -> Result<GroupRunResult, String> {
    let content =
        std::fs::read_to_string(tg_file).map_err(|e| format!("cannot read {}: {}", tg_file.display(), e))?;
    let groups = testgroups::parse(&content);
    let group = groups
        .iter()
        .find(|g| g.label == label)
        .ok_or_else(|| format!("testgroup \"{}\" not in {}", label, tg_file.display()))?
        .clone();
    let test_dir = tg_file.parent().ok_or("testgroup file has no parent directory")?.to_path_buf();

    let entries: Vec<_> = group
        .testlist
        .iter()
        .filter(|t| filter.map(|f| t.id == f || t.shell == f).unwrap_or(true))
        .cloned()
        .collect();
    if entries.is_empty() {
        return Err(match filter {
            Some(f) => format!("no test matching \"{}\" in testgroup \"{}\"", f, label),
            None => format!("testgroup \"{}\" has no tests registered", label),
        });
    }
    let full_run = filter.is_none();
    let out_dir = fresh_test_out_dir(tg_file, label);

    let budget = Duration::from_secs(timeout_min.max(1) * 60);
    let started = Instant::now();

    let mut runs = Vec::new();
    for entry in entries {
        let entry_gates = entry.gates;
        let remaining = budget.saturating_sub(started.elapsed());
        let script = test_dir.join(&entry.shell);
        let run = if remaining.is_zero() {
            let detail = format!("not run: group budget ({} min) exhausted", timeout_min);
            let log = log_body(&entry.shell, -1, "", "", &detail);
            TestRunResult {
                id: entry.id,
                name: entry.name,
                shell: entry.shell,
                outcome: Outcome::Error,
                exit_code: -1,
                pass: 0,
                total: 1,
                log,
                detail,
                gates: true,
            }
        } else if !script.is_file() {
            let detail = format!("script not found: {}", script.display());
            let log = log_body(&entry.shell, -1, "", "", &detail);
            TestRunResult {
                id: entry.id,
                name: entry.name,
                shell: entry.shell,
                outcome: Outcome::Error,
                exit_code: -1,
                pass: 0,
                total: 1,
                log,
                detail,
                gates: true,
            }
        } else {
            let (exit_code, stdout, stderr, timed_out) = run_script(&script, &test_dir, remaining, out_dir.as_deref());
            let detail = if timed_out {
                format!("timed out (group budget {} min); killed", timeout_min)
            } else {
                String::new()
            };
            let log = log_body(&entry.shell, exit_code, &stdout, &stderr, &detail);
            let outcome = match exit_code {
                0 => Outcome::Green,
                1 => Outcome::Red,
                _ => Outcome::Error,
            };
            // Per-test counts from the contract's ITER_RESULT trailer; a script
            // that doesn't report counts is counted as one test.
            let (pass, total) = parse_iter_result(&stdout).unwrap_or_else(|| match outcome {
                Outcome::Green => (1, 1),
                _ => (0, 1),
            });
            TestRunResult {
                id: entry.id,
                name: entry.name,
                shell: entry.shell,
                outcome,
                exit_code,
                pass,
                total,
                log,
                detail,
                gates: true,
            }
        };
        let run = TestRunResult { gates: entry_gates, ..run };
        runs.push(run);
    }

    // Only gating entries decide the colour and the group's pass/total; a
    // `gates:false` entry still runs and shows on its own log-header line.
    let gating = || runs.iter().filter(|r| r.gates);
    let outcome = if gating().any(|r| r.outcome == Outcome::Error) {
        Outcome::Error
    } else if gating().any(|r| r.outcome == Outcome::Red) {
        Outcome::Red
    } else {
        Outcome::Green
    };
    let pass: u64 = gating().map(|r| r.pass).sum();
    let total: u64 = gating().map(|r| r.total).sum();

    if full_run && stamp {
        stamp_group(tg_file, label, &workitems::now_iso(), outcome.as_str(), &format!("{}/{}", pass, total))?;
    }

    Ok(GroupRunResult { label: label.into(), tg_file: tg_file.to_path_buf(), test_dir, outcome, pass, total, runs, full_run })
}

/// Record one group's lastrun/result/counts in its registry file (F13).
/// Several groups share one file and their runs overlap, so the file is
/// re-read under a lock at the moment of writing and only this group's line
/// changes — writing back the snapshot read when the run STARTED used to erase
/// whatever a sibling group recorded meanwhile. The write goes to a temp file
/// and is renamed over the registry so a reader never sees half a file.
pub fn stamp_group(tg_file: &Path, label: &str, lastrun: &str, result: &str, counts: &str) -> Result<(), String> {
    let _lock = RegistryLock::acquire(tg_file)?;
    let content =
        std::fs::read_to_string(tg_file).map_err(|e| format!("cannot read {}: {}", tg_file.display(), e))?;
    let mut groups = testgroups::parse(&content);
    for g in groups.iter_mut().filter(|g| g.label == label) {
        g.lastrun = lastrun.into();
        g.result = result.into();
        g.counts = counts.into();
    }
    let updated = testgroups::update(&content, &groups);
    let tmp = tg_file.with_extension(format!("md.tmp.{}", std::process::id()));
    std::fs::write(&tmp, updated).map_err(|e| format!("cannot write {}: {}", tmp.display(), e))?;
    std::fs::rename(&tmp, tg_file).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        format!("cannot update {}: {}", tg_file.display(), e)
    })
}

/// An advisory lock on `<registry>.lock`, taken by creating the file
/// exclusively (no extra crate needed; works across processes). A lock file
/// older than `STALE` is taken to be left behind by a killed runner.
struct RegistryLock(PathBuf);

impl RegistryLock {
    const STALE: Duration = Duration::from_secs(60);
    const GIVE_UP: Duration = Duration::from_secs(30);

    fn acquire(tg_file: &Path) -> Result<RegistryLock, String> {
        let path = tg_file.with_extension("md.lock");
        let started = Instant::now();
        loop {
            match std::fs::OpenOptions::new().write(true).create_new(true).open(&path) {
                Ok(_) => return Ok(RegistryLock(path)),
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                    let stale = std::fs::metadata(&path)
                        .and_then(|m| m.modified())
                        .ok()
                        .and_then(|t| t.elapsed().ok())
                        .is_some_and(|age| age > Self::STALE);
                    if stale {
                        let _ = std::fs::remove_file(&path);
                        continue;
                    }
                    if started.elapsed() > Self::GIVE_UP {
                        return Err(format!("cannot lock {}: held for over {}s", path.display(), Self::GIVE_UP.as_secs()));
                    }
                    std::thread::sleep(Duration::from_millis(20));
                }
                Err(e) => return Err(format!("cannot lock {}: {}", path.display(), e)),
            }
        }
    }
}

impl Drop for RegistryLock {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

/// `bash <script>` in `cwd` with a hard deadline. The script leads its own process
/// group so a timeout kill takes its whole tree. Returns (exit code, stdout,
/// stderr, timed_out); spawn failures surface as exit -1 with the error on stderr.
/// `{topdir}/.iter/tests` — shared test programs (decided 2026-09-08), found
/// by walking up from the test dir to the first ancestor holding `.iter/`.
pub fn shared_tests_dir(from: &Path) -> Option<PathBuf> {
    from.ancestors().find(|d| d.join(".iter").is_dir()).map(|d| d.join(".iter").join("tests"))
}

/// `$ITER_TEST_OUT` (decided 2026-09-30): the one place a test script writes
/// its output files. It lies OUTSIDE the checkout (nothing to commit, nothing
/// swept into another item's commit) and is emptied at the start of every
/// run of the group, so only the last run is ever kept — never a history.
/// One folder per (registry file, group label).
pub fn test_out_dir(tg_file: &Path, label: &str) -> PathBuf {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    tg_file.canonicalize().unwrap_or_else(|_| tg_file.to_path_buf()).hash(&mut h);
    let safe: String = label.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '_' }).collect();
    std::env::temp_dir().join("iter-testout").join(format!("{safe}-{:08x}", h.finish() as u32))
}

fn fresh_test_out_dir(tg_file: &Path, label: &str) -> Option<PathBuf> {
    let dir = test_out_dir(tg_file, label);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).ok().map(|_| dir)
}

fn run_script(script: &Path, cwd: &Path, timeout: Duration, out_dir: Option<&Path>) -> (i32, String, String, bool) {
    let mut cmd = Command::new("bash");
    cmd.arg(script).current_dir(cwd).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    if let Some(out) = out_dir {
        cmd.env("ITER_TEST_OUT", out);
    }
    if let Some(shared) = shared_tests_dir(cwd) {
        cmd.env("ITER_TESTS_SHARED", shared);
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }
    let child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => return (-1, String::new(), format!("cannot spawn bash {}: {}", script.display(), e), false),
    };
    let pid = child.id();
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(child.wait_with_output());
    });
    match rx.recv_timeout(timeout) {
        Ok(Ok(output)) => (
            output.status.code().unwrap_or(-1),
            String::from_utf8_lossy(&output.stdout).into_owned(),
            String::from_utf8_lossy(&output.stderr).into_owned(),
            false,
        ),
        Ok(Err(e)) => (-1, String::new(), format!("wait failed: {}", e), false),
        Err(_) => {
            if cfg!(unix) {
                let _ = Command::new("sh").arg("-c").arg(format!("kill -9 -{}", pid)).status();
            } else {
                let _ = Command::new("kill").arg("-9").arg(pid.to_string()).status();
            }
            (-1, String::new(), String::new(), true)
        }
    }
}

/// The machine-readable trailer: last stdout line of the form
/// `ITER_RESULT pass=12 fail=2 total=14` (fail is tolerated but derived data —
/// pass/total are what the engine records).
fn parse_iter_result(stdout: &str) -> Option<(u64, u64)> {
    let line = stdout.lines().rev().find(|l| l.trim_start().starts_with("ITER_RESULT"))?;
    let mut pass = None;
    let mut total = None;
    for token in line.split_whitespace() {
        if let Some((k, v)) = token.split_once('=') {
            match k {
                "pass" => pass = v.parse().ok(),
                "total" => total = v.parse().ok(),
                _ => {}
            }
        }
    }
    Some((pass?, total?))
}

/// Verbatim capture for the runs/ history: everything the diagnosing agent (or the
/// UI's run browser) needs from one script execution.
fn log_body(shell: &str, exit_code: i32, stdout: &str, stderr: &str, detail: &str) -> String {
    let mut body = format!(
        "# iterapp test run\n# script: {}\n# time: {}\n# exit: {}\n",
        shell,
        workitems::now_iso(),
        exit_code
    );
    if !detail.is_empty() {
        body.push_str(&format!("# note: {}\n", detail));
    }
    body.push_str("\n--- stdout ---\n");
    body.push_str(stdout);
    body.push_str("\n--- stderr ---\n");
    body.push_str(stderr);
    body
}

/// Per-script cap inside a Log Detail row and the row's total cap: enough for
/// a follow-up agent to diagnose, small enough not to flood the store.
pub const LOG_DETAIL_PER_TEST_BYTES: usize = 8 * 1024;
pub const LOG_DETAIL_TOTAL_BYTES: usize = 64 * 1024;

/// "Log Header" (decided 2026-09-08): the deterministic, body-free summary of
/// one run — appended to the work item on EVERY run. `tg_rel` is the testgroup
/// file relative to the project top.
pub fn log_header(run: &GroupRunResult, tg_rel: &str, when: &str) -> String {
    let mut s = format!(
        "Test run {when} — testgroup \"{}\" in {tg_rel}\nresult: {}  pass {}/{}{}\n",
        run.label,
        run.outcome.as_str().to_uppercase(),
        run.pass,
        run.total,
        if run.full_run { "" } else { "  (filtered run; the group's recorded result is not updated)" }
    );
    for t in &run.runs {
        s.push_str(&format!(
            "- {:<5} {} ({}) [{}] {}/{}{}{}\n",
            match t.outcome {
                Outcome::Green => "pass",
                Outcome::Red => "FAIL",
                Outcome::Error => "ERROR",
            },
            t.id,
            t.name,
            t.shell,
            t.pass,
            t.total,
            if t.gates { "" } else { " (non-gating)" },
            if t.detail.is_empty() { String::new() } else { format!(" — {}", t.detail) }
        ));
    }
    s
}

/// "Log Detail" (decided 2026-09-08): only when the run is non-green — the
/// failing and erroring scripts' logs (tail-capped per script and overall) for
/// the follow-up agent.  Empty when everything passed.
pub fn log_detail(run: &GroupRunResult) -> String {
    let mut s = String::new();
    for t in run.runs.iter().filter(|t| t.outcome != Outcome::Green) {
        let log = tail_bytes(&t.log, LOG_DETAIL_PER_TEST_BYTES);
        let block = format!("## {} ({}) [{}] exit {}\n{}\n\n", t.id, t.name, t.shell, t.exit_code, log.trim_end());
        if s.len() + block.len() > LOG_DETAIL_TOTAL_BYTES {
            s.push_str(&format!("## … {} more failing script(s) omitted (row cap {} KB)\n", run.runs.iter().filter(|x| x.outcome != Outcome::Green).count(), LOG_DETAIL_TOTAL_BYTES / 1024));
            break;
        }
        s.push_str(&block);
    }
    s
}

fn tail_bytes(text: &str, max: usize) -> String {
    if text.len() <= max {
        return text.to_string();
    }
    let mut start = text.len() - max;
    while !text.is_char_boundary(start) {
        start += 1;
    }
    format!("… (first {} bytes omitted)\n{}", start, &text[start..])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testgroups::TestEntry;

    fn setup(name: &str, scripts: &[(&str, &str)]) -> (PathBuf, PathBuf) {
        let root = std::env::temp_dir().join(format!("iter-runtests-{}-{}", name, std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let test_dir = root.join("comp/test");
        std::fs::create_dir_all(&test_dir).unwrap();
        for (file, body) in scripts {
            std::fs::write(test_dir.join(file), body).unwrap();
        }
        let entries: Vec<TestEntry> = scripts
            .iter()
            .map(|(file, _)| TestEntry {
                id: file.trim_end_matches(".sh").into(),
                name: (*file).into(),
                desc: String::new(),
                shell: (*file).into(),
                gates: true,
                kind: String::new(),
            })
            .collect();
        let group = TestGroup { label: "g1".into(), testlist: entries, ..Default::default() };
        let content = testgroups::update("# tests\n", &[group]);
        let tg_file = test_dir.join("testgroup.iter.md");
        std::fs::write(&tg_file, content).unwrap();
        (root, tg_file)
    }

    #[test]
    fn green_red_error_exit_codes() {
        let (root, tg) = setup(
            "codes",
            &[
                ("t1.sh", "echo 'ITER_RESULT pass=3 fail=0 total=3'\nexit 0\n"),
                ("t2.sh", "echo 'ITER_RESULT pass=1 fail=1 total=2'\nexit 1\n"),
                ("t3.sh", "exit 7\n"),
            ],
        );
                let run = run_group(&tg, "g1", None, DEFAULT_GROUP_TIMEOUT_MIN).unwrap();
        assert_eq!(run.outcome, Outcome::Error, "error trumps red");
        assert_eq!(run.runs[0].outcome, Outcome::Green);
        assert_eq!(run.runs[1].outcome, Outcome::Red);
        assert_eq!(run.runs[2].outcome, Outcome::Error);
        assert_eq!((run.pass, run.total), (4, 6), "ITER_RESULT counts aggregate; t3 counts as 0/1");
        // Block updated + logs written.
        let content = std::fs::read_to_string(&tg).unwrap();
        let groups = testgroups::parse(&content);
        assert_eq!(groups[0].result, "error");
        assert_eq!(groups[0].counts, "4/6");
        assert!(!groups[0].lastrun.is_empty());
        assert!(!run.test_dir.join("runs").exists(), "no runs/ directory is written any more");
        let log = run.runs[0].log.clone();
        assert!(log.contains("ITER_RESULT"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn all_green_marks_group_passed() {
        let (root, tg) = setup("green", &[("t1.sh", "exit 0\n"), ("t2.sh", "echo ok\nexit 0\n")]);
                let run = run_group(&tg, "g1", None, DEFAULT_GROUP_TIMEOUT_MIN).unwrap();
        assert_eq!(run.outcome, Outcome::Green);
        assert_eq!((run.pass, run.total), (2, 2), "no ITER_RESULT → 1 test per script");
        let groups = testgroups::parse(&std::fs::read_to_string(&tg).unwrap());
        assert!(groups[0].is_green());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn test_out_keeps_only_the_last_run() {
        // run 1 leaves a file in $ITER_TEST_OUT; run 2 must start with it gone
        let script = "if [ -e \"$ITER_TEST_OUT/old.txt\" ]; then exit 1; fi\necho run > \"$ITER_TEST_OUT/old.txt\"\nexit 0\n";
        let (root, tg) = setup("testout", &[("t1.sh", script)]);
        assert_eq!(run_group(&tg, "g1", None, DEFAULT_GROUP_TIMEOUT_MIN).unwrap().outcome, Outcome::Green);
        let out = test_out_dir(&tg, "g1");
        assert!(out.join("old.txt").is_file(), "the output is kept after the run");
        assert!(!out.starts_with(&root), "outside the checkout");
        assert_eq!(run_group(&tg, "g1", None, DEFAULT_GROUP_TIMEOUT_MIN).unwrap().outcome, Outcome::Green, "emptied before the next run");
        let _ = std::fs::remove_dir_all(&out);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn filtered_run_does_not_update_group() {
        let (root, tg) = setup("filter", &[("t1.sh", "exit 0\n"), ("t2.sh", "exit 1\n")]);
                let run = run_group(&tg, "g1", Some("t1"), DEFAULT_GROUP_TIMEOUT_MIN).unwrap();
        assert!(!run.full_run);
        assert_eq!(run.outcome, Outcome::Green);
        assert_eq!(run.runs.len(), 1);
        let groups = testgroups::parse(&std::fs::read_to_string(&tg).unwrap());
        assert!(groups[0].result.is_empty(), "filtered run must not stamp the group");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn missing_script_is_error_not_red() {
        let (root, tg) = setup("missing", &[("t1.sh", "exit 0\n")]);
        // Register a second test whose script does not exist.
        let content = std::fs::read_to_string(&tg).unwrap();
        let mut groups = testgroups::parse(&content);
        groups[0].testlist.push(TestEntry { id: "ghost".into(), name: "ghost".into(), desc: String::new(), shell: "ghost.sh".into(), gates: true, kind: String::new() });
        std::fs::write(&tg, testgroups::update(&content, &groups)).unwrap();
                let run = run_group(&tg, "g1", None, DEFAULT_GROUP_TIMEOUT_MIN).unwrap();
        assert_eq!(run.outcome, Outcome::Error);
        assert!(run.runs[1].detail.contains("script not found"));
        let _ = std::fs::remove_dir_all(&root);
    }

    /// F20: a red `gates:false` entry beside a green one leaves the group
    /// Green, and the red entry still shows in the log header.
    #[test]
    fn non_gating_red_entry_does_not_colour_the_group() {
        let (root, tg) = setup("gates", &[("t1.sh", "exit 0\n"), ("inv.sh", "exit 1\n")]);
        let content = std::fs::read_to_string(&tg).unwrap();
        let mut groups = testgroups::parse(&content);
        groups[0].testlist[1].gates = false;
        std::fs::write(&tg, testgroups::update(&content, &groups)).unwrap();
        let run = run_group(&tg, "g1", None, DEFAULT_GROUP_TIMEOUT_MIN).unwrap();
        assert_eq!(run.outcome, Outcome::Green);
        assert_eq!((run.pass, run.total), (1, 1), "non-gating entry is out of the group's counts");
        let header = log_header(&run, "comp/test/testgroup.iter.md", "now");
        assert!(header.contains("FAIL  inv (inv.sh) [inv.sh] 0/1 (non-gating)"), "{header}");
        assert_eq!(testgroups::parse(&std::fs::read_to_string(&tg).unwrap())[0].result, "passed");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// F13: two groups in ONE registry file whose runs overlap (both read the
    /// file before either writes) each keep their own fresh stamp.
    #[test]
    fn concurrent_groups_in_one_registry_keep_both_stamps() {
        let root = std::env::temp_dir().join(format!("iter-runtests-race-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let test_dir = root.join("comp/test");
        std::fs::create_dir_all(&test_dir).unwrap();
        // Each script sleeps, so both runs have read the registry long before
        // either one writes — the interleaving that lost a result.
        std::fs::write(test_dir.join("a.sh"), "sleep 0.4\nexit 0\n").unwrap();
        std::fs::write(test_dir.join("b.sh"), "sleep 0.4\nexit 1\n").unwrap();
        let entry = |f: &str| TestEntry { id: f.trim_end_matches(".sh").into(), name: f.into(), desc: String::new(), shell: f.into(), gates: true, kind: String::new() };
        let groups = vec![
            TestGroup { label: "ga".into(), testlist: vec![entry("a.sh")], ..Default::default() },
            TestGroup { label: "gb".into(), testlist: vec![entry("b.sh")], ..Default::default() },
        ];
        let tg = test_dir.join("testgroup.iter.md");
        std::fs::write(&tg, testgroups::update("# tests\n", &groups)).unwrap();
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
        let handles: Vec<_> = ["ga", "gb"]
            .into_iter()
            .map(|label| {
                let (tg, barrier) = (tg.clone(), barrier.clone());
                std::thread::spawn(move || {
                    barrier.wait();
                    run_group(&tg, label, None, DEFAULT_GROUP_TIMEOUT_MIN).unwrap();
                })
            })
            .collect();
        for h in handles {
            h.join().unwrap();
        }
        let after = testgroups::parse(&std::fs::read_to_string(&tg).unwrap());
        assert_eq!(after.len(), 2);
        assert_eq!((after[0].result.as_str(), after[0].counts.as_str()), ("passed", "1/1"), "ga lost its stamp");
        assert_eq!((after[1].result.as_str(), after[1].counts.as_str()), ("failed", "0/1"), "gb lost its stamp");
        assert!(!tg.with_extension("md.lock").exists(), "the lock file is removed");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// F14: the same label in a Claude Code worktree copy and in the real
    /// tree resolves to the real tree's copy.
    #[test]
    fn locate_group_prefers_the_copy_outside_hidden_dirs() {
        let root = std::env::temp_dir().join(format!("iter-runtests-wt-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let group = TestGroup { label: "a".into(), ..Default::default() };
        let block = testgroups::update("# t\n", &[group]);
        for dir in [".claude/worktrees/x", "core", ".hidden/copy"] {
            std::fs::create_dir_all(root.join(dir)).unwrap();
            std::fs::write(root.join(dir).join("a.testgroup.iter.md"), &block).unwrap();
        }
        let (found, _) = locate_group(&root, "a").unwrap();
        assert_eq!(found, root.join("core/a.testgroup.iter.md"));
        // .claude is never walked at all; another hidden copy still loses.
        assert!(!testgroups::find_files(&root).iter().any(|p| p.starts_with(root.join(".claude"))));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn locate_group_finds_by_label() {
        let (root, tg) = setup("locate", &[("t1.sh", "exit 0\n")]);
        let (found, group) = locate_group(&root, "g1").unwrap();
        assert_eq!(found, tg);
        assert_eq!(group.label, "g1");
        assert!(locate_group(&root, "nope").is_err());
        let _ = std::fs::remove_dir_all(&root);
    }
}
