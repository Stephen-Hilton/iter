//! `iter <verb>` for agents and people: the queue verbs backed by iter_data
//! (add, ask, reject, block, wait, doc, critreview, capability, status) and
//! the checkout verbs (iter5): `runtests` (test nodes, standard result JSON),
//! `validate [--fix]` (nodefile conform), `sync` (one file-sync round),
//! `sweep`, `rag sync`, `init` (v5 scaffold), `migrate5` (iter4 → iter5 in a
//! copy), `markers`, `teststate`, `usecase`. Agents ask for the logical
//! thing; this does the deterministic work.
//!
//! Environment (set by the engine for every agent session): ITER_DATA_URL,
//! ITER_ENGINE_TOKEN, ITER_PROJECT, ITER_WORKID, ITER_AGENT, ITER_TOPDIR,
//! ITER_NODE, ITER_NODEFILE, ITER_PROVIDER, ITER_ACCOUNT.  The checkout verbs
//! also work from a plain shell inside the checkout: --project may then be a path.

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
    /// Block the CALLING work item on the cluster restart (on demand since
    /// 2026-09-13; refused when tonight's window skipped the rebuild): it parks
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
    /// Run a test node's scripts (iter5: `*.test.iter.md`, scripts from its
    /// `children.tests`; each prints the standard result JSON as its last
    /// line). No node: the calling item's node ($ITER_NODEFILE) — a test node,
    /// or every test node it links. Neutral by default; --broken / --fixed
    /// make a claim the engine records and gates on. A full run records the
    /// aggregated result on the test node in iter_data.
    Runtests {
        /// test node: id (or a unique tail), name, file stem or path
        node_pos: Option<String>,
        /// the same, as a flag (run_tests work items pass `--node <id>`)
        #[arg(long = "node", alias = "group")]
        node: Option<String>,
        /// narrow a NEUTRAL run to one script (file name or path)
        #[arg(long)]
        test: Option<String>,
        /// claim "the defect is still present": a fully green run means the calling item is stale (parked)
        #[arg(long)]
        broken: bool,
        /// claim "the defect is resolved" (completion gate): any red or error means the item cannot close
        #[arg(long)]
        fixed: bool,
        /// wall-clock budget (minutes) for the node's scripts together; overrun = killed → error
        #[arg(long = "timeout-min", default_value_t = iter_local::runtests::DEFAULT_TIMEOUT_MIN)]
        timeout_min: u64,
        /// do not post the result to iter_data
        #[arg(long)]
        no_record: bool,
    },
    /// Conform-check node files (every one under the checkout, or --file one);
    /// --fix rewrites them conformed.
    Validate {
        #[arg(long)]
        file: Option<String>,
        #[arg(long)]
        fix: bool,
    },
    /// Scaffold a v5 project in the current directory: global/<slug>.project.iter.md
    /// and global/requirements/{philosophy,bizreq,techreq}; never overwrites without --force
    Init {
        /// project name
        #[arg(long = "name", alias = "proj")]
        name: Option<String>,
        /// one-paragraph project description
        #[arg(long, default_value = "")]
        desc: String,
        /// creator recorded on the files (default: $USER)
        #[arg(long)]
        creator: Option<String>,
        #[arg(long)]
        force: bool,
    },
    /// One file-sync round now for this checkout's project: node files →
    /// iter_data (conformed), then the graph edits waiting for the checkout.
    Sync {
        /// sync without writing anything into the checkout
        #[arg(long)]
        read_only: bool,
        #[arg(long)]
        data_url: Option<String>,
    },
    /// Convert an iter4 checkout into iter5 node files in a COPY (never in place).
    Migrate5 {
        #[arg(long)]
        from: String,
        #[arg(long)]
        to: String,
        #[arg(long)]
        dry_run: bool,
        /// print the report as JSON
        #[arg(long)]
        json: bool,
    },
    /// Run the project's test nodes off the project graph (teststate per
    /// chain), record results, file fix / test / coverage / node-text items;
    /// --install-schedule turns the engine-owned test sweep on instead.
    Sweep {
        /// only this test node (id, name or path)
        #[arg(long, alias = "group")]
        node: Option<String>,
        #[arg(long)]
        dry_run: bool,
        /// record results but file no work items
        #[arg(long)]
        no_file: bool,
        #[arg(long, default_value_t = 30)]
        timeout_min: u64,
        #[arg(long)]
        install_schedule: bool,
        /// schedule interval for --install-schedule (30m, 4h, …)
        #[arg(long, default_value = "4h")]
        every: String,
        /// file at most this many node-text items per sweep (0 = skip the text check)
        #[arg(long, default_value_t = 10)]
        text_max: usize,
        /// file at most this many `test` items for code nodes with no tests per sweep (0 = skip)
        #[arg(long, default_value_t = 5)]
        tests_max: usize,
        /// file at most this many coverage top-up `test` items per sweep (0 = skip)
        #[arg(long, default_value_t = 5)]
        coverage_max: usize,
        #[arg(long)]
        data_url: Option<String>,
    },
    /// GraphRAG: `iter rag sync` re-indexes the node files (*.iter.md) whose
    /// text changed since the last sync, and drops the documents of deleted ones.
    Rag {
        /// sync (the only action)
        #[arg(default_value = "sync")]
        action: String,
        #[arg(long)]
        dry_run: bool,
        /// send every node file, not only the changed ones
        #[arg(long)]
        force: bool,
        #[arg(long)]
        data_url: Option<String>,
    },
    /// Every node file of the checkout, parsed, as JSON.
    Markers,
    /// The test gate on node files: set teststate (omit / include / block, or
    /// --clear back to inherit) on node files given by path, or --list them.
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
    /// Edit a use-case file's parts (children.codenodes).
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
    let here = || std::env::current_dir().unwrap_or_else(|_| ".".into());
    let conn_of = |data_url: Option<&str>| crate::sync::conn(&crate::sync::checkout_root(args.project.as_deref(), &here(), &topdir_of(&args)), args.project.as_deref(), data_url);
    match args.verb {
        Verb::Capability { ref name } => capability(&env(&args), name.clone()),
        Verb::Status => status(&env(&args)),
        Verb::Other(ref rest) => retired(rest),
        // local-file verbs: no server needed (claims / results are recorded when a connection exists)
        Verb::Runtests { ref node_pos, ref node, ref test, broken, fixed, timeout_min, no_record } => {
            let n = node.clone().or_else(|| node_pos.clone());
            std::process::exit(local::runtests(&topdir_of(&args), n.as_deref(), test.as_deref(), broken, fixed, timeout_min, !no_record))
        }
        Verb::Validate { ref file, fix } => std::process::exit(local::validate(&topdir_of(&args), file.as_deref(), fix)),
        Verb::Markers => std::process::exit(local::markers(&topdir_of(&args))),
        Verb::Init { ref name, ref desc, ref creator, force } => {
            let pname = name.clone().or_else(|| args.project.clone().filter(|p| !p.contains('/') && p != ".")).unwrap_or_default();
            if pname.trim().is_empty() {
                eprintln!("iter init: give the project name (--name <name>)");
                std::process::exit(2);
            }
            let who = creator.clone().or_else(|| std::env::var("USER").ok()).unwrap_or_default();
            match crate::init::init_project(&here(), pname.trim(), desc, &who, force) {
                Ok(files) => {
                    for f in &files {
                        println!("  wrote {f}");
                    }
                    println!(
                        "project {} scaffolded in {} ({} file(s)); next: create the project in iter_data and connect an engine to it \
                         (a `serves` edge with this topdir), then start the engine with `iter_engine --data-url URL --env-file PATH`",
                        pname.trim(), here().display(), files.len()
                    );
                    std::process::exit(0)
                }
                Err(e) => {
                    eprintln!("iter init: {e}");
                    std::process::exit(1)
                }
            }
        }
        Verb::Sync { read_only, ref data_url } => std::process::exit(crate::sync::sync_verb(&conn_of(data_url.as_deref()), read_only)),
        Verb::Migrate5 { ref from, ref to, dry_run, json } => match iter_local::migrate5::run(std::path::Path::new(from), std::path::Path::new(to), dry_run) {
            Ok(rep) => {
                if json {
                    println!("{}", serde_json::to_string_pretty(&rep).unwrap_or_default());
                } else {
                    rep.print();
                }
                std::process::exit(if rep.not_idempotent.is_empty() { 0 } else { 1 })
            }
            Err(e) => {
                eprintln!("iter migrate5: {e}");
                std::process::exit(2)
            }
        },
        Verb::Sweep { ref node, dry_run, no_file, timeout_min, install_schedule, ref every, text_max, tests_max, coverage_max, ref data_url } => {
            let c = conn_of(data_url.as_deref());
            if install_schedule {
                std::process::exit(crate::sweep::install_schedule(&c, every));
            }
            let quiet = no_file && !dry_run;
            let o = crate::sweep::SweepOpts {
                node: node.clone(), dry_run, file_items: !no_file, timeout_min,
                text_max: if quiet { 0 } else { text_max }, tests_max: if quiet { 0 } else { tests_max }, coverage_max: if quiet { 0 } else { coverage_max },
            };
            std::process::exit(crate::sweep::sweep_verb(&c, &o))
        }
        Verb::Rag { ref action, dry_run, force, ref data_url } => {
            if action != "sync" {
                eprintln!("iter rag: unknown action '{action}' (only `sync`)");
                std::process::exit(2);
            }
            std::process::exit(crate::rag::sync_verb(&conn_of(data_url.as_deref()), dry_run, force))
        }
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
        "`iter {verb}` is not an iter5 verb. Checkout verbs: runtests, validate, sync, sweep, rag, init, migrate5, markers, teststate, usecase. \
         Retired: ids and graph-apply (iter5 node files carry their own ids; graph edits reach the checkout through the engine's file sync), \
         and the V2-only verbs (testsweep, orphans, resolve)."
    ))
}

