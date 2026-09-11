//! `iter <verb>` for agents (decided 2026-09-04): the same logical verbs V2
//! gave agents (add, ask, reject, critreview, status, plus `doc` and
//! `capability`), backed by iter_data, PLUS the local-file verbs (runtests,
//! validate, markers, teststate, usecase) served natively by `iter_local`
//! since 2026-09-04 — nothing here calls or needs the V2 binary.  Agents ask
//! for the logical thing; this does the deterministic work.
//!
//! Environment (set by the engine for every agent session): ITER_DATA_URL,
//! ITER_ENGINE_TOKEN, ITER_PROJECT (name), ITER_WORKID, ITER_AGENT, ITER_TOPDIR,
//! ITER_MAINFILE.  The local-file verbs also work from a plain shell inside the
//! checkout (no engine, no server): --project may then be a path.

use crate::client::Api;
use clap::{Args, Subcommand};
use serde_json::{Value, json};

#[derive(Args, Debug)]
pub struct CliArgs {
    /// project name (defaults to $ITER_PROJECT; a V2-style path is ignored)
    #[arg(long, global = true)]
    project: Option<String>,
    #[command(subcommand)]
    verb: Verb,
}

#[derive(Subcommand, Debug)]
enum Verb {
    /// Create a work item (child of $ITER_WORKID when run inside an agent).
    /// Either --file <json> (V2 or V3 field names) or the flags below.
    Add {
        /// JSON file describing the item (title/name, type/agent, mainwork/request,
        /// codepath[s]/lockdirs, depends_on/blockedby, context, priority, model, question)
        #[arg(long)]
        file: Option<String>,
        /// agent type (code | plan | testwriter | …)
        #[arg(long = "type", alias = "agent")]
        item_type: Option<String>,
        #[arg(long, alias = "name")]
        title: Option<String>,
        /// the request text; @path reads a file
        #[arg(long, alias = "request")]
        mainwork: Option<String>,
        /// lock scope (repeatable); absolute, {topdir}-relative or repo-relative
        #[arg(long)]
        codepath: Vec<String>,
        /// 0–99, lower = sooner. Items created inside an agent INHERIT the creating
        /// item's number and ignore this; a root with no number gets an unused one in its band
        #[arg(long)]
        priority: Option<i64>,
        /// this item waits for the named item (id or unique suffix) AND everything it created (repeatable)
        #[arg(long = "depends-on")]
        depends_on: Vec<String>,
        /// wait for the named items' own completion only
        #[arg(long = "depends-on-shallow", default_value_t = false)]
        depends_on_shallow: bool,
        /// context file patterns for the new item (repeatable)
        #[arg(long)]
        context: Vec<String>,
        /// model override: opus | sonnet | haiku | fable
        #[arg(long)]
        model: Option<String>,
        /// raise it as a QUESTION for the human instead of runnable work
        #[arg(long)]
        question: Option<String>,
        /// tag (repeatable): text or text:#hex
        #[arg(long)]
        tag: Vec<String>,
        /// the usecase this item serves — becomes the engine-owned tag `usecase:<name>`,
        /// inherited by everything the item creates (children inherit it automatically)
        #[arg(long)]
        usecase: Option<String>,
        /// accepted for V2 compatibility; ignored
        #[arg(long, hide = true)]
        risk: Option<i64>,
        #[arg(long = "source-testgroup", hide = true)]
        source_testgroup: Option<String>,
        #[arg(long, hide = true)]
        automation: Option<String>,
        #[arg(long = "codepath-ignore", hide = true)]
        codepath_ignore: Vec<String>,
    },
    /// Ask the human a question from inside a running work item: the CALLING
    /// item moves to `question` when this turn ends and queues again once answered.
    Ask {
        #[arg(long)]
        question: Option<String>,
        /// read the question from a file (for anything multi-paragraph)
        #[arg(long)]
        file: Option<String>,
    },
    /// Reject the CALLING work item as invalid: it moves to `parked` with the
    /// reason recorded so a human re-evaluates; no retries are burned.
    Reject {
        #[arg(long)]
        reason: String,
    },
    /// Block the CALLING work item on the nightly cluster restart: it parks
    /// tagged `blocked-by-cluster-restart` with the attempt counter put back
    /// (no retry burned); the engine requeues it once the cluster is back up
    /// and healthy and strips the tag when it starts.
    Block {
        /// the block kind — the cluster's 02:00–06:00 PT restart window (the only kind today)
        #[arg(long = "cluster-restart")]
        cluster_restart: bool,
        /// what you needed the cluster for — the next run reads it back as its "previous attempt"
        #[arg(long)]
        reason: Option<String>,
    },
    /// Declare that the CALLING work item cannot finish until other items land:
    /// links them as dependencies of this item (deep: they and everything they
    /// create must close complete). When your turn then ends with NOT DONE lines,
    /// the close gate queues this item behind them — no bounce, no human question —
    /// and the engine re-runs it once they close.
    Wait {
        /// an item id or unique suffix this item must wait for (repeatable)
        #[arg(long = "on", required = true)]
        on: Vec<String>,
        /// what those items must land — recorded on this item
        #[arg(long)]
        reason: Option<String>,
    },
    /// Append a "doc" note to a work item (the calling one by default; works on closed items).
    Doc {
        text: Option<String>,
        #[arg(long)]
        file: Option<String>,
        /// another item's id or unique suffix
        #[arg(long)]
        id: Option<String>,
    },
    /// Synchronous critical review by the `_critic` persona; prints its
    /// feedback and records the round as a "review" row. Run again with
    /// --disposition to report what you did with it.
    Critreview {
        /// the material to review (plan text, change summary, …)
        #[arg(long)]
        file: Option<String>,
        /// context file the critic should also read (repeatable)
        #[arg(long)]
        context: Vec<String>,
        #[arg(long = "max-retry", default_value_t = 1)]
        max_retry: u32,
        /// revised | rejected | no-findings
        #[arg(long)]
        disposition: Option<String>,
        /// which round --disposition refers to (default: latest)
        #[arg(long)]
        round: Option<i64>,
    },
    /// Read a capability doc (no name: list them).
    Capability {
        name: Option<String>,
    },
    /// Open work for this project, run-order first.
    Status,
    /// Run a testgroup's scripts (the deterministic TDD runner). Neutral by
    /// default; --broken / --fixed make a claim the engine records and gates on.
    Runtests {
        /// testgroup label (as in the file's `iterapp:testgroups` block)
        #[arg(long)]
        group: String,
        /// narrow a NEUTRAL run to one test id/script (claims always run the whole group)
        #[arg(long)]
        test: Option<String>,
        /// claim "the defect is still present": a fully green group means the calling item is stale (parked)
        #[arg(long)]
        broken: bool,
        /// claim "the defect is resolved" (completion gate): any red or error means the item cannot close
        #[arg(long)]
        fixed: bool,
        /// wall-clock budget (minutes) for the group's scripts; overrun = killed → error
        #[arg(long = "timeout-min", default_value_t = iter_local::runtests::DEFAULT_GROUP_TIMEOUT_MIN)]
        timeout_min: u64,
    },
    /// Validate *.iter.md files (every one under the scan roots, or --file one).
    Validate {
        #[arg(long)]
        file: Option<String>,
        /// apply the safe corrections in place
        #[arg(long)]
        fix: bool,
        /// with --file: print the authoritative empty template for that file's role instead
        #[arg(long)]
        template: bool,
    },
    /// The structureV2 scan (nodes, use-cases, interfaces, testgroups) as JSON.
    Markers,
    /// The Test Loop gate: park / re-enter objects, or --list every object with its effective state.
    Teststate {
        #[arg(long)]
        omit: Vec<String>,
        #[arg(long)]
        include: Vec<String>,
        #[arg(long)]
        block: Vec<String>,
        #[arg(long)]
        clear: Vec<String>,
        #[arg(long)]
        list: bool,
    },
    /// Edit a use-case file's code node links (children.codenodes).
    Usecase {
        /// the *.usecase.iter.md file
        #[arg(long)]
        file: String,
        #[arg(long)]
        add: Vec<String>,
        #[arg(long)]
        remove: Vec<String>,
        #[arg(long)]
        list: bool,
    },
    /// Anything else: named so the message says what happened to the V2-only verbs.
    #[command(external_subcommand)]
    Other(Vec<String>),
}

