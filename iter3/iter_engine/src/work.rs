//! Execute one claimed workitem: enforced git prework, mainwork (exec:shell
//! or a headless claude agent), enforced git postwork, response detail,
//! close gate (spec: Close Gate), close, release locks. Runs on its own thread.

use crate::client::Api;
use crate::gate::{self, Evidence, Verdict};
use iter_core::{CloseGate, Project, WorkItem, close_gate_for, now_utc};
use serde_json::{Value, json};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// What a run produced.  `subtype` is Claude Code's result subtype
/// ("success", "error_max_turns", ...); exec items and text fallbacks say
/// "success".
#[derive(Debug, Clone, Default)]
pub struct RunOut {
    pub text: String,
    pub subtype: String,
    pub num_turns: u64,
    pub cost_usd: f64,
    /// uncached input tokens only (what the stream's `usage.input_tokens` reports)
    pub input_tokens: u64,
    pub output_tokens: u64,
    /// prompt tokens served from the cache — nearly all of an agent's context
    /// cost lives here, not in `input_tokens` (fixed 2026-09-08: the spend row
    /// used to record ~200 input tokens for a 130-turn session)
    pub cache_read_tokens: u64,
    /// prompt tokens written into the cache
    pub cache_create_tokens: u64,
    /// the claude session this run used (for session continuation)
    pub session_id: String,
}

/// Agents whose items get the "agentmemory" turn (decided 2026-09-08): the
/// ones that work inside a codepath.  Plan/usecase/ingest lock plan dirs; a
/// briefing there helps nobody.
pub const AGENTMEMORY_AGENTS: &[&str] = &["code", "refactor", "testwriter", "deploy"];

/// Stop requests the engine tick has seen for items this engine is running;
/// the wait loop kills the session the moment its workid appears.
pub static STOP_REQUESTED: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());
pub const STOPPED_BY_USER: &str = "STOPPED by user mid-run";
thread_local! {
    /// the workid the current worker thread is running (for the wait loop)
    static CURRENT_WORKID: std::cell::RefCell<String> = const { std::cell::RefCell::new(String::new()) };
}

thread_local! {
    /// what the end-of-run commit did for the item running on this thread
    /// (scoped commit, 2026-09-12); read by the close gate's evidence
    static LAST_COMMIT: std::cell::RefCell<Option<CommitOutcome>> = const { std::cell::RefCell::new(None) };
}

/// What the end-of-run commit did (bugfix 2026-09-12: it used to `git add -A`
/// the whole shared checkout, sweeping other agents' unfinished files into a
/// commit titled with this item's name, and the verifier then bounced the
/// finished work over files it never touched).
#[derive(Debug, Clone, Default)]
pub(crate) struct CommitOutcome {
    pub committed: bool,
    /// paths still dirty in the checkout afterwards — another item's, never listed
    pub outside_scope_dirty: usize,
    /// "lock scope (k paths)" or "whole tree (item has no lockdirs)"
    pub scope_label: String,
}

impl RunOut {
    fn plain(text: String) -> Self {
        Self { text, subtype: "success".into(), ..Default::default() }
    }
}

/// Everything close() needs to run the gate for an agent item.
struct GateCtx {
    gate: CloseGate,
    request: String,
    details: Vec<Value>,
    head_before: String,
    account: String,
    topdir: String,
}

/// Run one claimed item, then — session continuation (decided 2026-09-08) —
/// keep the same claude session for up to `session_chain_max` queued
/// neighbours: same lockdirs, same usecase, dependencies satisfied, and the
/// best of everything queued on that scope.  Each item is still its own
/// claim, gate, spend row and close.
pub fn execute(api: &Api, engine_name: &str, project: &Project, topdir: &str, item: WorkItem, account: &str) {
    let mut item = item;
    let mut prev: Option<crate::prompt::ChainPrev> = None;
    let max = project.session_chain_max;
    loop {
        let (complete, sid) = execute_one(api, engine_name, project, topdir, &item, account, prev.as_ref());
        let position = prev.as_ref().map(|p| p.position).unwrap_or(1);
        if !complete || item.agent == "exec" || sid.is_empty() || max <= 1 || position >= max {
            break;
        }
        match claim_chain_candidate(api, engine_name, project, &item) {
            Some(next) => {
                println!(
                    "[engine] chain: {} '{}' -> {} '{}' (same session, {} of {})",
                    short(&item.id), item.name, short(&next.id), next.name, position + 1, max
                );
                prev = Some(crate::prompt::ChainPrev { sid, prev_id: item.id.clone(), prev_name: item.name.clone(), position: position + 1, max });
                item = next;
            }
            None => break,
        }
    }
}

/// (closed complete?, session id) for one item.
fn execute_one(
    api: &Api,
    engine_name: &str,
    project: &Project,
    topdir: &str,
    item: &WorkItem,
    account: &str,
    chain: Option<&crate::prompt::ChainPrev>,
) -> (bool, String) {
    let item = item.clone();
    CURRENT_WORKID.with(|w| *w.borrow_mut() = item.id.clone());
    let details = fetch_details(api, project, &item);

    // a human answered the close-gate widget "accept": close without running
    if item.agent != "exec" && gate::accepted_by_human(&details) {
        println!("[engine] {} '{}': close-gate widget answered accept — closing without a run",
            short(&item.id), item.name);
        let out = RunOut::plain("closed complete by a human via the close-gate widget (accept)".into());
        close(api, engine_name, project, item, Ok(out), None, None);
        return (true, String::new());
    }

    let head_before = git_head(topdir);
    let result = run_all(api, project, topdir, &item, account, &details, chain);
    let sid = result.as_ref().map(|o| o.session_id.clone()).unwrap_or_default();
    // `iter ask` / `iter reject` move the item to question/parked mid-run; the
    // close must keep that state (and skip the gate) instead of completing it
    if item.agent != "exec" {
        if let Ok(fresh) = api.get(&format!("/api/projects/{}/workitems/{}", project.name, item.id)) {
            let st = fresh.get("state").and_then(|s| s.as_str()).unwrap_or("");
            if st == "question" || st == "parked" {
                println!("[engine] {} '{}': agent moved it to {} during the run — keeping that", short(&item.id), item.name, st);
                close_keep_state(api, project, item, result, st);
                return (false, sid);
            }
        }
    }
    let ctx = if item.agent == "exec" {
        None
    } else {
        let (agent_def, overrides) = agent_config(api, project, &item);
        Some(GateCtx {
            gate: close_gate_for(&agent_def, &overrides),
            request: request_text(&details, &item),
            details,
            head_before,
            account: account.to_string(),
            topdir: topdir.to_string(),
        })
    };
    let complete = close(api, engine_name, project, item, result, ctx, chain.map(|c| c.prev_id.as_str()));
    (complete, sid)
}

/// The queued neighbour a just-completed item's session may take on next
/// (session continuation): same lockdirs, same usecase tags, dependencies
/// satisfied, and the best (priority, age) of EVERY queued item overlapping
/// that scope — so chaining never lets a lineage jump a more urgent one.
/// Claims it (queued -> in-progress + central locks) exactly like a dispatch;
/// None when nothing fits or the claim/lock race is lost.
fn claim_chain_candidate(api: &Api, engine_name: &str, project: &Project, prev: &WorkItem) -> Option<WorkItem> {
    let rows = project_items(api, project);
    let items: Vec<WorkItem> = rows.iter().filter_map(|v| serde_json::from_value(v.clone()).ok()).collect();
    let by_id: std::collections::HashMap<String, &WorkItem> = items.iter().map(|i| (i.id.clone(), i)).collect();
    let kids = iter_core::children_index(&items);
    let now = now_utc();
    let mut prev_dirs = prev.lockdirs.clone();
    prev_dirs.sort();
    if prev_dirs.is_empty() {
        return None;
    }
    let prev_uc = iter_core::usecase_tags(&prev.tags);
    let runnable = |i: &WorkItem| -> bool {
        i.state == "queued"
            && i.id != prev.id
            && i.agent != "exec"
            && !i.needs_approval
            && !i.stop_requested
            && (i.retry_after.is_empty() || i.retry_after <= now)
            // a neighbour waiting out the cluster restart is dispatch's to release (it knows the cluster's health)
            && !iter_core::cluster::has_tag(&i.tags, iter_core::cluster::CLUSTER_RESTART_TAG)
            && iter_core::dependency_status(i, &by_id, &kids) == iter_core::DepStatus::Satisfied
    };
    let overlaps = |i: &WorkItem| i.lockdirs.iter().any(|d| prev_dirs.iter().any(|p| iter_core::paths_overlap(d, p)));
    let mut competitors: Vec<&WorkItem> = items.iter().filter(|i| runnable(i) && overlaps(i)).collect();
    competitors.sort_by_key(|i| (i.priority, i.ts.receive.clone()));
    let best = competitors.first().copied()?;
    let mut dirs = best.lockdirs.clone();
    dirs.sort();
    if dirs != prev_dirs || iter_core::usecase_tags(&best.tags) != prev_uc {
        return None; // the next thing due on this scope is not a neighbour: leave it to dispatch
    }
    // claim (mirrors engine::start_item)
    let mut claimed = serde_json::to_value(best).ok()?;
    claimed["state"] = json!("in-progress");
    claimed["run_now"] = json!(false);
    claimed["retry_after"] = json!("");
    claimed["blockedby_locks"] = json!([]);
    claimed["tags"] = json!(iter_core::claim_tags(&best.tags)); // the one home for the claim's tag rule
    claimed["engine"] = json!(engine_name);
    claimed["attempt"] = json!(best.attempt + 1);
    claimed["ts"]["start"] = json!(now_utc());
    let resp = api.put(&format!("/api/projects/{}/workitems/{}?expect_version={}", project.name, best.id, best.version), &claimed).ok()?;
    let claimed_item: WorkItem = serde_json::from_value(resp).ok()?;
    let mut acquired: Vec<String> = Vec::new();
    for d in &claimed_item.lockdirs {
        let res = api.post(
            &format!("/api/projects/{}/locks/acquire", project.name),
            &json!({"path": d, "kind": "lock", "engine": engine_name, "workid": claimed_item.id, "ttl_sec": 3900}),
        );
        if res.is_err() {
            for p in &acquired {
                let _ = api.post(&format!("/api/projects/{}/locks/release", project.name), &json!({"path": p, "workid": claimed_item.id}));
            }
            let mut back = serde_json::to_value(&claimed_item).ok()?;
            back["state"] = json!("queued");
            let _ = api.put(&format!("/api/projects/{}/workitems/{}?expect_version={}", project.name, claimed_item.id, claimed_item.version), &back);
            return None;
        }
        acquired.push(d.clone());
    }
    Some(claimed_item)
}

