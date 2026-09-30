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
pub const AGENTMEMORY_AGENTS: &[&str] = &["code", "refactor", "test", "testwriter", "deploy"];

/// Stop requests the engine tick has seen for items this engine is running;
/// the wait loop kills the session the moment its workid appears.
/// Held by every engine step that writes a checkout's git index (end-of-run
/// commits, graph edits), so two of them never race for `.git/index.lock`.
pub static GIT_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
pub static STOP_REQUESTED: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());
pub const STOPPED_BY_USER: &str = "STOPPED by user mid-run";

/// Why the engine took a run away from its worker (lease-bound locks,
/// decided 2026-09-28).  Unlike a user's stop (parked for review), neither
/// is the item's fault, so neither counts as a failed attempt.
#[derive(Debug, Clone, PartialEq)]
pub enum Revoke {
    /// the run lost its locks and another item holds one of its paths now:
    /// kill the session and put the item back in the queue
    Requeue(String),
    /// iter_data refused this run's lease: the record carries another run's
    /// lease (or none), so another run owns the item — kill the session and
    /// leave the record alone (writing it would end the other run's locks)
    Abandon(String),
}

/// Revocations the engine tick has issued for runs of this process, keyed
/// by (workid, lease): only the run holding that lease acts on one, so a
/// refusal racing a normal close can never kill a LATER run of the same item.
/// The wait loop kills a session the moment its run appears; the close reads
/// (and removes) the entry.
pub static REVOKED: std::sync::Mutex<Vec<(String, String, Revoke)>> = std::sync::Mutex::new(Vec::new());
pub const REVOKED_PREFIX: &str = "REVOKED by the engine: ";

pub fn revoke(workid: &str, lease: &str, why: Revoke) {
    if let Ok(mut v) = REVOKED.lock() {
        // entries for older runs of this item are dead: drop them
        v.retain(|(w, l, _)| w != workid || l == lease);
        if !v.iter().any(|(w, l, _)| w == workid && l == lease) {
            v.push((workid.to_string(), lease.to_string(), why));
        }
    }
}

fn revoked(workid: &str, lease: &str) -> Option<Revoke> {
    REVOKED.lock().ok().and_then(|v| v.iter().find(|(w, l, _)| w == workid && l == lease).map(|(_, _, r)| r.clone()))
}