struct Env {
    api: Api,
    project: String,
    workid: String,
    agent: String,
    topdir: String,
}

fn env(args: &CliArgs) -> Env {
    let url = std::env::var("ITER_DATA_URL").unwrap_or_default();
    let token = std::env::var("ITER_ENGINE_TOKEN").unwrap_or_default();
    if url.is_empty() || token.is_empty() {
        eprintln!("iter: ITER_DATA_URL / ITER_ENGINE_TOKEN are not set — this verb only works inside an engine-run work item");
        std::process::exit(2);
    }
    let mut project = std::env::var("ITER_PROJECT").unwrap_or_default();
    if let Some(p) = &args.project {
        if project.is_empty() && !p.contains('/') {
            project = p.clone();
        }
    }
    if project.is_empty() {
        eprintln!("iter: ITER_PROJECT is not set");
        std::process::exit(2);
    }
    Env {
        api: Api::new(&url, &token),
        project,
        workid: std::env::var("ITER_WORKID").unwrap_or_default(),
        agent: std::env::var("ITER_AGENT").unwrap_or_default(),
        topdir: std::env::var("ITER_TOPDIR").unwrap_or_default(),
    }
}

fn die(msg: String) -> ! {
    eprintln!("iter: {msg}");
    std::process::exit(1)
}

fn read_arg_or_file(text: Option<String>, file: Option<String>) -> String {
    if let Some(t) = text {
        if let Some(path) = t.strip_prefix('@') {
            return std::fs::read_to_string(path).unwrap_or_else(|e| die(format!("cannot read {path}: {e}")));
        }
        return t;
    }
    if let Some(path) = file {
        if path == "-" {
            use std::io::Read;
            let mut s = String::new();
            std::io::stdin().read_to_string(&mut s).ok();
            return s;
        }
        return std::fs::read_to_string(&path).unwrap_or_else(|e| die(format!("cannot read {path}: {e}")));
    }
    String::new()
}

fn items(e: &Env) -> Vec<Value> {
    e.api.get(&format!("/api/projects/{}/workitems", e.project)).ok().and_then(|v| v.as_array().cloned()).unwrap_or_default()
}

/// A workid or any unambiguous suffix (V2 convention: the last 12 chars).
fn resolve_id(e: &Env, all: &[Value], needle: &str) -> String {
    let n = needle.trim();
    let hits: Vec<String> = all
        .iter()
        .filter_map(|i| i.get("id").and_then(|x| x.as_str()))
        .filter(|id| *id == n || id.ends_with(n) || id.starts_with(n) || id.replace('-', "").ends_with(&n.replace('-', "")))
        .map(String::from)
        .collect();
    match hits.len() {
        1 => hits[0].clone(),
        0 => die(format!("no work item in '{}' matches '{n}'", e.project)),
        k => die(format!("'{n}' is ambiguous ({k} matches) — use more of the id")),
    }
}

/// absolute / {topdir}-relative / repo-relative -> "{topdir}/…"
fn lockdir(p: &str, topdir: &str) -> String {
    let top = topdir.trim_end_matches('/');
    let p = p.trim();
    if p.starts_with("{topdir}") {
        return p.to_string();
    }
    if !top.is_empty() {
        if p == top {
            return "{topdir}/".into();
        }
        if let Some(rest) = p.strip_prefix(top) {
            if rest.starts_with('/') {
                return format!("{{topdir}}{rest}");
            }
        }
    }
    if p.starts_with('/') || p.starts_with('~') {
        return p.to_string();
    }
    format!("{{topdir}}/{}", p.trim_start_matches("./"))
}

fn question_widget(question: &str) -> Value {
    let title: String = question.lines().find(|l| !l.trim().is_empty()).unwrap_or("Question").chars().take(150).collect();
    json!({
        "title": title,
        "summary": "",
        "detail": question,
        "fields": [{"key": "answer", "label": "Answer", "type": "text", "value": ""}]
    })
}

pub fn run(args: CliArgs) {
    match args.verb {
        Verb::Capability { ref name } => capability(&env(&args), name.clone()),
        Verb::Status => status(&env(&args)),
        Verb::Other(ref rest) => retired(rest),
        // local-file verbs: no server needed (claims are recorded when inside a run)
        Verb::Runtests { ref group, ref test, broken, fixed, timeout_min } => {
            std::process::exit(local::runtests(&topdir_of(&args), group, test.as_deref(), broken, fixed, timeout_min))
        }
        Verb::Validate { ref file, fix, template } => std::process::exit(local::validate(&topdir_of(&args), file.as_deref(), fix, template)),
        Verb::Markers => std::process::exit(local::markers(&topdir_of(&args))),
        Verb::Teststate { ref omit, ref include, ref block, ref clear, list } => {
            std::process::exit(local::teststate(&topdir_of(&args), omit, include, block, clear, list))
        }
        Verb::Usecase { ref file, ref add, ref remove, list } => std::process::exit(local::usecase(&topdir_of(&args), file, add, remove, list)),
        _ => {}
    }
    let e = env(&args);
    match args.verb {
        Verb::Add { file, item_type, title, mainwork, codepath, priority, depends_on, depends_on_shallow, context, model, question, tag, usecase, .. } => {
            add(&e, file, item_type, title, mainwork, codepath, priority, depends_on, depends_on_shallow, context, model, question, tag, usecase)
        }
        Verb::Ask { question, file } => ask(&e, read_arg_or_file(question, file)),
        Verb::Reject { reason } => reject(&e, &reason),
        Verb::Block { cluster_restart, reason } => block(&e, cluster_restart, reason),
        Verb::Wait { on, reason } => wait(&e, on, reason),
        Verb::Doc { text, file, id } => doc(&e, read_arg_or_file(text, file), id),
        Verb::Critreview { file, context, max_retry, disposition, round } => critreview(&e, file, context, max_retry, disposition, round),
        Verb::Capability { .. } | Verb::Status | Verb::Other(_) | Verb::Runtests { .. } | Verb::Validate { .. }
        | Verb::Markers | Verb::Teststate { .. } | Verb::Usecase { .. } => {}
        #[allow(unreachable_patterns)]
        _ => {}
    }
}

/// The checkout the local-file verbs work on: $ITER_TOPDIR inside a run; a
/// path given as --project; else the git root of the current directory (or
/// the current directory itself).
fn topdir_of(args: &CliArgs) -> std::path::PathBuf {
    if let Ok(t) = std::env::var("ITER_TOPDIR") {
        if !t.trim().is_empty() {
            return std::path::PathBuf::from(t.trim());
        }
    }
    if let Some(p) = &args.project {
        if p.contains('/') || p == "." {
            return std::path::PathBuf::from(p);
        }
    }
    let cwd = std::env::current_dir().unwrap_or_else(|_| ".".into());
    std::process::Command::new("git")
        .args(["rev-parse", "--show-toplevel"])
        .current_dir(&cwd)
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| std::path::PathBuf::from(String::from_utf8_lossy(&o.stdout).trim()))
        .unwrap_or(cwd)
}

/// The V2-only verbs (testsweep, orphans, resolve, …) are gone with the V2
/// binary; say so instead of "unknown subcommand".
fn retired(rest: &[String]) -> ! {
    let verb = rest.first().cloned().unwrap_or_default();
    die(format!(
        "`iter {verb}` is not a V3 verb. V3 serves runtests, validate, markers, teststate and usecase itself; \
         the V2-only verbs (testsweep, orphans, resolve) were retired with the V2 binary on 2026-09-04."
    ))
}

/// The local-file verbs, ported from V2's main.rs; the API is touched only to
/// record a claim on the calling work item.
mod local {
    use super::{Api, die};
    use iter_local::{markers, project, runtests as rt, testgroups, validate as val};
    use serde_json::json;
    use std::path::{Path, PathBuf};