fn short(id: &str) -> &str {
    &id[..8.min(id.len())]
}

pub(crate) fn fetch_details(api: &Api, project: &Project, item: &WorkItem) -> Vec<Value> {
    api.get(&format!("/api/projects/{}/workitems/{}/details", project.name, item.id))
        .ok()
        .and_then(|v| v.as_array().cloned())
        .unwrap_or_default()
}

/// request text is detail row key "request" (order 0), else the name
pub(crate) fn request_text(details: &[Value], item: &WorkItem) -> String {
    details
        .iter()
        .find(|d| d.get("key").and_then(|k| k.as_str()) == Some("request"))
        .and_then(|d| d.get("value").and_then(|v| v.as_str()).map(String::from))
        .unwrap_or_else(|| item.name.clone())
}

fn project_items(api: &Api, project: &Project) -> Vec<Value> {
    api.get(&format!("/api/projects/{}/workitems", project.name))
        .ok()
        .and_then(|v| v.as_array().cloned())
        .unwrap_or_default()
}

fn agent_config(api: &Api, project: &Project, item: &WorkItem) -> (Value, Value) {
    let agent_def = api.get(&format!("/api/agents/{}", item.agent)).unwrap_or(Value::Null);
    let overrides = project.agents.get(&item.agent).cloned().unwrap_or(Value::Null);
    (agent_def, overrides)
}

fn run_all(
    api: &Api,
    project: &Project,
    topdir: &str,
    item: &WorkItem,
    account: &str,
    details: &[Value],
    chain: Option<&crate::prompt::ChainPrev>,
) -> Result<RunOut, String> {
    let is_repo = std::path::Path::new(topdir).join(".git").exists();
    let has_remote = is_repo
        && run_shell(topdir, "git remote", 15).map(|o| !o.trim().is_empty()).unwrap_or(false);

    // git prework is engine-enforced (decided 2026-09-01), not optional
    if is_repo && has_remote {
        run_shell(topdir, "git pull --no-rebase", 120)?;
    }
    // prose steps (agent_tooling kind prepost) run as agent turns inside
    // run_claude; only the rest are engine-run shell steps
    let prose: std::collections::HashSet<String> = api
        .get("/api/tooling")
        .ok()
        .and_then(|v| v.as_array().cloned())
        .unwrap_or_default()
        .iter()
        .filter(|r| r.get("kind").and_then(|k| k.as_str()) == Some("prepost"))
        .filter_map(|r| r.get("name").and_then(|n| n.as_str()).map(String::from))
        .collect();
    for extra in item.prework.iter().filter(|p| !prose.contains(*p)) {
        run_named_ppw(api, project, topdir, extra, item)?;
    }

    let output = if item.agent == "exec" {
        if item.exec_shell.trim().is_empty() {
            return Err("exec item has empty exec_shell".into());
        }
        RunOut::plain(run_exec(api, project, topdir, item, agent_timeout(project, item, &Value::Null))?)
    } else {
        run_claude(api, project, topdir, item, account, details, chain)?
    };

    // git postwork is engine-enforced: changes are ALWAYS committed (and
    // pushed when a remote exists) — limited to the item's lock scope plus
    // the project's commit_extra_paths, so a sibling agent's unfinished
    // files are never committed under this item's name (2026-09-12)
    if is_repo {
        let outcome = commit_scoped(topdir, item, &project.commit_extra_paths)?;
        if outcome.committed {
            println!("[engine] {} committed its {}", short(&item.id), outcome.scope_label);
        }
        LAST_COMMIT.with(|c| *c.borrow_mut() = Some(outcome));
        if has_remote {
            run_shell(topdir, "git push", 180)?;
        }
    }
    for extra in item.postwork.iter().filter(|p| !prose.contains(*p)) {
        run_named_ppw(api, project, topdir, extra, item)?;
    }
    Ok(output)
}

fn sanitize(s: &str) -> String {
    s.replace('\'', "").chars().take(120).collect()
}

/// A path relative to `topdir` when it lies under it (the git pathspec form),
/// `.` for topdir itself, else the path as given.
fn rel_to_topdir(path: &str, topdir: &str) -> String {
    let top = topdir.trim_end_matches('/');
    let p = path.trim_end_matches('/');
    if p == top {
        return ".".into();
    }
    match p.strip_prefix(&format!("{top}/")) {
        Some(rest) if !rest.is_empty() => rest.to_string(),
        _ => p.to_string(),
    }
}

/// The git pathspecs an item's end-of-run commit — and its close-gate
/// evidence — is limited to: its `lockdirs` with `{topdir}` expanded and
/// made relative to the checkout, plus the project's `commit_extra_paths`.
/// A lock entry naming a single file is a valid pathspec as it is.  Empty
/// means the whole tree: the item has no lockdirs.  An entry outside the
/// checkout cannot be committed here and is dropped.
pub(crate) fn commit_scope(topdir: &str, item: &WorkItem, extra_paths: &[String]) -> Vec<String> {
    if item.lockdirs.is_empty() {
        return Vec::new();
    }
    let top = std::path::Path::new(topdir);
    let mut scope: Vec<String> = Vec::new();
    let mut push = |p: String| {
        if !p.is_empty() && !scope.contains(&p) {
            scope.push(p);
        }
    };
    for d in &item.lockdirs {
        let expanded = crate::prompt::expand_topdir_token(d, top);
        let rel = rel_to_topdir(&expanded, topdir);
        if rel.starts_with('/') || rel.starts_with("../") {
            continue; // not inside this checkout
        }
        push(rel);
    }
    for e in extra_paths {
        let e = e.trim().trim_start_matches("{topdir}/").trim_start_matches("./");
        if !e.is_empty() && !e.starts_with('/') && !e.starts_with("../") {
            push(e.trim_end_matches('/').to_string());
        }
    }
    scope
}

fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// The engine's one log line about what the scoped commit did not touch:
/// a count, never the paths — they belong to other items.
fn leftover_line(id8: &str, n: usize) -> String {
    format!("[engine] {id8} left {n} uncommitted path(s) outside its lock scope untouched")
}

/// The end-of-run commit, limited to `commit_scope`: stage only the scope,
/// commit only the paths that staged (an explicit list via
/// `--pathspec-from-file`, so a lock dir with nothing to commit cannot fail
/// the whole commit), then count what is still dirty.  An item with no
/// lockdirs keeps the whole-tree commit and says so in `scope_label`.
pub(crate) fn commit_scoped(topdir: &str, item: &WorkItem, extra_paths: &[String]) -> Result<CommitOutcome, String> {
    let scope = commit_scope(topdir, item, extra_paths);
    let msg = format!("iter: {} ({})", sanitize(&item.name), short(&item.id));
    let (scope_label, committed) = if scope.is_empty() {
        let _ = run_shell(topdir, "git add -A", 60);
        let ok = run_shell(topdir, &format!("git commit -m '{msg}'"), 60).is_ok();
        ("whole tree (item has no lockdirs)".to_string(), ok)
    } else {
        let spec = scope.iter().map(|p| shell_quote(p)).collect::<Vec<_>>().join(" ");
        let _ = run_shell(topdir, &format!("git add -A -- {spec}"), 60);
        // renames off: the commit's pathspec must name both the old and the new path
        let staged = run_shell(topdir, &format!("git diff --cached --name-only --no-renames -z -- {spec}"), 30).unwrap_or_default();
        let ok = if staged.trim_matches('\0').trim().is_empty() {
            false
        } else {
            static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
            let list = std::env::temp_dir().join(format!(
                "iter3-commit-{}-{}-{}.paths", short(&item.id), std::process::id(), SEQ.fetch_add(1, std::sync::atomic::Ordering::SeqCst)
            ));
            std::fs::write(&list, staged.as_bytes()).map_err(|e| format!("cannot write the commit path list: {e}"))?;
            let r = run_shell(
                topdir,
                &format!("git commit -m '{msg}' --pathspec-from-file={} --pathspec-file-nul", shell_quote(&list.to_string_lossy())),
                60,
            );
            let _ = std::fs::remove_file(&list);
            r.is_ok()
        };
        (format!("lock scope ({} paths)", scope.len()), ok)
    };
    let outside_scope_dirty = run_shell(topdir, "git status --porcelain", 30)
        .map(|s| s.lines().filter(|l| !l.trim().is_empty()).count())
        .unwrap_or(0);
    println!("{}", leftover_line(short(&item.id), outside_scope_dirty));
    Ok(CommitOutcome { committed, outside_scope_dirty, scope_label })
}

