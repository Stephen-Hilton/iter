//! The deterministic test runner (iter5 spec §3.5).
//!
//! A test node (`*.test.iter.md`) is metadata; its `children.tests` entries
//! are globs / paths of the scripts. Each script runs under `bash` in the test
//! node's directory, in its own process group, inside one shared wall-clock
//! budget per node, and prints the standard result JSON as the LAST line of
//! stdout (legacy `ITER_RESULT pass= fail= total=` is still accepted). The
//! scripts' results are aggregated into ONE standard [`TestResult`] for the
//! test node (the shape `POST …/graph/nodes/{id}/testresult` takes).
//!
//! Exit code: 0 = pass, 1 = fail, anything else (crash, missing tool, timeout
//! kill) = could not run — never mistaken for a red test.
//!
//! Script environment: `ITER_TEST_OUT` (an emptied scratch dir outside the
//! checkout, one per test node — the only place a script writes files),
//! `ITER_TESTS_SHARED` (`{topdir}/.iter/tests`), `ITER_TEST_NODE_ID`,
//! `ITER_TEST_NODE_NAME`, `ITER_TOPDIR`.

use iter_core::nodefile::{self, NodeDoc, NodeType};
use iter_core::testresult::{self, Bucket, Detail, Outcome, TestResult};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::mpsc;
use std::time::{Duration, Instant};

/// Default wall-clock budget (minutes) for one test node's scripts together.
pub const DEFAULT_TIMEOUT_MIN: u64 = 20;

/// One script's run.
#[derive(Debug, Clone)]
pub struct ScriptRun {
    /// the script, relative to the topdir
    pub script: String,
    pub result: TestResult,
    pub outcome: Outcome,
    /// -1 = killed / not started / spawn failure
    pub exit_code: i32,
    /// header + stdout + stderr, kept in memory only
    pub log: String,
    /// why it could not run (timeout, missing, budget), else ""
    pub detail: String,
}

/// One test node's run.
#[derive(Debug, Clone)]
pub struct NodeRun {
    pub id: String,
    pub name: String,
    /// `{topdir}/…` path of the test node file
    pub path: String,
    pub scripts: Vec<ScriptRun>,
    /// the aggregated standard result for the node
    pub result: TestResult,
    pub outcome: Outcome,
    /// every script ran (no filter)
    pub full_run: bool,
}

pub fn outcome_word(o: Outcome) -> &'static str {
    match o {
        Outcome::Pass => "passed",
        Outcome::Fail => "failed",
        Outcome::CouldNotRun => "error",
    }
}

/// `{topdir}/x` → real path (no escape checks: read-only use).
fn real(topdir: &Path, stored: &str) -> PathBuf {
    match stored.strip_prefix("{topdir}") {
        Some(rest) => topdir.join(rest.trim_start_matches('/')),
        None => PathBuf::from(stored),
    }
}

/// The script files a test node names (its `children.tests`), sorted,
/// deduped; a directory entry means every `*.sh` under it; node files are
/// never scripts. Missing literal paths are returned too (they run as
/// "could not run: script not found").
pub fn scripts_of(topdir: &Path, doc: &NodeDoc) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = Vec::new();
    for entry in &doc.children.tests {
        let expanded = nodefile::expand_entry(entry, &doc.path, None);
        let p = real(topdir, &expanded);
        let ps = p.to_string_lossy().into_owned();
        if ps.contains('*') || ps.contains('?') || ps.contains('[') {
            if let Ok(paths) = glob::glob(&ps) {
                out.extend(paths.flatten().filter(|x| x.is_file()));
            }
        } else if p.is_dir() {
            let mut stack = vec![p];
            while let Some(d) = stack.pop() {
                for e in std::fs::read_dir(&d).into_iter().flatten().flatten() {
                    let ep = e.path();
                    if ep.is_dir() {
                        if !crate::is_skip_dir(&e.file_name().to_string_lossy()) {
                            stack.push(ep);
                        }
                    } else if ep.extension().is_some_and(|x| x == "sh") {
                        out.push(ep);
                    }
                }
            }
        } else {
            out.push(p);
        }
    }
    out.retain(|p| !p.to_string_lossy().ends_with(".iter.md"));
    out.sort();
    out.dedup();
    out
}