    /// Record a claim row on the calling item when inside a run (no-op from a shell).
    /// A false --broken claim also parks the item (stale), like `iter reject`.
    /// Detail rows for a run + the optional fix item (decided 2026-09-08).
    /// Silent outside an engine-run work item (no ITER_DATA_URL/ITER_WORKID).
    fn report_run(topdir: &Path, tg_file: &Path, tg_rel: &str, run: &rt::GroupRunResult, header: &str, broken_claim: bool, group_auto_fix: bool) {
        let (Ok(url), Ok(token), Ok(project), Ok(workid)) = (
            std::env::var("ITER_DATA_URL"),
            std::env::var("ITER_ENGINE_TOKEN"),
            std::env::var("ITER_PROJECT"),
            std::env::var("ITER_WORKID"),
        ) else {
            return;
        };
        if url.is_empty() || token.is_empty() || project.is_empty() || workid.is_empty() {
            return;
        }
        let api = Api::new(&url, &token);
        let details = format!("/api/projects/{project}/workitems/{workid}/details");
        // one header (+ one detail) per testgroup per ATTEMPT (decided
        // 2026-09-10: an agent iterating on its tests logged 16 runs / 31 rows
        // on one item): a run of the same group during this attempt
        // overwrites the previous run's rows instead of appending
        let since = api
            .get(&format!("/api/projects/{project}/workitems/{workid}"))
            .ok()
            .and_then(|i| i.get("ts").and_then(|t| t.get("start")).and_then(|s| s.as_str()).map(String::from))
            .unwrap_or_default();
        let existing: Vec<serde_json::Value> = api.get(&details).ok().and_then(|v| v.as_array().cloned()).unwrap_or_default();
        let prior = prior_run_rows(&existing, &run.label, &since);
        let header_row = json!({"key": "log_header", "valuetype": "text", "value": header});
        match prior {
            Some((h, _)) => { let _ = api.put(&format!("{details}/{h}"), &header_row); }
            None => { let _ = api.post(&details, &header_row); }
        }
        let detail = if run.outcome == rt::Outcome::Green { String::new() } else { rt::log_detail(run) };
        match (prior, detail.is_empty()) {
            (Some((_, Some(d))), true) => {
                // the earlier run was red and this one is green: say so where the old output was
                let _ = api.put(&format!("{details}/{d}"), &json!({"key": "log_detail", "valuetype": "text",
                    "value": format!("(green on the latest run of testgroup \"{}\" at {}; the earlier failing output was replaced)", run.label, iter_core::now_utc())}));
            }
            (Some((_, Some(d))), false) => { let _ = api.put(&format!("{details}/{d}"), &json!({"key": "log_detail", "valuetype": "text", "value": detail})); }
            (_, false) => { let _ = api.post(&details, &json!({"key": "log_detail", "valuetype": "text", "value": detail})); }
            (_, true) => {}
        }
        if run.outcome == rt::Outcome::Green {
            return;
        }
        // fix items: only from full runs that are NOT an agent iterating on its
        // own code (a code/testwriter session sees red on purpose), and never
        // from a --broken claim (the red IS the expected reproduction)
        let agent = std::env::var("ITER_AGENT").unwrap_or_default();
        let iterating_agent = !agent.is_empty() && agent != "exec";
        if !run.full_run || broken_claim || iterating_agent {
            return;
        }
        let project_row: serde_json::Value = api.get(&format!("/api/projects/{project}")).unwrap_or(json!({}));
        let enabled = group_auto_fix || project_row.get("fix_on_test_failure").and_then(|b| b.as_bool()).unwrap_or(false);
        if !enabled {
            return;
        }
        let title = format!("Tests non-green: testgroup \"{}\" {}/{} in {}", run.label, run.pass, run.total, tg_rel);
        // one open fix item per testgroup: the `check:` + `container:` key
        // (iter_core::dedup, 2026-09-10) lets iter_data refuse the repeat and
        // book it on the open item — the old exact-title match is retired
        let key_tags = json!([
            {"text": format!("{}tests-non-green", iter_core::dedup::CHECK_TAG_PREFIX), "color": ""},
            {"text": format!("{}{}", iter_core::dedup::CONTAINER_TAG_PREFIX, run.label), "color": ""},
        ]);
        // the object under test: the parent of the nearest `tests` ancestor
        let object_dir = tg_file
            .ancestors()
            .find(|d| d.file_name().map(|n| n == "tests").unwrap_or(false))
            .and_then(|t| t.parent())
            .unwrap_or_else(|| tg_file.parent().unwrap_or(topdir))
            .to_path_buf();
        let object_rel = object_dir.strip_prefix(topdir).map(|p| p.to_string_lossy().into_owned()).unwrap_or_default();
        let lockdir = if object_rel.is_empty() { "{topdir}".to_string() } else { format!("{{topdir}}/{object_rel}") };
        let failing: Vec<String> = run.runs.iter().filter(|t| t.outcome != rt::Outcome::Green).map(|t| format!("{} ({})", t.id, t.name)).collect();
        let object = if object_rel.is_empty() { "the project".to_string() } else { object_rel.clone() };
        let request = format!(
            "The tests for {object} are not green: testgroup \"{label}\" in {tg_rel} finished {pass} of {total} passing on {when}. \
             This project files a fix item on every non-green run (fix_on_test_failure), so this item exists to find out why and put it right.\n\n\
             - Failing or erroring scripts: {failing}\n\
             - The full run summary is the \"log_header\" row and the failing scripts' output is the \"log_detail\" row on work item {workid}.\n\
             - Start by reproducing: `\"$ITER_BIN\" runtests --project \"$ITER_PROJECT\" --group \"{label}\" --broken`. If the group is green now the item is stale and the command parks it.\n\
             - Then fix the CODE the tests describe. If a test itself is wrong, say so explicitly in your output and fix the test instead.\n\
             - Finish with `--fixed` on the same group; the close gate needs an upheld --fixed claim.\n",
            label = run.label, pass = run.pass, total = run.total, when = iter_core::now_utc(), failing = failing.join(", "),
        );
        let requestedby = if agent.is_empty() { "user".to_string() } else { format!("agent:{agent}") };
        let body = json!({
            "name": title, "agent": "code", "state": "queued",
            "lockdirs": [lockdir], "blockedby": [], "context": [], "model": "", "tags": key_tags,
            "createdby": workid, "requestedby": requestedby, "prework": [], "postwork": [],
            "request": request,
        });
        match api.post(&format!("/api/projects/{project}/workitems"), &body) {
            Ok(created) if crate::cli::already_open(&created) => {
                let id = created.get("id").and_then(|i| i.as_str()).unwrap_or("").to_string();
                println!("fix item already open for testgroup \"{}\": {} (repeat #{} recorded on it, now P{}) — not filing another",
                    run.label, &id[..8.min(id.len())], created.get("repeats").and_then(|r| r.as_u64()).unwrap_or(0), created.get("priority").and_then(|p| p.as_i64()).unwrap_or(0));
            }
            Ok(created) => {
                let id = created.get("id").and_then(|i| i.as_str()).unwrap_or("").to_string();
                println!("filed fix item {} (code, P{}) for testgroup \"{}\"", &id[..8.min(id.len())], created.get("priority").and_then(|p| p.as_i64()).unwrap_or(0), run.label);
            }
            Err(e) => eprintln!("could not file the fix item: {e}"),
        }
    }

    /// The rows an earlier run of `label` wrote during THIS attempt (header
    /// rows at or after `since`, the attempt's start ts): the latest header's
    /// order, and the order of its detail row when that row sits right after
    /// it.  None = first run of this group this attempt (or no attempt ts).
    pub(crate) fn prior_run_rows(details: &[serde_json::Value], label: &str, since: &str) -> Option<(i64, Option<i64>)> {
        if since.is_empty() {
            return None;
        }
        fn key(d: &serde_json::Value) -> &str { d.get("key").and_then(|k| k.as_str()).unwrap_or("") }
        fn order(d: &serde_json::Value) -> i64 { d.get("order").and_then(|o| o.as_i64()).unwrap_or(-1) }
        let needle = format!("testgroup \"{label}\"");
        let header = details
            .iter()
            .filter(|d| key(d) == "log_header")
            .filter(|d| d.get("ts").and_then(|t| t.as_str()).map(|t| t >= since).unwrap_or(false))
            .filter(|d| d.get("value").and_then(|v| v.as_str()).map(|v| v.contains(&needle)).unwrap_or(false))
            .max_by_key(|d| order(d))?;
        let h = order(header);
        let detail = details.iter().find(|d| order(d) == h + 1 && key(d) == "log_detail").map(|d| order(d));
        Some((h, detail))
    }