/// The close gate's git evidence for one run, limited to `scope` (whole tree
/// when empty): the diffstat between the heads over the scope only, and the
/// commits in that range that carry this item's id.
pub(crate) fn git_run_evidence(topdir: &str, head_before: &str, head_after: &str, scope: &[String], id8: &str) -> (String, Vec<(String, String)>) {
    let spec = if scope.is_empty() {
        String::new()
    } else {
        format!(" -- {}", scope.iter().map(|p| shell_quote(p)).collect::<Vec<_>>().join(" "))
    };
    let diffstat = run_shell(topdir, &format!("git diff --stat {head_before} {head_after}{spec}"), 30)
        .map(|s| gate::clip(s.trim(), 4_000))
        .unwrap_or_default();
    let log = run_shell(topdir, &format!("git log --format=%h%x09%s {head_before}..{head_after}"), 30).unwrap_or_default();
    (diffstat, gate::commits_with_id(&log, id8))
}

/// Session timeout: the project's per-agent override, else the agent record's
/// `timeoutsec` (the Settings field), else 3600.
fn agent_timeout(project: &Project, item: &WorkItem, agent_def: &Value) -> u64 {
    project
        .agents
        .get(&item.agent)
        .and_then(|o| o.get("timeoutsec"))
        .and_then(|t| t.as_u64())
        .or_else(|| agent_def.get("timeoutsec").and_then(|t| t.as_u64()))
        .filter(|t| *t > 0)
        .unwrap_or(3600)
}

/// Named pre/postwork beyond the enforced git set, from iter3_project_prepostwork.
fn run_named_ppw(
    api: &Api,
    project: &Project,
    topdir: &str,
    name: &str,
    _item: &WorkItem,
) -> Result<(), String> {
    // the enforced git basics are implicit; ignore them if listed explicitly
    if ["git-pull", "git-commit", "git-push"].contains(&name) {
        return Ok(());
    }
    let rows = api
        .get(&format!("/api/projects/{}/prepostwork", project.name))
        .map_err(|e| e.to_string())?;
    let row = rows
        .as_array()
        .and_then(|a| {
            a.iter().find(|r| r.get("name").and_then(|n| n.as_str()) == Some(name)).cloned()
        })
        .ok_or_else(|| format!("prepostwork '{name}' not defined"))?;
    let shell = row.get("shell").and_then(|s| s.as_str()).unwrap_or("").to_string();
    let timeout = row.get("timeoutsec").and_then(|t| t.as_u64()).unwrap_or(30);
    let failhalt = row.get("failhalt").and_then(|f| f.as_bool()).unwrap_or(true);
    match run_shell(topdir, &shell, timeout) {
        Ok(_) => Ok(()),
        Err(e) if failhalt => Err(format!("prepostwork '{name}' failed: {e}")),
        Err(e) => {
            eprintln!("[engine] prepostwork '{name}' failed (failhalt=false): {e}");
            Ok(())
        }
    }
}

fn run_claude(
    api: &Api,
    project: &Project,
    topdir: &str,
    item: &WorkItem,
    account: &str,
    details: &[Value],
    chain: Option<&crate::prompt::ChainPrev>,
) -> Result<RunOut, String> {
    let agent_def = api
        .get(&format!("/api/agents/{}", item.agent))
        .map_err(|e| format!("agent '{}' not defined in iter_data: {e}", item.agent))?;
    let promptbody = agent_def.get("promptbody").and_then(|p| p.as_str()).unwrap_or("").to_string();
    let overrides = project.agents.get(&item.agent).cloned().unwrap_or(Value::Null);
    let model = if !item.model.trim().is_empty() {
        item.model.trim().to_string()
    } else {
        overrides.get("model").and_then(|m| m.as_str())
            .or_else(|| agent_def.get("model").and_then(|m| m.as_str())).unwrap_or("").to_string()
    };
    let flags = overrides.get("flags").and_then(|f| f.as_str())
        .or_else(|| agent_def.get("flags").and_then(|f| f.as_str())).unwrap_or("").to_string();
    let timeout = agent_timeout(project, item, &agent_def);

    // central tooling: shared rules, capability index, source instructions, prose steps
    let tooling_rows = api.get("/api/tooling").ok().and_then(|v| v.as_array().cloned()).unwrap_or_default();
    let tooling = crate::prompt::Tooling::from_rows(&tooling_rows);
    let top = std::path::Path::new(topdir);
    let head = crate::prompt::read_head(project, top);
    let codepath = item
        .lockdirs
        .first()
        .map(|d| crate::prompt::expand_topdir_token(d, top))
        .unwrap_or_else(|| topdir.to_string());
    let codepath = std::path::PathBuf::from(codepath.trim_end_matches('/'));

    // who asked: a workitem id in createdby means an agent handoff — name its type
    let createdby_agent = if !item.createdby.is_empty() && item.createdby.len() >= 32 {
        api.get(&format!("/api/projects/{}/workitems/{}", project.name, item.createdby))
            .ok().and_then(|p| p.get("agent").and_then(|a| a.as_str()).map(String::from)).unwrap_or_default()
    } else {
        String::new()
    };
    let last_response = details
        .iter()
        .filter(|d| d.get("key").and_then(|k| k.as_str()) == Some("response"))
        .max_by_key(|d| d.get("order").and_then(|o| o.as_i64()).unwrap_or(0))
        .and_then(|d| d.get("value").and_then(|v| v.as_str()).map(String::from))
        .unwrap_or_default();

    let spin_input = crate::prompt::SpinupInput {
        agent_body: &promptbody,
        tooling: &tooling,
        head: &head,
        project,
        item,
        codepath: &codepath,
        topdir: top,
        requestedby: &item.requestedby,
        createdby_agent: &createdby_agent,
        last_response_tail: &last_response,
        close_gate_paragraph: &format!("\n\n{}", gate::WORKER_CLOSE_GATE_PROMPT),
    };
    // a chained item joins an existing session: only the per-item part is sent
    let (spin, context_files, warnings) = match chain {
        Some(prev) => crate::prompt::chain_spinup(&spin_input, prev),
        None => crate::prompt::spinup(&spin_input),
    };
    for w in &warnings {
        eprintln!("[engine] {} context: {w}", short(&item.id));
    }

    // the request + an answered question (if the item came back from `question`)
    let request = request_text(details, item);
    let answered = crate::prompt::answered_question(details);
    if let Some((order, _, _)) = &answered {
        // mark the answer as shown so a later run does not repeat it
        if let Some(row) = details.iter().find(|d| d.get("order").and_then(|o| o.as_i64()) == Some(*order)) {
            let mut v = row.get("value").cloned().unwrap_or(Value::Null);
            v["surfaced"] = json!(true);
            let _ = api.put(
                &format!("/api/projects/{}/workitems/{}/details/{}", project.name, item.id, order),
                &json!({"key": "question", "valuetype": "json", "value": v}),
            );
        }
    }
    let mut main = crate::prompt::mainwork_prompt(&request, answered.map(|(_, q, a)| (q, a)));
    // what earlier attempts already filed (never on attempt 1; one list read)
    let created = if item.attempt > 1 || item.gate_bounces > 0 {
        gate::children_of(&project_items(api, project), &item.id)
    } else {
        Vec::new()
    };
    let feedback = gate::feedback_section(details, &created);
    if !feedback.is_empty() {
        main.push_str("\n\n");
        main.push_str(&feedback);
    }

    // turn sequence: prose prework → mainwork → prose postwork → self-check
    let mut turns: Vec<(String, String)> = Vec::new();
    for step in &item.prework {
        if let Some(body) = tooling.prepost.get(step) {
            turns.push((format!("prework:{step}"), body.clone()));
        }
    }
    turns.push(("mainwork".into(), main));
    for step in &item.postwork {
        if let Some(body) = tooling.prepost.get(step) {
            turns.push((format!("postwork:{step}"), body.clone()));
        }
    }
    // agent memory (decided 2026-09-08): the codepath's briefing for the next
    // agent, refreshed by every codepath-working agent before the self-check
    if AGENTMEMORY_AGENTS.contains(&item.agent.as_str()) && !item.lockdirs.is_empty() && codepath.is_dir() {
        turns.push(("agentmemory".into(), crate::prompt::agentmemory_prompt(&codepath)));
    }
    turns.push(("selfcheck".into(), crate::prompt::selfcheck_prompt(&promptbody, &tooling.shared)));

    // the agent's environment (V2 names kept so the shared rules still apply verbatim)
    let shim = write_iter_shim(topdir)?;
    let mut envs: Vec<(String, String)> = vec![
        ("ITER_BIN".into(), shim.clone()),
        ("ITER_PROJECT".into(), project.name.clone()),
        ("ITER_WORKID".into(), item.id.clone()),
        ("ITER_AGENT".into(), item.agent.clone()),
        ("ITER_TOPDIR".into(), topdir.to_string()),
        ("ITER_MAINFILE".into(), head.mainfile.to_string_lossy().into_owned()),
        ("ITER_CONTEXT_FILES".into(), head.context_files.iter().map(|p| p.to_string_lossy().into_owned()).collect::<Vec<_>>().join(":")),
        ("ITER_ITEM_CONTEXT_FILES".into(), context_files.iter().map(|p| p.to_string_lossy().into_owned()).collect::<Vec<_>>().join(":")),
        ("ITER_TEST_DIR".into(), "tests".into()),
        ("ITER_INTERFACE_DIR".into(), head.interface_dir.clone()),
        ("ITER_USECASE_DIR".into(), head.usecase_dir.clone()),
        ("ITER_DATA_URL".into(), api.base.clone()),
        ("ITER_ENGINE_TOKEN".into(), api.token.clone()),
        ("BASH_MAX_TIMEOUT_MS".into(), timeout.saturating_mul(1000).to_string()),
    ];
    if let Some(dir) = std::path::Path::new(&shim).parent() {
        let path = std::env::var("PATH").unwrap_or_default();
        envs.push(("PATH".into(), format!("{}:{}", dir.display(), path)));
    }
    let extra: Vec<String> = flags.split_whitespace().map(String::from).collect();
    let mut session = Session { sid: chain.map(|c| c.sid.clone()).unwrap_or_default(), cwd: codepath.to_string_lossy().into_owned(), model, extra, envs, timeout, account: account.to_string() };
    if !std::path::Path::new(&session.cwd).is_dir() {
        session.cwd = topdir.to_string();
    }
    let mut last = RunOut::default();
    let (mut usd, mut tin, mut tout, mut nturns) = (0.0f64, 0u64, 0u64, 0u64);
    let (mut tcr, mut tcc) = (0u64, 0u64);
    let total = turns.len();
    for (n, (label, prompt)) in turns.into_iter().enumerate() {
        let text = if n == 0 { format!("{spin}\n\n# Step: {label}\n{prompt}") } else { format!("# Step: {label}\n{prompt}") };
        println!("[engine] {} turn {}/{} {label}", short(&item.id), n + 1, total);
        let out = session.turn(project, &text)?;
        usd += out.cost_usd;
        tin += out.input_tokens;
        tout += out.output_tokens;
        tcr += out.cache_read_tokens;
        tcc += out.cache_create_tokens;
        nturns += out.num_turns.max(1);
        // a cut-off turn ends the run: the gate sees the subtype and holds the item
        let cut = out.subtype != "success";
        if label == "mainwork" || cut {
            last = out;
        }
        if cut {
            break;
        }
    }
    last.cost_usd = usd;
    last.input_tokens = tin;
    last.output_tokens = tout;
    last.cache_read_tokens = tcr;
    last.cache_create_tokens = tcc;
    last.num_turns = nturns;
    last.session_id = session.sid.clone();
    Ok(last)
}