/// Aggregate per-script results into one result for the node: buckets
/// summed, details concatenated (named `<script>: <detail>`), outcome =
/// could-not-run if any script could not run, else fail if any failed, else
/// pass. A node with no scripts could not run.
pub fn aggregate(name: &str, id: &str, runs: &[(String, TestResult, Outcome)]) -> (TestResult, Outcome) {
    let mut r = TestResult { name: name.to_string(), id: id.to_string(), ..Default::default() };
    let add = |a: &mut Bucket, b: &Bucket| {
        a.total += b.total;
        a.pass += b.pass;
        a.err += b.err;
    };
    for (script, res, _) in runs {
        add(&mut r.normal, &res.normal);
        add(&mut r.longtail, &res.longtail);
        add(&mut r.failure, &res.failure);
        for d in &res.details {
            r.details.push(Detail { name: format!("{script}: {}", d.name), ..d.clone() });
        }
    }
    let outcome = if runs.is_empty() || runs.iter().any(|x| x.2 == Outcome::CouldNotRun) {
        Outcome::CouldNotRun
    } else if runs.iter().any(|x| x.2 == Outcome::Fail) {
        Outcome::Fail
    } else {
        Outcome::Pass
    };
    r.overall_success = outcome == Outcome::Pass;
    (r, outcome)
}

/// Every test node file under the topdir (git-ignore aware).
pub fn test_node_files(topdir: &Path) -> Vec<PathBuf> {
    crate::walk::node_files(topdir, &[topdir.to_path_buf()])
        .into_iter()
        .filter(|p| nodefile::type_of(&p.to_string_lossy()) == Some(NodeType::Test))
        .collect()
}

/// Read + parse one test node file.
pub fn load_test_node(topdir: &Path, file: &Path) -> Result<NodeDoc, String> {
    let stored = crate::walk::topdir_path(topdir, file).ok_or_else(|| format!("{} is outside {}", file.display(), topdir.display()))?;
    if nodefile::type_of(&stored) != Some(NodeType::Test) {
        return Err(format!("{} is not a test node (*.test.iter.md)", file.display()));
    }
    let text = std::fs::read_to_string(file).map_err(|e| format!("cannot read {}: {e}", file.display()))?;
    nodefile::parse_tolerant(&stored, &text).map(|(d, _)| d).map_err(|e| format!("{}: {e}", file.display()))
}

/// Find a test node by id (or a unique id suffix), `{topdir}/` path,
/// path relative to the topdir, name, or file stem.
pub fn find_test_node(topdir: &Path, needle: &str) -> Result<(PathBuf, NodeDoc), String> {
    let n = needle.trim();
    let direct = if Path::new(n).is_absolute() { PathBuf::from(n) } else { real(topdir, if n.starts_with("{topdir}") { n.to_string() } else { format!("{{topdir}}/{n}") }.as_str()) };
    if direct.is_file() {
        return load_test_node(topdir, &direct).map(|d| (direct, d));
    }
    let mut hits = Vec::new();
    for f in test_node_files(topdir) {
        let Ok(doc) = load_test_node(topdir, &f) else { continue };
        let stem = f.file_name().map(|x| x.to_string_lossy().trim_end_matches(".test.iter.md").to_string()).unwrap_or_default();
        if doc.id == n || (n.len() >= 8 && doc.id.ends_with(n)) || doc.name == n || stem == n {
            hits.push((f, doc));
        }
    }
    match hits.len() {
        0 => Err(format!("no test node matches {n:?} under {}", topdir.display())),
        1 => Ok(hits.remove(0)),
        k => Err(format!("{n:?} matches {k} test nodes: {}", hits.iter().map(|h| h.1.path.clone()).collect::<Vec<_>>().join(", "))),
    }
}

/// Where `ITER_TEST_OUT` points for a node: outside the checkout, one dir
/// per node id, emptied before every run.
pub fn test_out_dir(id: &str) -> PathBuf {
    let safe: String = id.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' { c } else { '_' }).collect();
    std::env::temp_dir().join("iter-testout").join(if safe.is_empty() { "unnamed".into() } else { safe })
}