    fn record_claim(claim: &str, group: &str, run: &rt::GroupRunResult, upheld: bool, park_reason: Option<&str>) {
        let (Ok(url), Ok(token), Ok(project), Ok(workid)) = (
            std::env::var("ITER_DATA_URL"),
            std::env::var("ITER_ENGINE_TOKEN"),
            std::env::var("ITER_PROJECT"),
            std::env::var("ITER_WORKID"),
        ) else {
            return;
        };
        if url.is_empty() || token.is_empty() || project.is_empty() || workid.is_empty() {
            return;
        }
        let api = Api::new(&url, &token);
        let details = format!("/api/projects/{project}/workitems/{workid}/details");
        let _ = api.post(&details, &json!({"key": "claim", "valuetype": "json", "value": {
            "claim": claim, "group": group, "upheld": upheld, "outcome": run.outcome.as_str(),
            "counts": format!("{}/{}", run.pass, run.total), "ts": iter_core::now_utc(),
        }}));
        if let Some(reason) = park_reason {
            if let Ok(mut item) = api.get(&format!("/api/projects/{project}/workitems/{workid}")) {
                let version = item.get("version").and_then(|v| v.as_u64()).unwrap_or(1);
                item["state"] = json!("parked");
                item["lasterror"] = json!(reason.chars().take(400).collect::<String>());
                let _ = api.put(&format!("/api/projects/{project}/workitems/{workid}?expect_version={version}"), &item);
                let _ = api.post(&details, &json!({"key": "doc", "valuetype": "text", "value": reason}));
            }
        }
    }

    pub fn runtests(topdir: &Path, group: &str, test: Option<&str>, broken: bool, fixed: bool, timeout_min: u64) -> i32 {
        if broken && fixed {
            eprintln!("error: --broken and --fixed are mutually exclusive claims");
            return 2;
        }
        if (broken || fixed) && test.is_some() {
            eprintln!("error: claims are group-level; --test narrows only neutral runs");
            return 2;
        }
        let (tg_file, group_def) = match rt::locate_group(topdir, group) {
            Ok(found) => found,
            Err(e) => {
                eprintln!("error: {e}");
                return 2;
            }
        };
        let run = match rt::run_group(&tg_file, group, test, timeout_min) {
            Ok(r) => r,
            Err(e) => {
                eprintln!("error: {e}");
                return 2;
            }
        };
        // Log Header on every run, Log Detail only when non-green, both on the
        // running work item (decided 2026-09-08: run logs live centrally, not
        // under the tree); a non-green FULL run may file a fix item
        let when = iter_core::now_utc();
        let tg_rel = tg_file.strip_prefix(topdir).unwrap_or(&tg_file).to_string_lossy().into_owned();
        let header = rt::log_header(&run, &tg_rel, &when);
        print!("{header}");
        report_run(topdir, &tg_file, &tg_rel, &run, &header, broken, group_def.auto_fix);
        let pct = if run.total > 0 { run.pass * 100 / run.total } else { 0 };
        println!(
            "tests {}/{} {}% — testgroup \"{}\" {}{}",
            run.pass, run.total, pct, run.label, run.outcome.as_str().to_uppercase(),
            if run.full_run { "" } else { " (filtered run — the group's recorded result is not updated)" }
        );
        if broken {
            return match run.outcome {
                rt::Outcome::Red => {
                    record_claim("broken", group, &run, true, None);
                    println!("CLAIM UPHELD (--broken): the defect reproduces; proceed with the fix.");
                    0
                }
                rt::Outcome::Green => {
                    let reason = format!("stale item: --broken claim failed — testgroup \"{group}\" is fully green ({}/{})", run.pass, run.total);
                    record_claim("broken", group, &run, false, Some(&reason));
                    println!(
                        "CLAIM FALSE (--broken): testgroup \"{group}\" is fully green — this work item is STALE and has been parked. \
                         STOP NOW: touch no code and end your work immediately."
                    );
                    3
                }
                rt::Outcome::Error => {
                    let reason = format!("--broken claim aborted: testgroup \"{group}\" has script errors — \"couldn't run\" must not pass for \"defect reproduces\"");
                    record_claim("broken", group, &run, false, Some(&reason));
                    println!(
                        "CLAIM ABORTED (--broken): script error(s) in testgroup \"{group}\" — the tests could not run, which is not \
                         the same as the defect reproducing. STOP NOW: the item has been parked."
                    );
                    3
                }
            };
        }
        if fixed {
            return match run.outcome {
                rt::Outcome::Green => {
                    record_claim("fixed", group, &run, true, None);
                    println!("CLAIM UPHELD (--fixed): testgroup \"{group}\" is fully green.");
                    0
                }
                _ => {
                    record_claim("fixed", group, &run, false, None);
                    println!(
                        "CLAIM FALSE (--fixed): testgroup \"{group}\" is {} — the work is NOT done. The close gate will not \
                         let this item complete until a --fixed claim is upheld; report what remains.",
                        run.outcome.as_str()
                    );
                    3
                }
            };
        }
        match run.outcome {
            rt::Outcome::Green => 0,
            rt::Outcome::Red => 1,
            rt::Outcome::Error => 2,
        }
    }

    pub fn validate(topdir: &Path, file: Option<&str>, fix: bool, template: bool) -> i32 {
        let file = file.map(|f| { let p = PathBuf::from(f); if p.is_absolute() { p } else { topdir.join(p) } });
        if template {
            let Some(f) = file else {
                eprintln!("error: --template needs --file <path> to pick the role from the filename");
                return 2;
            };
            return match val::template_for(&f) {
                Ok(t) => {
                    println!("{t}");
                    0
                }
                Err(e) => {
                    eprintln!("error: {e}");
                    2
                }
            };
        }
        let roots = project::scan_roots(topdir);
        let report = match val::run(&roots, file.as_deref(), fix) {
            Ok(r) => r,
            Err(e) => {
                eprintln!("error: {e}");
                return 2;
            }
        };
        for f in &report.findings {
            println!("{:5} {:24} {}{} — {}", format!("{:?}", f.severity).to_uppercase(), f.code, if f.fixed { "[FIXED] " } else { "" }, f.file, f.message);
        }
        let remaining = report.findings.iter().filter(|f| !f.fixed).count();
        println!(
            "validate: {} file(s) checked, {} finding(s){}{}",
            report.files_checked, report.findings.len(),
            if report.fixed > 0 { format!(", {} fixed", report.fixed) } else { String::new() },
            if remaining > 0 { format!(", {remaining} remaining") } else { String::new() }
        );
        match report.worst() {
            Some(val::Severity::Error) | Some(val::Severity::Warn) => 1,
            _ => 0,
        }
    }

    pub fn markers(topdir: &Path) -> i32 {
        let (_p, scan) = markers::scan_project(topdir);
        match serde_json::to_string_pretty(&scan) {
            Ok(j) => {
                println!("{j}");
                0
            }
            Err(e) => {
                eprintln!("error: cannot serialize scan: {e}");
                1
            }
        }
    }