/// The `iter critreview` critic: a fresh session, no account routing beyond
/// the ambient token, bounded by the persona's timeout.
pub fn run_critic(cwd: &str, prompt: &str, model: &str, flags: &[String], timeout_sec: u64) -> Result<RunOut, String> {
    let mut cmd = Command::new("claude");
    cmd.arg("-p").arg(prompt).arg("--output-format").arg("stream-json").arg("--verbose");
    if !model.is_empty() {
        cmd.arg("--model").arg(model);
    }
    for f in flags {
        cmd.arg(f);
    }
    cmd.env_remove("ANTHROPIC_API_KEY").env_remove("ANTHROPIC_AUTH_TOKEN");
    cmd.current_dir(cwd).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    Ok(parse_claude_stream("", &wait_with_timeout(cmd, timeout_sec)?).1)
}

/// One headless claude session across several turns (`--resume`).
struct Session {
    sid: String,
    cwd: String,
    model: String,
    extra: Vec<String>,
    envs: Vec<(String, String)>,
    timeout: u64,
    account: String,
}

impl Session {
    fn turn(&mut self, project: &Project, prompt: &str) -> Result<RunOut, String> {
        let mut args: Vec<String> = Vec::new();
        if !self.sid.is_empty() {
            args.push("--resume".into());
            args.push(self.sid.clone());
        }
        args.extend(self.extra.iter().cloned());
        let raw = spawn_claude_env(project, &self.cwd, &self.account, prompt, &self.model, &args, self.timeout, &self.envs)?;
        let (sid, out) = parse_claude_stream(&self.account, &raw);
        if !sid.is_empty() {
            self.sid = sid;
        }
        Ok(out)
    }
}

/// `{topdir}/.iter/bin/iter` -> this binary's `cli` subcommand, so agents run
/// plain `iter add …` exactly as the shared rules say.
fn write_iter_shim(topdir: &str) -> Result<String, String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let dir = std::path::Path::new(topdir).join(".iter").join("bin");
    std::fs::create_dir_all(&dir).map_err(|e| format!("mkdir {}: {e}", dir.display()))?;
    let shim = dir.join("iter");
    let body = format!("#!/bin/sh\nexec \"{}\" cli \"$@\"\n", exe.display());
    if std::fs::read_to_string(&shim).ok().as_deref() != Some(body.as_str()) {
        std::fs::write(&shim, body).map_err(|e| format!("write {}: {e}", shim.display()))?;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&shim, std::fs::Permissions::from_mode(0o755));
    }
    Ok(shim.to_string_lossy().into_owned())
}

/// Parse `--output-format stream-json` output: one JSON object per line.
/// The `rate_limit_event` line is written to `account`'s usage snapshot
/// (spec: Usage%) and the `result` line becomes the RunOut (+ session id for
/// `--resume`).  A lone result object (older CLIs, test doubles) or plain
/// text still parse via `parse_claude_json`.
pub(crate) fn parse_claude_stream(account: &str, raw: &str) -> (String, RunOut) {
    let mut sid = String::new();
    let mut result: Option<RunOut> = None;
    for line in raw.lines() {
        let line = line.trim();
        if !line.starts_with('{') {
            continue;
        }
        let Ok(v) = serde_json::from_str::<Value>(line) else { continue };
        if crate::usage::record_event(account, &v) {
            continue;
        }
        if v.get("type").and_then(|t| t.as_str()) == Some("result") {
            if let Some(s) = v.get("session_id").and_then(|s| s.as_str()) {
                sid = s.to_string();
            }
            result = Some(parse_claude_json(line));
        }
    }
    if let Some(out) = result {
        return (sid, out);
    }
    // not a stream: a lone result object (possibly after warnings) or plain text
    let trimmed = raw.trim();
    let candidate = trimmed.find('{').map(|i| &trimmed[i..]).unwrap_or("");
    if let Ok(v) = serde_json::from_str::<Value>(candidate) {
        sid = v.get("session_id").and_then(|s| s.as_str()).unwrap_or("").to_string();
    }
    (sid, parse_claude_json(raw))
}

/// The token a session billed to `account` runs with (spec: account hot
/// reload, 2026-09-11).  `""` is the ambient CLI login (no accounts
/// configured): `Ok(None)`.  A named account resolves through the env store
/// — never `std::env`, never another account's variable — and a missing
/// value is an error the caller propagates, so the run fails loudly instead
/// of being billed to whichever other account happened to be set.
pub(crate) fn resolve_account_token(project: &Project, account: &str, env_file: &str) -> Result<Option<String>, String> {
    if account.is_empty() {
        return Ok(None);
    }
    match project.accounts.iter().find(|a| a.name == account) {
        Some(a) => match crate::envstore::get(&a.token_envar) {
            Some(tok) => Ok(Some(tok)),
            None => Err(format!("account '{account}' has no token: {} is not set in {env_file}", a.token_envar)),
        },
        None => Err(format!("account '{account}' has no token: (no token_envar configured) is not set in {env_file}")),
    }
}

/// The variable a named account reads its token from, if the project names it.
pub(crate) fn account_envar(project: &Project, account: &str) -> Option<String> {
    project.accounts.iter().find(|a| a.name == account).map(|a| a.token_envar.clone())
}

/// Spawn with an explicit environment (the multi-turn session path).
fn spawn_claude_env(
    project: &Project,
    cwd: &str,
    account: &str,
    prompt: &str,
    model: &str,
    extra_args: &[String],
    timeout_sec: u64,
    envs: &[(String, String)],
) -> Result<String, String> {
    let mut cmd = Command::new("claude");
    cmd.arg("-p").arg(prompt).arg("--output-format").arg("stream-json").arg("--verbose");
    if !model.is_empty() {
        cmd.arg("--model").arg(model);
    }
    for f in extra_args {
        cmd.arg(f);
    }
    if let Some(tok) = resolve_account_token(project, account, &crate::envstore::env_file())? {
        cmd.env("CLAUDE_CODE_OAUTH_TOKEN", tok);
    }
    cmd.env_remove("ANTHROPIC_API_KEY").env_remove("ANTHROPIC_AUTH_TOKEN");
    for (k, v) in envs {
        cmd.env(k, v);
    }
    cmd.current_dir(cwd).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    wait_with_timeout(cmd, timeout_sec)
}

/// Spawn one headless claude session (json result output) billed to the
/// chosen account; shared by the worker and the verifier.
pub(crate) fn spawn_claude(
    project: &Project,
    topdir: &str,
    account: &str,
    prompt: &str,
    model: &str,
    extra_args: &[String],
    timeout_sec: u64,
) -> Result<String, String> {
    let mut cmd = Command::new("claude");
    cmd.arg("-p").arg(prompt).arg("--output-format").arg("stream-json").arg("--verbose");
    // usage tracking: stream-json carries a rate_limit_event line per session;
    // parse_claude_stream writes it to THIS account's snapshot file
    if !model.is_empty() {
        cmd.arg("--model").arg(model);
    }
    for f in extra_args {
        cmd.arg(f);
    }
    // route billing to the CHOSEN account's token (ladder + exclusion picked
    // it); a named account whose token is not set is an error, never a
    // fallback to some other account's token (2026-09-11)
    if let Some(tok) = resolve_account_token(project, account, &crate::envstore::env_file())? {
        cmd.env("CLAUDE_CODE_OAUTH_TOKEN", tok);
    }
    cmd.env_remove("ANTHROPIC_API_KEY").env_remove("ANTHROPIC_AUTH_TOKEN");
    cmd.current_dir(topdir).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    wait_with_timeout(cmd, timeout_sec)
}