/// Run a test node's scripts (optionally only those whose file name or
/// relative path equals / contains `filter`).
pub fn run_node(topdir: &Path, doc: &NodeDoc, filter: Option<&str>, timeout_min: u64) -> Result<NodeRun, String> {
    let all = scripts_of(topdir, doc);
    let rel = |p: &Path| p.strip_prefix(topdir).map(iter_core::platform::slash).unwrap_or_else(|_| iter_core::platform::slash(p));
    let picked: Vec<PathBuf> = all
        .into_iter()
        .filter(|p| filter.map(|f| { let r = rel(p); r == f || r.ends_with(&format!("/{f}")) || p.file_name().is_some_and(|n| n.to_string_lossy() == f) }).unwrap_or(true))
        .collect();
    if picked.is_empty() && filter.is_some() {
        return Err(format!("no script matching {:?} in test node {:?}", filter.unwrap_or(""), doc.name));
    }
    let cwd = real(topdir, &nodefile::dir_of(&doc.path));
    let out_dir = test_out_dir(&doc.id);
    let _ = std::fs::remove_dir_all(&out_dir);
    let out_dir = std::fs::create_dir_all(&out_dir).ok().map(|_| out_dir);
    let budget = Duration::from_secs(timeout_min.max(1) * 60);
    let started = Instant::now();
    let mut scripts = Vec::new();
    for script in &picked {
        let r = rel(script);
        let remaining = budget.saturating_sub(started.elapsed());
        let run = if remaining.is_zero() {
            not_run(&r, &doc.name, &doc.id, format!("not run: the node's budget ({timeout_min} min) was used up"))
        } else if !script.is_file() {
            not_run(&r, &doc.name, &doc.id, format!("script not found: {r}"))
        } else {
            let env = vec![
                ("ITER_TEST_NODE_ID".to_string(), doc.id.clone()),
                ("ITER_TEST_NODE_NAME".to_string(), doc.name.clone()),
                ("ITER_TOPDIR".to_string(), topdir.to_string_lossy().into_owned()),
                ("ITER_TESTS_SHARED".to_string(), topdir.join(".iter/tests").to_string_lossy().into_owned()),
            ];
            let (code, stdout, stderr, timed_out) = run_script(script, &cwd, remaining, out_dir.as_deref(), &env);
            let detail = if timed_out { format!("timed out (node budget {timeout_min} min); killed") } else { String::new() };
            let exit = if timed_out { None } else { Some(code) };
            let (result, outcome) = testresult::evaluate(&stdout, exit, &doc.name, &doc.id);
            ScriptRun { log: log_body(&r, code, &stdout, &stderr, &detail), script: r, result, outcome, exit_code: if timed_out { -1 } else { code }, detail }
        };
        scripts.push(run);
    }
    let triples: Vec<(String, TestResult, Outcome)> = scripts.iter().map(|s| (s.script.clone(), s.result.clone(), s.outcome)).collect();
    let (result, outcome) = aggregate(&doc.name, &doc.id, &triples);
    Ok(NodeRun { id: doc.id.clone(), name: doc.name.clone(), path: doc.path.clone(), scripts, result, outcome, full_run: filter.is_none() })
}

fn not_run(script: &str, name: &str, id: &str, detail: String) -> ScriptRun {
    let (result, outcome) = testresult::evaluate("", None, name, id);
    ScriptRun { script: script.to_string(), result, outcome, exit_code: -1, log: log_body(script, -1, "", "", &detail), detail }
}

/// `bash <script>` in `cwd`, leading its own process group so a timeout kill
/// takes the whole tree. (exit code, stdout, stderr, timed_out); a spawn
/// failure is exit -1 with the error on stderr.
pub fn run_script(script: &Path, cwd: &Path, timeout: Duration, out_dir: Option<&Path>, env: &[(String, String)]) -> (i32, String, String, bool) {
    let mut cmd = iter_core::platform::bash();
    cmd.arg(script).current_dir(cwd).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    if let Some(out) = out_dir {
        cmd.env("ITER_TEST_OUT", out);
    }
    for (k, v) in env {
        cmd.env(k, v);
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }
    let child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => return (-1, String::new(), format!("cannot spawn bash {}: {e}", script.display()), false),
    };
    let pid = child.id();
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(child.wait_with_output());
    });
    match rx.recv_timeout(timeout) {
        Ok(Ok(o)) => (
            o.status.code().unwrap_or(-1),
            String::from_utf8_lossy(&o.stdout).into_owned(),
            String::from_utf8_lossy(&o.stderr).into_owned(),
            false,
        ),
        Ok(Err(e)) => (-1, String::new(), format!("wait failed: {e}"), false),
        Err(_) => {
            #[cfg(unix)]
            let _ = std::process::Command::new("sh").arg("-c").arg(format!("kill -9 -{pid} 2>/dev/null || kill -9 {pid}")).status();
            #[cfg(not(unix))]
            iter_core::platform::kill_tree(pid);
            // the reader thread ends once the group is gone
            let out = rx.recv_timeout(Duration::from_secs(5)).ok().and_then(|r| r.ok());
            let (so, se) = out.map(|o| (String::from_utf8_lossy(&o.stdout).into_owned(), String::from_utf8_lossy(&o.stderr).into_owned())).unwrap_or_default();
            (-1, so, se, true)
        }
    }
}