    pub fn teststate(topdir: &Path, omit: &[String], include: &[String], block: &[String], clear: &[String], list: bool) -> i32 {
        let scan_now = || markers::scan_project(topdir).1;
        let edits: Vec<(markers::TestStateAction, &[String])> = vec![
            (markers::TestStateAction::Omit, omit),
            (markers::TestStateAction::Include, include),
            (markers::TestStateAction::Block, block),
            (markers::TestStateAction::Clear, clear),
        ];
        let mut edited = 0usize;
        for (action, refs) in edits {
            for target in refs {
                match markers::teststate_apply(&scan_now(), target, action, false) {
                    Ok(summary) => {
                        println!("{summary}");
                        edited += 1;
                    }
                    Err(e) => {
                        eprintln!("error: {e}");
                        if edited > 0 {
                            eprintln!("note: {edited} earlier edit(s) in this invocation were already applied");
                        }
                        return 2;
                    }
                }
            }
        }
        if list || edited == 0 {
            let scan = scan_now();
            println!("teststate (flag → effective):");
            for n in &scan.nodes {
                let flag = if n.teststate.trim().is_empty() { "-".to_string() } else { n.teststate.trim().to_string() };
                let eff = match markers::effective_teststate(n, &scan.nodes) {
                    markers::TestState::Included => "included".to_string(),
                    markers::TestState::Omitted { value, by } => format!("OMITTED ({value} via {by})"),
                };
                println!("  object    {:40} {:10} {:8} → {}", n.key, n.name, flag, eff);
            }
            for u in &scan.usecases {
                let flag = if u.teststate.trim().is_empty() { "-".to_string() } else { u.teststate.trim().to_string() };
                let eff = match markers::own_teststate(&u.teststate, &u.file) {
                    markers::TestState::Included => "included".to_string(),
                    markers::TestState::Omitted { value, .. } => format!("OMITTED ({value})"),
                };
                println!("  usecase   {:51} {:8} → {}", u.name, flag, eff);
            }
            for i in &scan.interfaces {
                let flag = if i.teststate.trim().is_empty() { "-".to_string() } else { i.teststate.trim().to_string() };
                let eff = match markers::own_teststate(&i.teststate, &i.file) {
                    markers::TestState::Included => "included".to_string(),
                    markers::TestState::Omitted { value, .. } => format!("OMITTED ({value})"),
                };
                println!("  interface {:51} {:8} → {}", i.id, flag, eff);
            }
        }
        0
    }

    pub fn usecase(topdir: &Path, file: &str, add: &[String], remove: &[String], list: bool) -> i32 {
        let p = PathBuf::from(file);
        let path = if p.is_absolute() { p } else { topdir.join(p) };
        let filename = path.file_name().map(|f| f.to_string_lossy().into_owned()).unwrap_or_default();
        if markers::role_of(&filename) != Some(markers::Role::Usecase) {
            eprintln!("error: {} is not a *.usecase.iter.md file (the filename declares the nodetype)", path.display());
            return 2;
        }
        let content = match std::fs::read_to_string(&path) {
            Ok(c) => c,
            Err(e) => {
                eprintln!("error: cannot read {}: {e}", path.display());
                return 2;
            }
        };
        let front = markers::parse_front(&content);
        let mut codenodes = front.child("codenodes").unwrap_or_default();
        for r in remove {
            let r = r.trim();
            codenodes.retain(|x| x != r);
        }
        for a in add {
            let a = a.trim();
            if !a.is_empty() && !codenodes.iter().any(|x| x == a) {
                codenodes.push(a.to_string());
            }
        }
        if !add.is_empty() || !remove.is_empty() {
            if let Err(e) = markers::set_children_key(&path, "codenodes", &codenodes) {
                eprintln!("error: {e}");
                return 1;
            }
            println!("{}: codenodes updated ({} entr{})", path.display(), codenodes.len(), if codenodes.len() == 1 { "y" } else { "ies" });
        }
        if list {
            for c in &codenodes {
                println!("{c}");
            }
        }
        if add.is_empty() && remove.is_empty() && !list {
            eprintln!("nothing to do: pass --add, --remove, and/or --list");
            return 2;
        }
        let _ = testgroups::BLOCK_START; // (keeps the import honest: testgroups is used by validate)
        let _ = die;
        0
    }
}

fn add(
    e: &Env,
    file: Option<String>,
    item_type: Option<String>,
    title: Option<String>,
    mainwork: Option<String>,
    codepath: Vec<String>,
    priority: Option<i64>,
    depends_on: Vec<String>,
    depends_on_shallow: bool,
    context: Vec<String>,
    model: Option<String>,
    question: Option<String>,
    tag: Vec<String>,
    usecase: Option<String>,
) {
    let s = |v: &Value, keys: &[&str]| -> String {
        keys.iter().find_map(|k| v.get(*k).and_then(|x| x.as_str()).map(String::from)).unwrap_or_default()
    };
    let arr = |v: &Value, keys: &[&str]| -> Vec<String> {
        keys.iter()
            .find_map(|k| v.get(*k).and_then(|x| x.as_array()))
            .map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect())
            .unwrap_or_default()
    };
    let f: Value = match &file {
        Some(path) => serde_json::from_str(&std::fs::read_to_string(path).unwrap_or_else(|e| die(format!("cannot read {path}: {e}"))))
            .unwrap_or_else(|e| die(format!("{path} is not valid json: {e}"))),
        None => json!({}),
    };
    let name = title.clone().unwrap_or_else(|| s(&f, &["title", "name"]));
    if name.trim().is_empty() {
        die("a title is required (--title or \"title\" in --file)".into());
    }
    let agent = item_type.clone().unwrap_or_else(|| s(&f, &["type", "agent"]));
    let agent = if agent.is_empty() { "code".to_string() } else { agent };
    let request = read_arg_or_file(mainwork.clone(), None);
    let request = if request.is_empty() { s(&f, &["mainwork", "request"]) } else { request };
    let mut lockdirs: Vec<String> = codepath.iter().map(|c| lockdir(c, &e.topdir)).collect();
    if lockdirs.is_empty() {
        let mut cps = arr(&f, &["codepaths", "lockdirs"]);
        let single = s(&f, &["codepath"]);
        if cps.is_empty() && !single.is_empty() {
            cps.push(single);
        }
        lockdirs = cps.iter().map(|c| lockdir(c, &e.topdir)).collect();
    }
    let all = items(e);
    let mut blockedby: Vec<String> = depends_on.iter().map(|d| resolve_id(e, &all, d)).collect();
    if blockedby.is_empty() {
        blockedby = arr(&f, &["depends_on", "blockedby"]).iter().map(|d| resolve_id(e, &all, d)).collect();
    }
    if blockedby.iter().any(|b| b == &e.workid) {
        die("an item cannot depend on the item that creates it".into());
    }
    let shallow = depends_on_shallow || f.get("depends_on_shallow").and_then(|b| b.as_bool()).unwrap_or(false) || f.get("blockedby_shallow").and_then(|b| b.as_bool()).unwrap_or(false);
    let ctx = if context.is_empty() { arr(&f, &["context"]) } else { context.clone() };
    // `@path` reads the question from a file, exactly like --mainwork (fixed
    // 2026-09-04: a question filed as "@/tmp/q.md" used to store that literal
    // string, and the file stayed behind on the engine's machine)
    let question = question
        .clone()
        .map(|q| read_arg_or_file(Some(q), None))
        .or_else(|| { let q = s(&f, &["question"]); if q.is_empty() { None } else { Some(read_arg_or_file(Some(q), None)) } })
        .filter(|q| !q.trim().is_empty());
    let model = model.clone().unwrap_or_else(|| s(&f, &["model"]));
    // decided 2026-09-08: no default here — iter_data inherits the creating
    // item's priority (and usecase tags) or places a root in its band
    let prio: Option<i64> = priority.or_else(|| f.get("priority").and_then(|p| p.as_i64()));
    let usecase = usecase.clone().or_else(|| { let u = s(&f, &["usecase"]); if u.is_empty() { None } else { Some(u) } });
    let mut tags: Vec<Value> = tag
        .iter()
        .map(|t| match t.rsplit_once(':') {
            Some((text, color)) if color.starts_with('#') => json!({"text": text.trim(), "color": color}),
            _ => json!({"text": t.trim(), "color": ""}),
        })
        .collect();
    if let Some(a) = f.get("tags").and_then(|t| t.as_array()) {
        tags.extend(a.iter().cloned());
    }

    // birth state: a question parks; otherwise the parent agent's childstate
    // (project override first), default queued
    let state = if question.is_some() {
        "question".to_string()
    } else {
        let project: Value = e.api.get(&format!("/api/projects/{}", e.project)).unwrap_or(json!({}));
        let over = project.get("agents").and_then(|a| a.get(&e.agent)).and_then(|o| o.get("childstate")).and_then(|c| c.as_str()).map(String::from);
        let def = e.api.get(&format!("/api/agents/{}", e.agent)).ok().and_then(|a| a.get("childstate").and_then(|c| c.as_str()).map(String::from));
        over.or(def).filter(|c| !c.is_empty()).unwrap_or_else(|| "queued".into())
    };
    let requestedby = if e.agent.is_empty() { "user".to_string() } else { format!("agent:{}", e.agent) };
    let body = json!({
        "name": name.trim(), "agent": agent, "state": state, "priority": prio,
        "lockdirs": lockdirs, "blockedby": blockedby, "blockedby_shallow": shallow,
        "context": ctx, "model": model, "tags": tags, "usecase": usecase,
        "createdby": if e.workid.is_empty() { requestedby.clone() } else { e.workid.clone() },
        "requestedby": requestedby, "prework": [], "postwork": [],
        // the request text rides in the create (2026-09-10): iter_data writes
        // detail row 0 from it, and a create refused as a repeat still leaves
        // what this item observed on the survivor
        "request": request,
    });
    // lock scope sanity the server cannot see: a codepath that is a strict
    // ancestor of several code nodes locks an AREA, not the directory one
    // item writes (2026-09-07: a plan item locked devops, core/repos and
    // infra/repos for an hour)
    for w in area_warnings(&e.topdir, &lockdirs) {
        eprintln!("iter add: warning: {w}");
    }
    // iter_data enforces the agent's lock shape: a refusal is a 400 naming the
    // rule and the overlap count; warnings ride back on the created record
    let created = e.api.post(&format!("/api/projects/{}/workitems", e.project), &body).unwrap_or_else(|err| {
        let msg = serde_json::from_str::<Value>(&err.body).ok().and_then(|v| v.get("error").and_then(|x| x.as_str()).map(String::from)).unwrap_or_else(|| err.to_string());
        die(format!("create {}", if msg.starts_with("refused") { msg } else { format!("failed: {msg}") }))
    });
    for w in created.get("warnings").and_then(|w| w.as_array()).into_iter().flatten().filter_map(|w| w.as_str()) {
        eprintln!("iter add: {w}");
    }
    let id = created.get("id").and_then(|i| i.as_str()).unwrap_or("").to_string();
    // stage 1 repeat detection (iter_core::dedup): the same `check:` +
    // `container:` tags as an OPEN item = no new row; the open item was told
    // (doc row with this request text, repeats, priority) and comes back
    if already_open(&created) {
        println!("{}", add_outcome_line(&created));
        return;
    }
    if let Some(q) = question {
        let _ = e.api.post(
            &format!("/api/projects/{}/workitems/{}/details", e.project, id),
            &json!({"key": "question", "valuetype": "json", "value": question_widget(&q)}),
        );
    }
    println!("{}", add_outcome_line(&created));
}

