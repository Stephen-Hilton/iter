//! The tick loop: sync metadata (seq-gated with a periodic full-refresh
//! fallback), heartbeat, pick queued work, take central locks, run, close.

use crate::client::Api;
use iter_core::cluster::{self, CLUSTER_RESTART_REASON, CLUSTER_RESTART_TAG};
use iter_core::{BLOCKED_TAG_COLOR, BLOCKED_TAG_PREFIX, DepStatus, Engine, LockRow, Project, WorkItem, children_index, claim_tags, dependency_status, now_utc, paths_overlap, pick_account};
use serde_json::{Value, json};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

pub struct EngineRuntime {
    api: Api,
    name: String,
    /// last seq seen per (project, table)
    seen_seq: HashMap<(String, String), u64>,
    last_full_refresh: Instant,
    /// cached data
    projects: HashMap<String, Project>,
    items: HashMap<String, Vec<WorkItem>>,
    agents: HashMap<String, Value>,
    /// retry backoff: workid -> eligible-at
    deferred: HashMap<String, Instant>,
    /// running work, one entry per worker thread
    running: Vec<Running>,
    /// when the lock rows of the running items were last renewed
    last_renew: Instant,
    /// project -> when the lock sweep last ran
    last_sweep: HashMap<String, Instant>,
    /// deadlocks already announced (sorted member ids): one doc row per
    /// member and one log line per cycle per engine process (CR 5.5)
    announced_cycles: HashSet<Vec<String>>,
    /// ELI5 runs in flight (spec: Explain / ELI5): outside the cap, never in
    /// `running`, so they neither count toward maxagents nor delay a drain
    explaining: Vec<(String, std::thread::JoinHandle<()>)>,
    /// close-gate "accept" closes in flight (F11, 2026-09-20): no model
    /// time, so outside the cap and the usage hold, never in `running`
    accepting: Vec<(String, std::thread::JoinHandle<()>)>,
    /// (workid, version) whose details were read and held no live accept —
    /// read again only when the record changes
    accept_checked: HashSet<(String, u64)>,
    /// stage-2 dedup triages in flight (crate::dedup, 2026-09-10): the item
    /// is held out of dispatch while its judge runs on its own thread
    triaging: Vec<(String, std::thread::JoinHandle<()>)>,
    /// GraphRAG Summary agent workers in flight, one per project at most
    /// (crate::rag, 2026-09-29): outside the cap, like ELI5
    summarizing: Vec<(String, std::thread::JoinHandle<()>)>,
    /// GraphRAG re-index sweeps in flight (one per project), started when a
    /// map push found changed files
    rag_syncing: HashMap<String, std::thread::JoinHandle<()>>,
    /// items whose triage finished this process (the cache may not show the
    /// stamp for a tick; never start a second judge on the same item)
    triaged: std::collections::HashSet<String>,
    running_count: Arc<AtomicUsize>,
    /// iter4: project -> (hash of the last architecture map pushed, when the
    /// tree was last checked) — see crate::sync::sync_if_changed
    map_sync: HashMap<String, (String, Instant)>,
    pub max_ticks: Option<u64>,
    /// test_requested value already answered (never run the same nudge twice)
    last_test_handled: String,
    /// account -> when this engine last probed its usage ("" = ambient login)
    last_probe: HashMap<String, Instant>,
    /// project -> date the daily-budget hold was announced
    budget_hold: HashMap<String, String>,
    /// project -> last cluster-health verdict announced (healthy, why)
    cluster_state: HashMap<String, (bool, String)>,
    /// accounts are configured but none is under its stop% (set each tick):
    /// heartbeats then carry no usage snapshot — the ambient login's numbers
    /// would describe an account this engine is not running on
    holding: bool,
    /// the env_file path from .iter/config.json — named in every
    /// "no token" error so the reader knows which file to edit
    env_file: String,
    /// projects whose test sweep template is known to exist (2026-09-30:
    /// the engine creates it paused, once) -> when creating it last failed
    sweep_ensured: HashSet<String>,
    sweep_ensure_tried: HashMap<String, Instant>,
}

use crate::usage;

/// One worker thread.  `cur` is the (workid, lease) it is running NOW: a
/// session chain moves the thread on to other items (work::execute), and
/// lease renewal must follow the item actually running (CR 2026-09-25 6.4).
pub struct Running {
    agent: String,
    /// a test sweep run (2026-09-30): started on the timer, so it takes no
    /// cap slot — it neither counts toward maxagents nor the agent's type cap
    outside_cap: bool,
    project: String,
    cur: Arc<Mutex<(String, String)>>,
    handle: std::thread::JoinHandle<()>,
}

impl Running {
    fn current(&self) -> (String, String) {
        self.cur.lock().map(|c| c.clone()).unwrap_or_default()
    }
}

/// How often running items' locks are renewed: LOCK_RENEW_EVERY_SEC (60),
/// or `ITER_LOCK_RENEW_EVERY_SEC` — a test hook, so e2e can watch the
/// lost-lock handling without waiting out real minutes.
fn renew_every_sec() -> u64 {
    std::env::var("ITER_LOCK_RENEW_EVERY_SEC")
        .ok()
        .and_then(|v| v.trim().parse::<u64>().ok())
        .filter(|n| *n > 0)
        .unwrap_or(iter_core::LOCK_RENEW_EVERY_SEC)
}

/// maxagents ladder: among ">N%" gates where usage > N pick the LARGEST N
/// (most restrictive true gate — order-independent equivalent of the spec's
/// top-down list); fall through to "else" (default 4 when absent).
fn max_agents(gates: &BTreeMap<String, u32>, usage_pct: u8) -> u32 {
    let mut best: Option<(u8, u32)> = None;
    for (k, v) in gates {
        if let Some(n) = k.strip_prefix('>').and_then(|s| s.strip_suffix('%')).and_then(|s| s.parse::<u8>().ok()) {
            if usage_pct > n && best.map(|(bn, _)| n > bn).unwrap_or(true) {
                best = Some((n, *v));
            }
        }
    }
    if let Some((_, v)) = best {
        return v;
    }
    gates.get("else").copied().unwrap_or(4)
}

/// Why a queued item is not running this tick (spec: lock waits are visible,
/// 2026-09-07).  `holders` are the running items whose lock rows overlap it —
/// a dependency the webui nests it under; `reason` is the text behind the one
/// engine-owned "blocked by: …" tag.  A dependency wait carries neither: the
/// blocked-by nesting already shows it.
#[derive(Debug, Clone, Default)]
struct Wait {
    reason: Option<String>,
    /// (holder workid, the overlapping locked path)
    holders: Vec<(String, String)>,
}

/// The wait reason while the stage-2 dedup judge runs (renders "blocked by: dedup triage").
const DEDUP_TRIAGE_REASON: &str = "dedup triage";

/// "2026-09-07T14:05:31Z" -> "14:05Z" for the retry-after tag
fn hhmm(iso: &str) -> String {
    if iso.len() >= 16 { format!("{}Z", &iso[11..16]) } else { iso.to_string() }
}

/// "run now waits on running <id8> (started hh:mmZ, session limit ends it
/// by hh:mmZ)": the holder's start plus its session timeout bounds the wait.
fn run_now_wait_reason(project: &Project, holder: Option<&WorkItem>, holder_id: &str, agents: &HashMap<String, Value>) -> String {
    let id8 = &holder_id[..8.min(holder_id.len())];
    let Some(h) = holder else { return format!("run now waits on running {id8}") };
    let agent_def = agents.get(&h.agent).cloned().unwrap_or(Value::Null);
    let limit = crate::work::agent_timeout(project, h, &agent_def);
    let ends = chrono::DateTime::parse_from_rfc3339(&h.ts.start)
        .ok()
        .map(|t| (t.with_timezone(&chrono::Utc) + chrono::Duration::seconds(limit as i64)).format("%Y-%m-%dT%H:%M:%SZ").to_string());
    match ends {
        Some(e) => format!("run now waits on running {id8} (started {}, session limit ends it by {})", hhmm(&h.ts.start), hhmm(&e)),
        None => format!("run now waits on running {id8}"),
    }
}

fn expand_topdir(topdir: &str) -> String {
    if let Some(rest) = topdir.strip_prefix("~/") {
        if let Ok(home) = std::env::var("HOME") {
            return format!("{home}/{rest}");
        }
    }
    topdir.to_string()
}

/// The accounts whose token is not set in the engine's env_file right now
/// (spec: account hot reload, 2026-09-11): never picked, never probed,
/// never substituted.
fn tokenless_accounts(accounts: &[iter_core::Account]) -> Vec<String> {
    accounts.iter().filter(|a| crate::envstore::get(&a.token_envar).is_none()).map(|a| a.name.clone()).collect()
}

/// The project-wide hold reason when the ladder picks nothing (renders as the
/// "blocked by: …" tag on every queued item): every account is missing its
/// token, or every account is at its stop%.
fn hold_reason(all_tokenless: bool) -> &'static str {
    if all_tokenless { "no account token" } else { "accounts at stop%" }
}

/// The connectivity-test result for the engine record.  `token` is the R2
/// resolution for `account`: an error means the nudge is NOT run — the
/// result is red and names the variable — because a nudge with no token
/// falls back to the machine's ambient login and would report that login's
/// health and usage under the account's name (the 2026-09-11 false green).
/// An empty account IS the ambient login, and the result says so.
fn test_outcome(
    token: Result<Option<String>, String>,
    account: &str,
    envar: &str,
    requested: &str,
    run: impl FnOnce(Option<String>) -> Result<(crate::work::RunOut, u128), String>,
) -> Value {
    let label = if account.is_empty() { "default (ambient CLI login)".to_string() } else { account.to_string() };
    let token = match token {
        Ok(t) => t,
        Err(_) => {
            return json!({
                "requested": requested, "ts": now_utc(), "ok": false, "model": "haiku", "account": label,
                "error": format!("no token for account '{account}' ({envar} unset)"),
            });
        }
    };
    match run(token) {
        Ok((out, ms)) => json!({
            "requested": requested, "ts": now_utc(), "ok": out.subtype == "success",
            "ms": ms, "model": "haiku", "account": label,
            "text": out.text.chars().take(200).collect::<String>(), "subtype": out.subtype,
        }),
        Err(e) => json!({
            "requested": requested, "ts": now_utc(), "ok": false, "model": "haiku",
            "account": label, "error": e.chars().take(500).collect::<String>(),
        }),
    }
}

impl EngineRuntime {
    pub fn new(api: Api, name: String, env_file: String) -> Self {
        Self {
            api,
            name,
            env_file,
            seen_seq: HashMap::new(),
            last_full_refresh: Instant::now() - Duration::from_secs(86400 * 365),
            projects: HashMap::new(),
            items: HashMap::new(),
            agents: HashMap::new(),
            deferred: HashMap::new(),
            running: Vec::new(),
            last_renew: Instant::now() - Duration::from_secs(3600),
            last_sweep: HashMap::new(),
            announced_cycles: HashSet::new(),
            explaining: Vec::new(),
            accepting: Vec::new(),
            accept_checked: HashSet::new(),
            sweep_ensured: HashSet::new(),
            sweep_ensure_tried: HashMap::new(),
            triaging: Vec::new(),
            summarizing: Vec::new(),
            rag_syncing: HashMap::new(),
            triaged: std::collections::HashSet::new(),
            running_count: Arc::new(AtomicUsize::new(0)),
            max_ticks: None,
            last_test_handled: String::new(),
            holding: false,
            last_probe: HashMap::new(),
            map_sync: HashMap::new(),
            budget_hold: HashMap::new(),
            cluster_state: HashMap::new(),
        }
    }