fn log_body(script: &str, exit_code: i32, stdout: &str, stderr: &str, detail: &str) -> String {
    let mut body = format!("# iter test run\n# script: {script}\n# time: {}\n# exit: {exit_code}\n", crate::now_iso());
    if !detail.is_empty() {
        body.push_str(&format!("# note: {detail}\n"));
    }
    body.push_str("\n--- stdout ---\n");
    body.push_str(stdout);
    body.push_str("\n--- stderr ---\n");
    body.push_str(stderr);
    body
}

pub const LOG_DETAIL_PER_SCRIPT_BYTES: usize = 8 * 1024;
pub const LOG_DETAIL_TOTAL_BYTES: usize = 64 * 1024;

fn counts(r: &TestResult) -> String {
    let t = r.totals();
    format!("{}/{}", t.pass, t.total)
}

/// The body-free summary of one run (the work item's `log_header` row).
pub fn log_header(run: &NodeRun, when: &str) -> String {
    let mut s = format!(
        "Test run {when} — test node \"{}\" in {}\nresult: {}  pass {}{}\n",
        run.name,
        run.path.trim_start_matches("{topdir}/"),
        outcome_word(run.outcome).to_uppercase(),
        counts(&run.result),
        if run.full_run { "" } else { "  (filtered run; the node's recorded result is not updated)" }
    );
    for t in &run.scripts {
        s.push_str(&format!(
            "- {:<5} {} {}{}\n",
            match t.outcome {
                Outcome::Pass => "pass",
                Outcome::Fail => "FAIL",
                Outcome::CouldNotRun => "ERROR",
            },
            t.script,
            counts(&t.result),
            if t.detail.is_empty() { String::new() } else { format!(" — {}", t.detail) }
        ));
    }
    if run.scripts.is_empty() {
        s.push_str("- (no scripts: children.tests matches nothing)\n");
    }
    s
}

/// The failing / erroring scripts' logs, tail-capped (the `log_detail` row);
/// empty when everything passed.
pub fn log_detail(run: &NodeRun) -> String {
    let mut s = String::new();
    let bad: Vec<&ScriptRun> = run.scripts.iter().filter(|t| t.outcome != Outcome::Pass).collect();
    for t in &bad {
        let block = format!("## {} exit {}\n{}\n\n", t.script, t.exit_code, tail_bytes(&t.log, LOG_DETAIL_PER_SCRIPT_BYTES).trim_end());
        if s.len() + block.len() > LOG_DETAIL_TOTAL_BYTES {
            s.push_str(&format!("## … more failing scripts omitted (row cap {} KB)\n", LOG_DETAIL_TOTAL_BYTES / 1024));
            break;
        }
        s.push_str(&block);
    }
    s
}