/// The local-file verbs; the API is touched only to record a claim / result
/// (when the ITER_* connection is set).
mod local {
    use super::Api;
    use iter_core::nodefile::{self, NodeDoc, NodeType};
    use iter_core::testresult::Outcome;
    use iter_local::{runtests as rt, validate as val};
    use serde_json::json;
    use std::path::{Path, PathBuf};

    /// The log_detail text once a green run follows a red one in the same
    /// attempt: the note first, then the earlier failing output verbatim.
    pub(super) fn green_replaces_red_note(label: &str, ts: &str, red_output: &str) -> String {
        if red_output.trim().is_empty() {
            return format!("(green on the latest run of test node \"{label}\" at {ts}; the earlier run was not green and left no output)");
        }
        format!("(green on the latest run of test node \"{label}\" at {ts}; the earlier failing output is kept below)\n\n{red_output}")
    }

    /// The ITER_* connection of an engine-run item: (api, project, workid).
    fn run_conn() -> Option<(Api, String, String)> {
        let get = |k: &str| std::env::var(k).ok().filter(|v| !v.trim().is_empty());
        Some((Api::new(&get("ITER_DATA_URL")?, &get("ITER_ENGINE_TOKEN")?), get("ITER_PROJECT")?, get("ITER_WORKID").unwrap_or_default()))
    }