fn take_revoke(workid: &str, lease: &str) -> Option<Revoke> {
    let mut v = REVOKED.lock().ok()?;
    let i = v.iter().position(|(w, l, _)| w == workid && l == lease)?;
    Some(v.remove(i).2)
}
thread_local! {
    /// the workid the current worker thread is running (for the wait loop)
    static CURRENT_WORKID: std::cell::RefCell<String> = const { std::cell::RefCell::new(String::new()) };
    /// that run's lease (a revocation names the run it is for)
    static CURRENT_LEASE: std::cell::RefCell<String> = const { std::cell::RefCell::new(String::new()) };
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
/// `cur` is shared with the engine's renewal loop: it always holds the
/// (workid, lease) this thread is running now.
pub fn execute(api: &Api, engine_name: &str, project: &Project, topdir: &str, item: WorkItem, account: &str, cur: &std::sync::Mutex<(String, String)>) {
    let mut item = item;
    let mut prev: Option<crate::prompt::ChainPrev> = None;
    let max = project.session_chain_max;
    loop {
        let (complete, sid) = execute_one(api, engine_name, project, topdir, &item, account, prev.as_ref());
        let position = prev.as_ref().map(|p| p.position).unwrap_or(1);
        if !complete || item.is_shell() || sid.is_empty() || max <= 1 || position >= max {
            break;
        }
        match claim_chain_candidate(api, engine_name, project, &item) {
            Some(next) => {
                println!(
                    "[engine] chain: {} '{}' -> {} '{}' (same session, {} of {})",
                    short(&item.id), item.name, short(&next.id), next.name, position + 1, max
                );
                prev = Some(crate::prompt::ChainPrev { sid, prev_id: item.id.clone(), prev_name: item.name.clone(), position: position + 1, max });
                if let Ok(mut c) = cur.lock() {
                    *c = (next.id.clone(), next.lease.clone());
                }
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
    CURRENT_LEASE.with(|l| *l.borrow_mut() = item.lease.clone());
    let details = fetch_details(api, project, &item);

    // a human answered the close-gate widget "accept": close without running
    if !item.is_shell() && gate::accepted_by_human(&details) {
        println!("[engine] {} '{}': close-gate widget answered accept — closing without a run",
            short(&item.id), item.name);
        let out = RunOut::plain("closed complete by a human via the close-gate widget (accept)".into());
        close(api, engine_name, project, topdir, item, Ok(out), None, None);
        return (true, String::new());
    }

    let head_before = git_head(topdir);
    let result = run_all(api, project, topdir, &item, account, &details, chain);
    let sid = result.as_ref().map(|o| o.session_id.clone()).unwrap_or_default();
    // `iter ask` / `iter reject` move the item to question/parked mid-run; the
    // close must keep that state (and skip the gate) instead of completing it
    if !item.is_shell() {
        if let Ok(fresh) = api.get(&format!("/api/projects/{}/workitems/{}", project.name, item.id)) {
            let st = fresh.get("state").and_then(|s| s.as_str()).unwrap_or("");
            if st == "question" || st == "parked" {
                println!("[engine] {} '{}': agent moved it to {} during the run — keeping that", short(&item.id), item.name, st);
                close_keep_state(api, project, item, result, st);
                return (false, sid);
            }
        }
    }
    let ctx = if item.is_shell() {
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
    let complete = close(api, engine_name, project, topdir, item, result, ctx, chain.map(|c| c.prev_id.as_str()));
    (complete, sid)
}

/// Close an item a human accepted at the close gate (the engine claimed it
/// outside the cap, F11): the same close the in-run shortcut makes.
pub(crate) fn close_accepted(api: &Api, engine_name: &str, project: &Project, topdir: &str, item: WorkItem) {
    let out = RunOut::plain("closed complete by a human via the close-gate widget (accept)".into());
    close(api, engine_name, project, topdir, item, Ok(out), None, None);
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
            && !i.is_shell()
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
    // a new run, a new lease (CR 2026-09-25) — exactly as start_item does
    let lease = uuid::Uuid::new_v4().to_string();
    claimed["lease"] = json!(lease);
    let resp = api.put(&format!("/api/projects/{}/workitems/{}?expect_version={}", project.name, best.id, best.version), &claimed).ok()?;
    let claimed_item: WorkItem = serde_json::from_value(resp).ok()?;
    for d in &claimed_item.lockdirs {
        let res = api.post(
            &format!("/api/projects/{}/locks/acquire", project.name),
            &json!({"path": d, "kind": "lock", "engine": engine_name, "workid": claimed_item.id,
                    "lease": lease, "ttl_sec": iter_core::LOCK_LEASE_TTL_SEC}),
        );
        if res.is_err() {
            release_all(api, project, &claimed_item.id, &lease);
            let mut back = serde_json::to_value(&claimed_item).ok()?;
            back["state"] = json!("queued");
            back["lease"] = json!("");
            let _ = api.put(&format!("/api/projects/{}/workitems/{}?expect_version={}", project.name, claimed_item.id, claimed_item.version), &back);
            return None;
        }
    }
    Some(claimed_item)
}

fn short(id: &str) -> &str {
    &id[..8.min(id.len())]
}

/// Every lock row this run held, including rows the agent took itself
/// outside its lockdirs (CR 2026-09-25 I2): one `locks/release_all` for the
/// run's lease, retried with 1, 2 and 4 s of backoff.  Rows it cannot
/// release still die: the server deletes them when the close PUT clears the
/// lease, the sweep catches the rest, and nothing can renew them.
fn release_all(api: &Api, project: &Project, workid: &str, lease: &str) {
    let body = if lease.is_empty() { json!({"workid": workid}) } else { json!({"workid": workid, "lease": lease}) };
    for (n, wait) in [1u64, 2, 4, 0].iter().enumerate() {
        match api.post(&format!("/api/projects/{}/locks/release_all", project.name), &body) {
            Ok(_) => return,
            Err(e) if *wait == 0 || (e.status != 0 && e.status < 500) => {
                eprintln!("[engine] {}: releasing its locks failed (try {}): {e}", short(workid), n + 1);
                return;
            }
            Err(_) => std::thread::sleep(Duration::from_secs(*wait)),
        }
    }
}

/// A failure worth waiting out: the network (status 0) or the server (5xx).
pub(crate) fn is_transient(e: &crate::client::ApiError) -> bool {
    e.status == 0 || e.status >= 500
}

thread_local! {
    /// (first backoff, backoff cap, total budget) in ms for `with_retry`:
    /// 2 s doubling to 60 s for up to 10 minutes; tests shrink it
    static RETRY_POLICY: std::cell::Cell<(u64, u64, u64)> = const { std::cell::Cell::new((2_000, 60_000, 600_000)) };
}

/// Run `f` until it succeeds, fails with a non-transient error, or the retry
/// budget is spent (2026-09-22: every close-time write gave up on the first
/// transport error).  Returns the last result.
pub(crate) fn with_retry<T>(what: &str, mut f: impl FnMut() -> Result<T, crate::client::ApiError>) -> Result<T, crate::client::ApiError> {
    let (first, cap, budget) = RETRY_POLICY.with(|p| p.get());
    let start = Instant::now();
    let mut wait = first;
    let mut tries = 0u32;
    loop {
        match f() {
            Err(e) if is_transient(&e) && start.elapsed() + Duration::from_millis(wait) <= Duration::from_millis(budget) => {
                tries += 1;
                if tries == 1 {
                    eprintln!("[engine] {what}: {e} — retrying for up to {}s", budget / 1000);
                }
                std::thread::sleep(Duration::from_millis(wait));
                wait = (wait * 2).min(cap);
            }
            r => return r,
        }
    }
}

/// Where a close that could not reach iter_data waits for the next tick.
pub(crate) fn pending_close_dir(topdir: &str) -> std::path::PathBuf {
    std::path::Path::new(topdir).join(".iter").join("pending_close")
}

/// Write the intended final record (and the version it was built against)
/// to `<topdir>/.iter/pending_close/<workid>.json`: nothing the engine
/// decided is dropped — either the store has it or the disk does.
fn journal_close(topdir: &str, project: &str, workid: &str, expect_version: u64, lease: &str, record: &Value) -> Result<std::path::PathBuf, String> {
    let dir = pending_close_dir(topdir);
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let path = dir.join(format!("{workid}.json"));
    let body = json!({"project": project, "workid": workid, "expect_version": expect_version, "lease": lease,
                      "journaled": now_utc(), "record": record});
    std::fs::write(&path, serde_json::to_string_pretty(&body).unwrap()).map_err(|e| e.to_string())?;
    Ok(path)
}

/// Replay every journaled close under `topdir` (the tick calls it before
/// dispatch).  A record whose version still matches gets the journaled
/// write; one that moved on is superseded and dropped (the ghost repair
/// then judges the record); a replay that cannot reach iter_data stays for
/// the next tick.
pub(crate) fn replay_pending_closes(api: &Api, topdir: &str) {
    let Ok(entries) = std::fs::read_dir(pending_close_dir(topdir)) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|x| x.to_str()) != Some("json") {
            continue;
        }
        let Some(j) = std::fs::read_to_string(&path).ok().and_then(|t| serde_json::from_str::<Value>(&t).ok()) else {
            eprintln!("[engine] unreadable close journal {} — left in place", path.display());
            continue;
        };
        let s = |k: &str| j.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string();
        let (project, workid, lease) = (s("project"), s("workid"), s("lease"));
        let expect = j.get("expect_version").and_then(|v| v.as_u64()).unwrap_or(0);
        let item_path = format!("/api/projects/{project}/workitems/{workid}");
        let current = match api.get(&item_path) {
            Ok(v) => v,
            Err(e) if is_transient(&e) => continue,
            Err(_) => {
                let _ = std::fs::remove_file(&path); // the item is gone
                continue;
            }
        };
        let now_v = current.get("version").and_then(|v| v.as_u64()).unwrap_or(0);
        if now_v != expect {
            println!("[engine] journaled close for {workid} superseded (version {expect} -> {now_v})");
            let _ = std::fs::remove_file(&path);
            continue;
        }
        match api.put(&format!("{item_path}?expect_version={expect}"), &j["record"]) {
            Ok(_) => {
                println!("[engine] journaled close for {workid} replayed -> {}", j["record"]["state"].as_str().unwrap_or("?"));
                let _ = std::fs::remove_file(&path);
                let p = Project { name: project.clone(), ..Default::default() };
                release_all(api, &p, &workid, &lease);
            }
            Err(e) if is_transient(&e) => {}
            Err(e) => {
                println!("[engine] journaled close for {workid} dropped: {e}");
                let _ = std::fs::remove_file(&path);
            }
        }
    }
}

/// A ghost (2026-09-22): a record that says in-progress and names this
/// engine while no thread of this engine runs it and no journaled close is
/// waiting for it — and it started longer ago than its session timeout, so a
/// claim the cache has not caught up with is never mistaken for one.
pub(crate) fn is_ghost(item: &WorkItem, engine_name: &str, busy: &[String], journaled: bool, timeout_sec: u64, now: chrono::DateTime<chrono::Utc>) -> bool {
    if item.state != "in-progress" || item.engine != engine_name || busy.contains(&item.id) || journaled {
        return false;
    }
    match chrono::DateTime::parse_from_rfc3339(&item.ts.start) {
        Ok(t) => (now - t.with_timezone(&chrono::Utc)).num_seconds() > timeout_sec as i64,
        Err(_) => true, // no start stamp: nothing will ever come back for it
    }
}

/// Repair one ghost as a failed attempt: a doc row saying what was found,
/// then the same outcome a failed run gets (failed at maxattempts, else
/// queued behind the retry backoff), lease cleared, every lock released.
pub(crate) fn repair_ghost(api: &Api, engine_name: &str, project: &Project, item: &WorkItem) {
    let details = format!("/api/projects/{}/workitems/{}/details", project.name, item.id);
    let _ = api.post(&details, &json!({"key": "doc", "valuetype": "text", "value": format!(
        "engine {engine_name} found this record in-progress with no session behind it (last known: {}); treated as a failed attempt",
        if item.ts.start.is_empty() { "no start time" } else { item.ts.start.as_str() })}));
    let mut updated = serde_json::to_value(item).unwrap();
    updated["lease"] = json!("");
    updated["lasterror"] = json!("no session behind this in-progress record (the close was lost, or the engine restarted mid-run)");
    let maxattempts = project.failure.maxattempts.max(1);
    let to = if item.attempt >= maxattempts {
        updated["state"] = json!("failed");
        updated["ts"]["complete"] = json!(now_utc());
        "failed".to_string()
    } else {
        let delay = iter_core::retry_delay_sec(&project.failure, item.attempt);
        let until = chrono::Utc::now() + chrono::Duration::seconds(delay as i64);
        updated["state"] = json!("queued");
        updated["retry_after"] = json!(until.format("%Y-%m-%dT%H:%M:%SZ").to_string());
        format!("queued (retry after {delay}s)")
    };
    match api.put(&format!("/api/projects/{}/workitems/{}?expect_version={}", project.name, item.id, item.version), &updated) {
        Ok(_) => {
            println!("[engine] {} '{}': ghost in-progress repaired -> {to}", short(&item.id), item.name);
            release_all(api, project, &item.id, "");
        }
        Err(e) if e.status == 409 => {} // someone wrote first: the next tick re-judges
        Err(e) => eprintln!("[engine] {}: ghost repair failed: {e}", short(&item.id)),
    }
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

    let output = if item.is_shell() {
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
    // one index writer at a time in this engine: run threads and graph edits
    // share the checkout's .git/index (a second `git add` fails on index.lock)
    let _git = GIT_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let scope = commit_scope(topdir, item, extra_paths);
    let msg = format!("iter: {} ({})", sanitize(&item.name), short(&item.id));
    let (scope_label, committed) = if scope.is_empty() {
        let _ = git_shell(topdir, "git add -A", 60);
        let ok = git_shell(topdir, &format!("git commit -m '{msg}'"), 60).is_ok();
        ("whole tree (item has no lockdirs)".to_string(), ok)
    } else {
        let spec = scope.iter().map(|p| shell_quote(p)).collect::<Vec<_>>().join(" ");
        let _ = git_shell(topdir, &format!("git add -A -- {spec}"), 60);
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
            let r = git_shell(
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

/// What git says about one item for the close gate (F7-F9, 2026-09-28).
#[derive(Debug, Clone, Default)]
pub(crate) struct GitEvidence {
    /// `git show --stat` of this item's own commits, limited to the scope
    pub diffstat: String,
    /// (hash, subject): this item's commits over its whole life
    pub commits: Vec<(String, String)>,
    /// (path, hash): files its own commits changed outside the scope
    pub outside_scope_committed: Vec<(String, String)>,
    /// (hash, subject): other items' commits touching the scope this run
    pub other_scope_commits: Vec<(String, String)>,
}

/// Does `path` (relative to the checkout) fall inside one of the scope's
/// pathspecs — a directory, a file, or a glob from commit_extra_paths?
fn in_scope(path: &str, scope: &[String]) -> bool {
    scope.iter().any(|s| {
        let s = s.trim_end_matches('/');
        s == "." || path == s || path.starts_with(&format!("{s}/"))
            || glob::Pattern::new(s).map(|g| g.matches(path)).unwrap_or(false)
    })
}

/// The close gate's git evidence for one item.  Its own commits are every
/// commit since the item was received that carries its id (full id in the
/// message, or a `(first-8)` / `(last-12)` subject suffix), so work landed
/// by an earlier attempt still counts.  The diffstat is built from those
/// commits only, within the scope (whole tree when empty) — a sibling's
/// commit in the same folders is listed apart, never folded in — and the
/// files those commits changed outside the scope are named as this item's.
pub(crate) fn git_run_evidence(topdir: &str, head_before: &str, head_after: &str, scope: &[String], item: &WorkItem) -> GitEvidence {
    let spec = if scope.is_empty() {
        String::new()
    } else {
        format!(" -- {}", scope.iter().map(|p| shell_quote(p)).collect::<Vec<_>>().join(" "))
    };
    // the item's lifetime; without a receive stamp, this attempt's range
    let range = if item.ts.receive.is_empty() {
        format!("{head_before}..{head_after}")
    } else {
        format!("--since={} {head_after}", shell_quote(&item.ts.receive))
    };
    let log = run_shell(topdir, &format!("git log -n 2000 --format='{}' {range}", gate::COMMIT_LOG_FORMAT), 30).unwrap_or_default();
    let commits = gate::commits_with_id(&log, &item.id);
    let hashes: Vec<String> = commits.iter().map(|(h, _)| h.clone()).collect();
    let diffstat = if hashes.is_empty() {
        String::new()
    } else {
        run_shell(topdir, &format!("git show --stat=120 --format='%h %s' {}{spec}", hashes.join(" ")), 30)
            .map(|s| gate::clip(s.trim(), 4_000))
            .unwrap_or_default()
    };
    let mut outside: Vec<(String, String)> = Vec::new();
    if !scope.is_empty() {
        for h in &hashes {
            let files = run_shell(topdir, &format!("git show --name-only --no-renames --format= {h}"), 30).unwrap_or_default();
            for f in files.lines().map(str::trim).filter(|f| !f.is_empty()) {
                if !in_scope(f, scope) && !outside.iter().any(|(p, _)| p == f) {
                    outside.push((f.to_string(), h.clone()));
                }
            }
        }
    }
    let others = if head_before.is_empty() || head_before == head_after {
        Vec::new()
    } else {
        run_shell(topdir, &format!("git log --format=%h%x09%s {head_before}..{head_after}{spec}"), 30)
            .unwrap_or_default()
            .lines()
            .filter_map(|l| l.split_once('\t'))
            .filter(|(h, _)| !hashes.contains(&h.trim().to_string()))
            .map(|(h, s)| (h.trim().to_string(), s.trim().to_string()))
            .collect()
    };
    GitEvidence { diffstat, commits, outside_scope_committed: outside, other_scope_commits: others }
}

/// Session timeout: the project's per-agent override, else the agent record's
/// `timeoutsec` (the Settings field), else 3600.
pub(crate) fn agent_timeout(project: &Project, item: &WorkItem, agent_def: &Value) -> u64 {
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
    let mut extra: Vec<String> = flags.split_whitespace().map(String::from).collect();
    // every agent session gets the `iter` MCP server (work items, the map,
    // GraphRAG search); the guard deletes the config when the run ends
    let _mcp = mcp_config(api, &project.name, &item.id);
    if let Some(m) = &_mcp {
        extra.extend(m.args());
    }
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
/// The read-only tools of the `iter` MCP server, for sessions whose tool set
/// is fixed (ELI5, the close-gate verifier): search and look, never change.
pub(crate) const MCP_READ_TOOLS: &str = "mcp__iter__rag_search,mcp__iter__rag_docs,mcp__iter__rag_doc,mcp__iter__rag_status,\
mcp__iter__graph_lookup,mcp__iter__graph_node,mcp__iter__graph_neighbors,mcp__iter__graph_owner,mcp__iter__graph_usecase,mcp__iter__graph_stats,\
mcp__iter__workitem_get,mcp__iter__workitem_details,mcp__iter__workitem_list,mcp__iter__status,mcp__iter__capability";

/// The `iter` MCP server for one session (GraphRAG, 2026-09-29): iter_data's
/// stateless `POST /mcp`, with this engine's token and the calling item as
/// headers, so an agent searches the project's knowledge and files work as
/// tools. Written to the system temp directory (never the checkout, so the
/// token can never be committed), owner-only, removed when dropped.
pub(crate) struct McpConfig(std::path::PathBuf);

impl McpConfig {
    pub(crate) fn args(&self) -> Vec<String> {
        vec!["--mcp-config".to_string(), self.0.to_string_lossy().into_owned()]
    }
}

impl Drop for McpConfig {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

pub(crate) fn mcp_config(api: &Api, project: &str, workid: &str) -> Option<McpConfig> {
    if api.base.is_empty() || api.token.is_empty() {
        return None;
    }
    let dir = std::env::temp_dir().join("iter-mcp");
    std::fs::create_dir_all(&dir).ok()?;
    let path = dir.join(format!("{}-{}.json", workid.chars().filter(|c| c.is_ascii_alphanumeric() || *c == '-').collect::<String>(), uuid::Uuid::new_v4().simple()));
    let cfg = json!({"mcpServers": {"iter": {"type": "http", "url": format!("{}/mcp", api.base.trim_end_matches('/')),
        "headers": {"Authorization": format!("Bearer {}", api.token), "X-Iter-Project": project, "X-Iter-Workid": workid}}}});
    #[cfg(unix)]
    {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        let mut f = std::fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(&path).ok()?;
        f.write_all(cfg.to_string().as_bytes()).ok()?;
    }
    #[cfg(not(unix))]
    std::fs::write(&path, cfg.to_string()).ok()?;
    Some(McpConfig(path))
}

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
    let _mcp = mcp_config(api, &project.name, &item.id);
    let mut extra = vec![
        "--allowedTools".to_string(),
        if _mcp.is_some() { format!("Read,Glob,Grep,{MCP_READ_TOOLS}") } else { "Read,Glob,Grep".to_string() },
        "--disallowedTools".to_string(),
        "Bash,Edit,Write,MultiEdit,NotebookEdit,WebFetch,WebSearch,Agent".to_string(),
        "--max-turns".to_string(),
        "40".to_string(),
    ];
    if let Some(m) = &_mcp {
        extra.extend(m.args());
    }
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

/// A git step that writes the index: a lock held by a git process outside
/// this engine (an agent's own `git` in its session) is waited out for up to
/// ~10 s instead of failing the commit.
fn git_shell(cwd: &str, script: &str, timeout_sec: u64) -> Result<String, String> {
    let mut attempt = 0;
    loop {
        match run_shell(cwd, script, timeout_sec) {
            Err(e) if e.contains("index.lock") && attempt < 20 => {
                attempt += 1;
                std::thread::sleep(std::time::Duration::from_millis(500));
            }
            r => return r,
        }
    }
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
        // a shell run (exec, or a deterministic `test` item): the verbs it calls
        // act as the queue's own tooling, not as an agent iterating on code
        .env("ITER_SHELL", "1")
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
    let lease = CURRENT_LEASE.with(|l| l.borrow().clone());
    loop {
        let stop = !workid.is_empty() && STOP_REQUESTED.lock().map(|v| v.contains(&workid)).unwrap_or(false);
        let revoke = if workid.is_empty() { None } else { revoked(&workid, &lease) };
        if stop || revoke.is_some() {
            let pid = child.id();
            #[cfg(unix)]
            {
                let _ = Command::new("kill").args(["-TERM", "--", &format!("-{pid}")]).status();
            }
            let _ = child.kill();
            let _ = child.wait();
            return Err(match revoke {
                Some(Revoke::Requeue(n)) | Some(Revoke::Abandon(n)) if !stop => format!("{REVOKED_PREFIX}{n}"),
                _ => STOPPED_BY_USER.into(),
            });
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
                    // the whole process group, exactly as a stop does: a
                    // timed-out session's background builders and loops must
                    // not keep writing to the shared checkout (CR 2026-09-25
                    // 2.3 — before, only the direct child was killed)
                    #[cfg(unix)]
                    {
                        let _ = Command::new("kill").args(["-TERM", "--", &format!("-{}", child.id())]).status();
                    }
                    let _ = child.kill();
                    let _ = child.wait();
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

/// The `doc` rows present now that were not before the run (by `order`).
fn new_doc_rows(before: &[Value], now: &[Value]) -> Vec<String> {
    let max_before = before.iter().filter_map(|d| d.get("order").and_then(|o| o.as_i64())).max().unwrap_or(-1);
    now.iter()
        .filter(|d| d.get("key").and_then(|k| k.as_str()) == Some("doc"))
        .filter(|d| d.get("order").and_then(|o| o.as_i64()).unwrap_or(-1) > max_before)
        .filter_map(|d| d.get("value").and_then(|v| v.as_str()).map(String::from))
        .collect()
}

enum GateOutcome {
    Pass,
    /// the deterministic half passed and the verifier could not run twice:
    /// close on the worker's evidence, with a "verify" row saying so
    PassUnverified { reason: String },
    /// held back: source ("deterministic" | "verifier"), open list, reason;
    /// `to_human` forces the question state regardless of bounce budget
    Hold { source: &'static str, open: Vec<String>, reason: String, to_human: bool, advice: gate::Advice },
}

/// Run the close gate: deterministic checks first (free); the verifier only
/// when they all pass and a verify model is configured.
fn run_gate(api: &Api, project: &Project, item: &WorkItem, out: &RunOut, ctx: &GateCtx) -> (GateOutcome, Evidence) {
    // the detail rows as they stand NOW (2026-09-22): the run itself may have
    // written a passing `iter runtests --fixed` claim or dispositioned a
    // review; judging the rows fetched before the run bounced fixed work
    let fresh = fetch_details(api, project, item);
    let details: &[Value] = if fresh.is_empty() { &ctx.details } else { &fresh };
    let head_after = git_head(&ctx.topdir);
    // the evidence is limited to the same scope the end-of-run commit was:
    // the range between the heads holds every sibling's commits too
    let scope = commit_scope(&ctx.topdir, item, &project.commit_extra_paths);
    let commit = LAST_COMMIT.with(|c| c.borrow_mut().take()).unwrap_or_default();
    // a repository: this item's own commits over its whole life, not just
    // the range between the heads (an earlier attempt's work counts)
    let git = if !head_after.is_empty() { git_run_evidence(&ctx.topdir, &ctx.head_before, &head_after, &scope, item) } else { GitEvidence::default() };
    // always counted (2026-09-07): the verifier reads this line as engine
    // fact, and a 0 printed for an item whose gate never asked for children
    // contradicted a worker that had really filed three
    let children = gate::children_of(&project_items(api, project), &item.id).len();
    let ev = Evidence {
        result_subtype: out.subtype.clone(),
        num_turns: out.num_turns,
        head_before: ctx.head_before.clone(),
        head_after,
        diffstat: git.diffstat,
        children,
        open_reviews: gate::open_reviews(details),
        commit_scope: if commit.scope_label.is_empty() {
            if scope.is_empty() { "whole tree (item has no lockdirs)".into() } else { format!("lock scope ({} paths)", scope.len()) }
        } else {
            commit.scope_label.clone()
        },
        outside_scope_dirty: commit.outside_scope_dirty,
        commits: git.commits,
        outside_scope_committed: git.outside_scope_committed,
        other_scope_commits: git.other_scope_commits,
        notes: new_doc_rows(&ctx.details, details),
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
    if let Some(c) = gate::last_fixed_claim(details) {
        if !c.get("upheld").and_then(|b| b.as_bool()).unwrap_or(false) {
            open.push(format!(
                "the last `iter runtests --fixed` claim (at {}) was FALSE: testgroup \"{}\" is {} ({})",
                c.get("ts").and_then(|t| t.as_str()).unwrap_or("?"),
                c.get("group").and_then(|g| g.as_str()).unwrap_or("?"),
                c.get("outcome").and_then(|o| o.as_str()).unwrap_or("?"),
                c.get("counts").and_then(|o| o.as_str()).unwrap_or("?")
            ));
        }
    }
    // a commit from an earlier attempt satisfies it: the work is in the tree
    if ctx.gate.requires_commit && !ev.has_work() {
        open.push("no new git commit was produced (closegate.requires_commit)".into());
    }
    if !open.is_empty() {
        let reason = format!("deterministic close-gate check(s) failed: {}", open.join("; "));
        let advice = gate::deterministic_advice(&open);
        return (GateOutcome::Hold { source: "deterministic", open, reason, to_human: false, advice }, ev);
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
    let label = format!("{} '{}'", short(&item.id), item.name);
    let verdict = verify_with_retry(&label, &prompt, |p| {
        spawn_claude(project, &ctx.topdir, &ctx.account, p, ctx.gate.verify.trim(), &extra, 600)
            .map(|raw| parse_claude_stream(&ctx.account, &raw).1.text)
    });
    match verdict {
        Verdict::Complete => (GateOutcome::Pass, ev),
        Verdict::Unavailable { reason } => (GateOutcome::PassUnverified { reason: format!("verifier unavailable: {reason}") }, ev),
        Verdict::Incomplete { open, reason, advice } => (
            GateOutcome::Hold { source: "verifier", open, reason: format!("verifier: {reason}"), to_human: false, advice },
            ev,
        ),
        Verdict::Unclear { reason, advice } => (
            GateOutcome::Hold { source: "verifier", open: vec![], reason: format!("verifier unclear: {reason}"), to_human: true, advice },
            ev,
        ),
        Verdict::Unparsed { reason } => {
            let advice = gate::unparsed_advice(&ev);
            (GateOutcome::Hold { source: "verifier", open: vec![], reason: format!("verifier output did not parse twice: {reason}"), to_human: true, advice }, ev)
        }
    }
}

/// Run the verifier at most twice.  A session that could not run is not a
/// verdict about the work (2026-09-07): after two failures the result is
/// `Unavailable` and the item closes on the worker's evidence.  An answer
/// with no parseable verdict (R4, 2026-09-12) is retried once with a plain
/// "one JSON object" instruction before a human is asked (`Unparsed`).
fn verify_with_retry(label: &str, prompt: &str, mut run: impl FnMut(&str) -> Result<String, String>) -> Verdict {
    let mut verdict = Verdict::Unavailable { reason: String::new() };
    let mut p = prompt.to_string();
    for try_n in 1..=2 {
        match run(&p) {
            Ok(text) => {
                verdict = gate::parse_verdict(&text);
                match &verdict {
                    Verdict::Unparsed { reason } if try_n == 1 => {
                        eprintln!("[engine] {label}: verifier output did not parse ({}) — asking once more", gate::clip(reason, 200));
                        p = format!("{prompt}\n\nYour previous answer did not parse. Answer with exactly one JSON object and nothing else.");
                    }
                    _ => break,
                }
            }
            Err(e) => {
                eprintln!("[engine] {label}: verifier session failed (try {try_n}/2): {}", gate::clip(&e, 300));
                verdict = Verdict::Unavailable { reason: format!("verifier session failed twice: {}", gate::clip(&e, 500)) };
                if try_n == 1 && !cfg!(test) {
                    std::thread::sleep(Duration::from_secs(3));
                }
            }
        }
    }
    verdict
}

/// Put a revoked run's item back in the queue: lease cleared (the server then
/// deletes whatever rows the run still had), the attempt given back.  A
/// versioned write, re-read and retried once on a race.
fn requeue_after_revoke(api: &Api, project: &Project, item: &WorkItem, note: &str) {
    let path = format!("/api/projects/{}/workitems/{}", project.name, item.id);
    for _ in 0..2 {
        let Ok(mut fresh) = with_retry(&format!("re-read {}", short(&item.id)), || api.get(&path)) else { return };
        // only this run's record: a record that moved on (another lease, a
        // human edit to another state) is not ours to requeue
        if fresh.get("lease").and_then(|l| l.as_str()).unwrap_or("") != item.lease
            || fresh.get("state").and_then(|s| s.as_str()) != Some("in-progress")
        {
            return;
        }
        let version = fresh.get("version").and_then(|v| v.as_u64()).unwrap_or(0);
        fresh["state"] = json!("queued");
        fresh["lease"] = json!("");
        fresh["attempt"] = json!(item.attempt.saturating_sub(1));
        fresh["lasterror"] = json!(format!("re-queued by the engine: {note}"));
        match with_retry(&format!("requeue {}", short(&item.id)), || api.put(&format!("{path}?expect_version={version}"), &fresh)) {
            Ok(_) => return,
            Err(e) if e.status == 409 => continue,
            Err(e) => {
                eprintln!("[engine] could not re-queue {}: {e}", item.id);
                return;
            }
        }
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
    // the run is over: clear its lease on the record the agent already moved
    // (the server then deletes the lease's rows), retrying once on a race
    for _ in 0..2 {
        let Ok(mut fresh) = api.get(&format!("/api/projects/{}/workitems/{}", project.name, item.id)) else { break };
        if fresh.get("lease").and_then(|l| l.as_str()).unwrap_or("") != item.lease {
            break; // a newer run (or a human) owns the record now
        }
        let version = fresh.get("version").and_then(|v| v.as_u64()).unwrap_or(0);
        fresh["lease"] = json!("");
        match api.put(&format!("/api/projects/{}/workitems/{}?expect_version={version}", project.name, item.id), &fresh) {
            Err(e) if e.status == 409 => continue,
            _ => break,
        }
    }
    release_all(api, project, &item.id, &item.lease);
    println!("[engine] done {} '{}' -> {} (set by the agent)", short(&item.id), item.name, state);
}

/// Returns true when the item closed COMPLETE (the only outcome a session may chain from).
#[allow(clippy::too_many_arguments)]
fn close(api: &Api, _engine_name: &str, project: &Project, topdir: &str, item: WorkItem, result: Result<RunOut, String>, ctx: Option<GateCtx>, chained_from: Option<&str>) -> bool {
    // detail rows are APPENDED (iter_data allocates the order atomically);
    // an outage is ridden out (2026-09-22) so the response/error row exists
    // before the state flips
    let details_path = format!("/api/projects/{}/workitems/{}/details", project.name, item.id);
    let put_detail = |key: &str, valuetype: &str, value: Value| {
        let body = json!({"key": key, "valuetype": valuetype, "value": value});
        if let Err(e) = with_retry(&format!("append '{key}' detail to {}", short(&item.id)), || api.post(&details_path, &body)) {
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
    // a revocation that arrived after the session ended still applies: the
    // run no longer owns its paths (Requeue) or the item (Abandon)
    match take_revoke(&item.id, &item.lease) {
        Some(Revoke::Abandon(note)) => {
            put_detail("doc", "text", json!(format!("run abandoned by engine: {note} Nothing from this run was committed and the record was left to the run that owns it.")));
            println!("[engine] {} '{}': run abandoned — {note}", short(&item.id), item.name);
            // rows stamped with THIS run's lease only — never the owning run's
            release_all(api, project, &item.id, &item.lease);
            return false;
        }
        Some(Revoke::Requeue(note)) => {
            put_detail("doc", "text", json!(format!("run stopped by engine and re-queued: {note} Nothing from this run was committed; the attempt is not counted.")));
            println!("[engine] {} '{}': run stopped and re-queued — {note}", short(&item.id), item.name);
            requeue_after_revoke(api, project, &item, &note);
            release_all(api, project, &item.id, &item.lease);
            return false;
        }
        None => {}
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
        if let GateOutcome::Hold { source, open, reason, to_human, advice } = outcome {
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
                put_detail("question", "json", gate::question_widget(&item.name, bounce, ctx.gate.max_bounces, &reason, &open, &out.text, &advice, &ev));
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

    // close with a versioned write; on conflict re-read and retry once.  A
    // transport error or a 5xx is retried with backoff (with_retry), and a
    // write that still has not landed is journaled to disk for the tick to
    // replay (2026-09-22: a close lost in a network outage left two "ghost"
    // in-progress records holding cap slots for 7 h)
    let item_path = format!("/api/projects/{}/workitems/{}", project.name, item.id);
    for attempt in 0..2 {
        let fresh = if attempt == 0 {
            serde_json::to_value(&item).unwrap()
        } else {
            match with_retry(&format!("re-read {}", short(&item.id)), || api.get(&item_path)) {
                Ok(v) => v,
                Err(_) => break,
            }
        };
        let version = fresh.get("version").and_then(|v| v.as_u64()).unwrap_or(item.version);
        let mut updated = fresh.clone();
        // every exit from the run ends its lease (CR 2026-09-25): the server
        // deletes the lease's lock rows when this PUT lands
        updated["lease"] = json!("");
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
        let put_path = format!("{item_path}?expect_version={version}");
        match with_retry(&format!("close {}", short(&item.id)), || api.put(&put_path, &updated)) {
            Ok(_) => break,
            Err(e) if e.status == 409 && attempt == 0 => continue,
            Err(e) if is_transient(&e) => {
                eprintln!("[engine] close failed for {}: {e}", item.id);
                match journal_close(topdir, &project.name, &item.id, version, &item.lease, &updated) {
                    Ok(path) => println!("[engine] close journaled for {} ({})", item.id, path.display()),
                    Err(je) => eprintln!("[engine] close LOST for {}: cannot journal it ({je})", item.id),
                }
                break;
            }
            Err(e) => {
                eprintln!("[engine] close failed for {}: {e}", item.id);
                break;
            }
        }
    }

    // release every lock this run held — lockdirs and anything the agent took
    release_all(api, project, &item.id, &item.lease);
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

    #[test]
    fn notes_the_run_recorded_reach_the_gate() {
        let before = vec![json!({"order": 0, "key": "request", "value": "do it"}), json!({"order": 1, "key": "doc", "value": "old note"})];
        let mut now = before.clone();
        now.push(json!({"order": 2, "key": "doc", "value": "the answer"}));
        now.push(json!({"order": 3, "key": "spend", "value": {"usd": 1}}));
        assert_eq!(new_doc_rows(&before, &now), vec!["the answer".to_string()]);
        let ev = crate::gate::Evidence { notes: vec!["the answer".into()], ..Default::default() };
        assert!(ev.describe_for_test().contains("during this attempt, verbatim from the item's record: 1\n    the answer"));
    }

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
        let ev = git_run_evidence(&d, &before, &after, &["a".into()], &item_with(&["{topdir}/a"]));
        assert!(ev.diffstat.contains("a/x.rs"), "{}", ev.diffstat);
        assert!(!ev.diffstat.contains("b/y.rs"), "{}", ev.diffstat);
        assert_eq!(ev.commits.len(), 1);
        assert_eq!(ev.commits[0].1, "iter: mine (11112222)");
        assert!(ev.other_scope_commits.is_empty(), "the sibling touched b/, not this scope");
        // whole tree when the item has no lockdirs: still only its own commits
        let all = git_run_evidence(&d, &before, &after, &[], &item_with(&[]));
        assert!(all.diffstat.contains("a/x.rs") && !all.diffstat.contains("b/y.rs"), "{}", all.diffstat);
    }

    fn commit(d: &str, rel: &str, msg: &str) {
        dirty(d, rel);
        run_shell(d, &format!("git add -A && git commit -qm {}", shell_quote(msg)), 30).unwrap();
    }

    /// F7: attempt 1's commit (id only in the body) and attempt 2's commit
    /// are both this item's evidence, though only attempt 2 lies between
    /// the heads.  Fails before F7: the range and the (id8)-subject rule
    /// saw attempt 2 only.
    #[test]
    fn evidence_spans_every_attempt_and_the_id_in_the_body() {
        let d = temp_repo("f7");
        let mut item = item_with(&["{topdir}/a"]);
        item.ts.receive = "2000-01-01T00:00:00Z".into();
        commit(&d, "a/first.rs", &format!("attempt one work\n\nfor {}", item.id));
        let before = git_head(&d);
        commit(&d, "a/second.rs", "iter: scoped (11112222)");
        let after = git_head(&d);
        let ev = git_run_evidence(&d, &before, &after, &["a".into()], &item);
        let subjects: Vec<&str> = ev.commits.iter().map(|(_, s)| s.as_str()).collect();
        assert_eq!(subjects, vec!["iter: scoped (11112222)", "attempt one work"]);
        assert!(ev.diffstat.contains("a/first.rs") && ev.diffstat.contains("a/second.rs"), "{}", ev.diffstat);
    }

    /// F8: a file this item committed outside its lock scope (a ledger row)
    /// is named as committed by it, never as "left uncommitted".
    #[test]
    fn a_path_committed_outside_the_scope_is_this_items_evidence() {
        let d = temp_repo("f8");
        let item = item_with(&["{topdir}/a"]);
        let before = git_head(&d);
        dirty(&d, "a/x.rs");
        commit(&d, "LEDGER.md", "iter: scoped (11112222)");
        let after = git_head(&d);
        let ev = git_run_evidence(&d, &before, &after, &["a".into()], &item);
        assert_eq!(ev.outside_scope_committed.len(), 1);
        assert_eq!(ev.outside_scope_committed[0].0, "LEDGER.md");
        let evidence = gate::Evidence { head_before: before, head_after: after, diffstat: ev.diffstat, commits: ev.commits,
            outside_scope_committed: ev.outside_scope_committed, commit_scope: "lock scope (1 paths)".into(), ..Default::default() };
        let text = evidence.describe_for_test();
        assert!(text.contains("Committed by this item outside its lock scope: LEDGER.md"), "{text}");
        assert!(!text.contains("LEDGER.md were left uncommitted") && !text.contains("left uncommitted"), "{text}");
    }

    /// F9: a sibling's commit inside this item's folders is not in the
    /// diffstat; it is listed as another item's commit.
    #[test]
    fn a_siblings_commit_in_the_same_scope_is_listed_apart() {
        let d = temp_repo("f9");
        let item = item_with(&["{topdir}/a"]);
        let before = git_head(&d);
        commit(&d, "a/mine.rs", "iter: scoped (11112222)");
        commit(&d, "a/theirs.rs", "iter: sibling (99998888)");
        let after = git_head(&d);
        let ev = git_run_evidence(&d, &before, &after, &["a".into()], &item);
        assert!(ev.diffstat.contains("a/mine.rs") && !ev.diffstat.contains("a/theirs.rs"), "{}", ev.diffstat);
        assert_eq!(ev.other_scope_commits.len(), 1);
        assert_eq!(ev.other_scope_commits[0].1, "iter: sibling (99998888)");
    }

    #[test]
    fn commit_scope_is_relative_and_drops_outside_entries() {
        let item = WorkItem { lockdirs: vec!["{topdir}/src/".into(), "{topdir}/data/one.tsv".into(), "/elsewhere/x".into(), "{topdir}/src".into()], ..Default::default() };
        assert_eq!(commit_scope("/repo", &item, &["./Agent_Recommendations.md".into(), "{topdir}/*.agentmemory.iter.md".into(), "".into()]),
            vec!["src".to_string(), "data/one.tsv".into(), "Agent_Recommendations.md".into(), "*.agentmemory.iter.md".into()]);
        assert!(commit_scope("/repo", &WorkItem::default(), &["x".into()]).is_empty(), "no lockdirs = whole tree, extras irrelevant");
    }

    /// F6 (CR 2.3): a timed-out session's background processes die with it
    /// — the whole process group is signalled, as a stop does.  Before, only
    /// the direct child was killed and the background `sleep` survived.
    #[cfg(unix)]
    #[test]
    fn timeout_kills_the_whole_process_group() {
        let marker = format!("sleep {}", 3000 + std::process::id() % 997);
        let mut cmd = Command::new("bash");
        cmd.arg("-c").arg(format!("{marker} & sleep 30")).stdout(Stdio::piped()).stderr(Stdio::piped());
        let r = wait_with_timeout(cmd, 1);
        assert!(r.unwrap_err().contains("timed out after 1s"));
        std::thread::sleep(Duration::from_millis(500));
        let alive = Command::new("pgrep").args(["-f", &marker]).output().map(|o| !o.stdout.is_empty()).unwrap_or(false);
        if alive {
            let _ = Command::new("pkill").args(["-f", &marker]).status();
        }
        assert!(!alive, "the background `{marker}` outlived the timeout");
    }

    /// F1 (CR 5.3 fact 2, 8.2): every exit from a run clears the lease in
    /// its final PUT and releases every row of that lease — not just the
    /// lockdirs — for a completed, a failed and a stopped run.
    #[test]
    fn close_clears_the_lease_and_releases_every_lock_on_every_branch() {
        let srv = crate::client::fake::serve(|_, _, _| Some((200, json!({}))));
        let api = srv.api();
        let project = Project { name: "p".into(), ..Default::default() };
        let item = WorkItem { id: "11112222-3333-4444-5555-666677778888".into(), project: "p".into(), agent: "code".into(),
            version: 4, attempt: 1, lease: "L9".into(), lockdirs: vec!["{topdir}/a".into()], ..Default::default() };
        STOP_REQUESTED.lock().unwrap().push(item.id.clone());
        let results: Vec<Result<RunOut, String>> = vec![Ok(RunOut::plain("done".into())), Err("boom".into()), Err(STOPPED_BY_USER.into())];
        for r in results {
            close(&api, "E1", &project, "/nonexistent", item.clone(), r, None, None);
        }
        let puts = srv.calls_to("PUT", "/workitems/11112222");
        assert_eq!(puts.len(), 3);
        let states: Vec<&str> = puts.iter().map(|p| p["state"].as_str().unwrap_or("")).collect();
        assert_eq!(states, vec!["complete", "failed", "parked"]);
        assert!(puts.iter().all(|p| p["lease"] == ""), "{puts:?}");
        let rel = srv.calls_to("POST", "/locks/release_all");
        assert_eq!(rel.len(), 3);
        assert!(rel.iter().all(|b| b["workid"] == item.id.as_str() && b["lease"] == "L9"));
        assert!(srv.calls.lock().unwrap().iter().all(|(_, p, _)| !p.ends_with("/locks/release")), "no per-path release any more");
    }

    /// Revocations (2026-09-28) name the run by its lease: an entry for one
    /// run never touches a later run of the same item.
    #[test]
    fn revocations_are_keyed_by_the_runs_lease() {
        let w = "revk0001-0000-4000-8000-000000000001";
        revoke(w, "L1", Revoke::Requeue("r".into()));
        assert_eq!(revoked(w, "L2"), None);
        assert_eq!(revoked(w, "L1"), Some(Revoke::Requeue("r".into())));
        // a newer run's revocation drops the older run's entry
        revoke(w, "L2", Revoke::Abandon("a".into()));
        assert_eq!(take_revoke(w, "L1"), None);
        assert_eq!(take_revoke(w, "L2"), Some(Revoke::Abandon("a".into())));
        assert_eq!(revoked(w, "L2"), None, "taken");
    }

    /// A revocation kills the running session's whole process group, as a
    /// stop does, and the run ends with the REVOKED error, not STOPPED.
    #[cfg(unix)]
    #[test]
    fn revoke_kills_the_session_group() {
        let w = "revk0002-0000-4000-8000-000000000002";
        CURRENT_WORKID.with(|c| *c.borrow_mut() = w.into());
        CURRENT_LEASE.with(|c| *c.borrow_mut() = "LR".into());
        let marker = format!("sleep {}", 4000 + std::process::id() % 997);
        let mut cmd = Command::new("bash");
        cmd.arg("-c").arg(format!("{marker} & sleep 30")).stdout(Stdio::piped()).stderr(Stdio::piped());
        let t = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(400));
            revoke(w, "LR", Revoke::Requeue("another item took a path".into()));
        });
        let started = Instant::now();
        let r = wait_with_timeout(cmd, 20);
        t.join().unwrap();
        let err = r.unwrap_err();
        assert!(err.starts_with(REVOKED_PREFIX) && err.contains("another item took a path"), "{err}");
        assert!(started.elapsed() < Duration::from_secs(10));
        std::thread::sleep(Duration::from_millis(500));
        let alive = Command::new("pgrep").args(["-f", &marker]).output().map(|o| !o.stdout.is_empty()).unwrap_or(false);
        if alive {
            let _ = Command::new("pkill").args(["-f", &marker]).status();
        }
        assert!(!alive, "the background `{marker}` outlived the revocation");
        let _ = take_revoke(w, "LR");
        CURRENT_WORKID.with(|c| c.borrow_mut().clear());
        CURRENT_LEASE.with(|c| c.borrow_mut().clear());
    }

    /// The close of a revoked run: Requeue puts the item back in the queue
    /// with its attempt given back and the lease cleared; Abandon writes no
    /// record at all (the item belongs to another run) — only a doc row and
    /// a release of this run's own lease.
    #[test]
    fn close_of_a_revoked_run_requeues_or_leaves_the_record_alone() {
        let record = json!({"id": "revk0003-0000-4000-8000-000000000003", "project": "p", "state": "in-progress",
            "agent": "code", "version": 5, "attempt": 2, "lease": "LQ", "lockdirs": ["{topdir}/a"]});
        let rec = record.clone();
        let srv = crate::client::fake::serve(move |m, path, _| {
            if m == "GET" && path.contains("/workitems/revk0003") && !path.contains("/details") {
                return Some((200, rec.clone()));
            }
            Some((200, json!({})))
        });
        let project = Project { name: "p".into(), ..Default::default() };
        let item: WorkItem = serde_json::from_value(record).unwrap();
        revoke(&item.id, "LQ", Revoke::Requeue("item 9999aaaabbbb now holds {topdir}/a".into()));
        close(&srv.api(), "E1", &project, "/nonexistent", item.clone(), Err(format!("{REVOKED_PREFIX}x")), None, None);
        let puts = srv.calls_to("PUT", "/workitems/revk0003");
        assert_eq!(puts.len(), 1, "{puts:?}");
        assert_eq!((puts[0]["state"].as_str(), puts[0]["attempt"].as_u64(), puts[0]["lease"].as_str()), (Some("queued"), Some(1), Some("")));
        assert!(puts[0]["lasterror"].as_str().unwrap().contains("9999aaaabbbb"));
        assert!(srv.calls_to("POST", "/details").iter().any(|d| d["value"].as_str().unwrap_or("").starts_with("run stopped by engine and re-queued")));

        let srv = crate::client::fake::serve(|_, _, _| Some((200, json!({}))));
        revoke(&item.id, "LQ", Revoke::Abandon("iter_data refused this run's lease".into()));
        close(&srv.api(), "E1", &project, "/nonexistent", item.clone(), Err(format!("{REVOKED_PREFIX}x")), None, None);
        assert!(srv.calls_to("PUT", "/workitems/").is_empty(), "the record belongs to another run");
        assert!(srv.calls_to("POST", "/details").iter().any(|d| d["value"].as_str().unwrap_or("").starts_with("run abandoned by engine")));
        let rel = srv.calls_to("POST", "/locks/release_all");
        assert!(rel.iter().all(|b| b["lease"] == "LQ"), "only this run's own lease: {rel:?}");
    }

    fn tmpdir(tag: &str) -> String {
        let d = std::env::temp_dir().join(format!("iter4-f3-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d.to_string_lossy().into_owned()
    }

    /// F3 R1 (2026-09-22): a close PUT that fails with a transport error is
    /// retried until it lands; past the budget the intended record is
    /// journaled to disk instead of being dropped.
    #[test]
    fn close_rides_out_an_outage_then_journals_past_the_budget() {
        RETRY_POLICY.with(|p| p.set((10, 20, 400)));
        let project = Project { name: "p".into(), ..Default::default() };
        let item = WorkItem { id: "aaaabbbb-3333-4444-5555-666677778888".into(), project: "p".into(), agent: "exec".into(),
            version: 7, lease: "L1".into(), ..Default::default() };
        for (k, journaled) in [(3usize, false), (10_000, true)] {
            let fails = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
            let f2 = fails.clone();
            let srv = crate::client::fake::serve(move |m, _, _| {
                if m == "PUT" && f2.fetch_add(1, std::sync::atomic::Ordering::SeqCst) < k {
                    return None; // the network is down
                }
                Some((200, json!({})))
            });
            let top = tmpdir(&format!("close{k}"));
            let started = Instant::now();
            close(&srv.api(), "E1", &project, &top, item.clone(), Ok(RunOut::plain("ok".into())), None, None);
            let file = pending_close_dir(&top).join(format!("{}.json", item.id));
            assert_eq!(file.exists(), journaled, "k={k}");
            if journaled {
                let j: Value = serde_json::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap();
                assert_eq!((j["expect_version"].as_u64(), j["record"]["state"].as_str(), j["record"]["lease"].as_str()), (Some(7), Some("complete"), Some("")));
                assert!(started.elapsed() < Duration::from_secs(5), "returned within the budget");
            } else {
                assert_eq!(srv.calls_to("PUT", "/workitems/aaaabbbb").len(), 4, "three lost, the fourth landed");
            }
        }
    }

    /// F3 R2: a journal whose version still matches is replayed and removed;
    /// a stale one is removed without a PUT; an unreachable server keeps it.
    #[test]
    fn journaled_close_is_replayed_or_superseded() {
        let top = tmpdir("replay");
        journal_close(&top, "p", "match-1", 5, "L1", &json!({"id": "match-1", "state": "queued"})).unwrap();
        journal_close(&top, "p", "stale-2", 5, "L2", &json!({"id": "stale-2", "state": "queued"})).unwrap();
        let srv = crate::client::fake::serve(|m, path, _| {
            if m == "GET" && path.ends_with("/match-1") { return Some((200, json!({"version": 5}))); }
            if m == "GET" && path.ends_with("/stale-2") { return Some((200, json!({"version": 9}))); }
            Some((200, json!({})))
        });
        replay_pending_closes(&srv.api(), &top);
        let puts = srv.calls_to("PUT", "/workitems/");
        assert_eq!(puts.len(), 1);
        assert_eq!(puts[0]["id"], "match-1");
        assert!(srv.calls_to("PUT", "stale-2").is_empty());
        assert_eq!(std::fs::read_dir(pending_close_dir(&top)).unwrap().count(), 0);
        // iter_data unreachable: the file stays for the next tick
        journal_close(&top, "p", "match-1", 5, "L1", &json!({"id": "match-1"})).unwrap();
        let down = crate::client::fake::serve(|_, _, _| None);
        replay_pending_closes(&down.api(), &top);
        assert_eq!(std::fs::read_dir(pending_close_dir(&top)).unwrap().count(), 1);
    }

    /// F10: the gate judges the `--fixed` claim as it stands at gate time.
    /// The details fetched at dispatch hold the 09:02 false claim; the run
    /// then wrote an upheld one — the gate passes.  A false claim that is
    /// still the latest holds, naming when it was made.
    #[test]
    fn gate_reads_the_fixed_claim_written_during_the_run() {
        let claim = |order: i64, upheld: bool, ts: &str| json!({"order": order, "key": "claim", "valuetype": "json",
            "value": {"claim": "fixed", "group": "g", "upheld": upheld, "outcome": if upheld { "green" } else { "red" }, "counts": "154/154", "ts": ts}});
        let at_dispatch = vec![claim(3, false, "2026-09-22T09:02:00Z")];
        let project = Project { name: "p".into(), ..Default::default() };
        let item = WorkItem { id: "169419d1-0000-4000-8000-000000000001".into(), agent: "code".into(), ..Default::default() };
        let ctx = |details: Vec<Value>| GateCtx { gate: CloseGate { verify: String::new(), ..Default::default() }, request: "r".into(),
            details, head_before: String::new(), account: String::new(), topdir: "/nonexistent".into() };
        let out = RunOut::plain("fixed it".into());
        let now = vec![claim(3, false, "2026-09-22T09:02:00Z"), claim(9, true, "2026-09-22T10:40:00Z")];
        let srv = crate::client::fake::serve(move |m, path, _| {
            if m == "GET" && path.ends_with("/details") { return Some((200, json!(now))); }
            Some((200, json!([])))
        });
        let (outcome, _) = run_gate(&srv.api(), &project, &item, &out, &ctx(at_dispatch.clone()));
        assert!(matches!(outcome, GateOutcome::Pass), "the fresh upheld claim passes the gate");
        let stale = crate::client::fake::serve(move |m, path, _| {
            if m == "GET" && path.ends_with("/details") { return Some((200, json!([claim(3, false, "2026-09-22T09:02:00Z")]))); }
            Some((200, json!([])))
        });
        match run_gate(&stale.api(), &project, &item, &out, &ctx(at_dispatch)).0 {
            GateOutcome::Hold { reason, .. } => assert!(reason.contains("(at 2026-09-22T09:02:00Z) was FALSE"), "{reason}"),
            _ => panic!("a false latest claim must hold"),
        }
    }

    /// F12 R4: garbage then valid JSON = the second answer, no human asked;
    /// garbage twice = Unparsed (the gate then asks with the R4 widget); a
    /// session that fails twice is Unavailable, never a question.
    #[test]
    fn verifier_parse_failure_retries_once_before_asking() {
        let mut answers = vec!["{key: garbage".to_string(), r#"{"verdict":"complete"}"#.to_string()].into_iter();
        let mut prompts: Vec<String> = Vec::new();
        let v = verify_with_retry("t", "P", |p| { prompts.push(p.to_string()); Ok(answers.next().unwrap()) });
        assert_eq!(v, Verdict::Complete);
        assert!(prompts[1].contains("Answer with exactly one JSON object and nothing else"));
        let v = verify_with_retry("t", "P", |_| Ok("no json at all".into()));
        assert!(matches!(v, Verdict::Unparsed { .. }));
        let mut calls = 0;
        let v = verify_with_retry("t", "P", |_| { calls += 1; Err("spawn failed".into()) });
        assert!(matches!(v, Verdict::Unavailable { .. }) && calls == 2);
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