/// The last `max` bytes of a text (char-boundary safe).
pub fn tail_bytes(text: &str, max: usize) -> String {
    if text.len() <= max {
        return text.to_string();
    }
    let mut start = text.len() - max;
    while !text.is_char_boundary(start) {
        start += 1;
    }
    format!("… (first {start} bytes omitted)\n{}", &text[start..])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup(name: &str, scripts: &[(&str, &str)]) -> (PathBuf, NodeDoc) {
        let top = std::env::temp_dir().join(format!("iter_local_runner5_{name}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&top);
        std::fs::create_dir_all(top.join("comp/tests")).unwrap();
        for (f, body) in scripts {
            std::fs::write(top.join("comp/tests").join(f), body).unwrap();
        }
        let mut doc = NodeDoc::new(NodeType::Test, "comp tests", "t", "2026-10-02 00:00:00Z");
        doc.path = "{topdir}/comp/comp.test.iter.md".into();
        std::fs::write(top.join("comp/comp.test.iter.md"), nodefile::render(&doc)).unwrap();
        (iter_core::platform::canonicalize(&top).unwrap(), doc)
    }

    const STD_OK: &str = r#"echo '{"overall_success":true,"normal":{"total":3,"pass":3,"err":0},"longtail":{"total":1,"pass":1,"err":0},"failure":{"total":0,"pass":0,"err":0},"details":[{"name":"t1","bucket":"normal","pass":true,"msg":""}]}'"#;

    #[test]
    fn default_glob_finds_scripts_and_aggregates_standard_json() {
        let (top, doc) = setup("std", &[("comp01.sh", STD_OK), ("comp02.sh", "echo noise\necho 'ITER_RESULT pass=2 fail=0 total=2'\n"), ("other.sh", "exit 1")]);
        let s = scripts_of(&top, &doc);
        assert_eq!(s.len(), 2, "default glob is {{thisfilestem}}*.sh: {s:?}");
        let run = run_node(&top, &doc, None, 1).unwrap();
        assert_eq!(run.outcome, Outcome::Pass);
        assert!(run.result.overall_success);
        assert_eq!(run.result.normal, Bucket { total: 5, pass: 5, err: 0 }, "legacy maps to normal and sums");
        assert_eq!(run.result.longtail.total, 1);
        assert_eq!(run.result.id, doc.id);
        assert_eq!(run.result.name, "comp tests");
        assert_eq!(run.result.details[0].name, "comp/tests/comp01.sh: t1");
        let parsed = testresult::parse_last_line(&run.result.to_line()).unwrap();
        assert_eq!(parsed, run.result, "the aggregate is itself a standard result line");
    }

    #[test]
    fn fail_and_could_not_run() {
        let (top, doc) = setup("codes", &[("comp01.sh", STD_OK), ("comp02.sh", "echo 'ITER_RESULT pass=1 fail=1 total=2'\nexit 1\n")]);
        let run = run_node(&top, &doc, None, 1).unwrap();
        assert_eq!(run.outcome, Outcome::Fail);
        assert!(!run.result.overall_success);
        assert_eq!(run.result.normal, Bucket { total: 5, pass: 4, err: 1 });
        assert!(log_detail(&run).contains("comp02.sh exit 1"));
        assert!(log_header(&run, "now").contains("FAIL  comp/tests/comp02.sh 1/2"));
        std::fs::write(top.join("comp/tests/comp03.sh"), "exit 7").unwrap();
        let run = run_node(&top, &doc, None, 1).unwrap();
        assert_eq!(run.outcome, Outcome::CouldNotRun, "could-not-run trumps fail");
        let one = run_node(&top, &doc, Some("comp01.sh"), 1).unwrap();
        assert_eq!((one.outcome, one.scripts.len(), one.full_run), (Outcome::Pass, 1, false));
        assert!(run_node(&top, &doc, Some("nope.sh"), 1).is_err());
    }

    #[test]
    fn exit_zero_with_red_json_is_a_fail_and_env_is_set() {
        let (top, doc) = setup("env", &[(
            "comp01.sh",
            r#"test -n "$ITER_TEST_OUT" && test -d "$ITER_TEST_OUT" || exit 3
test "$ITER_TEST_NODE_ID" = "$EXPECT" 2>/dev/null || true
echo "{\"id\":\"$ITER_TEST_NODE_ID\",\"normal\":{\"total\":2,\"pass\":1,\"err\":1}}""#,
        )]);
        let run = run_node(&top, &doc, None, 1).unwrap();
        assert_eq!(run.outcome, Outcome::Fail);
        assert_eq!(run.scripts[0].result.id, doc.id, "the script saw ITER_TEST_NODE_ID");
    }

    #[test]
    fn timeout_kills_the_group_and_counts_as_could_not_run() {
        let (top, doc) = setup("timeout", &[("comp01.sh", "sleep 30 & sleep 30\n")]);
        let t = Instant::now();
        let (code, _, _, timed_out) = run_script(&top.join("comp/tests/comp01.sh"), &top, Duration::from_millis(300), None, &[]);
        assert!(timed_out && code == -1);
        assert!(t.elapsed() < Duration::from_secs(10));
        let _ = doc;
    }

    #[test]
    fn no_scripts_is_could_not_run_and_dirs_expand() {
        let (top, mut doc) = setup("dirs", &[]);
        let run = run_node(&top, &doc, None, 1).unwrap();
        assert_eq!(run.outcome, Outcome::CouldNotRun);
        std::fs::create_dir_all(top.join("comp/tests/deep")).unwrap();
        std::fs::write(top.join("comp/tests/deep/a.sh"), "exit 0").unwrap();
        doc.children.tests = vec!["tests/".into(), "{thisfiledir}/missing.sh".into()];
        let s = scripts_of(&top, &doc);
        assert_eq!(s.len(), 2);
        let run = run_node(&top, &doc, None, 1).unwrap();
        assert_eq!(run.outcome, Outcome::CouldNotRun, "the missing script could not run");
        assert!(run.scripts.iter().any(|x| x.detail.contains("script not found")));
    }

    #[test]
    fn find_by_id_name_stem_path() {
        let (top, doc) = setup("find", &[]);
        for n in [doc.id.as_str(), "comp tests", "comp", "comp/comp.test.iter.md", "{topdir}/comp/comp.test.iter.md"] {
            assert_eq!(find_test_node(&top, n).unwrap().1.id, doc.id, "{n}");
        }
        assert!(find_test_node(&top, "nothing").is_err());
    }
}