/// ELI5 (spec: Explain / ELI5): one read-only session of the `explain` agent
/// on a work item, run at once — outside the agent cap, no queue, no locks
/// (it writes nothing in the repo) — whose whole output lands on the item as
/// an "explained" detail row.  `agent_def` is the project's `explain` agent
/// record when one exists (model / timeout / body); its flags are ignored:
/// the tool set here is fixed to Read, Glob, Grep.
pub fn explain(api: &Api, project: &Project, topdir: &str, item: &WorkItem, account: &str, agent_def: &Value) {
    let started = Instant::now();
    let details = fetch_details(api, project, item);
    let top = std::path::Path::new(topdir);
    let head = crate::prompt::read_head(project, top);
    let codepath = item
        .lockdirs
        .first()
        .map(|d| crate::prompt::expand_topdir_token(d, top))
        .unwrap_or_else(|| topdir.to_string());
    let codepath = std::path::PathBuf::from(codepath.trim_end_matches('/'));
    let body = agent_def.get("promptbody").and_then(|b| b.as_str()).unwrap_or("");
    // a stub body (the header line alone) means "use the built-in persona"
    let body = if body.trim().lines().count() > 3 { body.to_string() } else { crate::prompt::EXPLAIN_DEFAULT_BODY.to_string() };
    let prompt = crate::prompt::explain_prompt(&crate::prompt::ExplainInput {
        agent_body: &body, head: &head, project, item, details: &details, codepath: &codepath, topdir: top,
    });
    let model = agent_def.get("model").and_then(|m| m.as_str()).unwrap_or("sonnet").trim().to_string();
    let timeout = agent_def.get("timeoutsec").and_then(|t| t.as_u64()).unwrap_or(900).clamp(60, 3600);
    let extra = vec![
        "--allowedTools".to_string(),
        "Read,Glob,Grep".to_string(),
        "--disallowedTools".to_string(),
        "Bash,Edit,Write,MultiEdit,NotebookEdit,WebFetch,WebSearch,Agent".to_string(),
        "--max-turns".to_string(),
        "40".to_string(),
    ];
    let details_path = format!("/api/projects/{}/workitems/{}/details", project.name, item.id);
    let result = spawn_claude(project, topdir, account, &prompt, &model, &extra, timeout)
        .map(|raw| parse_claude_stream(account, &raw).1);
    let secs = started.elapsed().as_secs();
    let value = match &result {
        Ok(out) if out.subtype == "success" && !out.text.trim().is_empty() => out.text.trim().to_string(),
        Ok(out) => format!("Could not explain this item: the session ended with '{}' after {secs}s.{}",
            out.subtype, if out.text.trim().is_empty() { String::new() } else { format!("\n\n{}", out.text.trim()) }),
        Err(e) => format!("Could not explain this item: {}", e.chars().take(800).collect::<String>()),
    };
    if let Err(e) = api.post(&details_path, &json!({"key": "explained", "valuetype": "text", "value": value})) {
        eprintln!("[engine] could not append the explanation to {}: {e}", short(&item.id));
    }
    if let Ok(out) = &result {
        if out.cost_usd > 0.0 || out.input_tokens > 0 {
            let _ = api.post(&details_path, &json!({"key": "spend", "valuetype": "json", "value": {"usd": out.cost_usd,
                "input_tokens": out.input_tokens, "output_tokens": out.output_tokens,
                "cache_read_tokens": out.cache_read_tokens, "cache_create_tokens": out.cache_create_tokens,
                "turns": out.num_turns, "agent": "explain"}}));
            let _ = api.post(&format!("/api/projects/{}/spend", project.name),
                &json!({"usd": out.cost_usd, "input_tokens": out.input_tokens, "output_tokens": out.output_tokens,
                    "cache_read_tokens": out.cache_read_tokens, "cache_create_tokens": out.cache_create_tokens, "workid": item.id}));
        }
    }
    if let Err(e) = api.delete(&format!("/api/projects/{}/workitems/{}/explain", project.name, item.id)) {
        eprintln!("[engine] could not clear explain_requested on {}: {e}", short(&item.id));
    }
    println!("[engine] explained {} '{}' in {secs}s ({})", short(&item.id), item.name,
        match &result { Ok(o) => format!("{}, ${:.3}", o.subtype, o.cost_usd), Err(_) => "failed".into() });
}

/// Connectivity nudge (spec: engine chip "test"): `claude -p "."` on haiku
/// with no other context, billed to `account`'s token when one is configured.
/// Proves the CLI + token work; its rate_limit_event line refreshes the
/// account's usage snapshot as a side effect.  A named account always comes
/// with its token (the caller resolved it, or refused to nudge); with no
/// token the run is the ambient CLI login and its numbers are filed under
/// the default snapshot, never under a real account's name (2026-09-11).
pub fn nudge(token: Option<String>, account: &str, cwd: &str) -> Result<(RunOut, u128), String> {
    debug_assert!(token.is_some() || account.is_empty(), "a named account must not nudge without its token");
    let usage_key = if token.is_some() { account } else { "" };
    let started = Instant::now();
    let mut cmd = Command::new("claude");
    cmd.arg("-p").arg(".").arg("--output-format").arg("stream-json").arg("--verbose").arg("--model").arg("haiku").arg("--max-turns").arg("1");
    if let Some(tok) = token {
        cmd.env("CLAUDE_CODE_OAUTH_TOKEN", tok);
    }
    cmd.env_remove("ANTHROPIC_API_KEY").env_remove("ANTHROPIC_AUTH_TOKEN");
    cmd.current_dir(cwd).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    let raw = wait_with_timeout(cmd, 120)?;
    Ok((parse_claude_stream(usage_key, &raw).1, started.elapsed().as_millis()))
}

/// The result object — the last line of stream-json, or all of
/// `--output-format json`: {"type":"result",
/// "subtype":"success"|"error_max_turns"|..., "result":"<final text>",
/// "num_turns":N, ...}.  Anything that is not that object is treated as
/// plain text output (older CLIs, test doubles).
fn parse_claude_json(raw: &str) -> RunOut {
    let trimmed = raw.trim();
    let candidate = trimmed
        .find('{')
        .map(|i| &trimmed[i..])
        .unwrap_or("");
    if let Ok(v) = serde_json::from_str::<Value>(candidate) {
        if v.get("type").and_then(|t| t.as_str()) == Some("result") || v.get("result").is_some() {
            let text = match v.get("result") {
                Some(Value::String(s)) => s.clone(),
                Some(other) if !other.is_null() => other.to_string(),
                _ => String::new(),
            };
            return RunOut {
                text,
                subtype: v.get("subtype").and_then(|s| s.as_str()).unwrap_or("success").to_string(),
                num_turns: v.get("num_turns").and_then(|n| n.as_u64()).unwrap_or(0),
                cost_usd: v.get("total_cost_usd").and_then(|c| c.as_f64()).unwrap_or(0.0),
                input_tokens: v.get("usage").and_then(|u| u.get("input_tokens")).and_then(|n| n.as_u64()).unwrap_or(0),
                output_tokens: v.get("usage").and_then(|u| u.get("output_tokens")).and_then(|n| n.as_u64()).unwrap_or(0),
                cache_read_tokens: v.get("usage").and_then(|u| u.get("cache_read_input_tokens")).and_then(|n| n.as_u64()).unwrap_or(0),
                cache_create_tokens: v.get("usage").and_then(|u| u.get("cache_creation_input_tokens")).and_then(|n| n.as_u64()).unwrap_or(0),
                session_id: v.get("session_id").and_then(|s| s.as_str()).unwrap_or("").to_string(),
            };
        }
    }
    RunOut::plain(raw.to_string())
}

fn run_shell(cwd: &str, script: &str, timeout_sec: u64) -> Result<String, String> {
    let mut cmd = Command::new("bash");
    cmd.arg("-c")
        .arg(script)
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    wait_with_timeout(cmd, timeout_sec)
}

/// An exec item's shell, with the same addressing the agent branch gives a
/// claude session (built 2026-09-09): `ITER_WORKID`, `ITER_PROJECT`,
/// `ITER_TOPDIR`, `ITER_DATA_URL`, `ITER_ENGINE_TOKEN` and the `iter` shim on
/// PATH — so a window script can POST a detail row on its own clone.
fn run_exec(api: &Api, project: &Project, topdir: &str, item: &WorkItem, timeout_sec: u64) -> Result<String, String> {
    let mut cmd = Command::new("bash");
    cmd.arg("-c")
        .arg(&item.exec_shell)
        .current_dir(topdir)
        .env("ITER_WORKID", &item.id)
        .env("ITER_PROJECT", &project.name)
        .env("ITER_AGENT", &item.agent)
        .env("ITER_TOPDIR", topdir)
        .env("ITER_DATA_URL", &api.base)
        .env("ITER_ENGINE_TOKEN", &api.token)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Ok(shim) = write_iter_shim(topdir) {
        cmd.env("ITER_BIN", &shim);
        if let Some(dir) = std::path::Path::new(&shim).parent() {
            cmd.env("PATH", format!("{}:{}", dir.display(), std::env::var("PATH").unwrap_or_default()));
        }
    }
    wait_with_timeout(cmd, timeout_sec)
}

fn git_head(topdir: &str) -> String {
    if !std::path::Path::new(topdir).join(".git").exists() {
        return String::new();
    }
    run_shell(topdir, "git rev-parse HEAD", 15).map(|s| s.trim().to_string()).unwrap_or_default()
}