pub(crate) fn already_open(created: &Value) -> bool {
    created.get("already_open").and_then(|b| b.as_bool()).unwrap_or(false)
}

/// The one line `iter add` prints: "added <id> …" for a new row, or
/// "already open: <id> …" when iter_data refused a repeat (exit 0 either way —
/// the work exists, which is what the caller wanted).
fn add_outcome_line(created: &Value) -> String {
    let id = created.get("id").and_then(|i| i.as_str()).unwrap_or("");
    let tail = &id[id.len().saturating_sub(12)..];
    if already_open(created) {
        format!(
            "already open: {id} ({tail}) state={} P{} repeats={} — the same check and container is already filed; your request text was recorded on it. Do not file it again.",
            created.get("state").and_then(|s| s.as_str()).unwrap_or(""),
            created.get("priority").and_then(|p| p.as_i64()).unwrap_or(0),
            created.get("repeats").and_then(|r| r.as_u64()).unwrap_or(0),
        )
    } else {
        format!(
            "added {id} ({tail}) state={} agent={}",
            created.get("state").and_then(|s| s.as_str()).unwrap_or(""),
            created.get("agent").and_then(|a| a.as_str()).unwrap_or(""),
        )
    }
}

/// Warn for every lockdir that is a strict ancestor of two or more code
/// nodes' source directories (the structureV2 scan): that is an area lock.
/// Silent outside a checkout (no topdir) or when the scan finds nothing.
fn area_warnings(topdir: &str, lockdirs: &[String]) -> Vec<String> {
    if topdir.trim().is_empty() || !std::path::Path::new(topdir).is_dir() {
        return vec![];
    }
    let (_, scan) = iter_local::markers::scan_project(std::path::Path::new(topdir));
    let top = topdir.trim_end_matches('/');
    let mut out = Vec::new();
    for d in lockdirs {
        let abs = d.replace("{topdir}", top);
        let abs = abs.trim_end_matches('/');
        let mut covered: Vec<&str> = Vec::new();
        for n in &scan.nodes {
            for cd in &n.codedirs {
                let cd = cd.trim_end_matches('/');
                if cd != abs && cd.starts_with(abs) && cd.as_bytes().get(abs.len()) == Some(&b'/') && !covered.contains(&n.name.as_str()) {
                    covered.push(n.name.as_str());
                }
            }
        }
        if covered.len() >= 2 {
            out.push(format!(
                "codepath {d} is an area covering {} code nodes ({}); every item under it waits while this one runs — lock the directory the work writes",
                covered.len(),
                covered.iter().take(6).cloned().collect::<Vec<_>>().join(", ")
            ));
        }
    }
    out
}

fn calling_item(e: &Env) -> Value {
    if e.workid.is_empty() {
        die("ITER_WORKID is not set — this verb only works inside an engine-run work item".into());
    }
    e.api.get(&format!("/api/projects/{}/workitems/{}", e.project, e.workid)).unwrap_or_else(|err| die(format!("cannot load the calling item: {err}")))
}

fn set_state(e: &Env, mut item: Value, state: &str, note: Option<&str>) {
    let version = item.get("version").and_then(|v| v.as_u64()).unwrap_or(1);
    item["state"] = json!(state);
    if let Some(n) = note {
        item["lasterror"] = json!(n);
    }
    e.api
        .put(&format!("/api/projects/{}/workitems/{}?expect_version={}", e.project, e.workid, version), &item)
        .unwrap_or_else(|err| die(format!("state change failed: {err}")));
}

fn ask(e: &Env, question: String) {
    if question.trim().is_empty() {
        die("the question is empty (--question or --file)".into());
    }
    let item = calling_item(e);
    let _ = e.api
        .post(
            &format!("/api/projects/{}/workitems/{}/details", e.project, e.workid),
            &json!({"key": "question", "valuetype": "json", "value": question_widget(&question)}),
        )
        .unwrap_or_else(|err| die(format!("could not record the question: {err}")));
    set_state(e, item, "question", None);
    println!("question recorded — this work item parks in `question` when this turn ends and queues again once a human answers. Finish your turn now.");
}

fn reject(e: &Env, reason: &str) {
    let item = calling_item(e);
    let _ = e.api.post(
        &format!("/api/projects/{}/workitems/{}/details", e.project, e.workid),
        &json!({"key": "doc", "valuetype": "text", "value": format!("rejected by the {} agent: {}", e.agent, reason.trim())}),
    );
    set_state(e, item, "parked", Some(&format!("rejected: {}", reason.trim().chars().take(400).collect::<String>())));
    println!("rejected — this work item parks for human review when this turn ends. Finish your turn now.");
}