    /// Detail rows for a run (one header + one detail per test node per
    /// attempt) and the optional fix item. Silent outside an engine-run item.
    fn report_run(run: &rt::NodeRun, header: &str, broken_claim: bool) {
        let Some((api, project, workid)) = run_conn() else { return };
        if workid.is_empty() {
            return;
        }
        let details = format!("/api/projects/{project}/workitems/{workid}/details");
        let since = api
            .get(&format!("/api/projects/{project}/workitems/{workid}"))
            .ok()
            .and_then(|i| i.get("ts").and_then(|t| t.get("start")).and_then(|s| s.as_str()).map(String::from))
            .unwrap_or_default();
        let existing: Vec<serde_json::Value> = api.get(&details).ok().and_then(|v| v.as_array().cloned()).unwrap_or_default();
        let prior = prior_run_rows(&existing, &run.name, &since);
        let header_row = json!({"key": "log_header", "valuetype": "text", "value": header});
        match prior {
            Some((h, _)) => { let _ = api.put(&format!("{details}/{h}"), &header_row); }
            None => { let _ = api.post(&details, &header_row); }
        }
        let detail = if run.outcome == Outcome::Pass { String::new() } else { rt::log_detail(run) };
        match (prior, detail.is_empty()) {
            (Some((_, Some(d))), true) => {
                let old = existing.iter().find(|r| r.get("order").and_then(|o| o.as_i64()) == Some(d)).and_then(|r| r.get("value")).and_then(|v| v.as_str()).unwrap_or("");
                let _ = api.put(&format!("{details}/{d}"), &json!({"key": "log_detail", "valuetype": "text",
                    "value": green_replaces_red_note(&run.name, &iter_core::now_utc(), old)}));
            }
            (Some((_, Some(d))), false) => { let _ = api.put(&format!("{details}/{d}"), &json!({"key": "log_detail", "valuetype": "text", "value": detail})); }
            (_, false) => { let _ = api.post(&details, &json!({"key": "log_detail", "valuetype": "text", "value": detail})); }
            (_, true) => {}
        }
        if run.outcome != Outcome::Fail || !run.full_run || broken_claim {
            return;
        }
        // fix items: never from an agent iterating on its own code (it sees red on purpose)
        let agent = std::env::var("ITER_AGENT").unwrap_or_default();
        if !agent.is_empty() && agent != "exec" && agent != "test" && std::env::var("ITER_SHELL").is_err() {
            return;
        }
        let project_row: serde_json::Value = api.get(&format!("/api/projects/{project}")).unwrap_or(json!({}));
        if !project_row.get("fix_on_test_failure").and_then(|b| b.as_bool()).unwrap_or(false) {
            return;
        }
        let tot = run.result.totals();
        let dir = format!("{}/", nodefile::dir_of(&run.path));
        let failing: Vec<String> = run.scripts.iter().filter(|t| t.outcome != Outcome::Pass).map(|t| t.script.clone()).collect();
        let request = format!(
            "The tests of test node \"{name}\" ({path}) are failing: {pass} of {total} passing on {when}. \
             This project files a fix item on every failing run (fix_on_test_failure).\n\n\
             - Failing scripts: {failing}\n\
             - The run summary is the \"log_header\" row and the failing output the \"log_detail\" row on work item {workid}.\n\
             - Reproduce: `\"$ITER_BIN\" runtests \"{path}\" --broken`. If it is green now the item is stale and the command parks it.\n\
             - Fix the CODE the tests describe; if a test itself is wrong, say so and fix the test.\n\
             - Finish with `\"$ITER_BIN\" runtests \"{path}\" --fixed`; the close gate needs an upheld --fixed claim.\n",
            name = run.name, path = run.path, pass = tot.pass, total = tot.total, when = iter_core::now_utc(), failing = failing.join(", "),
        );
        let body = json!({
            "name": format!("Tests failing: \"{}\" {}/{}", run.name, tot.pass, tot.total), "agent": "code", "state": "queued",
            "lockdirs": [dir], "blockedby": [], "context": [], "model": "",
            "tags": [
                {"text": format!("{}tests-non-green", iter_core::dedup::CHECK_TAG_PREFIX), "color": ""},
                {"text": format!("{}{}", iter_core::dedup::CONTAINER_TAG_PREFIX, run.name), "color": ""},
            ],
            "createdby": workid, "requestedby": if agent.is_empty() { "user".to_string() } else { format!("agent:{agent}") },
            "prework": [], "postwork": [], "request": request,
        });
        match api.post(&format!("/api/projects/{project}/workitems"), &body) {
            Ok(created) if crate::cli::already_open(&created) => println!("fix item already open for \"{}\" — repeat recorded on it", run.name),
            Ok(created) => println!("filed fix item {} (code) for \"{}\"", created.get("id").and_then(|i| i.as_str()).unwrap_or("?"), run.name),
            Err(e) => eprintln!("could not file the fix item: {e}"),
        }
    }