    pub fn run(&mut self) {
        let mut ticks: u64 = 0;
        loop {
            ticks += 1;
            let engine = match self.api.get(&format!("/api/engines/{}", self.name)) {
                Ok(v) => match serde_json::from_value::<Engine>(v) {
                    Ok(e) => e,
                    Err(e) => {
                        eprintln!("[engine] bad engine record: {e}");
                        std::thread::sleep(Duration::from_secs(5));
                        continue;
                    }
                },
                Err(e) if e.status == 404 => {
                    // self-register (decided 2026-09-04): a new engine creates its own
                    // record; projects are assigned to it afterwards via the webui gear
                    let host = std::process::Command::new("hostname").output().ok()
                        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string()).unwrap_or_default();
                    let row = json!({"name": self.name, "host": host, "state": "Stopped", "last_seen": "",
                        "ticksec": 5, "full_refresh_minutes": 360, "account": "",
                        "queuelock": {"retryms": 50, "breaksec": 60}, "projects": {}});
                    match self.api.put(&format!("/api/engines/{}", self.name), &row) {
                        Ok(_) => println!("[engine] registered '{}' with iter_data — assign it projects via the webui (engine gear -> projects served)", self.name),
                        Err(e2) => eprintln!("[engine] cannot self-register '{}': {e2}", self.name),
                    }
                    std::thread::sleep(Duration::from_secs(2));
                    continue;
                }
                Err(e) => {
                    eprintln!("[engine] cannot load engine '{}' from iter_data ({e})", self.name);
                    std::thread::sleep(Duration::from_secs(5));
                    continue;
                }
            };

            self.tick(&engine);

            if let Some(max) = self.max_ticks {
                if ticks >= max {
                    println!("[engine] max ticks reached, draining running work");
                    while self.prune_running() > 0
                        || { self.explaining.retain(|(_, h)| !h.is_finished()); !self.explaining.is_empty() }
                        || { self.triaging.retain(|(_, h)| !h.is_finished()); !self.triaging.is_empty() }
                        || { self.accepting.retain(|(_, h)| !h.is_finished()); !self.accepting.is_empty() }
                        || { self.summarizing.retain(|(_, h)| !h.is_finished()); !self.summarizing.is_empty() }
                    {
                        std::thread::sleep(Duration::from_millis(200));
                    }
                    // runs that finished during the drain may have changed the tree
                    self.sync_maps(&engine);
                    return;
                }
            }
            std::thread::sleep(Duration::from_secs(engine.ticksec.max(1)));
        }
    }

    /// `claude -p "."` on haiku for the active account; the result and the
    /// refreshed usage snapshot go back on the engine record.
    fn run_test(&mut self, engine: &Engine, chosen: &str) {
        // which account to test: the engine's chosen one; while holding (no
        // account pickable) the chosen one is "" but accounts ARE configured,
        // and the ambient CLI login must not stand in for them — test the
        // first account by order that has a token, else fail naming the first
        // account without one.  "" is the ambient login only when no project
        // configures any account.
        let mut configured: Vec<iter_core::Account> = Vec::new();
        for pn in engine.projects.keys() {
            if let Some(p) = self.projects.get(pn) {
                for a in &p.accounts {
                    if !configured.iter().any(|x| x.name == a.name) {
                        configured.push(a.clone());
                    }
                }
            }
        }
        configured.sort_by_key(|a| a.order);
        let tested: String = if !chosen.is_empty() || configured.is_empty() {
            chosen.to_string()
        } else {
            configured
                .iter()
                .find(|a| crate::envstore::get(&a.token_envar).is_some())
                .or(configured.first())
                .map(|a| a.name.clone())
                .unwrap_or_default()
        };
        let account = tested.as_str();
        // token: the one resolution rule (work::resolve_account_token), from
        // whichever project defines the account; a missing token fails the
        // test outright — nudging without it would test the ambient login
        let owner = self.projects.values().find(|p| p.accounts.iter().any(|a| a.name == account));
        let token = match owner {
            Some(p) => crate::work::resolve_account_token(p, account, &self.env_file),
            None => crate::work::resolve_account_token(&Project::default(), account, &self.env_file),
        };
        let envar = owner.and_then(|p| crate::work::account_envar(p, account)).unwrap_or_else(|| "no token_envar configured".into());
        let cwd = engine
            .projects
            .values()
            .next()
            .and_then(|d| d.dirs.get("topdir"))
            .map(|t| expand_topdir(t))
            .filter(|t| std::path::Path::new(t).is_dir())
            .unwrap_or_else(|| std::env::temp_dir().to_string_lossy().to_string());
        println!("[engine] connectivity test requested at {} (account '{}')", engine.test_requested, if account.is_empty() { "default" } else { account });
        let result = test_outcome(token, account, &envar, &engine.test_requested, |tok| crate::work::nudge(tok, account, &cwd));
        if result["ok"].as_bool().unwrap_or(false) {
            println!("[engine] connectivity test OK");
        } else {
            println!("[engine] connectivity test FAILED{}", result["error"].as_str().map(|e| format!(": {e}")).unwrap_or_default());
        }
        // the record keeps the engine's CHOSEN account label and usage; the
        // tested account is named inside test_result
        let _ = self.api.post(
            &format!("/api/engines/{}/heartbeat", self.name),
            &json!({"test_result": result, "clear_test": true, "account": chosen,
                    "usage": if self.holding { Value::Null } else { usage::snapshot_json(chosen, chrono::Utc::now()).unwrap_or(Value::Null) }}),
        );
    }

    /// One probe per stale account (all of the project's accounts, not just
    /// the chosen one — the ladder needs every account's number to switch).
    /// No accounts configured = the ambient CLI login, which only the haiku
    /// nudge can reach.
    fn probe_stale_accounts(&mut self, engine: &Engine, now: chrono::DateTime<chrono::Utc>) {
        let stale_sec = (engine.probe_stale_min * 60) as i64;
        let mut any_accounts = false;
        let mut targets: Vec<(String, String)> = Vec::new(); // (account, token)
        let mut skipped: Vec<(String, String)> = Vec::new(); // (account, envar) — no token set
        for project_name in engine.projects.keys() {
            if let Some(p) = self.projects.get(project_name) {
                for a in &p.accounts {
                    any_accounts = true;
                    if targets.iter().any(|(n, _)| n == &a.name) || skipped.iter().any(|(n, _)| n == &a.name) {
                        continue;
                    }
                    match crate::envstore::get(&a.token_envar) {
                        Some(tok) => targets.push((a.name.clone(), tok)),
                        None => skipped.push((a.name.clone(), a.token_envar.clone())),
                    }
                }
            }
        }
        let due = |this: &Self, name: &str| -> bool {
            let age = usage::read_usage(name).and_then(|u| u.age_sec(now)).unwrap_or(i64::MAX);
            let since = this.last_probe.get(name).map(|t| t.elapsed().as_secs() as i64).unwrap_or(i64::MAX);
            age > stale_sec && since > stale_sec
        };
        if !any_accounts {
            if due(self, "") {
                self.last_probe.insert(String::new(), Instant::now());
                println!("[engine] default usage snapshot stale — nudging haiku through the CLI");
                self.run_test(engine, "");
            }
            return;
        }
        for (name, tok) in targets {
            if !due(self, &name) {
                continue;
            }
            self.last_probe.insert(name.clone(), Instant::now());
            match usage::probe_and_record(&name, &tok) {
                Ok(u) => println!(
                    "[engine] usage probe '{name}': 5h {:.0}% 7d {:.0}% ({}{})",
                    u.five_hour_pct, u.seven_day_pct, u.status, if u.is_using_overage { ", OVERAGE" } else { "" }
                ),
                Err(e) => eprintln!("[engine] usage probe '{name}' failed: {e}"),
            }
        }
        // an account with no token cannot be probed: say so, once per stale
        // period (the same `due` rule as a real probe), instead of silently
        // never reporting its usage
        for (name, envar) in skipped {
            if !due(self, &name) {
                continue;
            }
            self.last_probe.insert(name.clone(), Instant::now());
            println!("[engine] usage probe '{name}' skipped: {envar} not set");
        }
    }

    /// Re-read the env_file when it changed (or `force`), refreshing every
    /// `*_TOKEN` key plus each `token_envar` the served projects name; one
    /// log line per reload that changed something, key names only.
    fn reload_env(&self, force: bool) {
        let declared: std::collections::HashSet<String> =
            self.projects.values().flat_map(|p| p.accounts.iter().map(|a| a.token_envar.clone())).collect();
        if let Some(ch) = crate::envstore::reload_if_changed(&declared, force) {
            println!("[engine] {}", ch.log_line());
        }
    }

    /// One read-only `explain` session per item flagged `explain_requested`
    /// that this engine is not already explaining.
    fn start_explains(&mut self, engine: &Engine, project: &Project, account: &str) {
        self.explaining.retain(|(_, h)| !h.is_finished());
        let Some(topdir) = engine.projects.get(&project.name).and_then(|d| d.dirs.get("topdir")).map(|t| expand_topdir(t)) else { return };
        let wanted: Vec<WorkItem> = self
            .items
            .get(&project.name)
            .map(|v| v.iter().filter(|i| !i.explain_requested.is_empty() && (i.explain_engine.is_empty() || i.explain_engine == self.name)).cloned().collect())
            .unwrap_or_default();
        for item in wanted {
            if self.explaining.iter().any(|(id, _)| id == &item.id) {
                continue;
            }
            // one engine per ELI5: iter_data assigned one at random when the
            // button was pressed; an unassigned one goes to whoever claims first
            if let Err(e) = self.api.post(
                &format!("/api/projects/{}/workitems/{}/explain/claim", project.name, item.id),
                &json!({"engine": self.name}),
            ) {
                if e.status != 409 {
                    eprintln!("[engine] ELI5 claim failed for {}: {e}", &item.id[..8.min(item.id.len())]);
                }
                continue;
            }
            let agent_def = self.agents.get("explain").cloned().unwrap_or(Value::Null);
            println!(
                "[engine] ELI5 requested at {} for {} '{}' — explaining now, outside the cap (running {})",
                item.explain_requested, &item.id[..8.min(item.id.len())], item.name, self.running.len()
            );
            let api = self.api.clone();
            let project = project.clone();
            let topdir = topdir.clone();
            let account = account.to_string();
            let workid = item.id.clone();
            let handle = std::thread::spawn(move || {
                crate::work::explain(&api, &project, &topdir, &item, &account, &agent_def);
            });
            self.explaining.push((workid, handle));
        }
    }

    /// Start a Summary agent worker for each served project whose GraphRAG
    /// work is waiting (heartbeat `rag_waiting`) and has no worker yet.
    fn start_summaries(&mut self, engine: &Engine, reply: &Value, account: &str) {
        self.summarizing.retain(|(_, h)| !h.is_finished());
        if self.holding {
            return;
        }
        // workers per project: the `summary` agent record's `max` (default 2)
        let per_project = self.agents.get("summary").and_then(|a| a.get("max")).and_then(|m| m.as_u64()).unwrap_or(2).clamp(1, 8) as usize;
        for project_name in crate::rag::waiting_projects(reply) {
            let n = reply["rag_waiting"][&project_name].as_u64().unwrap_or(0) as usize;
            let running = self.summarizing.iter().filter(|(p, _)| p == &project_name).count();
            // one worker per ~8 waiting summaries (one job's worth), up to the cap
            let want = per_project.min(n.div_ceil(8).max(1));
            for _ in running..want {
                let Some(project) = self.projects.get(&project_name).cloned() else { continue };
                let Some(topdir) = engine.projects.get(&project_name).and_then(|d| d.dirs.get("topdir")).map(|t| expand_topdir(t)) else { continue };
                if !std::path::Path::new(&topdir).is_dir() {
                    continue;
                }
                let api = self.api.clone();
                let name = self.name.clone();
                let account = account.to_string();
                let agent_def = self.agents.get("summary").cloned().unwrap_or(Value::Null);
                println!("[engine] {project_name}: GraphRAG has {n} chunk/document summaries waiting — Summary agent worker {} of {want} starting, outside the cap", self.summarizing.iter().filter(|(p, _)| p == &project_name).count() + 1);
                let handle = std::thread::spawn(move || {
                    crate::rag::summarize_waiting(&api, &name, &project, &topdir, &account, &agent_def);
                });
                self.summarizing.push((project_name.clone(), handle));
            }
        }
    }

    fn running_by_project(&self) -> BTreeMap<String, usize> {
        let mut m: BTreeMap<String, usize> = BTreeMap::new();
        for r in &self.running {
            *m.entry(r.project.clone()).or_default() += 1;
        }
        m
    }

    fn prune_running(&mut self) -> usize {
        let before = self.running.len();
        self.running.retain(|r| !r.handle.is_finished());
        if self.running.len() < before {
            // a run just finished and may have changed the tree (a graph edit,
            // an agent adding node files): re-check every map on this tick.
            // Two edits pushing their own snapshots concurrently can leave the
            // older tree on top; this push of the settled tree corrects it.
            for (_, at) in self.map_sync.values_mut() {
                *at = Instant::now() - Duration::from_secs(3600);
            }
        }
        self.running.len()
    }

    /// Keep the lock rows of every run this engine executes alive (CR
    /// 2026-09-25 option (e)): every LOCK_RENEW_EVERY_SEC, one `locks/renew`
    /// per running item with its lease.  Called before anything else in the
    /// tick — a project on hold or at its budget still has running items.
    /// Renewal is on a timer, not on agent activity: a silent two-hour turn
    /// keeps its locks while this process is alive and reaches iter_data.
    ///
    /// Two ways a run can find it no longer holds its locks (decided
    /// 2026-09-28), and what happens:
    /// - renewal REFUSED (409, stale lease): the item's record carries another
    ///   run's lease or none, so another run owns the item → the session is
    ///   killed and the record left alone (`Revoke::Abandon`);
    /// - renewal renewed fewer rows than the item has lockdirs (they expired
    ///   during an outage longer than the lease and the sweep removed them) →
    ///   the paths are re-taken if free and the run continues; if another item
    ///   took one meanwhile the session is killed and the item re-queued
    ///   (`Revoke::Requeue`), its attempt not counted.
    fn renew_leases(&mut self) {
        if self.last_renew.elapsed() < Duration::from_secs(renew_every_sec()) {
            return;
        }
        self.last_renew = Instant::now();
        let runs: Vec<(String, String, String)> = self
            .running
            .iter()
            .map(|r| {
                let (w, l) = r.current();
                (r.project.clone(), w, l)
            })
            .filter(|(_, w, l)| !w.is_empty() && !l.is_empty())
            .collect();
        for (project, workid, lease) in runs {
            let res = self.api.post(
                &format!("/api/projects/{project}/locks/renew"),
                &json!({"workid": workid, "lease": lease, "ttl_sec": iter_core::LOCK_LEASE_TTL_SEC}),
            );
            match res {
                Ok(v) => {
                    let renewed = v.get("renewed").and_then(|n| n.as_u64()).unwrap_or(0) as usize;
                    let lockdirs: Vec<String> = self
                        .items
                        .get(&project)
                        .and_then(|items| items.iter().find(|i| i.id == workid))
                        .map(|i| i.lockdirs.clone())
                        .unwrap_or_default();
                    if renewed < lockdirs.len() {
                        self.retake_locks(&project, &workid, &lease, &lockdirs, renewed);
                    }
                }
                Err(e) if e.status == 409 => {
                    let reason = serde_json::from_str::<Value>(&e.body)
                        .ok()
                        .and_then(|v| v.get("error").and_then(|m| m.as_str()).map(String::from))
                        .unwrap_or_else(|| e.body.clone());
                    println!("[engine] {}: lease refused ({reason}) — another run owns the item; stopping this run", &workid[..8.min(workid.len())]);
                    crate::work::revoke(&workid, &lease, crate::work::Revoke::Abandon(format!(
                        "at {} iter_data refused this run's lease ({reason}); another run owns the item.", now_utc())));
                }
                Err(e) => eprintln!("[engine] {}: lock renewal failed: {e}", &workid[..8.min(workid.len())]),
            }
        }
    }

    /// A run whose lock rows vanished (see renew_leases): take every lockdir
    /// back under the same lease.  All free → the run continues, with a note;
    /// any held by another item → release what was re-taken and revoke the
    /// run for a re-queue, naming the item that holds the path.
    fn retake_locks(&mut self, project: &str, workid: &str, lease: &str, lockdirs: &[String], renewed: usize) {
        let id8 = &workid[..8.min(workid.len())];
        let mut taken_by: Option<(String, String)> = None;
        for d in lockdirs {
            let r = self.api.post(
                &format!("/api/projects/{project}/locks/acquire"),
                &json!({"path": d, "workid": workid, "lease": lease, "engine": self.name, "ttl_sec": iter_core::LOCK_LEASE_TTL_SEC}),
            );
            if let Err(e) = r {
                let holder = serde_json::from_str::<Value>(&e.body)
                    .ok()
                    .and_then(|v| v.pointer("/current/workid").and_then(|w| w.as_str()).map(String::from))
                    .unwrap_or_else(|| format!("unknown ({e})"));
                taken_by = Some((d.clone(), holder));
                break;
            }
        }
        match taken_by {
            None => {
                println!("[engine] {id8}: {} of {} lock row(s) had lapsed — re-taken, the run continues", lockdirs.len().saturating_sub(renewed), lockdirs.len());
                let _ = self.api.post(
                    &format!("/api/projects/{project}/workitems/{workid}/details"),
                    &json!({"key": "doc", "valuetype": "text", "value": format!(
                        "at {} this run's lock rows had lapsed (iter_data unreachable for longer than the {}-second lease); every path was still free, so the engine re-took them and the run continued.",
                        now_utc(), iter_core::LOCK_LEASE_TTL_SEC)}),
                );
            }
            Some((path, holder)) => {
                let h12 = &holder[holder.len().saturating_sub(12)..];
                println!("[engine] {id8}: lock rows lapsed and {path} is now held by {h12} — stopping the run and re-queueing it");
                let _ = self.api.post(&format!("/api/projects/{project}/locks/release_all"), &json!({"workid": workid, "lease": lease}));
                crate::work::revoke(workid, lease, crate::work::Revoke::Requeue(format!(
                    "at {} its lock rows had lapsed (iter_data unreachable for longer than the {}-second lease) and item {h12} now holds {path}.",
                    now_utc(), iter_core::LOCK_LEASE_TTL_SEC)));
            }
        }
    }

    /// iter4 (spec R5): keep each project's architecture map in iter_data in
    /// step with the checkout. Ids are repaired and the snapshot pushed at
    /// start and then whenever its hash moves; the tree is re-read at most
    /// once a minute per project (a scan of a large repo is not free).
    fn sync_maps(&mut self, engine: &Engine) {
        for (name, dirs) in &engine.projects {
            let Some(topdir) = dirs.dirs.get("topdir").map(|t| expand_topdir(t)) else { continue };
            let due = self.map_sync.get(name).map(|(_, at)| at.elapsed() >= Duration::from_secs(60)).unwrap_or(true);
            if !due || !std::path::Path::new(&topdir).is_dir() {
                continue;
            }
            let last = self.map_sync.get(name).map(|(h, _)| h.clone()).unwrap_or_default();
            let hash = crate::sync::sync_if_changed_ro(&self.api, name, std::path::Path::new(&topdir), &last, dirs.read_only);
            // GraphRAG: a map whose files changed (or this engine's first look)
            // re-indexes the node files whose text changed — hash-diffed, so an
            // unchanged file costs nothing; one sweep per project at a time
            if hash != last && !hash.is_empty() {
                self.rag_syncing.retain(|_, h| !h.is_finished());
                if !self.rag_syncing.contains_key(name) {
                    let (api, project, top) = (self.api.clone(), name.clone(), topdir.clone());
                    let handle = std::thread::spawn(move || match crate::rag::sync(&api, &project, std::path::Path::new(&top), false, false) {
                        Ok(r) if r.sent > 0 || r.removed > 0 => println!(
                            "[engine] {project}: GraphRAG re-indexed after the map changed — {} node file(s) sent (+{} ~{} -{}){}",
                            r.sent, r.added, r.changed, r.removed, if r.failed.is_empty() { String::new() } else { format!(", {} failed", r.failed.len()) }
                        ),
                        Ok(_) => {}
                        Err(e) => eprintln!("[engine] {project}: GraphRAG re-index skipped: {e}"),
                    });
                    self.rag_syncing.insert(name.clone(), handle);
                }
            }
            self.map_sync.insert(name.clone(), (hash, Instant::now()));
        }
    }

    fn tick(&mut self, engine: &Engine) {
        self.prune_running();
        self.renew_leases();
        // the env_file may have changed since the last tick: an account token
        // added, rotated or removed takes effect here, without a restart
        self.reload_env(false);
        self.sync_maps(engine);

        // account selection: exclusion-with-fallback against other LIVE engines —
        // "Running" per the record AND heartbeated within three ticks (fixed
        // 2026-09-04: a killed engine never writes Stopped, so its stale record
        // kept reserving its account forever)
        let now_ts = chrono::Utc::now();
        let in_use: Vec<String> = self
            .api
            .get("/api/engines")
            .ok()
            .and_then(|v| v.as_array().cloned())
            .unwrap_or_default()
            .iter()
            .filter(|e| {
                e.get("name").and_then(|n| n.as_str()) != Some(self.name.as_str())
                    && e.get("state").and_then(|s| s.as_str()) == Some("Running")
                    && {
                        let tick = e.get("ticksec").and_then(|t| t.as_i64()).unwrap_or(5).max(1);
                        e.get("last_seen")
                            .and_then(|s| s.as_str())
                            .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
                            .map(|seen| (now_ts - seen.with_timezone(&chrono::Utc)).num_seconds() <= 3 * tick + 5)
                            .unwrap_or(false)
                    }
            })
            .filter_map(|e| e.get("account").and_then(|a| a.as_str()).map(String::from))
            .filter(|a| !a.is_empty())
            .collect();

        let now = chrono::Utc::now();
        let mut chosen_account = String::new();
        for project_name in engine.projects.keys() {
            if let Some(p) = self.projects.get(project_name) {
                let map = usage::usage_map(&p.accounts, now);
                let tokenless = tokenless_accounts(&p.accounts);
                if let Some(acct) = pick_account(&p.accounts, &map, &in_use, &tokenless) {
                    chosen_account = acct.name.clone();
                    break;
                }
            }
        }

        // heartbeat: actual state + account + the account's usage snapshot,
        // every tick (every claude session's rate_limit_event line and the
        // idle probe refresh it, so a run's cost shows up on the next tick).
        // The account is sent even when "" (holding: nothing pickable) so the
        // record's label and its usage always describe the same account.
        // hold: accounts are configured but none is under its stop% — usage
        // goes up as null (the ambient login's numbers are not this engine's;
        // showing them was the 2026-09-11 "Running on default | 5h 1%" bug)
        // and the webui shows "Suspended, no usage left".
        // accounts/next: every account's windows + reset times and the one
        // that comes back first, so the record says WHEN work resumes.
        let all_accounts: Vec<iter_core::Account> = {
            let mut v: Vec<iter_core::Account> = Vec::new();
            for pn in engine.projects.keys() {
                if let Some(p) = self.projects.get(pn) {
                    for a in &p.accounts {
                        if !v.iter().any(|x| x.name == a.name) {
                            v.push(a.clone());
                        }
                    }
                }
            }
            v
        };
        self.holding = chosen_account.is_empty() && !all_accounts.is_empty();
        // the hold names its cause: no token set anywhere is not "no usage left"
        let all_tokenless = !all_accounts.is_empty() && all_accounts.iter().all(|a| crate::envstore::get(&a.token_envar).is_none());
        let accounts = usage::accounts_json(&all_accounts, &in_use, now);
        let next = usage::next_json(&accounts);
        let heartbeat = self.api.post(
            &format!("/api/engines/{}/heartbeat", self.name),
            &json!({"state": "Running", "account": chosen_account,
                    // the threads behind this engine's in-progress records: the
                    // webui warns when the store counts more (a ghost, 2026-09-22)
                    "running": self.running.len(), "running_by_project": self.running_by_project(),
                    "hold": if !self.holding { "" } else if all_tokenless { "no account token" } else { "all accounts at stop%" },
                    "usage": if self.holding { Value::Null } else { usage::snapshot_json(&chosen_account, now).unwrap_or(Value::Null) },
                    "accounts": accounts, "next": next}),
        );
        // datasync (2026-09-29): graph edits waiting for an engine — the first
        // engine to see them applies them now, not behind the agent queue
        if let Ok(reply) = &heartbeat {
            for p in crate::datasync::waiting_projects(reply) {
                // a read-only checkout is never written: its edits wait for another engine
                if engine.projects.get(&p).map(|d| d.read_only).unwrap_or(false) {
                    continue;
                }
                if let Some(topdir) = engine.projects.get(&p).and_then(|d| d.dirs.get("topdir")).map(|t| expand_topdir(t)) {
                    if std::path::Path::new(&topdir).is_dir() {
                        crate::datasync::apply_waiting(&self.api, &self.name, &p, &topdir);
                    }
                }
            }
        }

        // GraphRAG (2026-09-29): chunk / document summaries waiting — one
        // Summary agent worker per project, on its own thread, billed to the
        // chosen account; a holding engine writes none
        if let Ok(reply) = &heartbeat {
            self.start_summaries(engine, reply, &chosen_account);
        }

        // stop requests for items THIS engine is running (workitem_stop.md)
        for items in self.items.values() {
            for i in items.iter().filter(|i| i.stop_requested && i.state == "in-progress" && i.engine == self.name) {
                if let Ok(mut v) = crate::work::STOP_REQUESTED.lock() {
                    if !v.contains(&i.id) {
                        println!("[engine] stop requested for {} '{}' — killing its session", &i.id[..8], i.name);
                        v.push(i.id.clone());
                    }
                }
            }
        }

        // metadata + queue sync, seq-gated with the periodic full-refresh fallback
        let full_refresh = self.last_full_refresh.elapsed()
            > Duration::from_secs(engine.full_refresh_minutes.max(1) * 60);
        if full_refresh {
            self.last_full_refresh = Instant::now();
            // a stamp can lie (clock moved, a file restored with an old
            // mtime): the periodic refresh re-reads the env_file regardless
            self.reload_env(true);
        }

        for project_name in engine.projects.keys() {
            let versions = self
                .api
                .get(&format!("/api/projects/{project_name}/versions"))
                .ok()
                .and_then(|v| v.as_array().cloned())
                .unwrap_or_default();
            let seq_of = |table: &str| -> u64 {
                versions
                    .iter()
                    .find(|r| r.get("table").and_then(|t| t.as_str()) == Some(table))
                    .and_then(|r| r.get("seq").and_then(|s| s.as_u64()))
                    .unwrap_or(0)
            };

            let reload = |rt: &mut Self, table: &str| -> bool {
                let key = (project_name.clone(), table.to_string());
                let now_seq = seq_of(table);
                let changed = rt.seen_seq.get(&key).copied() != Some(now_seq);
                if changed || full_refresh {
                    rt.seen_seq.insert(key, now_seq);
                    true
                } else {
                    false
                }
            };

            if reload(self, "project") {
                if let Ok(v) = self.api.get(&format!("/api/projects/{project_name}")) {
                    if let Ok(p) = serde_json::from_value::<Project>(v) {
                        self.projects.insert(project_name.clone(), p);
                        // a new or renamed account may name a variable the
                        // file already holds: re-read it against the new set
                        if !full_refresh {
                            self.reload_env(true);
                        }
                    }
                }
            }
            if reload(self, "agent") {
                if let Ok(v) = self.api.get("/api/agents") {
                    for a in v.as_array().cloned().unwrap_or_default() {
                        if let Some(n) = a.get("name").and_then(|n| n.as_str()) {
                            self.agents.insert(n.to_string(), a.clone());
                        }
                    }
                }
            }
            if reload(self, "workitem") {
                if let Ok(v) = self.api.get(&format!("/api/projects/{project_name}/workitems")) {
                    let items: Vec<WorkItem> = v
                        .as_array()
                        .cloned()
                        .unwrap_or_default()
                        .into_iter()
                        .filter_map(|i| serde_json::from_value(i).ok())
                        .collect();
                    self.items.insert(project_name.clone(), items);
                }
            }

            let Some(project) = self.projects.get(project_name).cloned() else { continue };
            if !engine.projects.get(project_name).map(|d| d.read_only).unwrap_or(true) {
                self.ensure_test_sweep(project_name);
            }
            // a close that could not reach iter_data is replayed first, then
            // this engine's own ghosts are repaired — whatever the project
            // state: a ghost holds a cap slot and blocks a drain (2026-09-22)
            if let Some(td) = engine.projects.get(project_name).and_then(|d| d.dirs.get("topdir")) {
                let td = expand_topdir(td);
                crate::work::replay_pending_closes(&self.api, &td);
                self.repair_ghosts(&project, &td);
            }
            // ELI5 requests (spec: Explain / ELI5) run at once whatever the
            // project state or cap says: a human pressed the button, the run is
            // read-only, and nothing waits on it
            self.start_explains(engine, &project, &chosen_account);
            if project.state != "Running" {
                // Draining is transitional: ask iter_data to settle it to Stopped
                // once nothing is in progress anywhere
                if project.state == "Draining" {
                    if let Ok(r) = self.api.post(&format!("/api/projects/{project_name}/settle"), &json!({})) {
                        if r.get("settled").and_then(|b| b.as_bool()).unwrap_or(false) {
                            println!("[engine] {project_name}: drained -> Stopped");
                        }
                    }
                }
                // Draining/Stopped: finish running work, start nothing new,
                // fire no schedules
                continue;
            }
            // idle usage probe (spec: Usage%), BEFORE picking: with nothing
            // running, every account whose snapshot is older than
            // probe_stale_min gets one direct 1-token probe (the response
            // headers carry the 5h/7d numbers; ~9 tokens, no claude process)
            // so the ladder and the maxagents gates see real numbers
            if engine.probe_stale_min > 0 && self.running.is_empty() {
                self.probe_stale_accounts(engine, now);
            }
            let Some(dirs) = engine.projects.get(project_name) else { continue };
            // read-only checkout: no schedules fire and no work item runs here
            // (GraphRAG and the map are handled above and in sync_maps)
            if dirs.read_only {
                continue;
            }
            self.fire_schedules(&project);
            let topdir = expand_topdir(dirs.dirs.get("topdir").map(String::as_str).unwrap_or("."));
            self.dispatch(engine, &project, &topdir, &in_use);
        }

        // connectivity test requested from the webui: one haiku nudge, then
        // report the outcome (and the refreshed usage) via heartbeat.  After
        // the sync, so a request handled on a fresh process's first tick sees
        // the projects' accounts (before: none known -> the ambient login)
        if !engine.test_requested.is_empty() && engine.test_requested != self.last_test_handled {
            self.last_test_handled = engine.test_requested.clone();
            self.run_test(engine, &chosen_account);
        }
    }

    /// Running work that holds a cap slot (test sweep runs do not).
    fn capped_running(&self) -> usize {
        self.running.iter().filter(|r| !r.outside_cap).count()
    }

    /// Every project gets one test sweep template (decided 2026-09-30),
    /// created PAUSED by the first engine that sees none — a person turns it
    /// on (Resume schedule) and sets its interval.  iter_data refuses a second
    /// one, so racing engines make exactly one.  A failed create is retried
    /// at most every ten minutes.
    fn ensure_test_sweep(&mut self, project_name: &str) {
        if self.sweep_ensured.contains(project_name) {
            return;
        }
        let Some(items) = self.items.get(project_name) else { return };
        if items.iter().any(|i| i.is_test_sweep_template()) {
            self.sweep_ensured.insert(project_name.to_string());
            return;
        }
        if self.sweep_ensure_tried.get(project_name).map(|t| t.elapsed() < Duration::from_secs(600)).unwrap_or(false) {
            return;
        }
        let body = iter_core::test_sweep_template_body("paused", iter_core::TEST_SWEEP_EVERY_MIN);
        match self.api.post(&format!("/api/projects/{project_name}/workitems"), &body) {
            Ok(v) => {
                println!("[engine] {project_name}: created the test sweep (paused) {}", v["id"].as_str().unwrap_or("?"));
                self.sweep_ensured.insert(project_name.to_string());
            }
            Err(e) if e.status == 409 => {
                self.sweep_ensured.insert(project_name.to_string());
            }
            Err(e) => {
                eprintln!("[engine] {project_name}: could not create the test sweep: {e}");
                self.sweep_ensure_tried.insert(project_name.to_string(), Instant::now());
            }
        }
    }

    /// Fire due scheduled templates (itersched port). Race-safe across
    /// engines: claiming last_fired via a versioned write happens BEFORE the
    /// clone, so a 409 means another engine won this occurrence — skip.
    fn fire_schedules(&mut self, project: &Project) {
        let items = self.items.get(&project.name).cloned().unwrap_or_default();
        let now = chrono::Utc::now();
        for tpl in items.iter().filter(|i| i.state == "scheduled") {
            let Some(sched) = &tpl.sched else { continue };
            // dedup: while ANY clone is open, the schedule does not fire
            let open_clone = items
                .iter()
                .any(|i| i.source_schedule == tpl.id && iter_core::sched::is_open_state(&i.state));
            if open_clone {
                continue;
            }
            let last_completed = items
                .iter()
                .filter(|i| i.source_schedule == tpl.id && i.state == "complete")
                .filter_map(|i| iter_core::sched::parse_iso(&i.ts.complete))
                .max();
            if !iter_core::sched::due(sched, &tpl.ts.receive, now, last_completed) {
                continue;
            }
            // claim the fire
            let mut claimed = serde_json::to_value(tpl).unwrap();
            claimed["sched"]["last_fired"] = json!(now_utc());
            if self
                .api
                .put(
                    &format!(
                        "/api/projects/{}/workitems/{}?expect_version={}",
                        project.name, tpl.id, tpl.version
                    ),
                    &claimed,
                )
                .is_err()
            {
                continue; // lost the race (or transient) — next check re-evaluates
            }
            let clone = iter_core::sched::clone_from(tpl);
            match self.api.post(
                &format!("/api/projects/{}/workitems", project.name),
                &serde_json::to_value(&clone).unwrap(),
            ) {
                Ok(v) => println!(
                    "[engine] schedule '{}' fired -> {}",
                    tpl.name,
                    v.get("id").and_then(|i| i.as_str()).unwrap_or("?")
                ),
                Err(e) => eprintln!("[engine] schedule '{}' clone failed: {e}", tpl.name),
            }
        }
    }

    fn dispatch(&mut self, engine: &Engine, project: &Project, topdir: &str, in_use: &[String]) {
        let project_name = project.name.clone();
        let items = self.items.get(&project_name).cloned().unwrap_or_default();
        let by_id: HashMap<String, &WorkItem> = items.iter().map(|i| (i.id.clone(), i)).collect();

        // cluster-restart block (built 2026-09-09, iter_core::cluster): one
        // health verdict per tick, taken only when some open item carries the
        // durable tag.  A tagged QUEUED item is never picked while the cluster
        // is unavailable; a tagged PARKED item (what `iter block` leaves) is
        // requeued — tag kept — the moment the cluster is back up and healthy,
        // and the claim strips the tag when the item starts.
        let is_cluster_tagged = |i: &WorkItem| cluster::has_tag(&i.tags, CLUSTER_RESTART_TAG);
        let cluster_tagged_open = items.iter().any(|i| (i.state == "queued" || i.state == "parked") && is_cluster_tagged(i));
        let cluster_healthy = if cluster_tagged_open { self.cluster_health(project, &items) } else { true };

        // a project-wide hold: nothing starts this tick, and every item that
        // would otherwise be dispatchable is told why (spec: lock waits are
        // visible, 2026-09-07 — "queued and idle" must never be silent)
        let mut hold: Option<String> = None;

        // real usage drives both the account ladder and the maxagents gates
        let now = chrono::Utc::now();
        let usage_pct: u8;
        let account: Option<iter_core::Account>;
        if project.accounts.is_empty() {
            // single-account setup: the default snapshot (V2-compatible)
            usage_pct = usage::effective_pct_for("", now);
            account = None;
        } else {
            let map = usage::usage_map(&project.accounts, now);
            // an account with no token in the env_file is never picked — the
            // item is not claimed, so it can never be billed to another
            // account's token by the spawn path (2026-09-11)
            let tokenless = tokenless_accounts(&project.accounts);
            match pick_account(&project.accounts, &map, in_use, &tokenless) {
                Some(a) => {
                    usage_pct = map.get(&a.name).copied().unwrap_or(0);
                    account = Some(a.clone());
                }
                None => {
                    // every account is missing its token, or all accounts are
                    // at/over their stop%: stop all activity and monitor (a
                    // token appears on the next reload; expiry zeroes windows)
                    let all_tokenless = tokenless.len() == project.accounts.len();
                    if all_tokenless {
                        println!(
                            "[engine] {project_name}: no account token is set — {} accounts configured, none usable",
                            project.accounts.len()
                        );
                    } else {
                        println!(
                            "[engine] {project_name}: all accounts at stop% — holding until a usage window resets"
                        );
                    }
                    usage_pct = 100;
                    account = None;
                    hold = Some(hold_reason(all_tokenless).into());
                }
            }
        }
        if let Some(u) = account.as_ref().and_then(|a| usage::read_usage(&a.name)) {
            if let Some(age) = u.age_sec(now) {
                if age > usage::SNAPSHOT_STALE_WARN_SEC && !self.running.is_empty() {
                    eprintln!(
                        "[engine] warning: usage snapshot for '{}' is {age}s old",
                        account.as_ref().map(|a| a.name.as_str()).unwrap_or("default")
                    );
                }
            }
        }
        let cap = max_agents(&project.maxagents, usage_pct) as usize;
        let account_name = account.as_ref().map(|a| a.name.clone()).unwrap_or_default();

        // maxdailycost (spec): null = unlimited, 0 = spend nothing, >0 = $/day cap
        if let Some(capusd) = project.maxdailycost {
            let today = now_utc()[..10].to_string();
            let spent = self
                .api
                .get(&format!("/api/projects/{project_name}/spend"))
                .ok()
                .and_then(|v| v.as_array().cloned())
                .unwrap_or_default()
                .iter()
                .find(|r| r.get("date").and_then(|d| d.as_str()) == Some(today.as_str()))
                .and_then(|r| r.get("usd").and_then(|u| u.as_f64()))
                .unwrap_or(0.0);
            if capusd <= 0.0 || spent >= capusd {
                if self.budget_hold.get(&project_name) != Some(&today) {
                    println!("[engine] {project_name}: daily budget {} (${spent:.2} of ${capusd:.2}) — picking nothing today", if capusd <= 0.0 { "is zero" } else { "reached" });
                    self.budget_hold.insert(project_name.clone(), today);
                }
                hold.get_or_insert_with(|| "daily budget".into());
            }
        }
        let now_iso = now_utc();

        // lease-bound locks (CR 2026-09-25): once a minute the sweep removes
        // rows whose holder is not running under the row's lease — with
        // enforcement off it only lists them, the rollout's dry-run signal
        self.sweep_locks(project);

        // current central lock rows (locks + reservations).  Only LIVE lock
        // rows count (iter_core::live_lock_rows): unexpired, and held by an
        // item running under the row's lease — a row left by an item that is
        // not running never makes a waiter (the 2026-09-25 cycle); an expired
        // row is free (acquire treats it so; seen live 2026-09-08)
        let lock_rows: Vec<LockRow> = self
            .api
            .get(&format!("/api/projects/{project_name}/locks"))
            .ok()
            .and_then(|v| v.as_array().cloned())
            .unwrap_or_default()
            .into_iter()
            .filter_map(|r| serde_json::from_value(r).ok())
            .collect();
        let live = iter_core::live_lock_rows(&lock_rows, &by_id, &now_iso);

        // the cluster is back: requeue every parked tagged item that is NOT still
        // in flight.  `iter block` parks the item mid-run, seconds to minutes
        // before its turn ends; requeueing inside that gap would let the run
        // close through the gate (seen in e2e 2026-09-09).  In flight = running
        // on this engine, or still holding a live central lock row (another
        // engine's run releases its rows only when the run is over).
        if cluster_healthy {
            let in_flight = |i: &WorkItem| {
                self.running.iter().any(|r| r.current().0 == i.id)
                    || live.iter().any(|r| r.workid == i.id)
            };
            for i in items.iter().filter(|i| i.state == "parked" && is_cluster_tagged(i) && !in_flight(i)) {
                self.requeue_after_cluster_restart(project, i);
            }
        }

        // dependency gate: DEEP (workitem_dependency.md) — a blocker counts only
        // when it and everything it created closed complete; a failed blocker
        // simply keeps its dependents waiting (reopen + complete releases them)
        let kids = children_index(&items);
        let deps_satisfied = |item: &WorkItem| -> bool {
            dependency_status(item, &by_id, &kids) == DepStatus::Satisfied
        };
        // the running items whose live lock rows overlap this item's lockdirs:
        // (holder workid, the locked path) — the holder is a dependency the
        // engine knows about and, since 2026-09-07, records (`blockedby_locks`)
        let lock_holders = |item: &WorkItem| -> Vec<(String, String)> { iter_core::lock_holders(item, &live) };
        let scope_blocked = |item: &WorkItem| -> bool { !lock_holders(item).is_empty() };

        // classify every queued item once: approval / backoff / lock / hold.
        // A dependency wait carries no reason — the blocked-by nesting shows
        // it.  The pick loop below refines the rest (cap, reservation, agent cap).
        let mut waits: HashMap<String, Wait> = HashMap::new();
        // a parked item waiting out the restart shows the same reason as a
        // queued one (it is requeued above the moment the cluster is healthy)
        for i in items.iter().filter(|i| i.state == "parked" && cluster::blocks(i, cluster_healthy)) {
            waits.insert(i.id.clone(), Wait { reason: Some(CLUSTER_RESTART_REASON.into()), holders: vec![] });
        }
        // finished triages: remember them (the cache may lag the stamp a tick)
        self.triaging.retain(|(id, h)| {
            if h.is_finished() {
                self.triaged.insert(id.clone());
                false
            } else {
                true
            }
        });
        // the test sweep (2026-09-30): a run of the engine-owned template
        // starts as soon as it is queued — no cap slot, no usage or budget
        // hold (a shell run spends no model time).  It still needs the
        // project Running: dispatch is only reached then.
        let mut sweep_started: Vec<String> = Vec::new();
        for item in items.iter().filter(|i| i.state == "queued" && i.is_test_sweep_run() && !i.needs_approval && deps_satisfied(i)) {
            println!("[engine] test sweep run {} starts on its timer, outside the cap", &item.id[..8.min(item.id.len())]);
            if self.start_item(engine, project, topdir, item, &account_name) {
                sweep_started.push(item.id.clone());
            }
        }
        let in_triage = |i: &WorkItem| self.triaging.iter().any(|(id, _)| id == &i.id);
        for i in items.iter().filter(|i| i.state == "queued") {
            let mut w = Wait::default();
            if in_triage(i) {
                w.reason = Some(DEDUP_TRIAGE_REASON.into());
            } else if i.needs_approval {
                w.reason = Some("needs approval".into());
            } else if !i.retry_after.is_empty() && i.retry_after > now_iso {
                w.reason = Some(format!("retry after {}", hhmm(&i.retry_after)));
            } else if cluster::blocks(i, cluster_healthy) {
                w.reason = Some(CLUSTER_RESTART_REASON.into()); // renders "blocked by: cluster restart"
            } else {
                match dependency_status(i, &by_id, &kids) {
                    DepStatus::Satisfied => {
                        w.holders = lock_holders(i);
                        if let Some((holder, path)) = w.holders.first() {
                            w.reason = Some(if i.run_now {
                                // run_now never preempts a running holder (CR
                                // 4(d)): say whose run it waits for and when
                                // that run's session limit ends it at the latest
                                run_now_wait_reason(project, by_id.get(holder).copied(), holder, &self.agents)
                            } else {
                                format!("lock {path}")
                            });
                        } else if let Some(h) = &hold {
                            w.reason = Some(h.clone());
                        }
                    }
                    // every dependency wait names what it waits on (2026-09-13:
                    // a silent wait sat 15.5 h): "waiting on <id12>", "… (follow-up
                    // of blocker <b12>)", "blocker <id12> failed", "sibling <id12>
                    // goes first (mutual follow-up wait)", and a declared loop
                    // "dependency cycle with <id12>"
                    other => w.reason = other.reason(),
                }
            }
            waits.insert(i.id.clone(), w);
        }
        // the wait-for graph (CR 2026-09-25 option (b)): every loop through
        // declared, deep and lock edges, tagged on each member and noted once
        Self::detect_deadlocks(&self.api, &self.name, &mut self.announced_cycles, project, &items, &by_id, &live, &mut waits);
        // a human already accepted it at the close gate: closing uses no model
        // time, so it waits neither for a cap slot nor out a usage hold
        let mut accepted_now = Self::close_accepted(&self.api, &self.name, &mut self.accept_checked, &mut self.accepting, project, topdir, &items);
        for id in &accepted_now {
            waits.remove(id);
        }
        for id in sweep_started {
            waits.remove(&id);
            accepted_now.push(id); // started: kept out of the pick loop below
        }
        if hold.is_some() {
            self.reconcile_waits(project, &items, &waits);
            return;
        }

        // stage-2 dedup triage (crate::dedup, 2026-09-10): every newly created
        // queued item is judged against its open neighbours BEFORE its first
        // dispatch — on its own thread, one tick of hold, never when the
        // project is held (the judge spends).  A judge failure stamps the
        // item and it dispatches next tick as today.
        let awaiting_triage: Vec<&WorkItem> = items
            .iter()
            .filter(|i| crate::dedup::needs_triage(i) && !self.triaged.contains(&i.id) && !in_triage(i))
            .collect();
        for i in awaiting_triage {
            let api = self.api.clone();
            let project_c = project.clone();
            let topdir_c = topdir.to_string();
            let snapshot = items.clone();
            let item = i.clone();
            let account = account_name.clone();
            let agent_def = self.agents.get(iter_core::dedup::JUDGE_AGENT).cloned().unwrap_or(Value::Null);
            let handle = std::thread::spawn(move || {
                let judge = crate::dedup::ClaudeJudge {
                    api: &api, project: &project_c, topdir: &topdir_c, account: &account, agent_def: &agent_def, workid: &item.id,
                };
                crate::dedup::triage(&api, &project_c, &snapshot, &item, &judge);
            });
            self.triaging.push((i.id.clone(), handle));
            if let Some(w) = waits.get_mut(&i.id) {
                w.reason.get_or_insert_with(|| DEDUP_TRIAGE_REASON.into());
            }
        }
        let held_for_triage: std::collections::HashSet<String> = items
            .iter()
            .filter(|i| self.triaging.iter().any(|(id, _)| id == &i.id) || (crate::dedup::needs_triage(i) && !self.triaged.contains(&i.id)))
            .map(|i| i.id.clone())
            .collect();

        // Run Now (operator override, 2026-09-04): a queued item flagged run_now
        // starts as soon as its dependencies are complete and no lock overlaps,
        // even when the maxagents cap is full.  The cap then stays saturated
        // until enough running work finishes to bring the count back under it,
        // so nothing else starts in the meantime.
        let mut started_now: Vec<String> = accepted_now;
        let run_now: Vec<&WorkItem> = items
            .iter()
            .filter(|i| i.run_now && i.state == "queued" && !i.needs_approval && !cluster::blocks(i, cluster_healthy) && deps_satisfied(i) && !scope_blocked(i))
            .collect();
        for item in run_now {
            println!(
                "[engine] run-now override: starting {} '{}' (running {} / cap {})",
                &item.id[..8.min(item.id.len())], item.name, self.capped_running(), cap
            );
            if self.start_item(engine, project, topdir, item, &account_name) {
                started_now.push(item.id.clone());
                waits.remove(&item.id);
            }
        }
        let running_now = self.capped_running();
        let set_reason = |waits: &mut HashMap<String, Wait>, id: &str, r: String| {
            if let Some(w) = waits.get_mut(id) {
                if w.reason.is_none() {
                    w.reason = Some(r);
                }
            }
        };

        let mut queued: Vec<&WorkItem> = items
            .iter()
            .filter(|i| i.state == "queued" && !i.needs_approval && !started_now.contains(&i.id))
            .filter(|i| !held_for_triage.contains(&i.id)) // stage-2 dedup: judged before the first dispatch
            .filter(|i| i.retry_after.is_empty() || i.retry_after <= now_iso) // failure backoff
            .filter(|i| !cluster::blocks(i, cluster_healthy)) // cluster-restart block
            .filter(|i| {
                self.deferred
                    .get(&i.id)
                    .map(|until| Instant::now() >= *until)
                    .unwrap_or(true)
            })
            .filter(|i| deps_satisfied(i))
            .collect();
        queued.sort_by_key(|i| (i.priority, i.ts.receive.clone()));

        if running_now >= cap {
            for i in &queued {
                set_reason(&mut waits, &i.id, format!("usage cap ({running_now}/{cap})"));
            }
            self.reconcile_waits(project, &items, &waits);
            return;
        }

        // scope reservation (central "reserve" rows): the best dispatchable-but-
        // scope-blocked item reserves its paths so new overlapping work stops
        // being admitted; strictly-better priority still barges.
        // Reservations live under their own key (`reserve:<path>`, 2026-09-25)
        // so they never make a strictly-better item's lock on the same path fail.
        let reserver: Option<&WorkItem> = queued.iter().copied().find(|i| scope_blocked(i));
        if let Some(r) = reserver {
            for d in &r.lockdirs {
                let _ = self.api.post(
                    &format!("/api/projects/{project_name}/locks/acquire"),
                    &json!({"path": d, "kind": "reserve", "engine": self.name,
                            "workid": r.id, "ttl_sec": 600}),
                );
            }
        }
        let reserved: Vec<(String, i64, String)> = lock_rows
            .iter()
            .filter(|r| r.kind == "reserve" && (r.expires.is_empty() || r.expires >= now_iso))
            // a reservation means "I am next for this scope": it only holds while
            // its holder is still queued. Since reservations got their own row
            // (F17) a holder that started, closed, or went to question leaves the
            // row behind until it expires, and honouring it then blocked the
            // scope for nobody (found by e2e: an answered item waited on a
            // reservation held by a question item)
            .filter_map(|r| by_id.get(&r.workid).filter(|i| i.state == "queued").map(|i| (r.path.clone(), i.priority, r.workid.clone())))
            .collect();

        let mut slots = cap.saturating_sub(running_now);
        for item in queued {
            if slots == 0 {
                set_reason(&mut waits, &item.id, format!("usage cap ({}/{cap})", self.capped_running()));
                continue;
            }
            if scope_blocked(item) {
                continue; // reason "lock …" already set above
            }
            // reservation gate: overlapping a reserved scope requires strictly
            // better (lower) priority than the reserver; the reserver is exempt
            let gated = item.lockdirs.iter().find_map(|d| {
                reserved.iter().find(|(path, rprio, rworkid)| {
                    rworkid != &item.id && paths_overlap(d, path) && item.priority >= *rprio
                })
            });
            if let Some((_, rprio, rworkid)) = gated {
                set_reason(&mut waits, &item.id, format!("reserved by {} (P{rprio})", &rworkid[..8.min(rworkid.len())]));
                continue;
            }
            // per-agent-type cap (project override "max", else agent default)
            let type_running =
                self.running.iter().filter(|r| r.agent == item.agent && !r.outside_cap).count();
            let type_max = project
                .agents
                .get(&item.agent)
                .and_then(|o| o.get("max"))
                .and_then(|m| m.as_u64())
                .or_else(|| {
                    self.agents.get(&item.agent).and_then(|a| a.get("max")).and_then(|m| m.as_u64())
                })
                .unwrap_or(4) as usize;
            if !item.is_shell() && type_running >= type_max {
                set_reason(&mut waits, &item.id, format!("agent cap ({} {type_running}/{type_max})", item.agent));
                continue;
            }
            if self.start_item(engine, project, topdir, item, &account_name) {
                slots -= 1;
                waits.remove(&item.id);
            }
        }
        self.reconcile_waits(project, &items, &waits);
    }

    /// Write the wait state onto every open item whose recorded state differs
    /// from what this tick derived (spec: lock waits are visible, 2026-09-07):
    /// `blockedby_locks` (the lock holders — a dependency the webui nests the
    /// waiter under) and the one engine-owned "blocked by: …" tag.  Queued
    /// items get the derived state; other open items get both cleared (a
    /// human parked or paused a waiter).  One versioned PUT per changed item;
    /// a 409 means someone wrote first and the next tick re-derives it.
    fn reconcile_waits(&self, project: &Project, items: &[WorkItem], waits: &HashMap<String, Wait>) {
        for item in items {
            if item.state == "complete" || item.state == "failed" {
                continue; // closed items are immutable
            }
            let w = if item.state == "queued" || item.state == "parked" { waits.get(&item.id).cloned().unwrap_or_default() } else { Wait::default() };
            let want_holders: Vec<String> = w.holders.iter().map(|(h, _)| h.clone()).collect();
            let want_tag = w.reason.as_ref().map(|r| format!("{BLOCKED_TAG_PREFIX}{r}"));
            let have_tag = item.tags.iter().find(|t| t.text.starts_with(BLOCKED_TAG_PREFIX)).map(|t| t.text.clone());
            if item.blockedby_locks == want_holders && have_tag == want_tag {
                continue;
            }
            let sid = &item.id[..8.min(item.id.len())];
            for (h, path) in &w.holders {
                if !item.blockedby_locks.contains(h) {
                    println!("[engine] {sid} blocked by {} (lock {path})", &h[..8.min(h.len())]);
                }
            }
            for h in &item.blockedby_locks {
                if !want_holders.contains(h) {
                    println!("[engine] {sid} no longer blocked by {} (lock released)", &h[..8.min(h.len())]);
                }
            }
            match (&have_tag, &want_tag) {
                (_, Some(t)) if have_tag.as_ref() != Some(t) => println!("[engine] {sid} '{}': {t}", item.name),
                (Some(t), None) => println!("[engine] {sid} '{}': cleared '{t}'", item.name),
                _ => {}
            }
            let mut updated = serde_json::to_value(item).unwrap();
            updated["blockedby_locks"] = json!(want_holders);
            let mut tags: Vec<Value> = item
                .tags
                .iter()
                .filter(|t| !t.text.starts_with(BLOCKED_TAG_PREFIX))
                .map(|t| json!({"text": t.text, "color": t.color}))
                .collect();
            if let Some(t) = &want_tag {
                tags.push(json!({"text": t, "color": BLOCKED_TAG_COLOR}));
            }
            updated["tags"] = json!(tags);
            if let Err(e) = self.api.put(
                &format!("/api/projects/{}/workitems/{}?expect_version={}", project.name, item.id, item.version),
                &updated,
            ) {
                if e.status != 409 {
                    eprintln!("[engine] could not record the wait state on {sid}: {e}");
                }
            }
        }
    }

    /// One cluster-health verdict for this project (iter_core::cluster::evaluate):
    /// the newest restart-window clone's state and its `clusterhealth` row,
    /// else the wall clock in the configured zone.  Announced on change only.
    fn cluster_health(&mut self, project: &Project, items: &[WorkItem]) -> bool {
        let cfg = &project.cluster_restart;
        let clone = cluster::newest_clone(cfg, items);
        let details: Vec<Value> = clone
            .and_then(|c| self.api.get(&format!("/api/projects/{}/workitems/{}/details", project.name, c.id)).ok())
            .and_then(|v| v.as_array().cloned())
            .unwrap_or_default();
        let h = cluster::evaluate(cfg, clone, &details, chrono::Utc::now());
        let announced = self.cluster_state.get(&project.name).map(|(ok, why)| (*ok, why.as_str()));
        if announced != Some((h.healthy, h.why.as_str())) {
            println!("[engine] {}: cluster {} — {}", project.name, if h.healthy { "back up and healthy" } else { "unavailable" }, h.why);
            self.cluster_state.insert(project.name.clone(), (h.healthy, h.why.clone()));
        }
        h.healthy
    }

    /// The cluster is back: a parked item still tagged `blocked-by-cluster-restart`
    /// goes to queued with the tag kept (the claim strips it, so the row stays
    /// truthful right up to the moment the agent starts) and a "doc" row
    /// naming the release.  A 409 means someone wrote first; next tick re-derives.
    fn requeue_after_cluster_restart(&self, project: &Project, item: &WorkItem) {
        let why = self.cluster_state.get(&project.name).map(|(_, w)| w.clone()).unwrap_or_default();
        let mut updated = serde_json::to_value(item).unwrap();
        updated["state"] = json!("queued");
        updated["retry_after"] = json!("");
        match self.api.put(
            &format!("/api/projects/{}/workitems/{}?expect_version={}", project.name, item.id, item.version),
            &updated,
        ) {
            Ok(_) => {
                println!("[engine] {} '{}': cluster back up and healthy — requeued (parked -> queued, tag kept until it starts)", &item.id[..8.min(item.id.len())], item.name);
                let _ = self.api.post(
                    &format!("/api/projects/{}/workitems/{}/details", project.name, item.id),
                    &json!({"key": "doc", "valuetype": "text", "value": format!(
                        "requeued by engine {} at {}: the cluster is back up and healthy ({why}); `blocked-by-cluster-restart` comes off when the item starts",
                        self.name, now_utc()
                    )}),
                );
            }
            Err(e) if e.status == 409 => {}
            Err(e) => eprintln!("[engine] could not requeue {} after the cluster restart: {e}", &item.id[..8.min(item.id.len())]),
        }
    }

    /// F11 (pdy-dev badb3f42e, 2026-09-20: an accept sat 40+ minutes behind
    /// "usage cap (3/2)"): every queued item whose latest close-gate widget a
    /// human answered "accept" is claimed and closed complete on a short
    /// thread of its own — no slot, no hold.  Only items the gate has held
    /// before (a bounce, or a close-gate lasterror) have their details read,
    /// once per record version.  Returns the ids claimed.
    #[allow(clippy::too_many_arguments)]
    fn close_accepted(
        api: &Api,
        engine_name: &str,
        checked: &mut HashSet<(String, u64)>,
        accepting: &mut Vec<(String, std::thread::JoinHandle<()>)>,
        project: &Project,
        topdir: &str,
        items: &[WorkItem],
    ) -> Vec<String> {
        accepting.retain(|(_, h)| !h.is_finished());
        let mut out: Vec<String> = Vec::new();
        for i in items.iter().filter(|i| i.state == "queued" && !i.is_shell() && (i.gate_bounces > 0 || i.lasterror.starts_with("close gate"))) {
            if accepting.iter().any(|(id, _)| id == &i.id) || checked.contains(&(i.id.clone(), i.version)) {
                continue;
            }
            let details = crate::work::fetch_details(api, project, i);
            if !crate::gate::accepted_by_human(&details) {
                checked.insert((i.id.clone(), i.version));
                continue;
            }
            // claim it exactly as a dispatch would (one engine wins), then close
            let mut claimed = serde_json::to_value(i).unwrap();
            claimed["state"] = json!("in-progress");
            claimed["engine"] = json!(engine_name);
            claimed["lease"] = json!(uuid::Uuid::new_v4().to_string());
            claimed["tags"] = json!(claim_tags(&i.tags));
            claimed["blockedby_locks"] = json!([]);
            claimed["ts"]["start"] = json!(now_utc());
            let Ok(Ok(item)) = api
                .put(&format!("/api/projects/{}/workitems/{}?expect_version={}", project.name, i.id, i.version), &claimed)
                .map(serde_json::from_value::<WorkItem>)
            else {
                continue;
            };
            println!("[engine] {} '{}': close-gate widget answered accept — closing now, outside the cap", &i.id[..8.min(i.id.len())], i.name);
            let (api, engine_name, project, topdir) = (api.clone(), engine_name.to_string(), project.clone(), topdir.to_string());
            out.push(item.id.clone());
            let id = item.id.clone();
            let handle = std::thread::spawn(move || crate::work::close_accepted(&api, &engine_name, &project, &topdir, item));
            accepting.push((id, handle));
        }
        out
    }

    /// Every in-progress record of this project that names this engine but
    /// has no thread here, no journaled close and started longer ago than its
    /// session timeout is repaired as a failed attempt (work::repair_ghost).
    /// Only this engine's own records: another engine's may be a live session
    /// on another machine.
    fn repair_ghosts(&mut self, project: &Project, topdir: &str) {
        let now = chrono::Utc::now();
        let mut busy: Vec<String> = self.running.iter().map(|r| r.current().0).collect();
        busy.extend(self.explaining.iter().filter(|(_, h)| !h.is_finished()).map(|(id, _)| id.clone()));
        busy.extend(self.triaging.iter().map(|(id, _)| id.clone()));
        busy.extend(self.accepting.iter().filter(|(_, h)| !h.is_finished()).map(|(id, _)| id.clone()));
        let journal = crate::work::pending_close_dir(topdir);
        let ghosts: Vec<WorkItem> = self
            .items
            .get(&project.name)
            .map(|v| {
                v.iter()
                    .filter(|i| {
                        let agent_def = self.agents.get(&i.agent).cloned().unwrap_or(Value::Null);
                        let timeout = crate::work::agent_timeout(project, i, &agent_def);
                        let journaled = journal.join(format!("{}.json", i.id)).exists();
                        crate::work::is_ghost(i, &self.name, &busy, journaled, timeout, now)
                    })
                    .cloned()
                    .collect()
            })
            .unwrap_or_default();
        for g in ghosts {
            crate::work::repair_ghost(&self.api, &self.name, project, &g);
        }
    }

    /// Once per LOCK_RENEW_EVERY_SEC per project: `locks/sweep`, dry-run
    /// while the project does not enforce leases.  Each row it removes (or
    /// would remove) is one log line; dry-run lines are printed once.
    fn sweep_locks(&mut self, project: &Project) {
        let due = self.last_sweep.get(&project.name).map(|t| t.elapsed() >= Duration::from_secs(iter_core::LOCK_RENEW_EVERY_SEC)).unwrap_or(true);
        if !due {
            return;
        }
        self.last_sweep.insert(project.name.clone(), Instant::now());
        let Ok(v) = self.api.post(&format!("/api/projects/{}/locks/sweep", project.name), &json!({"dry_run": false})) else { return };
        for r in v.get("removed").and_then(|x| x.as_array()).cloned().unwrap_or_default() {
            let s = |k: &str| r.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string();
            let (path, workid) = (s("path"), s("workid"));
            println!(
                "[engine] {}: lock sweep removed {} {path} held by {} ({})",
                project.name, s("kind"), &workid[..8.min(workid.len())], s("why")
            );
        }
    }

    /// The deadlock detector (CR 2026-09-25 5.5), after classification.  A
    /// cycle through a lock held by an item that is not running (possible
    /// only before enforcement, or through a bug) is broken by releasing that
    /// holder's rows.  Any other cycle is dependency-only and is not edited:
    /// every member's wait reason becomes "deadlock: a → b → … (kinds)" and
    /// each member gets one doc row the first time this engine sees it.
    /// (An associated fn over the fields it needs: dispatch holds other
    /// borrows of `self` across the call.)
    #[allow(clippy::too_many_arguments)]
    fn detect_deadlocks(
        api: &Api,
        engine_name: &str,
        announced: &mut HashSet<Vec<String>>,
        project: &Project,
        items: &[WorkItem],
        by_id: &HashMap<String, &WorkItem>,
        live: &[&LockRow],
        waits: &mut HashMap<String, Wait>,
    ) {
        let cycles = iter_core::find_wait_cycles(&iter_core::wait_edges(items, live));
        for c in cycles {
            let text = iter_core::waitgraph::describe_cycle(&c);
            let holders = iter_core::waitgraph::not_running_lock_holders(&c, by_id);
            if !holders.is_empty() {
                for h in &holders {
                    let released = api
                        .post(&format!("/api/projects/{}/locks/release_all", project.name), &json!({"workid": h}))
                        .ok()
                        .and_then(|v| v.get("released").and_then(|r| r.as_array()).map(|a| a.len()))
                        .unwrap_or(0);
                    println!("[engine] {}: released {released} lock rows of {} (not running) to break {text}", project.name, &h[..8.min(h.len())]);
                    let note = format!(
                        "deadlock resolved by engine {} at {}: released {released} lock rows of {}, which was not running; cycle {text}",
                        engine_name, now_utc(), &h[h.len().saturating_sub(12)..]
                    );
                    let waiters: Vec<&str> = c.iter().filter(|e| &e.to == h).map(|e| e.from.as_str()).collect();
                    for id in std::iter::once(h.as_str()).chain(waiters) {
                        let _ = api.post(
                            &format!("/api/projects/{}/workitems/{}/details", project.name, id),
                            &json!({"key": "doc", "valuetype": "text", "value": note}),
                        );
                    }
                }
                continue;
            }
            let mut members: Vec<String> = c.iter().map(|e| e.from.clone()).collect();
            members.sort();
            for m in &members {
                if let Some(w) = waits.get_mut(m) {
                    w.reason = Some(format!("deadlock: {text}"));
                }
            }
            if announced.insert(members.clone()) {
                println!("[engine] {}: deadlock {text}", project.name);
                for m in &members {
                    let _ = api.post(
                        &format!("/api/projects/{}/workitems/{}/details", project.name, m),
                        &json!({"key": "doc", "valuetype": "text", "value": format!(
                            "deadlock seen by engine {} at {}: {text}. No member can start until one of these links is removed; the engine does not edit dependencies.",
                            engine_name, now_utc())}),
                    );
                }
            }
        }
    }

    fn start_item(
        &mut self,
        _engine: &Engine,
        project: &Project,
        topdir: &str,
        item: &WorkItem,
        account: &str,
    ) -> bool {
        // claim: queued -> in-progress via versioned write (loses race gracefully)
        let mut claimed = serde_json::to_value(item).unwrap();
        claimed["state"] = json!("in-progress");
        claimed["run_now"] = json!(false); // the override is consumed by this start
        claimed["retry_after"] = json!("");
        // the wait is over: drop the lock-derived dependency, the engine's tag
        // and the durable cluster-restart tag (iter_core::claim_tags)
        claimed["blockedby_locks"] = json!([]);
        claimed["tags"] = json!(claim_tags(&item.tags));
        claimed["engine"] = json!(self.name);
        claimed["attempt"] = json!(item.attempt + 1);
        claimed["ts"]["start"] = json!(now_utc());
        // the run's lease (CR 2026-09-25): written in the same versioned claim
        // that makes the item in-progress; every lock row this run takes is
        // stamped with it and dies when the close clears it
        let lease = uuid::Uuid::new_v4().to_string();
        claimed["lease"] = json!(lease);
        let resp = self.api.put(
            &format!(
                "/api/projects/{}/workitems/{}?expect_version={}",
                project.name, item.id, item.version
            ),
            &claimed,
        );
        let claimed_item: WorkItem = match resp.and_then(|v| {
            serde_json::from_value(v).map_err(|e| crate::client::ApiError { status: 0, body: e.to_string() })
        }) {
            Ok(i) => i,
            Err(e) => {
                if e.status == 409 {
                    // usually another engine won the race; if this repeats every
                    // tick for the same item the row's version is out of step
                    println!("[engine] claim conflict on {} (v{}) — another engine took it, or its version is stale", &item.id[..8], item.version);
                } else {
                    eprintln!("[engine] claim failed for {}: {e}", item.id);
                }
                return false;
            }
        };

        // central locks for every lockdir: short rows, renewed every minute
        // by renew_leases while the run lasts (the old 65-minute rows lapsed
        // mid-run and taught agents to renew locks themselves)
        for d in &item.lockdirs {
            let res = self.api.post(
                &format!("/api/projects/{}/locks/acquire", project.name),
                &json!({"path": d, "kind": "lock", "engine": self.name,
                        "workid": item.id, "lease": lease, "ttl_sec": iter_core::LOCK_LEASE_TTL_SEC}),
            );
            if res.is_err() {
                let _ = self.api.post(
                    &format!("/api/projects/{}/locks/release_all", project.name),
                    &json!({"workid": item.id, "lease": lease}),
                );
                // lost the lock race: put it back to queued, lease cleared
                let mut back = serde_json::to_value(&claimed_item).unwrap();
                back["state"] = json!("queued");
                back["lease"] = json!("");
                let _ = self.api.put(
                    &format!(
                        "/api/projects/{}/workitems/{}?expect_version={}",
                        project.name, item.id, claimed_item.version
                    ),
                    &back,
                );
                self.deferred.insert(item.id.clone(), Instant::now() + Duration::from_secs(5));
                return false;
            }
        }

        println!(
            "[engine] start {} '{}' (agent {}, P{})",
            &item.id[..8.min(item.id.len())],
            item.name,
            item.agent,
            item.priority
        );
        let api = self.api.clone();
        let engine_name = self.name.clone();
        let project = project.clone();
        let topdir = topdir.to_string();
        let run_item = claimed_item;
        let counter = self.running_count.clone();
        counter.fetch_add(1, Ordering::SeqCst);
        let agent_type = run_item.agent.clone();
        let outside_cap = run_item.is_test_sweep_run();
        let project_name = project.name.clone();
        let cur = Arc::new(Mutex::new((run_item.id.clone(), lease)));
        let thread_cur = cur.clone();
        let account = account.to_string();
        let handle = std::thread::spawn(move || {
            crate::work::execute(&api, &engine_name, &project, &topdir, run_item, &account, &thread_cur);
            counter.fetch_sub(1, Ordering::SeqCst);
        });
        self.running.push(Running { agent: agent_type, outside_cap, project: project_name, cur, handle });
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hold_reason_names_the_missing_token() {
        assert_eq!(hold_reason(true), "no account token");
        assert_eq!(hold_reason(false), "accounts at stop%");
    }

    /// A tokenless account's test is red and never nudges (a nudge with no
    /// token tests the ambient login and files its usage under the account's
    /// name); an empty account is the ambient login and the result says so.
    #[test]
    fn test_result_for_a_tokenless_account_is_not_ok() {
        let ok_run = |text: &str| crate::work::RunOut { subtype: "success".into(), text: text.into(), ..Default::default() };
        let r = test_outcome(
            Err("account 'Dev4' has no token: DEV4_TOKEN is not set in /x/.env".into()),
            "Dev4", "DEV4_TOKEN", "2026-09-11T10:00:00Z",
            |_| panic!("must not nudge without the account's token"),
        );
        assert_eq!(r["ok"], false);
        assert_eq!(r["account"], "Dev4");
        assert_eq!(r["error"], "no token for account 'Dev4' (DEV4_TOKEN unset)");
        let r = test_outcome(Ok(None), "", "", "t", |tok| {
            assert!(tok.is_none());
            Ok((ok_run("hi"), 5))
        });
        assert_eq!(r["ok"], true);
        assert_eq!(r["account"], "default (ambient CLI login)");
        let r = test_outcome(Ok(Some("tok".into())), "Dev1", "DEV1_TOKEN", "t", |tok| {
            assert_eq!(tok.as_deref(), Some("tok"));
            Ok((ok_run("hi"), 5))
        });
        assert_eq!((r["ok"].as_bool(), r["account"].as_str()), (Some(true), Some("Dev1")));
        let r = test_outcome(Ok(Some("tok".into())), "Dev1", "DEV1_TOKEN", "t", |_| Err("claude exited 1".into()));
        assert_eq!((r["ok"].as_bool(), r["error"].as_str()), (Some(false), Some("claude exited 1")));
    }

    fn runtime(api: crate::client::Api) -> EngineRuntime {
        EngineRuntime::new(api, "E1".into(), "/x/.env".into())
    }
    fn running(workid: &str, lease: &str) -> Running {
        Running { agent: "code".into(), outside_cap: false, project: "p".into(), cur: Arc::new(Mutex::new((workid.into(), lease.into()))), handle: std::thread::spawn(|| {}) }
    }

    /// F4 (CR option (e)): a run far longer than the 600 s lease lifetime
    /// keeps its rows, because the engine renews them every minute with the
    /// run's lease — on a timer, whatever the agent is doing.
    #[test]
    fn renewal_keeps_a_long_run_holding_its_rows() {
        let clock = Arc::new(Mutex::new(0i64));
        let expires = Arc::new(Mutex::new(iter_core::LOCK_LEASE_TTL_SEC));
        let (c2, e2) = (clock.clone(), expires.clone());
        let srv = crate::client::fake::serve(move |m, path, body| {
            if m == "POST" && path.ends_with("/locks/renew") {
                if body["lease"] == "L1" {
                    *e2.lock().unwrap() = *c2.lock().unwrap() + body["ttl_sec"].as_i64().unwrap();
                    return Some((200, json!({"renewed": 1})));
                }
                return Some((409, json!({"refused": "stale lease", "error": "refused: lease L0 is not the live lease"})));
            }
            Some((200, json!({})))
        });
        let mut rt = runtime(srv.api());
        rt.running.push(running("11112222-aaaa", "L1"));
        for minute in 0..=13 {
            *clock.lock().unwrap() = minute * 60; // 13 minutes > the 10-minute lease
            rt.last_renew = Instant::now() - Duration::from_secs(iter_core::LOCK_RENEW_EVERY_SEC);
            rt.renew_leases();
            assert!(*expires.lock().unwrap() > *clock.lock().unwrap(), "minute {minute}: the row lapsed");
        }
        assert_eq!(srv.calls_to("POST", "/locks/renew").len(), 14);
        assert_eq!(srv.calls_to("POST", "/locks/renew")[0]["workid"], "11112222-aaaa");
        // not due yet: no call
        rt.renew_leases();
        assert_eq!(srv.calls_to("POST", "/locks/renew").len(), 14);
        // a refused renewal: another run owns the item — this run is revoked
        // (Abandon), once, however many renewals are refused
        rt.running = vec![running("33334444-bbbb", "L0")];
        for _ in 0..3 {
            rt.last_renew = Instant::now() - Duration::from_secs(120);
            rt.renew_leases();
        }
        let mine: Vec<_> = crate::work::REVOKED.lock().unwrap().iter().filter(|(w, _, _)| w == "33334444-bbbb").cloned().collect();
        assert_eq!(mine.len(), 1, "{mine:?}");
        assert_eq!(mine[0].1, "L0");
        assert!(matches!(&mine[0].2, crate::work::Revoke::Abandon(n) if n.contains("another run owns the item")));
        crate::work::REVOKED.lock().unwrap().retain(|(w, _, _)| w != "33334444-bbbb");
    }

    /// Lapsed rows (2026-09-28): a renewal that renews fewer rows than the
    /// item has lockdirs means they expired during an outage.  Every path
    /// still free → re-taken under the same lease, the run continues, one
    /// note; a path held by another item → this run's rows released and the
    /// run revoked for a re-queue naming the holder.
    #[test]
    fn lapsed_rows_are_retaken_or_the_run_is_requeued() {
        let held = Arc::new(Mutex::new(false));
        let h2 = held.clone();
        let srv = crate::client::fake::serve(move |m, path, body| {
            if m == "POST" && path.ends_with("/locks/renew") {
                return Some((200, json!({"renewed": 0})));
            }
            if m == "POST" && path.ends_with("/locks/acquire") {
                if *h2.lock().unwrap() && body["path"] == "{topdir}/b" {
                    return Some((409, json!({"error": "conflict", "current": {"workid": "77778888-0000-4000-8000-9999aaaabbbb", "path": "{topdir}/b"}})));
                }
                return Some((200, body.clone()));
            }
            Some((200, json!({})))
        });
        let mut rt = runtime(srv.api());
        let item = WorkItem { id: "lapse001-0000".into(), project: "p".into(), state: "in-progress".into(),
            lockdirs: vec!["{topdir}/a".into(), "{topdir}/b".into()], lease: "LL".into(), ..Default::default() };
        rt.items.insert("p".into(), vec![item]);
        rt.running.push(running("lapse001-0000", "LL"));
        // all free: both re-taken with the run's lease, a note, no revocation
        rt.last_renew = Instant::now() - Duration::from_secs(120);
        rt.renew_leases();
        let acq = srv.calls_to("POST", "/locks/acquire");
        assert_eq!(acq.len(), 2);
        assert!(acq.iter().all(|b| b["lease"] == "LL" && b["workid"] == "lapse001-0000"));
        assert!(srv.calls_to("POST", "/workitems/lapse001-0000/details")[0]["value"].as_str().unwrap().contains("re-took them"));
        assert!(!crate::work::REVOKED.lock().unwrap().iter().any(|(w, _, _)| w == "lapse001-0000"));
        // b taken meanwhile: rows released (this lease only), run revoked for a re-queue
        *held.lock().unwrap() = true;
        rt.last_renew = Instant::now() - Duration::from_secs(120);
        rt.renew_leases();
        let rel = srv.calls_to("POST", "/locks/release_all");
        assert_eq!((rel.len(), rel[0]["lease"].as_str()), (1, Some("LL")));
        let mine: Vec<_> = crate::work::REVOKED.lock().unwrap().iter().filter(|(w, _, _)| w == "lapse001-0000").cloned().collect();
        assert!(matches!(&mine[0].2, crate::work::Revoke::Requeue(n) if n.contains("9999aaaabbbb") && n.contains("{topdir}/b")), "{mine:?}");
        crate::work::REVOKED.lock().unwrap().retain(|(w, _, _)| w != "lapse001-0000");
        // an item without lockdirs renews nothing and that is fine
        let mut rt = runtime(srv.api());
        rt.items.insert("p".into(), vec![WorkItem { id: "nolocks-0001".into(), ..Default::default() }]);
        rt.running.push(running("nolocks-0001", "LN"));
        let before = srv.calls_to("POST", "/locks/acquire").len();
        rt.last_renew = Instant::now() - Duration::from_secs(120);
        rt.renew_leases();
        assert_eq!(srv.calls_to("POST", "/locks/acquire").len(), before);
    }

    /// F3 R3: a record in-progress on THIS engine, older than its session
    /// timeout and with no thread behind it, is repaired as a failed attempt
    /// (doc row, queued behind the backoff, lease cleared, locks released);
    /// a claim one second old and another engine's record are left alone.
    #[test]
    fn ghost_in_progress_is_repaired_and_controls_are_not() {
        let srv = crate::client::fake::serve(|_, _, _| Some((200, json!({}))));
        let mut rt = runtime(srv.api());
        let old = (chrono::Utc::now() - chrono::Duration::hours(3)).format("%Y-%m-%dT%H:%M:%SZ").to_string();
        let fresh = (chrono::Utc::now() - chrono::Duration::seconds(1)).format("%Y-%m-%dT%H:%M:%SZ").to_string();
        let mk = |id: &str, engine: &str, start: &str| {
            let mut w = WorkItem { id: id.into(), project: "p".into(), state: "in-progress".into(), engine: engine.into(),
                agent: "code".into(), attempt: 1, version: 3, lease: "L".into(), ..Default::default() };
            w.ts.start = start.into();
            w
        };
        rt.items.insert("p".into(), vec![mk("ghost-0001", "E1", &old), mk("fresh-0002", "E1", &fresh), mk("other-0003", "E2", &old)]);
        let mut project = Project { name: "p".into(), ..Default::default() };
        project.failure.maxattempts = 5;
        let top = std::env::temp_dir().join(format!("iter4-ghost-{}", std::process::id()));
        rt.repair_ghosts(&project, &top.to_string_lossy());
        let puts = srv.calls_to("PUT", "/workitems/");
        assert_eq!(puts.len(), 1, "{puts:?}");
        assert_eq!((puts[0]["id"].as_str(), puts[0]["state"].as_str(), puts[0]["lease"].as_str()), (Some("ghost-0001"), Some("queued"), Some("")));
        assert!(!puts[0]["retry_after"].as_str().unwrap().is_empty());
        let docs = srv.calls_to("POST", "/workitems/ghost-0001/details");
        assert!(docs[0]["value"].as_str().unwrap().contains("in-progress with no session behind it"));
        assert_eq!(srv.calls_to("POST", "/locks/release_all")[0]["workid"], "ghost-0001");
        assert!(srv.calls_to("POST", "fresh-0002").is_empty() && srv.calls_to("POST", "other-0003").is_empty());
        // a running thread for it: not a ghost
        rt.running.push(running("ghost-0001", "L"));
        let before = srv.calls_to("PUT", "/workitems/").len();
        rt.repair_ghosts(&project, &top.to_string_lossy());
        assert_eq!(srv.calls_to("PUT", "/workitems/").len(), before);
    }

    /// F11: with the cap full (2/2), a queued item whose close-gate widget a
    /// human answered "accept" is claimed and closed complete on its own
    /// thread, and carries the "closed complete by a human" row; an item the
    /// gate never held is not even read.
    #[test]
    fn human_accept_closes_while_the_cap_is_full() {
        let mut w = crate::gate::question_widget("n", 1, 1, "why", &[], "last", &crate::gate::Advice::default(), &crate::gate::Evidence::default());
        w["fields"][0]["value"] = json!("accept");
        let details = json!([{"order": 1, "key": "response", "value": "r1"}, {"order": 2, "key": "question", "valuetype": "json", "value": w}]);
        let srv = crate::client::fake::serve(move |m, path, body| {
            if m == "GET" && path.ends_with("/details") { return Some((200, details.clone())); }
            if m == "PUT" { return Some((200, body.clone())); }
            Some((200, json!({})))
        });
        let mut rt = runtime(srv.api());
        rt.running.push(running("busy-1", "L1"));
        rt.running.push(running("busy-2", "L2"));
        let accepted = WorkItem { id: "acc00001-0000".into(), project: "p".into(), agent: "code".into(), state: "queued".into(),
            gate_bounces: 1, version: 5, ..Default::default() };
        let plain = WorkItem { id: "plain0002-0000".into(), project: "p".into(), agent: "code".into(), state: "queued".into(), version: 2, ..Default::default() };
        let project = Project { name: "p".into(), ..Default::default() };
        let got = EngineRuntime::close_accepted(&rt.api, &rt.name, &mut rt.accept_checked, &mut rt.accepting, &project, "/nonexistent", &[accepted, plain]);
        assert_eq!(got, vec!["acc00001-0000".to_string()]);
        for (_, h) in rt.accepting.drain(..) {
            h.join().unwrap();
        }
        let states: Vec<String> = srv.calls_to("PUT", "/workitems/acc00001").iter().map(|b| b["state"].as_str().unwrap_or("").to_string()).collect();
        assert_eq!(states, vec!["in-progress", "complete"]);
        let rows = srv.calls_to("POST", "/workitems/acc00001-0000/details");
        assert!(rows.iter().any(|r| r["value"].as_str().unwrap_or("").contains("closed complete by a human")), "{rows:?}");
        assert!(srv.calls_to("GET", "plain0002").is_empty(), "never held by the gate: details not read");
        assert_eq!(rt.running.len(), 2, "no slot taken");
    }

    /// A run-now item waiting on a running holder is told whose run and
    /// when that run's session limit ends it (CR 4(d)).
    #[test]
    fn run_now_wait_names_the_holder_and_its_session_limit() {
        let mut h = WorkItem { id: "abcdef12-0000".into(), agent: "code".into(), ..Default::default() };
        h.ts.start = "2026-09-25T10:00:00Z".into();
        let mut agents = HashMap::new();
        agents.insert("code".to_string(), json!({"timeoutsec": 7200}));
        assert_eq!(
            run_now_wait_reason(&Project::default(), Some(&h), &h.id, &agents),
            "run now waits on running abcdef12 (started 10:00Z, session limit ends it by 12:00Z)"
        );
        assert_eq!(run_now_wait_reason(&Project::default(), None, "zzzzzzzz9", &agents), "run now waits on running zzzzzzzz");
    }

    #[test]
    fn retry_after_tag_shows_hhmm() {
        assert_eq!(hhmm("2026-09-07T14:05:31Z"), "14:05Z");
        assert_eq!(hhmm("bad"), "bad");
    }

    /// The pdy-dev ladder as configured 2026-09-04: keys are parsed as ">N%"
    /// (no hard-coded levels); the most restrictive true gate wins.
    #[test]
    fn max_agents_evaluates_every_gt_percent_key() {
        let gates: BTreeMap<String, u32> =
            [(">90%", 2), (">95%", 1), (">99%", 0), ("else", 4)].into_iter().map(|(k, v)| (k.to_string(), v)).collect();
        for (pct, want) in [(0, 4), (50, 4), (90, 4), (91, 2), (95, 2), (96, 1), (99, 1), (100, 0)] {
            assert_eq!(max_agents(&gates, pct), want, "usage {pct}%");
        }
        // no gates at all -> the default of 4; an "else"-only map -> its value
        assert_eq!(max_agents(&BTreeMap::new(), 97), 4);
        let only_else: BTreeMap<String, u32> = [("else".to_string(), 7)].into_iter().collect();
        assert_eq!(max_agents(&only_else, 97), 7);
    }
}