fn wait_with_timeout(mut cmd: Command, timeout_sec: u64) -> Result<String, String> {
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0); // so a stop can take the whole tree down
    }
    let mut child = cmd.spawn().map_err(|e| format!("spawn failed: {e}"))?;
    // drain both pipes on their own threads: stream-json echoes every message,
    // and a full 64K pipe would block the child forever if read only at exit
    let slurp = |pipe: Option<Box<dyn std::io::Read + Send>>| -> Option<std::thread::JoinHandle<String>> {
        pipe.map(|mut s| std::thread::spawn(move || { let mut b = String::new(); let _ = s.read_to_string(&mut b); b }))
    };
    let mut out_h = slurp(child.stdout.take().map(|s| Box::new(s) as Box<dyn std::io::Read + Send>));
    let mut err_h = slurp(child.stderr.take().map(|s| Box::new(s) as Box<dyn std::io::Read + Send>));
    let deadline = Instant::now() + Duration::from_secs(timeout_sec.max(1));
    let workid = CURRENT_WORKID.with(|w| w.borrow().clone());
    loop {
        if !workid.is_empty() && STOP_REQUESTED.lock().map(|v| v.contains(&workid)).unwrap_or(false) {
            let pid = child.id();
            #[cfg(unix)]
            {
                let _ = Command::new("kill").args(["-TERM", "--", &format!("-{pid}")]).status();
            }
            let _ = child.kill();
            let _ = child.wait();
            return Err(STOPPED_BY_USER.into());
        }
        match child.try_wait() {
            Ok(Some(status)) => {
                let out = out_h.take().and_then(|h| h.join().ok()).unwrap_or_default();
                let err = err_h.take().and_then(|h| h.join().ok()).unwrap_or_default();
                if status.success() {
                    return Ok(out);
                }
                return Err(format!("exit {:?}: {}{}", status.code(), out, err));
            }
            Ok(None) => {
                if Instant::now() > deadline {
                    let _ = child.kill();
                    return Err(format!("timed out after {timeout_sec}s"));
                }
                std::thread::sleep(Duration::from_millis(200));
            }
            Err(e) => return Err(format!("wait failed: {e}")),
        }
    }
}

/// The gate's decision for a successful agent run.
/// How the close gate held an item.
enum GateHold {
    /// the verdict bounces it: requeue (or question once the budget is spent)
    Bounce { reason: String, to_question: bool },
    /// the record names open blockers: queued behind them, no bounce
    Waiting { reason: String, on: Vec<String> },
}

/// The blockers the item (re-read: the agent may have linked them this run)
/// still waits on — `gate::waiting_on` over the project's current items.
fn declared_blockers(api: &Api, project: &Project, item: &WorkItem) -> Vec<String> {
    let fresh: WorkItem = match api
        .get(&format!("/api/projects/{}/workitems/{}", project.name, item.id))
        .ok()
        .and_then(|v| serde_json::from_value(v).ok())
    {
        Some(i) => i,
        None => return vec![],
    };
    if fresh.blockedby.is_empty() {
        return vec![];
    }
    let items: Vec<WorkItem> = api
        .get(&format!("/api/projects/{}/workitems", project.name))
        .ok()
        .and_then(|v| serde_json::from_value(v).ok())
        .unwrap_or_default();
    gate::waiting_on(&fresh, &items)
}