    /// The rows an earlier run of `label` wrote during THIS attempt: the
    /// latest header's order, and its detail row's when it sits right after.
    pub(crate) fn prior_run_rows(details: &[serde_json::Value], label: &str, since: &str) -> Option<(i64, Option<i64>)> {
        if since.is_empty() {
            return None;
        }
        fn key(d: &serde_json::Value) -> &str { d.get("key").and_then(|k| k.as_str()).unwrap_or("") }
        fn order(d: &serde_json::Value) -> i64 { d.get("order").and_then(|o| o.as_i64()).unwrap_or(-1) }
        let needles = [format!("test node \"{label}\""), format!("testgroup \"{label}\"")];
        let header = details
            .iter()
            .filter(|d| key(d) == "log_header")
            .filter(|d| d.get("ts").and_then(|t| t.as_str()).map(|t| t >= since).unwrap_or(false))
            .filter(|d| d.get("value").and_then(|v| v.as_str()).map(|v| needles.iter().any(|n| v.contains(n.as_str()))).unwrap_or(false))
            .max_by_key(|d| order(d))?;
        let h = order(header);
        let detail = details.iter().find(|d| order(d) == h + 1 && key(d) == "log_detail").map(|d| order(d));
        Some((h, detail))
    }

    fn record_claim(claim: &str, run: &rt::NodeRun, upheld: bool, park_reason: Option<&str>) {
        let Some((api, project, workid)) = run_conn() else { return };
        if workid.is_empty() {
            return;
        }
        let details = format!("/api/projects/{project}/workitems/{workid}/details");
        let tot = run.result.totals();
        let _ = api.post(&details, &json!({"key": "claim", "valuetype": "json", "value": {
            "claim": claim, "group": run.name, "node": run.id, "upheld": upheld, "outcome": rt::outcome_word(run.outcome),
            "counts": format!("{}/{}", tot.pass, tot.total), "ts": iter_core::now_utc(),
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

    /// Post a full run's result on the test node. The server files its own
    /// fix item for a red result unless told not to: it is told not to when
    /// this run expects red (a --broken claim, or an agent iterating on its own
    /// code) or when the project's fix_on_test_failure makes `report_run` file
    /// the (richer) item itself — one red test, one work item.
    fn record_result(run: &rt::NodeRun, broken_claim: bool) {
        let Some((api, project, workid)) = run_conn() else { return };
        let mut body = json!({"result": run.result, "outcome": rt::outcome_word(run.outcome)});
        if run.outcome != Outcome::Pass {
            let agent = std::env::var("ITER_AGENT").unwrap_or_default();
            let iterating = !agent.is_empty() && agent != "exec" && agent != "test" && std::env::var("ITER_SHELL").is_err();
            let engine_files = api.get(&format!("/api/projects/{project}")).ok()
                .and_then(|p| p.get("fix_on_test_failure").and_then(|b| b.as_bool())).unwrap_or(false);
            if broken_claim || iterating || engine_files {
                body["file_workitem"] = json!(false);
            }
        }
        if !workid.is_empty() {
            body["workid"] = json!(workid);
        }
        if let Err(e) = api.post(&format!("/api/projects/{project}/graph/nodes/{}/testresult", run.id), &body) {
            eprintln!("could not record the result on {}: {e}", run.path);
        }
    }

    /// The test nodes a node file links (its `children.tests` entries that
    /// are test node files on disk).
    fn linked_test_nodes(topdir: &Path, file: &Path) -> Result<Vec<(PathBuf, NodeDoc)>, String> {
        let sp = iter_local::walk::topdir_path(topdir, file).ok_or_else(|| format!("{} is outside {}", file.display(), topdir.display()))?;
        let text = std::fs::read_to_string(file).map_err(|e| format!("{}: {e}", file.display()))?;
        let (doc, _) = nodefile::parse_tolerant(&sp, &text).map_err(|e| e.to_string())?;
        if doc.nodetype == NodeType::Test {
            return Ok(vec![(file.to_path_buf(), doc)]);
        }
        let all: Vec<String> = rt::test_node_files(topdir).iter().filter_map(|p| iter_local::walk::topdir_path(topdir, p)).collect();
        let mut out = Vec::new();
        for e in &doc.children.tests {
            for p in nodefile::resolve(e, &sp, &all) {
                let real = topdir.join(p.trim_start_matches("{topdir}/"));
                if let Ok(d) = rt::load_test_node(topdir, &real) {
                    out.push((real, d));
                }
            }
        }
        if out.is_empty() {
            return Err(format!("{} links no test node (children.tests)", doc.path));
        }
        Ok(out)
    }

    pub fn runtests(topdir: &Path, node: Option<&str>, test: Option<&str>, broken: bool, fixed: bool, timeout_min: u64, record: bool) -> i32 {
        if broken && fixed {
            eprintln!("error: --broken and --fixed are mutually exclusive claims");
            return 2;
        }
        if (broken || fixed) && test.is_some() {
            eprintln!("error: claims are node-level; --test narrows only neutral runs");
            return 2;
        }
        let targets = match node {
            Some(n) => match rt::find_test_node(topdir, n) {
                Ok(t) => Ok(vec![t]),
                Err(e) => {
                    // a non-test node file: its linked test nodes
                    let p = PathBuf::from(n.trim_start_matches("{topdir}/"));
                    let p = if p.is_absolute() { p } else { topdir.join(p) };
                    if p.is_file() { linked_test_nodes(topdir, &p) } else { Err(e) }
                }
            },
            None => match std::env::var("ITER_NODEFILE").ok().filter(|f| !f.trim().is_empty()) {
                Some(f) => {
                    let p = PathBuf::from(f.replace("{topdir}", &topdir.to_string_lossy()));
                    linked_test_nodes(topdir, &p)
                }
                None => Err("name the test node (id, name, file stem or path); outside an item there is no $ITER_NODEFILE to default to".to_string()),
            },
        };
        let targets = match targets {
            Ok(t) => t,
            Err(e) => {
                eprintln!("error: {e}");
                return 2;
            }
        };
        if (broken || fixed) && targets.len() != 1 {
            eprintln!("error: a claim needs exactly one test node; {} matched — name it", targets.len());
            return 2;
        }
        let mut worst = 0;
        for (_, doc) in &targets {
            let run = match rt::run_node(topdir, doc, test, timeout_min) {
                Ok(r) => r,
                Err(e) => {
                    eprintln!("error: {e}");
                    return 2;
                }
            };
            let header = rt::log_header(&run, &iter_core::now_utc());
            print!("{header}");
            report_run(&run, &header, broken);
            if run.full_run && record {
                record_result(&run, broken);
            }
            let tot = run.result.totals();
            println!("{}", run.result.to_line());
            println!(
                "tests {}/{} — test node \"{}\" {}{}",
                tot.pass, tot.total, run.name, rt::outcome_word(run.outcome).to_uppercase(),
                if run.full_run { "" } else { " (filtered run — the node's recorded result is not updated)" }
            );
            if broken {
                return match run.outcome {
                    Outcome::Fail => {
                        record_claim("broken", &run, true, None);
                        println!("CLAIM UPHELD (--broken): the defect reproduces; proceed with the fix.");
                        0
                    }
                    Outcome::Pass => {
                        let reason = format!("stale item: --broken claim failed — test node \"{}\" is fully green ({}/{})", run.name, tot.pass, tot.total);
                        record_claim("broken", &run, false, Some(&reason));
                        println!("CLAIM FALSE (--broken): test node \"{}\" is fully green — this work item is STALE and has been parked. STOP NOW: touch no code and end your work immediately.", run.name);
                        3
                    }
                    Outcome::CouldNotRun => {
                        let reason = format!("--broken claim aborted: test node \"{}\" could not run — \"couldn't run\" must not pass for \"defect reproduces\"", run.name);
                        record_claim("broken", &run, false, Some(&reason));
                        println!("CLAIM ABORTED (--broken): test node \"{}\" could not run, which is not the same as the defect reproducing. STOP NOW: the item has been parked.", run.name);
                        3
                    }
                };
            }
            if fixed {
                return if run.outcome == Outcome::Pass {
                    record_claim("fixed", &run, true, None);
                    println!("CLAIM UPHELD (--fixed): test node \"{}\" is fully green.", run.name);
                    0
                } else {
                    record_claim("fixed", &run, false, None);
                    println!("CLAIM FALSE (--fixed): test node \"{}\" is {} — the work is NOT done. The close gate will not let this item complete until a --fixed claim is upheld; report what remains.", run.name, rt::outcome_word(run.outcome));
                    3
                };
            }
            worst = worst.max(match run.outcome {
                Outcome::Pass => 0,
                Outcome::Fail => 1,
                Outcome::CouldNotRun => 2,
            });
        }
        worst
    }

    pub fn validate(topdir: &Path, file: Option<&str>, fix: bool) -> i32 {
        let file = file.map(|f| {
            let p = PathBuf::from(f);
            if p.is_absolute() { p } else { topdir.join(p) }
        });
        let report = match val::run(topdir, file.as_deref(), fix) {
            Ok(r) => r,
            Err(e) => {
                eprintln!("error: {e}");
                return 2;
            }
        };
        for f in &report.files {
            let state = if f.fixed { "FIXED " } else if f.changed { "NOT CONFORMED " } else { "" };
            println!("{state}{}", f.path);
            for x in &f.findings {
                println!("    {:22} {}", x.code, x.msg);
            }
        }
        for (id, paths) in &report.duplicate_ids {
            println!("DUPLICATE ID {id}: {}", paths.join(", "));
        }
        println!(
            "validate: {} node file(s) checked, {} not conformed{}, {} finding(s) conform cannot fix, {} duplicate id(s)",
            report.files_checked,
            report.files.iter().filter(|f| f.changed).count(),
            if fix { format!(" ({} fixed)", report.files.iter().filter(|f| f.fixed).count()) } else { String::new() },
            report.persistent(),
            report.duplicate_ids.len()
        );
        report.exit_code()
    }

    fn node_files(topdir: &Path) -> Vec<(PathBuf, NodeDoc)> {
        iter_local::walk::node_files(topdir, &[topdir.to_path_buf()])
            .into_iter()
            .filter_map(|f| {
                let sp = iter_local::walk::topdir_path(topdir, &f)?;
                if !nodefile::type_of(&sp).is_some_and(nodefile::is_synced) {
                    return None;
                }
                let t = std::fs::read_to_string(&f).ok()?;
                nodefile::parse_tolerant(&sp, &t).ok().map(|(d, _)| (f, d))
            })
            .collect()
    }

    pub fn markers(topdir: &Path) -> i32 {
        let docs: Vec<NodeDoc> = node_files(topdir).into_iter().map(|(_, d)| d).collect();
        println!("{}", serde_json::to_string_pretty(&docs).unwrap_or_default());
        0
    }

    fn resolve_file(topdir: &Path, f: &str) -> PathBuf {
        let p = PathBuf::from(f.trim().trim_start_matches("{topdir}/"));
        if p.is_absolute() { p } else { topdir.join(p) }
    }

    /// Rewrite one node file through `f` (parse → edit → conform → write).
    fn edit_node(topdir: &Path, file: &Path, f: impl FnOnce(&mut NodeDoc) -> Result<(), String>) -> Result<NodeDoc, String> {
        let sp = iter_local::walk::topdir_path(topdir, file).ok_or_else(|| format!("{} is outside {}", file.display(), topdir.display()))?;
        let text = std::fs::read_to_string(file).map_err(|e| format!("cannot read {}: {e}", file.display()))?;
        let now = nodefile::now_ts();
        let c = nodefile::conform(&sp, &text, &now, "");
        let mut doc = c.doc.ok_or_else(|| format!("{} is not a node file", file.display()))?;
        let before = nodefile::semantic_hash(&doc);
        f(&mut doc)?;
        let out = nodefile::conform_against(&sp, &nodefile::render(&doc), &now, "", Some(&before)).text;
        std::fs::write(file, out).map_err(|e| format!("cannot write {}: {e}", file.display()))?;
        Ok(doc)
    }

    pub fn teststate(topdir: &Path, omit: &[String], include: &[String], block: &[String], clear: &[String], list: bool) -> i32 {
        let mut edited = 0;
        for (value, files) in [("omit", omit), ("include", include), ("block", block), ("inherit", clear)] {
            for f in files {
                let p = resolve_file(topdir, f);
                match edit_node(topdir, &p, |d| {
                    d.teststate = value.to_string();
                    Ok(())
                }) {
                    Ok(d) => {
                        println!("{}: teststate {value}", d.path);
                        edited += 1;
                    }
                    Err(e) => {
                        eprintln!("error: {e}");
                        return 2;
                    }
                }
            }
        }
        if list || edited == 0 {
            for (_, d) in node_files(topdir) {
                let ts = if d.teststate.is_empty() { "inherit" } else { d.teststate.as_str() };
                if ts != "inherit" || list {
                    println!("  {:10} {:8} {}", d.nodetype.as_str(), ts, d.path);
                }
            }
        }
        0
    }

    pub fn usecase(topdir: &Path, file: &str, add: &[String], remove: &[String], list: bool) -> i32 {
        let path = resolve_file(topdir, file);
        if nodefile::type_of(&path.to_string_lossy()) != Some(NodeType::Usecase) {
            eprintln!("error: {} is not a *.usecase.iter.md file (the filename declares the nodetype)", path.display());
            return 2;
        }
        if add.is_empty() && remove.is_empty() && !list {
            eprintln!("nothing to do: pass --add, --remove, and/or --list");
            return 2;
        }
        let res = edit_node(topdir, &path, |d| {
            for r in remove {
                d.children.codenodes.retain(|x| x != r.trim());
            }
            for a in add {
                let a = a.trim();
                if !a.is_empty() && !d.children.codenodes.iter().any(|x| x == a) {
                    d.children.codenodes.push(a.to_string());
                }
            }
            Ok(())
        });
        match res {
            Ok(d) => {
                if !add.is_empty() || !remove.is_empty() {
                    println!("{}: codenodes updated ({} entr{})", d.path, d.children.codenodes.len(), if d.children.codenodes.len() == 1 { "y" } else { "ies" });
                }
                if list {
                    for c in &d.children.codenodes {
                        println!("{c}");
                    }
                }
                0
            }
            Err(e) => {
                eprintln!("error: {e}");
                1
            }
        }
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
    if let Err(msg) = check_add_file(&f) {
        die(format!("{}: {msg}", file.as_deref().unwrap_or("--file")));
    }
    let name = title.clone().unwrap_or_else(|| s(&f, &["title", "name"]));
    if name.trim().is_empty() {
        die("a title is required (--title or \"title\" in --file)".into());
    }
    let agent = item_type.clone().unwrap_or_else(|| s(&f, &["type", "agent"]));
    let agent = if agent.is_empty() { "code".to_string() } else { agent };
    let request = read_arg_or_file(mainwork.clone(), None);
    let request = if request.is_empty() { s(&f, &["mainwork", "request"]) } else { request };
    // an item with no instructions wastes a session and gives the dedup
    // judge nothing to compare (2026-09-16, 2026-09-21)
    let has_question = question.as_deref().is_some_and(|q| !q.trim().is_empty()) || !s(&f, &["question"]).trim().is_empty();
    if let Err(msg) = check_add_request(&agent, &request, has_question) {
        die(msg);
    }
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

/// The keys `iter add --file` reads, and the JSON type each must have
/// (2026-09-16: a `--file` with "description" instead of "mainwork" was
/// stored with no request and no lock scope, and reported "added").
const ADD_FILE_KEYS: &[(&str, &str)] = &[
    ("title", "string"), ("name", "string"), ("type", "string"), ("agent", "string"),
    ("mainwork", "string"), ("request", "string"), ("codepath", "string"), ("question", "string"),
    ("model", "string"), ("usecase", "string"),
    ("codepaths", "strings"), ("lockdirs", "strings"), ("depends_on", "strings"), ("blockedby", "strings"), ("context", "strings"),
    ("depends_on_shallow", "bool"), ("blockedby_shallow", "bool"), ("priority", "integer"), ("tags", "array"),
];

/// Refuse a `--file` with a key `add` does not read, or a value of the
/// wrong shape, naming the key — never a silent "added".
fn check_add_file(f: &Value) -> Result<(), String> {
    let Some(obj) = f.as_object() else { return Err("must be a JSON object".into()) };
    for (k, v) in obj {
        let Some((_, want)) = ADD_FILE_KEYS.iter().find(|(name, _)| name == k) else {
            let known: Vec<&str> = ADD_FILE_KEYS.iter().map(|(n, _)| *n).collect();
            return Err(format!("unknown key \"{k}\" (iter add reads: {})", known.join(", ")));
        };
        let ok = match *want {
            "string" => v.is_string() || v.is_null(),
            "strings" => v.is_null() || v.as_array().is_some_and(|a| a.iter().all(|x| x.is_string())),
            "bool" => v.is_boolean(),
            "integer" => v.is_i64() || v.is_u64() || v.is_null(),
            _ => v.is_array() || v.is_null(),
        };
        if !ok {
            let shape = if *want == "strings" { "an array of strings" } else { want };
            return Err(format!("key \"{k}\" must be {shape}, got {v}"));
        }
    }
    Ok(())
}

/// Every agent item needs a real request (an exec item runs its shell; a
/// question item carries the question).  A placeholder is not a request.
fn check_add_request(agent: &str, request: &str, has_question: bool) -> Result<(), String> {
    let r = request.trim();
    if r.to_ascii_uppercase().starts_with("PLACEHOLDER") {
        return Err("refused: the request is a placeholder — write the real instructions (--mainwork or \"mainwork\" in --file)".into());
    }
    if r.is_empty() && agent != "exec" && !iter_core::is_test_agent(agent) && !has_question {
        return Err("a request is required for a non-exec item (--mainwork, or \"mainwork\"/\"request\" in --file)".into());
    }
    Ok(())
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
/// nodes' code directories (their `children.codedirs`): that is an area lock.
/// Silent outside a checkout (no topdir) or when no code node is found.
fn area_warnings(topdir: &str, lockdirs: &[String]) -> Vec<String> {
    use iter_core::nodefile::{self, NodeType};
    let top_p = std::path::Path::new(topdir);
    if topdir.trim().is_empty() || !top_p.is_dir() {
        return vec![];
    }
    let mut nodes: Vec<(String, Vec<String>)> = Vec::new();
    for f in iter_local::walk::node_files(top_p, &[top_p.to_path_buf()]) {
        let Some(sp) = iter_local::walk::topdir_path(top_p, &f) else { continue };
        if nodefile::type_of(&sp) != Some(NodeType::Code) {
            continue;
        }
        let Ok(t) = std::fs::read_to_string(&f) else { continue };
        let Ok((d, _)) = nodefile::parse_tolerant(&sp, &t) else { continue };
        let dirs = d.children.codedirs.iter().map(|e| nodefile::expand_entry(e, &sp, None).trim_end_matches("**").trim_end_matches('/').to_string()).collect();
        nodes.push((d.name, dirs));
    }
    let mut out = Vec::new();
    for d in lockdirs {
        let abs = if d.starts_with("{topdir}") { d.clone() } else { d.replacen(topdir.trim_end_matches('/'), "{topdir}", 1) };
        let abs = abs.trim_end_matches('/');
        let mut covered: Vec<&str> = Vec::new();
        for (name, cds) in &nodes {
            for cd in cds {
                if cd != abs && cd.starts_with(abs) && cd.as_bytes().get(abs.len()) == Some(&b'/') && !covered.contains(&name.as_str()) {
                    covered.push(name.as_str());
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
    // since 2026-09-13 the cluster rebuilds only on demand: when tonight's
    // restart window said it skipped the rebuild, nothing is coming to wait
    // for — parking would only cost a pointless requeue and re-run
    if let Some(msg) = skipped_rebuild_refusal(e) {
        die(msg);
    }
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

/// The refusal `iter block --cluster-restart` gives when the newest restart
/// window (completed within 24 h) posted a `{"skipped": true}` health row.
fn skipped_rebuild_refusal(e: &Env) -> Option<String> {
    let project: iter_core::Project = e.api.get(&format!("/api/projects/{}", e.project)).ok().and_then(|v| serde_json::from_value(v).ok())?;
    let all: Vec<iter_core::WorkItem> = items(e).into_iter().filter_map(|v| serde_json::from_value(v).ok()).collect();
    let clone = iter_core::cluster::newest_clone(&project.cluster_restart, &all)?;
    let details: Vec<Value> = e.api.get(&format!("/api/projects/{}/workitems/{}/details", e.project, clone.id)).ok().and_then(|v| v.as_array().cloned()).unwrap_or_default();
    iter_core::cluster::rebuild_skipped(Some(clone), &details, chrono::Utc::now()).then(|| skipped_rebuild_text(&clone.id))
}

fn skipped_rebuild_text(clone_id: &str) -> String {
    format!(
        "refused: tonight's cluster rebuild was skipped (restart window {} posted \"skipped\"), so there is no restart to wait for — the cluster stays up. Do the work now; if the cluster is really unavailable, say so with `iter ask`.",
        &clone_id[clone_id.len().saturating_sub(12)..]
    )
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
        .unwrap_or_else(|err| {
            // iter_data refuses a link that closes a loop (400 naming the path)
            let msg = serde_json::from_str::<Value>(&err.body).ok().and_then(|v| v.get("error").and_then(|x| x.as_str()).map(String::from)).unwrap_or_else(|| err.to_string());
            die(format!("wait {}", if msg.starts_with("refused") { msg } else { format!("failed: {msg}") }))
        });
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
    // every loop in the wait-for graph (CR 2026-09-25 6.7): nothing in it
    // can start until a link is removed
    if let Ok(d) = e.api.get(&format!("/api/projects/{}/deadlocks", e.project)) {
        for c in d.get("cycles").and_then(|c| c.as_array()).cloned().unwrap_or_default() {
            println!("{}", deadlock_line(&c));
        }
        let stale = d.get("stale_lock_rows").and_then(|s| s.as_array()).map(|a| a.len()).unwrap_or(0);
        if stale > 0 {
            println!("{stale} lock row(s) are held by items that are not running");
        }
    }
    std::process::exit(0);
}

/// "deadlock: a054fa190fb2 -(lock {topdir}/x/src)-> ed93b98a2679 -(blocker)-> 63491afb13c0 -(blocker)-> a054fa190fb2"
fn deadlock_line(cycle: &Value) -> String {
    let short = |x: &str| x[x.len().saturating_sub(12)..].to_string();
    let edges = cycle.get("edges").and_then(|e| e.as_array()).cloned().unwrap_or_default();
    let mut out = String::from("deadlock:");
    for (k, e) in edges.iter().enumerate() {
        let s = |key: &str| e.get(key).and_then(|x| x.as_str()).unwrap_or("").to_string();
        if k == 0 {
            out.push_str(&format!(" {}", short(&s("from"))));
        }
        let label = match s("kind").as_str() {
            "lock" => format!("lock {}", s("path")),
            "deep" => format!("deep via {}", short(&s("via"))),
            other => other.to_string(),
        };
        out.push_str(&format!(" -({label})-> {}", short(&s("to"))));
    }
    out
}


#[cfg(test)]
mod tests {
    use super::*;

    /// F15: a --file key add does not read, or a value of the wrong shape,
    /// is refused naming the key; an empty or placeholder request is refused
    /// for an agent item.
    #[test]
    fn add_file_refuses_unknown_keys_and_empty_requests() {
        let e = check_add_file(&json!({"title": "t", "description": "x"})).unwrap_err();
        assert!(e.contains("\"description\""), "{e}");
        let e = check_add_file(&json!({"title": "t", "codepaths": "core/x"})).unwrap_err();
        assert!(e.contains("\"codepaths\"") && e.contains("array of strings"), "{e}");
        assert!(check_add_file(&json!({"title": "t", "mainwork": "do it", "codepaths": ["core/x"], "priority": 3, "tags": []})).is_ok());
        assert!(check_add_file(&json!(["not", "an", "object"])).is_err());
        assert!(check_add_request("code", "", false).is_err());
        assert!(check_add_request("code", "PLACEHOLDER - replaced immediately by the filing agent", false).is_err());
        assert!(check_add_request("exec", "", false).is_ok());
        assert!(check_add_request("code", "", true).is_ok(), "a question item carries its question");
        assert!(check_add_request("code", "fix the red group", false).is_ok());
    }

    #[test]
    fn skipped_rebuild_refusal_names_the_window_and_the_way_out() {
        let t = skipped_rebuild_text("11112222-3333-4444-5555-aaaabbbbcccc");
        assert!(t.starts_with("refused: tonight's cluster rebuild was skipped (restart window aaaabbbbcccc"), "{t}");
        assert!(t.contains("iter ask"));
    }

    /// F19: the note written when green follows red keeps the red output
    /// instead of claiming it is gone.
    #[test]
    fn green_after_red_note_keeps_the_failing_output() {
        let red = "## t2 (red, exit 1)\nassertion failed: 40/42";
        let n = local::green_replaces_red_note("pdyadmin-keys-dev", "2026-09-20T10:00:00Z", red);
        assert!(n.starts_with("(green on the latest run of test node \"pdyadmin-keys-dev\""), "{n}");
        assert!(n.contains("kept below") && n.ends_with(red), "{n}");
        assert!(!n.contains("was replaced"));
    }

    #[test]
    fn deadlock_line_names_every_hop_and_its_kind() {
        let c = json!({"edges": [
            {"from": "e84f5d4f-fb75-417e-afea-a054fa190fb2", "to": "b5c2bc8f-0ab9-49bf-9f0a-ed93b98a2679", "kind": "lock", "path": "{topdir}/x/src"},
            {"from": "b5c2bc8f-0ab9-49bf-9f0a-ed93b98a2679", "to": "5d68139b-c7b7-43ea-a066-63491afb13c0", "kind": "blocker"},
            {"from": "5d68139b-c7b7-43ea-a066-63491afb13c0", "to": "e84f5d4f-fb75-417e-afea-a054fa190fb2", "kind": "deep", "via": "x-000000000001"}]});
        assert_eq!(deadlock_line(&c), "deadlock: a054fa190fb2 -(lock {topdir}/x/src)-> ed93b98a2679 -(blocker)-> 63491afb13c0 -(deep via 000000000001)-> a054fa190fb2");
    }

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