/// `iter block --cluster-restart [--reason …]` (built 2026-09-09; plan:
/// iter3/plans/cluster_restart_block.buildplan.md §1).  One versioned PUT —
/// `set_state` is not reused because it leaves `attempt` alone, and giving
/// the attempt back is the whole point.
fn block(e: &Env, cluster_restart: bool, reason: Option<String>) {
    if !cluster_restart {
        die("nothing to block on: pass --cluster-restart".into());
    }
    let reason = reason.unwrap_or_default();
    let reason = reason.trim();
    let reason: String = if reason.is_empty() { "the cluster is required for this work".into() } else { reason.chars().take(400).collect() };
    let mut item = calling_item(e);
    let version = item.get("version").and_then(|v| v.as_u64()).unwrap_or(1);
    let before = item.get("attempt").and_then(|a| a.as_u64()).unwrap_or(0);
    iter_core::cluster::apply_block(&mut item, &reason);
    let after = item.get("attempt").and_then(|a| a.as_u64()).unwrap_or(0);
    let _ = e.api.post(
        &format!("/api/projects/{}/workitems/{}/details", e.project, e.workid),
        &json!({"key": "doc", "valuetype": "text", "value": format!(
            "blocked by cluster restart (the {} agent): {reason} — attempt put back from {before} to {after}; parked until the cluster is back up and healthy",
            e.agent
        )}),
    );
    e.api
        .put(&format!("/api/projects/{}/workitems/{}?expect_version={}", e.project, e.workid, version), &item)
        .unwrap_or_else(|err| die(format!("block failed: {err}")));
    println!(
        "blocked on the cluster restart — this work item parks when this turn ends, tagged `{}`, with its attempt put back to {after} (it was {before}). The engine requeues it once the cluster is back up and healthy and strips the tag when it starts; your reason is what the next run reads back. Finish your turn now.",
        iter_core::cluster::CLUSTER_RESTART_TAG
    );
}

/// The calling item's new `blockedby`: existing links kept, new ids added
/// once each, never the item itself.
fn merge_blockers(existing: &[String], add: &[String], me: &str) -> Result<Vec<String>, String> {
    let mut out: Vec<String> = existing.to_vec();
    for id in add {
        if id == me {
            return Err("an item cannot wait on itself".into());
        }
        if !out.contains(id) {
            out.push(id.clone());
        }
    }
    Ok(out)
}

/// `iter wait --on <id>…` (decided 2026-09-10): one versioned PUT on the
/// calling item adding the ids to `blockedby` (deep), plus a "doc" row.  The
/// close gate reads the links back: an incomplete verdict with open blockers
/// queues the item behind them instead of bouncing it (gate::waiting_on).
fn wait(e: &Env, on: Vec<String>, reason: Option<String>) {
    let all = items(e);
    let ids: Vec<String> = on.iter().map(|n| resolve_id(e, &all, n)).collect();
    let mut item = calling_item(e);
    let version = item.get("version").and_then(|v| v.as_u64()).unwrap_or(1);
    let existing: Vec<String> = item.get("blockedby").and_then(|b| b.as_array()).map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect()).unwrap_or_default();
    let merged = merge_blockers(&existing, &ids, &e.workid).unwrap_or_else(|m| die(m));
    let added: Vec<&String> = merged.iter().filter(|m| !existing.contains(m)).collect();
    if added.is_empty() {
        println!("already waiting on {} — nothing to add", ids.iter().map(|i| &i[i.len().saturating_sub(12)..]).collect::<Vec<_>>().join(", "));
        return;
    }
    let reason = reason.unwrap_or_default();
    let reason: String = reason.trim().chars().take(400).collect();
    item["blockedby"] = json!(merged);
    e.api
        .put(&format!("/api/projects/{}/workitems/{}?expect_version={}", e.project, e.workid, version), &item)
        .unwrap_or_else(|err| die(format!("wait failed: {err}")));
    let _ = e.api.post(
        &format!("/api/projects/{}/workitems/{}/details", e.project, e.workid),
        &json!({"key": "doc", "valuetype": "text", "value": format!(
            "waiting on {} (the {} agent, attempt {}){}",
            added.iter().map(|a| a.as_str()).collect::<Vec<_>>().join(", "), e.agent,
            item.get("attempt").and_then(|a| a.as_u64()).unwrap_or(0),
            if reason.is_empty() { String::new() } else { format!(": {reason}") }
        )}),
    );
    println!(
        "waiting on {} — linked as {} dependenc{} of this item. Finish what you can, then end your turn listing what still waits on them as NOT DONE lines: \
         the close gate queues this item behind them (no bounce, no question) and the engine re-runs it once they close complete.",
        added.iter().map(|a| &a[a.len().saturating_sub(12)..]).collect::<Vec<_>>().join(", "),
        added.len(), if added.len() == 1 { "y" } else { "ies" }
    );
}

fn doc(e: &Env, text: String, id: Option<String>) {
    if text.trim().is_empty() {
        die("doc text is empty".into());
    }
    let target = match id {
        Some(n) => {
            let all = items(e);
            resolve_id(e, &all, &n)
        }
        None if !e.workid.is_empty() => e.workid.clone(),
        None => die("no target: pass --id or run inside a work item".into()),
    };
    match e.api.post(
        &format!("/api/projects/{}/workitems/{}/details", e.project, target),
        &json!({"key": "doc", "valuetype": "text", "value": text.trim_end()}),
    ) {
        Ok(row) => println!("doc #{} appended to {target}", row.get("order").and_then(|o| o.as_i64()).unwrap_or(-1)),
        Err(err) => die(format!("doc rejected: {err}")),
    }
}

fn critreview(e: &Env, file: Option<String>, context: Vec<String>, max_retry: u32, disposition: Option<String>, round: Option<i64>) {
    let details_path = format!("/api/projects/{}/workitems/{}/details", e.project, e.workid);
    if let Some(d) = disposition {
        if !["revised", "rejected", "no-findings"].contains(&d.as_str()) {
            die("--disposition must be revised | rejected | no-findings".into());
        }
        let details = e.api.get(&details_path).ok().and_then(|v| v.as_array().cloned()).unwrap_or_default();
        let mut reviews: Vec<Value> = details.into_iter().filter(|x| x.get("key").and_then(|k| k.as_str()) == Some("review")).collect();
        reviews.sort_by_key(|x| x.get("order").and_then(|o| o.as_i64()).unwrap_or(0));
        let target = match round {
            Some(r) => reviews.into_iter().find(|x| x.get("value").and_then(|v| v.get("round")).and_then(|n| n.as_i64()) == Some(r)),
            None => reviews.pop(),
        };
        let Some(mut row) = target else { die("no critique round to report on — run `iter critreview --file <material>` first".into()) };
        let order = row.get("order").and_then(|o| o.as_i64()).unwrap_or(0);
        row["value"]["disposition"] = json!(d);
        e.api
            .put(&format!("{details_path}/{order}"), &json!({"key": "review", "valuetype": "json", "value": row["value"]}))
            .unwrap_or_else(|err| die(format!("could not record the disposition: {err}")));
        println!("critreview: round {} disposition = {d}", row["value"]["round"]);
        return;
    }
    let Some(path) = file else { die("--file <material> is required (or --disposition to report on a round)".into()) };
    if e.workid.is_empty() {
        die("ITER_WORKID is not set — critreview records against the calling work item".into());
    }
    let material = std::fs::read_to_string(&path).unwrap_or_else(|err| die(format!("cannot read {path}: {err}")));
    let critic = e.api.get("/api/tooling/_critic").unwrap_or_else(|_| die("no `_critic` tooling row is defined (Agent Tooling in the webui)".into()));
    let persona = critic.get("body").and_then(|b| b.as_str()).unwrap_or("").to_string();
    let model = critic.get("model").and_then(|m| m.as_str()).unwrap_or("opus").to_string();
    let flags: Vec<String> = critic.get("flags").and_then(|f| f.as_str()).unwrap_or("").split_whitespace().map(String::from).collect();
    let timeout = critic.get("timeoutsec").and_then(|t| t.as_u64()).unwrap_or(1800);
    let mut prompt = format!("{persona}\n\n# Material under review (from {path})\n\n{material}\n");
    if !context.is_empty() {
        prompt.push_str("\n# Context files (read them before judging)\n");
        for c in &context {
            prompt.push_str(&format!("- {c}\n"));
        }
    }
    let cwd = if e.topdir.is_empty() { ".".to_string() } else { e.topdir.clone() };
    let mut last_err = String::new();
    for attempt in 1..=max_retry.max(1) {
        match crate::work::run_critic(&cwd, &prompt, &model, &flags, timeout) {
            Ok(out) if !out.text.trim().is_empty() => {
                let details = e.api.get(&details_path).ok().and_then(|v| v.as_array().cloned()).unwrap_or_default();
                let round = details
                    .iter()
                    .filter(|x| x.get("key").and_then(|k| k.as_str()) == Some("review"))
                    .filter_map(|x| x.get("value").and_then(|v| v.get("round")).and_then(|n| n.as_i64()))
                    .max()
                    .unwrap_or(0)
                    + 1;
                let _ = e.api.post(
                    &details_path,
                    &json!({"key": "review", "valuetype": "json", "value": {
                        "round": round, "persona": "_critic", "agent_type": e.agent,
                        "critique": out.text.trim(), "disposition": "",
                        "material": material.chars().take(60_000).collect::<String>(),
                        "created_at": iter_core::now_utc(),
                    }}),
                );
                println!("{}", out.text.trim());
                eprintln!("critreview: recorded as round {round}. When you have acted on it, report back with: iter critreview --disposition <revised|rejected|no-findings> --round {round}");
                return;
            }
            Ok(_) => last_err = "critic returned empty output".into(),
            Err(err) => last_err = err,
        }
        eprintln!("critreview: attempt {attempt} failed ({last_err})");
    }
    die(format!("critical review failed: {last_err}"));
}