enum GateOutcome {
    Pass,
    /// the deterministic half passed and the verifier could not run twice:
    /// close on the worker's evidence, with a "verify" row saying so
    PassUnverified { reason: String },
    /// held back: source ("deterministic" | "verifier"), open list, reason;
    /// `to_human` forces the question state regardless of bounce budget
    Hold { source: &'static str, open: Vec<String>, reason: String, to_human: bool },
}

/// Run the close gate: deterministic checks first (free); the verifier only
/// when they all pass and a verify model is configured.
fn run_gate(api: &Api, project: &Project, item: &WorkItem, out: &RunOut, ctx: &GateCtx) -> (GateOutcome, Evidence) {
    let head_after = git_head(&ctx.topdir);
    // the evidence is limited to the same scope the end-of-run commit was:
    // the range between the heads holds every sibling's commits too
    let scope = commit_scope(&ctx.topdir, item, &project.commit_extra_paths);
    let commit = LAST_COMMIT.with(|c| c.borrow_mut().take()).unwrap_or_default();
    let (diffstat, commits) = if !head_after.is_empty() && head_after != ctx.head_before && !ctx.head_before.is_empty() {
        git_run_evidence(&ctx.topdir, &ctx.head_before, &head_after, &scope, short(&item.id))
    } else {
        (String::new(), Vec::new())
    };
    // always counted (2026-09-07): the verifier reads this line as engine
    // fact, and a 0 printed for an item whose gate never asked for children
    // contradicted a worker that had really filed three
    let children = gate::children_of(&project_items(api, project), &item.id).len();
    let ev = Evidence {
        result_subtype: out.subtype.clone(),
        num_turns: out.num_turns,
        head_before: ctx.head_before.clone(),
        head_after,
        diffstat,
        children,
        open_reviews: gate::open_reviews(&ctx.details),
        commit_scope: if commit.scope_label.is_empty() {
            if scope.is_empty() { "whole tree (item has no lockdirs)".into() } else { format!("lock scope ({} paths)", scope.len()) }
        } else {
            commit.scope_label.clone()
        },
        outside_scope_dirty: commit.outside_scope_dirty,
        commits,
    };

    let mut open: Vec<String> = Vec::new();
    if out.subtype != "success" {
        open.push(format!("the agent session ended with '{}' (cut off, not finished)", out.subtype));
    }
    if ev.open_reviews > 0 {
        open.push(format!("{} review row(s) recorded without a disposition", ev.open_reviews));
    }
    if ctx.gate.requires_children && ev.children == 0 {
        open.push("no workitems were created by this item (closegate.requires_children)".into());
    }
    // `iter runtests --fixed` is the TDD completion gate: the LAST fixed-claim
    // must have been upheld (a later upheld claim clears an earlier false one)
    if let Some(c) = gate::last_fixed_claim(&ctx.details) {
        if !c.get("upheld").and_then(|b| b.as_bool()).unwrap_or(false) {
            open.push(format!(
                "the last `iter runtests --fixed` claim was FALSE: testgroup \"{}\" is {} ({})",
                c.get("group").and_then(|g| g.as_str()).unwrap_or("?"),
                c.get("outcome").and_then(|o| o.as_str()).unwrap_or("?"),
                c.get("counts").and_then(|o| o.as_str()).unwrap_or("?")
            ));
        }
    }
    if ctx.gate.requires_commit && !ev.committed() {
        open.push("no new git commit was produced (closegate.requires_commit)".into());
    }
    if !open.is_empty() {
        let reason = format!("deterministic close-gate check(s) failed: {}", open.join("; "));
        return (GateOutcome::Hold { source: "deterministic", open, reason, to_human: false }, ev);
    }

    if ctx.gate.verify.trim().is_empty() {
        return (GateOutcome::Pass, ev);
    }
    let prompt = gate::verifier_prompt(&item.name, &ctx.request, &out.text, &ev);
    let extra = vec![
        "--allowedTools".to_string(),
        "Read,Glob,Grep".to_string(),
        "--max-turns".to_string(),
        ctx.gate.verify_max_turns.max(1).to_string(),
    ];
    // a verifier that could not run is not a verdict about the work
    // (2026-09-07): try once more, then close on the worker's evidence with
    // the verify row marked "unavailable" — never the human queue
    let mut verdict = Verdict::Unavailable { reason: String::new() };
    for try_n in 1..=2 {
        match spawn_claude(project, &ctx.topdir, &ctx.account, &prompt, ctx.gate.verify.trim(), &extra, 600) {
            Ok(raw) => {
                verdict = gate::parse_verdict(&parse_claude_stream(&ctx.account, &raw).1.text);
                break;
            }
            Err(e) => {
                eprintln!("[engine] {} '{}': verifier session failed (try {try_n}/2): {}", short(&item.id), item.name, gate::clip(&e, 300));
                verdict = Verdict::Unavailable { reason: format!("verifier session failed twice: {}", gate::clip(&e, 500)) };
                if try_n == 1 {
                    std::thread::sleep(Duration::from_secs(3));
                }
            }
        }
    }
    match verdict {
        Verdict::Complete => (GateOutcome::Pass, ev),
        Verdict::Unavailable { reason } => (GateOutcome::PassUnverified { reason: format!("verifier unavailable: {reason}") }, ev),
        Verdict::Incomplete { open, reason } => (
            GateOutcome::Hold { source: "verifier", open, reason: format!("verifier: {reason}"), to_human: false },
            ev,
        ),
        Verdict::Unclear { reason } => (
            GateOutcome::Hold { source: "verifier", open: vec![], reason: format!("verifier unclear: {reason}"), to_human: true },
            ev,
        ),
    }
}

/// Close-out when the agent itself moved the item (question via `iter ask`,
/// parked via `iter reject`): record the response, keep the state, free locks.
fn close_keep_state(api: &Api, project: &Project, item: WorkItem, result: Result<RunOut, String>, state: &str) {
    let details_path = format!("/api/projects/{}/workitems/{}/details", project.name, item.id);
    let (key, text) = match &result {
        Ok(out) => ("response", out.text.clone()),
        Err(e) => ("error", e.clone()),
    };
    let _ = api.post(&details_path, &json!({"key": key, "valuetype": "text", "value": text}));
    for d in &item.lockdirs {
        let _ = api.post(&format!("/api/projects/{}/locks/release", project.name), &json!({"path": d, "workid": item.id}));
    }
    println!("[engine] done {} '{}' -> {} (set by the agent)", short(&item.id), item.name, state);
}

/// Returns true when the item closed COMPLETE (the only outcome a session may chain from).
fn close(api: &Api, _engine_name: &str, project: &Project, item: WorkItem, result: Result<RunOut, String>, ctx: Option<GateCtx>, chained_from: Option<&str>) -> bool {
    // detail rows are APPENDED (iter_data allocates the order atomically)
    let details_path = format!("/api/projects/{}/workitems/{}/details", project.name, item.id);
    let put_detail = |key: &str, valuetype: &str, value: Value| {
        if let Err(e) = api.post(&details_path, &json!({"key": key, "valuetype": valuetype, "value": value})) {
            eprintln!("[engine] could not append '{key}' detail to {}: {e}", item.id);
        }
    };

    let (ok, text) = match &result {
        Ok(out) => (true, out.text.clone()),
        Err(e) => (false, e.clone()),
    };
    put_detail(if ok { "response" } else { "error" }, "text", json!(text));

    // cost accounting: a "spend" row on the item + the project's daily total
    if let Ok(out) = &result {
        if out.cost_usd > 0.0 || out.input_tokens > 0 {
            put_detail("spend", "json", json!({"usd": out.cost_usd, "input_tokens": out.input_tokens, "output_tokens": out.output_tokens,
                "cache_read_tokens": out.cache_read_tokens, "cache_create_tokens": out.cache_create_tokens,
                "turns": out.num_turns, "agent": item.agent, "attempt": item.attempt,
                "chained_from": chained_from.unwrap_or("")}));
            let _ = api.post(&format!("/api/projects/{}/spend", project.name),
                &json!({"usd": out.cost_usd, "input_tokens": out.input_tokens, "output_tokens": out.output_tokens,
                    "cache_read_tokens": out.cache_read_tokens, "cache_create_tokens": out.cache_create_tokens, "workid": item.id}));
        }
    }
    let stopped = matches!(&result, Err(e) if e == STOPPED_BY_USER);
    if stopped {
        if let Ok(mut v) = STOP_REQUESTED.lock() {
            v.retain(|w| w != &item.id);
        }
    }

    // the close gate decides what "ok" closes to
    let mut gate_hold: Option<GateHold> = None;
    if let (Ok(out), Some(ctx)) = (&result, &ctx) {
        let (outcome, ev) = run_gate(api, project, &item, out, ctx);
        if let GateOutcome::PassUnverified { reason } = &outcome {
            put_detail("verify", "json", gate::verify_row(item.gate_bounces, "verifier", "unavailable", &[], reason, &ev));
            println!(
                "[engine] close gate: verifier could not run for {} '{}' — closing on the worker's evidence ({})",
                short(&item.id), item.name, gate::clip(reason, 200)
            );
        }
        if let GateOutcome::Hold { source, open, reason, to_human } = outcome {
            // a declared block wins (decided 2026-09-10): when the record now
            // names blockers that are still open — linked DURING this run,
            // since dispatch needed them satisfied — an incomplete verdict
            // queues the item behind them, counts no bounce and asks no human;
            // the dependency gate re-runs it once they close.  An "unclear"
            // verdict (to_human) still goes to the human.
            let waiting = if to_human { vec![] } else { declared_blockers(api, project, &item) };
            if !waiting.is_empty() {
                put_detail("verify", "json", gate::waiting_row(item.gate_bounces, source, &open, &reason, &waiting, &ev));
                println!(
                    "[engine] close gate: {} '{}' is waiting on {} open blocker(s) [{}] — queued behind them, no bounce counted: {}",
                    short(&item.id), item.name, waiting.len(),
                    waiting.iter().map(|w| short(w)).collect::<Vec<_>>().join(", "),
                    gate::clip(&reason, 160)
                );
                gate_hold = Some(GateHold::Waiting { reason: gate::clip(&reason, 300), on: waiting });
            } else {
            let bounce = item.gate_bounces + 1;
            let to_question = to_human || item.gate_bounces >= ctx.gate.max_bounces;
            put_detail("verify", "json", gate::verify_row(bounce, source, if to_human { "unclear" } else { "incomplete" }, &open, &reason, &ev));
            if to_question {
                put_detail("question", "json", gate::question_widget(&item.name, bounce, &reason, &open, &out.text));
            }
            println!(
                "[engine] close gate held {} '{}' (bounce {}, {}): {}",
                short(&item.id), item.name, bounce,
                if to_question { "-> question" } else { "-> queued" },
                gate::clip(&reason, 200)
            );
            gate_hold = Some(GateHold::Bounce { reason: gate::clip(&reason, 500), to_question });
            }
        }
    }

    // close with a versioned write; on conflict re-read and retry once
    for attempt in 0..2 {
        let fresh = if attempt == 0 {
            serde_json::to_value(&item).unwrap()
        } else {
            match api.get(&format!("/api/projects/{}/workitems/{}", project.name, item.id)) {
                Ok(v) => v,
                Err(_) => break,
            }
        };
        let version = fresh.get("version").and_then(|v| v.as_u64()).unwrap_or(item.version);
        let mut updated = fresh.clone();
        if stopped {
            // workitem_stop.md: parked for human review, never retried
            updated["state"] = json!("parked");
            updated["stop_requested"] = json!(false);
            updated["lasterror"] = json!(STOPPED_BY_USER);
        } else if let Some(GateHold::Bounce { reason, to_question }) = &gate_hold {
            updated["state"] = json!(if *to_question { "question" } else { "queued" });
            updated["gate_bounces"] = json!(item.gate_bounces + 1);
            updated["lasterror"] = json!(format!("close gate: {reason}"));
        } else if let Some(GateHold::Waiting { reason, on }) = &gate_hold {
            // queued behind the declared blockers; the dispatch dependency
            // gate holds it there and the bounce budget is untouched
            updated["state"] = json!("queued");
            updated["lasterror"] = json!(format!("close gate: waiting on {} open blocker(s) [{}] — {reason}", on.len(),
                on.iter().map(|w| &w[w.len().saturating_sub(12)..]).collect::<Vec<_>>().join(", ")));
        } else if ok {
            updated["state"] = json!("complete");
            updated["ts"]["complete"] = json!(now_utc());
            updated["lasterror"] = json!("");
        } else {
            let maxattempts = project.failure.maxattempts.max(1);
            updated["lasterror"] = json!(text.chars().take(2000).collect::<String>());
            if item.attempt >= maxattempts {
                updated["state"] = json!("failed");
                updated["ts"]["complete"] = json!(now_utc());
            } else {
                // retry after the project's backoff (V2 retry_backoff_sec)
                let delay = iter_core::retry_delay_sec(&project.failure, item.attempt);
                let until = chrono::Utc::now() + chrono::Duration::seconds(delay as i64);
                updated["state"] = json!("queued");
                updated["retry_after"] = json!(until.format("%Y-%m-%dT%H:%M:%SZ").to_string());
                println!("[engine] {} '{}': attempt {} failed, retry after {}s", short(&item.id), item.name, item.attempt, delay);
            }
        }
        match api.put(
            &format!(
                "/api/projects/{}/workitems/{}?expect_version={}",
                project.name, item.id, version
            ),
            &updated,
        ) {
            Ok(_) => break,
            Err(e) if e.status == 409 && attempt == 0 => continue,
            Err(e) => {
                eprintln!("[engine] close failed for {}: {e}", item.id);
                break;
            }
        }
    }

    // release every lock this workitem held
    for d in &item.lockdirs {
        let _ = api.post(
            &format!("/api/projects/{}/locks/release", project.name),
            &json!({"path": d, "workid": item.id}),
        );
    }
    println!(
        "[engine] done {} '{}' -> {}",
        short(&item.id),
        item.name,
        match (&gate_hold, ok) {
            _ if stopped => "parked (stopped by user)",
            (Some(GateHold::Bounce { to_question: true, .. }), _) => "question (close gate)",
            (Some(GateHold::Bounce { to_question: false, .. }), _) => "queued (close gate bounce)",
            (Some(GateHold::Waiting { .. }), _) => "queued (waiting on declared blockers)",
            (None, true) => "complete",
            (None, false) => "failed/retry",
        }
    );
    !stopped && gate_hold.is_none() && ok
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A throwaway git repo with one seed commit; returns its path.
    fn temp_repo(tag: &str) -> String {
        let dir = std::env::temp_dir().join(format!("iter3-scoped-commit-{}-{}", std::process::id(), tag));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("a")).unwrap();
        std::fs::create_dir_all(dir.join("b")).unwrap();
        std::fs::write(dir.join("README"), "seed\n").unwrap();
        std::fs::write(dir.join("a/.keep"), "").unwrap();
        std::fs::write(dir.join("b/.keep"), "").unwrap();
        let d = dir.to_string_lossy().into_owned();
        run_shell(&d, "git init -q && git add -A && git -c user.email=t@t -c user.name=t commit -qm seed && git config user.email t@t && git config user.name t", 30).unwrap();
        d
    }
    fn dirty(d: &str, rel: &str) {
        let p = std::path::Path::new(d).join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, format!("work {}\n", rel)).unwrap();
    }
    fn head_files(d: &str) -> Vec<String> {
        run_shell(d, "git show --format= --name-only HEAD", 15).unwrap().lines().map(String::from).filter(|l| !l.is_empty()).collect()
    }
    fn item_with(lockdirs: &[&str]) -> WorkItem {
        WorkItem { id: "11112222-3333-4444-5555-666677778888".into(), name: "scoped".into(), lockdirs: lockdirs.iter().map(|s| s.to_string()).collect(), ..Default::default() }
    }

    /// The end-of-run commit stays inside the item's lockdirs: a sibling's
    /// dirty file in another directory is neither committed nor listed.
    #[test]
    fn end_of_run_commit_stays_inside_lockdirs() {
        let d = temp_repo("t1");
        dirty(&d, "a/x.rs");
        dirty(&d, "b/y.rs");
        let item = item_with(&["{topdir}/a"]);
        let out = commit_scoped(&d, &item, &[]).unwrap();
        assert!(out.committed);
        assert_eq!(head_files(&d), vec!["a/x.rs".to_string()]);
        let status = run_shell(&d, "git status --porcelain", 15).unwrap();
        assert!(status.contains("b/y.rs"), "b/y.rs must still be dirty: {status}");
        assert_eq!(out.outside_scope_dirty, 1);
        assert!(out.scope_label.starts_with("lock scope"), "{}", out.scope_label);
        assert!(run_shell(&d, "git log -1 --format=%s", 15).unwrap().trim().ends_with("(11112222)"));
    }

    #[test]
    fn a_single_file_lockdir_is_a_valid_pathspec() {
        let d = temp_repo("t2");
        dirty(&d, "a/only.tsv");
        dirty(&d, "a/other.tsv");
        let item = item_with(&["{topdir}/a/only.tsv"]);
        let out = commit_scoped(&d, &item, &[]).unwrap();
        assert!(out.committed);
        assert_eq!(head_files(&d), vec!["a/only.tsv".to_string()]);
        assert_eq!(out.outside_scope_dirty, 1);
    }

    #[test]
    fn extra_paths_are_committed_with_the_scope() {
        let d = temp_repo("t3");
        dirty(&d, "a/x.rs");
        dirty(&d, "Agent_Recommendations.md");
        dirty(&d, "b/y.rs");
        dirty(&d, "b/deep/notes.agentmemory.iter.md");
        let item = item_with(&["{topdir}/a/"]);
        let out = commit_scoped(&d, &item, &["Agent_Recommendations.md".into(), "*.agentmemory.iter.md".into()]).unwrap();
        assert!(out.committed);
        let mut files = head_files(&d);
        files.sort();
        assert_eq!(files, vec!["Agent_Recommendations.md".to_string(), "a/x.rs".into(), "b/deep/notes.agentmemory.iter.md".into()]);
        assert_eq!(out.outside_scope_dirty, 1);
    }

    #[test]
    fn no_lockdirs_sweeps_and_says_so() {
        let d = temp_repo("t4");
        dirty(&d, "a/x.rs");
        dirty(&d, "b/y.rs");
        let item = item_with(&[]);
        let out = commit_scoped(&d, &item, &[]).unwrap();
        let mut files = head_files(&d);
        files.sort();
        assert_eq!(files, vec!["a/x.rs".to_string(), "b/y.rs".into()]);
        assert_eq!(out.scope_label, "whole tree (item has no lockdirs)");
        assert_eq!(out.outside_scope_dirty, 0);
    }

    #[test]
    fn leftover_paths_are_counted_not_listed() {
        let line = leftover_line("11112222", 1);
        assert!(line.contains("left 1 uncommitted path(s) outside its lock scope untouched"), "{line}");
        assert!(!line.contains("b/y.rs"));
        // nothing to commit inside the scope: not committed, the sibling's file counted
        let d = temp_repo("t5");
        dirty(&d, "b/y.rs");
        let out = commit_scoped(&d, &item_with(&["{topdir}/a"]), &[]).unwrap();
        assert!(!out.committed);
        assert_eq!(out.outside_scope_dirty, 1);
    }

    /// The evidence between two heads names only this item's scope and only
    /// the commits carrying its id, even when a sibling committed in between.
    #[test]
    fn evidence_diffstat_ignores_sibling_commits() {
        let d = temp_repo("t6");
        let before = git_head(&d);
        dirty(&d, "a/x.rs");
        run_shell(&d, "git add -A && git commit -qm 'iter: mine (11112222)'", 30).unwrap();
        dirty(&d, "b/y.rs");
        run_shell(&d, "git add -A && git commit -qm 'iter: sibling (99998888)'", 30).unwrap();
        let after = git_head(&d);
        let (diffstat, commits) = git_run_evidence(&d, &before, &after, &["a".into()], "11112222");
        assert!(diffstat.contains("a/x.rs"), "{diffstat}");
        assert!(!diffstat.contains("b/y.rs"), "{diffstat}");
        assert_eq!(commits.len(), 1);
        assert_eq!(commits[0].1, "iter: mine (11112222)");
        // whole tree when the item has no lockdirs
        let (all, _) = git_run_evidence(&d, &before, &after, &[], "11112222");
        assert!(all.contains("b/y.rs"));
    }

    #[test]
    fn commit_scope_is_relative_and_drops_outside_entries() {
        let item = WorkItem { lockdirs: vec!["{topdir}/src/".into(), "{topdir}/data/one.tsv".into(), "/elsewhere/x".into(), "{topdir}/src".into()], ..Default::default() };
        assert_eq!(commit_scope("/repo", &item, &["./Agent_Recommendations.md".into(), "{topdir}/*.agentmemory.iter.md".into(), "".into()]),
            vec!["src".to_string(), "data/one.tsv".into(), "Agent_Recommendations.md".into(), "*.agentmemory.iter.md".into()]);
        assert!(commit_scope("/repo", &WorkItem::default(), &["x".into()]).is_empty(), "no lockdirs = whole tree, extras irrelevant");
    }

    /// A named account whose variable is unset is an error naming the
    /// variable and the file — never another account's token (2026-09-11).
    #[test]
    fn a_named_account_without_a_token_is_an_error() {
        crate::envstore::set_for_test("WORK_T7_A_TOKEN", "tok-a");
        crate::envstore::unset_for_test("WORK_T7_B_TOKEN");
        let p = Project {
            accounts: vec![
                iter_core::Account { name: "A".into(), token_envar: "WORK_T7_A_TOKEN".into(), order: 1, ..Default::default() },
                iter_core::Account { name: "B".into(), token_envar: "WORK_T7_B_TOKEN".into(), order: 2, ..Default::default() },
            ],
            ..Default::default()
        };
        let f = "/x/.iter/.env";
        assert_eq!(resolve_account_token(&p, "A", f), Ok(Some("tok-a".into())));
        assert_eq!(resolve_account_token(&p, "B", f), Err("account 'B' has no token: WORK_T7_B_TOKEN is not set in /x/.iter/.env".into()));
        assert_eq!(resolve_account_token(&p, "", f), Ok(None), "no account = the ambient login");
        assert_eq!(resolve_account_token(&p, "Ghost", f), Err("account 'Ghost' has no token: (no token_envar configured) is not set in /x/.iter/.env".into()));
        assert_eq!(account_envar(&p, "B").as_deref(), Some("WORK_T7_B_TOKEN"));
        assert_eq!(account_envar(&p, "Ghost"), None);
    }

    #[test]
    fn claude_json_result_is_parsed_and_text_falls_back() {
        let out = parse_claude_json(r#"{"type":"result","subtype":"error_max_turns","num_turns":40,"result":"partial"}"#);
        assert_eq!((out.text.as_str(), out.subtype.as_str(), out.num_turns), ("partial", "error_max_turns", 40));
        let out = parse_claude_json("plain words from an older cli");
        assert_eq!(out.subtype, "success");
        assert_eq!(out.text, "plain words from an older cli");
        // leading noise before the object is tolerated
        let out = parse_claude_json("warn: x\n{\"type\":\"result\",\"subtype\":\"success\",\"result\":\"ok\"}");
        assert_eq!(out.text, "ok");
    }

    #[test]
    fn stream_json_yields_result_sid_and_writes_the_usage_snapshot() {
        let dir = std::env::temp_dir().join(format!("iter3-usage-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        // SAFETY: test-only; nothing else in this process reads ITER_USAGE_DIR concurrently
        unsafe { std::env::set_var("ITER_USAGE_DIR", &dir) };
        let raw = concat!(
            "{\"type\":\"system\",\"subtype\":\"init\",\"session_id\":\"sid-1\"}\n",
            "{\"type\":\"assistant\",\"message\":{\"content\":[{\"type\":\"text\",\"text\":\"hi\"}]}}\n",
            "{\"type\":\"rate_limit_event\",\"rate_limit_info\":{\"status\":\"allowed\",\"isUsingOverage\":false,",
            "\"unifiedWindows\":{\"five_hour\":{\"utilization\":0.25,\"resetsAt\":99999999999},",
            "\"seven_day\":{\"utilization\":0.5,\"resetsAt\":99999999999}}}}\n",
            "{\"type\":\"result\",\"subtype\":\"success\",\"session_id\":\"sid-1\",\"num_turns\":2,",
            "\"total_cost_usd\":0.01,\"usage\":{\"input_tokens\":10,\"output_tokens\":3,",
            "\"cache_read_input_tokens\":5000,\"cache_creation_input_tokens\":700},\"result\":\"done\"}\n"
        );
        let (sid, out) = parse_claude_stream("Acct", raw);
        assert_eq!(sid, "sid-1");
        assert_eq!((out.text.as_str(), out.subtype.as_str(), out.num_turns, out.input_tokens), ("done", "success", 2, 10));
        // the cached prompt tokens are the real context cost; the stream keeps
        // them out of `input_tokens`, so the spend row must carry them apart
        assert_eq!((out.cache_read_tokens, out.cache_create_tokens), (5000, 700));
        let u = crate::usage::read_usage("Acct").expect("snapshot written from the stream");
        assert!((u.five_hour_pct - 25.0).abs() < 1e-9 && (u.seven_day_pct - 50.0).abs() < 1e-9);
        assert_eq!(u.source, "stream");
        // a lone result object (fake claude / older cli) still parses
        let (sid, out) = parse_claude_stream("Acct", "{\"type\":\"result\",\"subtype\":\"success\",\"session_id\":\"s2\",\"result\":\"x\"}");
        assert_eq!((sid.as_str(), out.text.as_str()), ("s2", "x"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