fn capability(e: &Env, name: Option<String>) {
    let rows = e.api.get("/api/tooling").ok().and_then(|v| v.as_array().cloned()).unwrap_or_default();
    let caps: Vec<&Value> = rows.iter().filter(|r| r.get("kind").and_then(|k| k.as_str()) == Some("capability")).collect();
    match name {
        None => {
            for c in caps {
                println!("{}: {}", c.get("name").and_then(|n| n.as_str()).unwrap_or(""), c.get("desc").and_then(|d| d.as_str()).unwrap_or(""));
            }
        }
        Some(n) => {
            let want = n.trim().trim_start_matches('_').trim_end_matches(".md");
            let hit = caps.iter().find(|c| {
                let cn = c.get("name").and_then(|x| x.as_str()).unwrap_or("");
                cn == n || cn.trim_start_matches('_') == want
            });
            match hit {
                Some(c) => println!("{}", c.get("body").and_then(|b| b.as_str()).unwrap_or("")),
                None => die(format!("no capability named '{n}' — run `iter capability` to list them")),
            }
        }
    }
    std::process::exit(0);
}

fn status(e: &Env) {
    let mut all = items(e);
    let order = |s: &str| match s {
        "in-progress" => 0,
        "queued" => 1,
        "question" => 2,
        "paused" => 3,
        "parked" => 4,
        "scheduled" => 5,
        "failed" => 6,
        _ => 7,
    };
    all.retain(|i| !matches!(i.get("state").and_then(|s| s.as_str()), Some("complete") | Some("failed")));
    all.sort_by_key(|i| (order(i.get("state").and_then(|s| s.as_str()).unwrap_or("")), i.get("priority").and_then(|p| p.as_i64()).unwrap_or(5)));
    for i in &all {
        let id = i.get("id").and_then(|x| x.as_str()).unwrap_or("");
        let blocked: Vec<String> = i.get("blockedby").and_then(|b| b.as_array()).map(|a| a.iter().filter_map(|x| x.as_str()).map(|x| x[x.len().saturating_sub(12)..].to_string()).collect()).unwrap_or_default();
        println!(
            "{:<11} P{:<2} {:<10} {}  {}{}",
            i.get("state").and_then(|s| s.as_str()).unwrap_or(""),
            i.get("priority").and_then(|p| p.as_i64()).unwrap_or(5),
            i.get("agent").and_then(|a| a.as_str()).unwrap_or(""),
            &id[id.len().saturating_sub(12)..],
            i.get("name").and_then(|n| n.as_str()).unwrap_or(""),
            if blocked.is_empty() { String::new() } else { format!("  blocked-by: {}", blocked.join(",")) }
        );
    }
    println!("{} open work item(s) in {}", all.len(), e.project);
    std::process::exit(0);
}


#[cfg(test)]
mod tests {
    use super::*;

    fn row(order: i64, key: &str, ts: &str, value: &str) -> Value {
        json!({"order": order, "key": key, "ts": ts, "value": value})
    }

    /// Test-run rows collapse to one pair per group per attempt: the latest
    /// header of the same group since the attempt started is reused (with
    /// its adjacent detail row); earlier attempts, other groups and a
    /// missing attempt ts never match.
    #[test]
    fn prior_run_rows_finds_this_attempts_latest_run_of_the_group() {
        let since = "2026-09-11T00:56:24Z";
        let details = vec![
            row(0, "request", "2026-09-09T17:46:00Z", "…"),
            row(1, "log_header", "2026-09-10T16:56:04Z", "Test run 2026-09-10T16:56:04Z — testgroup \"pdy-core-intake-test\" in x"),
            row(2, "log_detail", "2026-09-10T16:56:04Z", "## failing"),
            row(34, "log_header", "2026-09-11T00:58:27Z", "Test run 2026-09-11T00:58:27Z — testgroup \"pdy-core-intake-test\" in x"),
            row(35, "log_header", "2026-09-11T00:59:05Z", "Test run 2026-09-11T00:59:05Z — testgroup \"pdy-core-intake-test\" in x"),
            row(36, "log_detail", "2026-09-11T00:59:05Z", "## failing"),
            row(39, "log_header", "2026-09-11T01:00:31Z", "Test run 2026-09-11T01:00:31Z — testgroup \"pdy-core-intake-dev\" in y"),
        ];
        assert_eq!(local::prior_run_rows(&details, "pdy-core-intake-test", since), Some((35, Some(36))), "latest header this attempt, with its detail");
        assert_eq!(local::prior_run_rows(&details, "pdy-core-intake-dev", since), Some((39, None)), "green run had no detail row");
        assert_eq!(local::prior_run_rows(&details, "other-group", since), None);
        assert_eq!(local::prior_run_rows(&details, "pdy-core-intake-test", "2026-09-11T01:30:00Z"), None, "a new attempt starts fresh");
        assert_eq!(local::prior_run_rows(&details, "pdy-core-intake-test", ""), None, "no attempt ts = append as before");
    }

    /// `iter wait`: links are added once each, existing ones kept, self refused.
    #[test]
    fn merge_blockers_adds_once_and_refuses_self() {
        let out = merge_blockers(&["a".into()], &["b".into(), "a".into(), "b".into()], "me").unwrap();
        assert_eq!(out, vec!["a", "b"]);
        assert!(merge_blockers(&[], &["me".into()], "me").is_err());
    }

    /// Case 10: for a stage-1 repeat `iter add` prints "already open: <id>"
    /// (and returns normally, exit 0); a new row still prints "added <id>".
    #[test]
    fn add_prints_already_open_for_a_repeat() {
        let twin = json!({"id": "f2a3c1e4-9b1c-4d3e-8f7a-f89259cb05e0", "state": "queued", "priority": 38, "repeats": 1, "agent": "code", "already_open": true});
        let line = add_outcome_line(&twin);
        assert!(line.starts_with("already open: f2a3c1e4-9b1c-4d3e-8f7a-f89259cb05e0 (f89259cb05e0)"), "{line}");
        assert!(line.contains("P38") && line.contains("repeats=1"));
        assert!(already_open(&twin));
        let fresh = json!({"id": "0b7e1c2d-1111-4d3e-8f7a-aaaaaaaaaaaa", "state": "queued", "priority": 40, "agent": "code"});
        assert_eq!(add_outcome_line(&fresh), "added 0b7e1c2d-1111-4d3e-8f7a-aaaaaaaaaaaa (aaaaaaaaaaaa) state=queued agent=code");
        assert!(!already_open(&fresh));
    }
}
