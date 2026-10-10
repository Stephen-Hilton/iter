//! The tick loop (iter5: one engine per machine, spec §4).  Every loop:
//! read the engine record and its assignments (which projects, which
//! checkout, which accounts), heartbeat, run each served project's file
//! sync hook, then per project: metadata (seq-gated, periodic full refresh),
//! schedules, and — after the engine-side checks (caps, budget, account,
//! holds) — ask iter_data for the next item (`POST …/next`, claimed and
//! locked on the server) and run it.

use crate::assign::{Assignment, Assignments, core_accounts, expand_topdir};
use crate::client::Api;
use iter_core::cluster::{self, CLUSTER_RESTART_REASON, CLUSTER_RESTART_TAG};
use iter_core::{BLOCKED_TAG_COLOR, BLOCKED_TAG_PREFIX, DepStatus, Engine, LockRow, Project, WorkItem, children_index, claim_tags, dependency_status, now_utc, pick_account};
use serde_json::{Value, json};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

pub struct EngineRuntime {
    api: Api,
    name: String,
    /// the latest assignments (GET /api/engines/{name}/assignments)
    assignments: Assignments,
    /// last seq seen per (project, table)
    seen_seq: HashMap<(String, String), u64>,
    last_full_refresh: Instant,
    /// cached data; a project's `accounts` and `state` are overlaid from its
    /// assignment every tick (the settings graph owns them)
    projects: HashMap<String, Project>,
    items: HashMap<String, Vec<WorkItem>>,
    agents: HashMap<String, Value>,
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
    /// items whose triage finished this process (the cache may not show the
    /// stamp for a tick; never start a second judge on the same item)
    triaged: std::collections::HashSet<String>,
    running_count: Arc<AtomicUsize>,
    /// per served project: the file-sync service's state (crate::filesync)
    filesync: HashMap<String, crate::filesync::FileSyncState>,
    pub max_ticks: Option<u64>,
    /// test_requested value already answered (never run the same nudge twice)
    last_test_handled: String,
    /// the probe_requested stamp last answered (the webui's usage refresh)
    last_probe_request: String,
    /// "project/agent" already told it has no permission flags
    unarmed_said: HashSet<String>,
    /// account -> when this engine last probed its usage ("" = ambient login)
    last_probe: HashMap<String, Instant>,
    /// project -> date the daily-budget hold was announced
    budget_hold: HashMap<String, String>,
    /// project -> last cluster-health verdict announced (healthy, why)
    cluster_state: HashMap<String, (bool, String)>,
    /// project -> when this engine last saw the cluster turn healthy (its first
    /// verdict counts as a turn): only an item that last started before then
    /// is released by the recovery — see `released_by_recovery`
    cluster_healthy_since: HashMap<String, String>,
    /// project -> last `next` refusal logged (so a steady reason logs once)
    next_said: HashMap<String, String>,
    /// accounts are configured but none is under its stop% (set each tick):
    /// heartbeats then carry no usage snapshot — the ambient login's numbers
    /// would describe an account this engine is not running on
    holding: bool,
    /// the --env-file path — named in every "no token" error so the reader
    /// knows which file to edit
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

/// The OS a self-registering engine records: Linux's PRETTY_NAME (e.g.
/// "Debian GNU/Linux 13 (trixie)"), else the platform family.
fn detected_os() -> String {
    if let Ok(rel) = std::fs::read_to_string("/etc/os-release") {
        if let Some(v) = rel.lines().find_map(|l| l.strip_prefix("PRETTY_NAME=")) {
            return v.trim_matches('"').to_string();
        }
    }
    match std::env::consts::OS {
        "windows" => "Windows".into(),
        "macos" => "macOS".into(),
        other => other.to_string(),
    }
}

/// "2026-09-07T14:05:31Z" -> "14:05Z" for the retry-after tag
fn hhmm(iso: &str) -> String {
    if iso.len() >= 16 { format!("{}Z", &iso[11..16]) } else { iso.to_string() }
}

/// May the cluster's recovery release a parked `blocked-by-cluster-restart`
/// item?  Only when the item last started before the cluster turned healthy
/// (or never started): a run that began on a healthy cluster and still parked
/// itself on the tag was not stopped by a restart, and requeueing it would
/// loop forever (2026-10-09: pdy-dev's cluster was decommissioned, an agent
/// parked on the tag and was rerun 125 times in 45 min).  Such an item stays
/// parked for a person.  Both are "%Y-%m-%dT%H:%M:%SZ", so text order is time order.
fn released_by_recovery(item_start: &str, healthy_since: &str) -> bool {
    item_start.is_empty() || (!healthy_since.is_empty() && item_start < healthy_since)
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

/// The accounts whose token is not set in the engine's env_file right now
/// (spec: account hot reload, 2026-09-11): never picked, never probed,
/// never substituted.
fn tokenless_accounts(accounts: &[iter_core::Account]) -> Vec<String> {
    accounts.iter().filter(|a| crate::envstore::get(&a.token_envar).is_none()).map(|a| a.name.clone()).collect()
}

/// Can this agent do its work in a headless `claude -p` run? Only when its
/// effective flags (the project's override, else the agent record's) name a
/// permission mode — `--dangerously-skip-permissions`, `--permission-mode …`
/// or `--allowedTools …`; without one every write, command and read outside
/// its folder is refused. An agent meant to run read-only says so with
/// `readonly: true` on its record. An agent with no record is left to
/// iter_data (the run fails naming it).
fn agent_can_work(project: &Project, def: Option<&Value>) -> bool {
    let Some(def) = def else { return true };
    if def.get("readonly").and_then(|v| v.as_bool()).unwrap_or(false) {
        return true;
    }
    let id = iter_core::settings::record_id(def);
    let flags = project.agents.get(&id).and_then(|o| o.get("flags")).and_then(|f| f.as_str())
        .or_else(|| def.get("flags").and_then(|f| f.as_str()))
        .unwrap_or("");
    flags.split_whitespace().any(|f| {
        f == "--dangerously-skip-permissions" || f == "--permission-mode" || f.starts_with("--permission-mode=")
            || f == "--allowedTools" || f == "--allowed-tools" || f.starts_with("--allowedTools=") || f.starts_with("--allowed-tools=")
    })
}

/// The accounts the ladder may not pick: no token in the env_file, or the
/// account's switch is Stopped (2026-10-08).  Never substituted: with all of
/// them unpickable the engine holds rather than fall back to the ambient login.
fn unpickable_accounts(accounts: &[iter_core::Account], stopped: &[String]) -> Vec<String> {
    let mut v = tokenless_accounts(accounts);
    for a in accounts.iter().filter(|a| stopped.contains(&a.name)) {
        if !v.contains(&a.name) {
            v.push(a.name.clone());
        }
    }
    v
}

/// The project-wide hold reason when the ladder picks nothing (renders as the
/// "blocked by: …" tag on every queued item): every account is switched off,
/// every account is unusable (no token, or switched off), or every account is
/// at its stop%.
fn hold_reason(accounts: &[iter_core::Account], stopped: &[String], unpickable: &[String]) -> &'static str {
    if !accounts.is_empty() && accounts.iter().all(|a| stopped.contains(&a.name)) {
        "accounts switched off"
    } else if unpickable.len() == accounts.len() {
        "no account token"
    } else {
        "accounts at stop%"
    }
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
            assignments: Assignments::default(),
            seen_seq: HashMap::new(),
            last_full_refresh: Instant::now() - Duration::from_secs(86400 * 365),
            projects: HashMap::new(),
            items: HashMap::new(),
            agents: HashMap::new(),
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
            triaged: std::collections::HashSet::new(),
            running_count: Arc::new(AtomicUsize::new(0)),
            filesync: HashMap::new(),
            max_ticks: None,
            last_test_handled: String::new(),
            last_probe_request: String::new(),
            unarmed_said: HashSet::new(),
            holding: false,
            last_probe: HashMap::new(),
            budget_hold: HashMap::new(),
            cluster_state: HashMap::new(),
            cluster_healthy_since: HashMap::new(),
            next_said: HashMap::new(),
        }
    }

    /// The engine record, self-registering it when iter_data has none
    /// (spec §4.1: `PUT /api/engines/{name}` if missing).  None = not
    /// available this round (the caller waits and retries).
    fn load_engine(&self) -> Option<Engine> {
        match self.api.get(&format!("/api/engines/{}", self.name)) {
            Ok(v) => match serde_json::from_value::<Engine>(v) {
                Ok(e) => Some(e),
                Err(e) => {
                    eprintln!("[engine] bad engine record: {e}");
                    None
                }
            },
            Err(e) if e.status == 404 => {
                let host = std::process::Command::new("hostname").output().ok()
                    .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string()).unwrap_or_default();
                let row = json!({"name": self.name, "host": host, "state": "Stopped", "last_seen": "",
                    "ticksec": 5, "full_refresh_minutes": 360, "account": "",
                    "queuelock": {"retryms": 50, "breaksec": 60},
                    // a starting point a human refines on the settings node
                    "operating_system": detected_os(), "os_agent_instructions": ""});
                match self.api.put(&format!("/api/engines/{}", self.name), &row) {
                    Ok(_) => println!("[engine] registered '{}' with iter_data — connect it to projects in the settings graph (serves edges)", self.name),
                    Err(e2) => eprintln!("[engine] cannot self-register '{}': {e2}", self.name),
                }
                None
            }
            Err(e) => {
                eprintln!("[engine] cannot load engine '{}' from iter_data ({e})", self.name);
                None
            }
        }
    }

    /// Refresh `self.assignments`; on failure the last good set stands.
    fn load_assignments(&mut self) {
        match self
            .api
            .get(&format!("/api/engines/{}/assignments", self.name))
            .map_err(|e| e.to_string())
            .and_then(|v| Assignments::parse(&v))
        {
            Ok(a) => {
                let before: Vec<&str> = self.assignments.projects.iter().map(|p| p.project.as_str()).collect();
                let after: Vec<&str> = a.projects.iter().map(|p| p.project.as_str()).collect();
                if before != after {
                    println!("[engine] serving {} project(s): {}", after.len(), if after.is_empty() { "none".into() } else { after.join(", ") });
                }
                let changed = a != self.assignments;
                self.assignments = a;
                if changed {
                    // a new or renamed account may name a variable the env
                    // file already holds: re-read it against the new set
                    self.reload_env(true);
                }
            }
            Err(e) => eprintln!("[engine] cannot read the assignments of '{}' ({e}) — keeping the last ones", self.name),
        }
    }

    pub fn run(&mut self) {
        let mut ticks: u64 = 0;
        loop {
            ticks += 1;
            let Some(engine) = self.load_engine() else {
                std::thread::sleep(Duration::from_secs(if ticks == 1 { 2 } else { 5 }));
                continue;
            };
            // started with the engine's display name (the server resolves it):
            // from here on the engine goes by its stable id, which is what its
            // claims, locks and edges carry
            if !engine.id.is_empty() && engine.id != self.name {
                println!("[engine] '{}' is the display name of engine id '{}' — using the id", self.name, engine.id);
                self.name = engine.id.clone();
            }
            crate::prompt::set_engine_env(crate::prompt::EngineEnv {
                name: if engine.name.is_empty() { self.name.clone() } else { engine.name.clone() },
                os: engine.operating_system.clone(),
                os_instructions: engine.os_agent_instructions.clone(),
            });
            self.load_assignments();
            self.tick(&engine);

            if let Some(max) = self.max_ticks {
                if ticks >= max {
                    println!("[engine] max ticks reached, draining running work");
                    self.drain();
                    return;
                }
            }
            std::thread::sleep(Duration::from_secs(engine.ticksec.max(1)));
        }
    }

    /// Wait for every worker thread of this process to finish.
    fn drain(&mut self) {
        while self.prune_running() > 0
            || { self.explaining.retain(|(_, h)| !h.is_finished()); !self.explaining.is_empty() }
            || {
                // a finished triage is remembered, exactly as dispatch does
                let triaged = &mut self.triaged;
                self.triaging.retain(|(id, h)| if h.is_finished() { triaged.insert(id.clone()); false } else { true });
                !self.triaging.is_empty()
            }
            || { self.accepting.retain(|(_, h)| !h.is_finished()); !self.accepting.is_empty() }
            || { self.summarizing.retain(|(_, h)| !h.is_finished()); !self.summarizing.is_empty() }
        {
            std::thread::sleep(Duration::from_millis(100));
        }
    }

    /// Every account any served project may bill, once each, by order.
    fn assigned_accounts(&self) -> Vec<crate::assign::AssignedAccount> {
        let mut v: Vec<crate::assign::AssignedAccount> = Vec::new();
        for a in &self.assignments.projects {
            for acct in self.assignments.accounts_of(a) {
                if !v.iter().any(|x| x.name == acct.name) {
                    v.push(acct);
                }
            }
        }
        v.sort_by_key(|a| a.order);
        v
    }

    /// The served project's accounts whose switch is Stopped.
    fn stopped_accounts(&self, project: &str) -> Vec<String> {
        self.assignments.get(project).map(|a| a.accounts.iter().filter(|x| x.stopped).map(|x| x.name.clone()).collect()).unwrap_or_default()
    }

    /// The served project's checkout (expanded), if it is served.
    fn topdir_of(&self, project: &str) -> Option<String> {
        self.assignments.get(project).map(|a| expand_topdir(&a.topdir)).filter(|t| !t.is_empty())
    }

    /// The connectivity nudge on the active account; the result and the
    /// refreshed usage snapshot go back on the engine record.
    fn run_test(&mut self, engine: &Engine, chosen: &str) {
        // which account to test: the engine's chosen one; while holding (no
        // account pickable) the chosen one is "" but accounts ARE configured,
        // and the ambient login must not stand in for them — test the first
        // account by order that has a token, else fail naming the first
        // account without one.  "" is the ambient login only when no served
        // project names any account.
        let configured = self.assigned_accounts();
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
        // token: the one resolution rule (work::resolve_account_token) over the
        // assigned accounts; a missing token fails the test outright
        let holder = Project { accounts: core_accounts(&configured), ..Default::default() };
        let token = crate::work::resolve_account_token(&holder, account, &self.env_file);
        let envar = crate::work::account_envar(&holder, account).unwrap_or_else(|| "no token_envar configured".into());
        let cwd = self
            .assignments
            .projects
            .iter()
            .map(|a| expand_topdir(&a.topdir))
            .find(|t| !t.is_empty() && std::path::Path::new(t).is_dir())
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

    /// One usage refresh per stale account (every assigned account, not just
    /// the chosen one — the ladder needs every account's number to switch),
    /// asked of the account's provider with no dispatch (`get_usage(…, None)`:
    /// claude probes, mock reads ITER_MOCK_USAGE).  No accounts configured =
    /// the ambient login, which only the haiku nudge can reach.
    fn probe_stale_accounts(&mut self, engine: &Engine, now: chrono::DateTime<chrono::Utc>) {
        self.probe_accounts(engine, now, false);
    }

    /// probe_stale_accounts, or with `force` every account now (the webui's
    /// usage refresh, `probe_requested`), stale or not.
    fn probe_accounts(&mut self, engine: &Engine, now: chrono::DateTime<chrono::Utc>, force: bool) {
        let stale_sec = (engine.probe_stale_min * 60) as i64;
        let accounts = self.assigned_accounts();
        let due = |this: &Self, name: &str| -> bool {
            let age = usage::read_usage(name).and_then(|u| u.age_sec(now)).unwrap_or(i64::MAX);
            let since = this.last_probe.get(name).map(|t| t.elapsed().as_secs() as i64).unwrap_or(i64::MAX);
            force || (age > stale_sec && since > stale_sec)
        };
        if accounts.is_empty() {
            if due(self, "") {
                self.last_probe.insert(String::new(), Instant::now());
                println!("[engine] default usage snapshot stale — nudging the ambient login");
                self.run_test(engine, "");
            }
            return;
        }
        for a in accounts {
            if !due(self, &a.name) {
                continue;
            }
            self.last_probe.insert(a.name.clone(), Instant::now());
            // an account with no token cannot be probed: say so, once per
            // stale period, instead of silently never reporting its usage
            let Some(tok) = crate::envstore::get(&a.token_envar) else {
                println!("[engine] usage probe '{}' skipped: {} not set", a.name, a.token_envar);
                continue;
            };
            match crate::provider::record_usage(&a.provider, &a.name, Some(&tok), None) {
                Some(u) => println!(
                    "[engine] usage probe '{}' ({}): 5h {:.0}% 7d {:.0}% ({}{})",
                    a.name, a.provider, u.five_hour_pct, u.seven_day_pct, u.status, if u.is_using_overage { ", OVERAGE" } else { "" }
                ),
                None => eprintln!("[engine] usage probe '{}' ({}) returned nothing", a.name, a.provider),
            }
        }
    }

    /// Re-read the env_file when it changed (or `force`), refreshing every
    /// `*_TOKEN` key, each `token_envar` the assignments name, and the mock
    /// provider's usage keys; one log line per reload that changed
    /// something, key names only.
    fn reload_env(&self, force: bool) {
        let mut declared: HashSet<String> = HashSet::new();
        for a in self.assigned_accounts() {
            declared.insert(a.token_envar.clone());
            declared.extend(crate::provider::mock::mock_usage_keys(&a.name));
        }
        declared.insert("ITER_MOCK_USAGE".into());
        if let Some(ch) = crate::envstore::reload_if_changed(&declared, force) {
            println!("[engine] {}", ch.log_line());
        }
    }

    /// One read-only `explain` session per item flagged `explain_requested`
    /// that this engine is not already explaining.
    fn start_explains(&mut self, project: &Project, topdir: &str, account: &str) {
        self.explaining.retain(|(_, h)| !h.is_finished());
        let wanted: Vec<WorkItem> = self
            .items
            .get(project.key())
            .map(|v| v.iter().filter(|i| !i.explain_requested.is_empty() && (i.explain_engine.is_empty() || i.explain_engine == self.name)).cloned().collect())
            .unwrap_or_default();
        for item in wanted {
            if self.explaining.iter().any(|(id, _)| id == &item.id) {
                continue;
            }
            // one engine per ELI5: iter_data assigned one at random when the
            // button was pressed; an unassigned one goes to whoever claims first
            if let Err(e) = self.api.post(
                &format!("/api/projects/{}/workitems/{}/explain/claim", project.key(), item.id),
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
            let topdir = topdir.to_string();
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
    fn start_summaries(&mut self, reply: &Value, account: &str) {
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
                let Some(topdir) = self.topdir_of(&project_name) else { continue };
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
        self.running.retain(|r| !r.handle.is_finished());
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


    /// The file-sync hooks for one served project (crate::filesync, owned
    /// by ENG-FILES): the designer build when the heartbeat reply's
    /// `build_waiting` names it (never on a read-only checkout), then the
    /// per-tick service with `files_waiting` from the same reply.
    fn run_filesync(&mut self, a: &Assignment, reply: &Value) {
        let topdir = std::path::PathBuf::from(expand_topdir(&a.topdir));
        if !a.read_only && crate::assign::reply_names(reply, "build_waiting", &a.project) {
            if let Err(e) = crate::filesync::build(&self.api, &self.name, &a.project, &topdir) {
                eprintln!("[engine] {}: build failed: {e}", a.project);
            }
        }
        // GraphRAG document rows (store_doc / remove_doc / gitignore_path)
        if !a.read_only && crate::datasync::waiting_projects(reply).iter().any(|p| p == &a.project) {
            let n = crate::datasync::apply_waiting(&self.api, &self.name, &a.project, &topdir);
            if n > 0 {
                println!("[engine] {}: {n} document edit(s) applied", a.project);
            }
        }
        let waiting = crate::assign::reply_names(reply, "files_waiting", &a.project);
        let st = self.filesync.entry(a.project.clone()).or_default();
        if let Err(e) = crate::filesync::tick(&self.api, &self.name, &a.project, &topdir, a.read_only, waiting, st) {
            eprintln!("[engine] {}: file sync: {e}", a.project);
        }
    }

    fn tick(&mut self, engine: &Engine) {
        self.prune_running();
        self.renew_leases();
        // the env_file may have changed since the last tick: an account token
        // added, rotated or removed takes effect here, without a restart
        self.reload_env(false);
        let served: Vec<Assignment> = self.assignments.projects.clone();
        // which provider (and bills-edge model) each account dispatches to
        for a in &served {
            let accts: Vec<(String, String, String)> =
                self.assignments.accounts_of(a).into_iter().map(|x| (x.name, x.provider, x.model)).collect();
            crate::provider::register_accounts(&a.project, &accts);
        }

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
                iter_core::settings::record_id(e) != self.name
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

        // the account the engine record shows: the first served project's
        // pick (each project still picks its own in dispatch)
        let now = chrono::Utc::now();
        let mut chosen_account = String::new();
        for a in &served {
            let accts = core_accounts(&self.assignments.accounts_of(a));
            let map = usage::usage_map(&accts, now);
            let unpickable = unpickable_accounts(&accts, &self.stopped_accounts(&a.project));
            if let Some(acct) = pick_account(&accts, &map, &in_use, &unpickable, &usage::resets7d_map(&accts, now)) {
                chosen_account = acct.name.clone();
                break;
            }
        }

        // heartbeat: actual state + account + the account's usage snapshot,
        // every tick.  hold: accounts are configured but none is under its
        // stop% — usage goes up as null and the webui shows "Suspended, no
        // usage left"; accounts/next: every account's windows + reset times
        // and the one that comes back first.
        let assigned = self.assigned_accounts();
        let all_stopped: Vec<String> = assigned.iter().filter(|a| a.stopped).map(|a| a.name.clone()).collect();
        let all_accounts = core_accounts(&assigned);
        self.holding = chosen_account.is_empty() && !all_accounts.is_empty();
        let all_off = !all_accounts.is_empty() && all_stopped.len() == all_accounts.len();
        let all_unusable = !all_accounts.is_empty() && unpickable_accounts(&all_accounts, &all_stopped).len() == all_accounts.len();
        let accounts = usage::accounts_json(&all_accounts, &in_use, now);
        let next = usage::next_json(&accounts);
        let heartbeat = self.api.post(
            &format!("/api/engines/{}/heartbeat", self.name),
            &json!({"state": "Running", "account": chosen_account,
                    "projects": served.iter().map(|a| a.project.clone()).collect::<Vec<_>>(),
                    // the threads behind this engine's in-progress records: the
                    // webui warns when the store counts more (a ghost, 2026-09-22)
                    "running": self.running.len(), "running_by_project": self.running_by_project(),
                    "hold": if !self.holding { "" } else if all_off { "accounts switched off" } else if all_unusable { "no account token" } else { "all accounts at stop%" },
                    "usage": if self.holding { Value::Null } else { usage::snapshot_json(&chosen_account, now).unwrap_or(Value::Null) },
                    "accounts": accounts, "next": next}),
        );
        let reply = heartbeat.as_ref().ok().cloned().unwrap_or(Value::Null);

        // node-file sync and the designer build (crate::filesync, ENG-FILES):
        // every served project, every tick, before any agent work
        for a in &served {
            self.run_filesync(a, &reply);
        }

        // GraphRAG (2026-09-29): chunk / document summaries waiting — Summary
        // agent workers per project, on their own threads, billed to the
        // chosen account; a holding engine writes none
        self.start_summaries(&reply, &chosen_account);

        // stop requests for items THIS engine is running (workitem_stop.md)
        for items in self.items.values() {
            for i in items.iter().filter(|i| i.stop_requested && i.state == "in-progress" && i.engine == self.name) {
                if let Ok(mut v) = crate::work::STOP_REQUESTED.lock() {
                    if !v.contains(&i.id) {
                        println!("[engine] stop requested for {} '{}' — killing its session", &i.id[..8.min(i.id.len())], i.name);
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

        for a in &served {
            let project_name = &a.project;
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
                if changed || full_refresh || !rt.projects.contains_key(project_name) {
                    rt.seen_seq.insert(key, now_seq);
                    true
                } else {
                    false
                }
            };

            if reload(self, "project") {
                if let Ok(v) = self.api.get(&format!("/api/projects/{project_name}")) {
                    if let Ok(mut p) = serde_json::from_value::<Project>(v) {
                        // the stable id is the key everywhere below (a server older than ids sends none)
                        if p.id.is_empty() {
                            p.id = project_name.clone();
                        }
                        self.projects.insert(project_name.clone(), p);
                    }
                }
            }
            if reload(self, "agent") {
                if let Ok(v) = self.api.get("/api/agents") {
                    for ag in v.as_array().cloned().unwrap_or_default() {
                        // keyed by the agent's stable id (what a work item's `agent` names)
                        let n = iter_core::settings::record_id(&ag);
                        if !n.is_empty() {
                            self.agents.insert(n.to_string(), ag.clone());
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

            // the settings graph owns accounts and run state: overlay them
            // from the assignment onto the cached project record
            let accts = core_accounts(&self.assignments.accounts_of(a));
            let Some(project) = self.projects.get_mut(project_name).map(|p| {
                p.accounts = accts;
                if !a.state.trim().is_empty() {
                    p.state = a.state.clone();
                }
                p.clone()
            }) else {
                continue;
            };
            let topdir = expand_topdir(&a.topdir);
            let has_checkout = !topdir.is_empty() && std::path::Path::new(&topdir).is_dir();
            if !a.read_only {
                self.ensure_test_sweep(project_name);
            }
            // a close that could not reach iter_data is replayed first, then
            // this engine's own ghosts are repaired — whatever the project
            // state: a ghost holds a cap slot and blocks a drain (2026-09-22)
            if has_checkout {
                crate::work::replay_pending_closes(&self.api, &topdir);
                self.repair_ghosts(&project, &topdir);
                // ELI5 requests run at once whatever the project state or cap
                // says: a human pressed the button, the run is read-only
                self.start_explains(&project, &topdir, &chosen_account);
            }
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
                if !a.stopped_by.is_empty() {
                    let said = format!("the {} switch is Stopped", a.stopped_by);
                    if self.next_said.get(project_name) != Some(&said) {
                        println!("[engine] {project_name}: {said} — starting nothing new");
                        self.next_said.insert(project_name.clone(), said);
                    }
                }
                continue;
            }
            // idle usage refresh, BEFORE picking: with nothing running, every
            // account whose snapshot is older than probe_stale_min is asked
            // of its provider so the ladder and the maxagents gates see real numbers
            if engine.probe_stale_min > 0 && self.running.is_empty() {
                self.probe_stale_accounts(engine, now);
            }
            // read-only checkout: no schedules fire and no work item runs here
            if a.read_only {
                continue;
            }
            // no checkout yet (a designed project before its build): nothing to run in
            if !has_checkout {
                let said = format!("no checkout at '{topdir}'");
                if self.next_said.get(project_name) != Some(&said) {
                    println!("[engine] {project_name}: {said} — not dispatching until it exists (build it from the designer)");
                    self.next_said.insert(project_name.clone(), said);
                }
                continue;
            }
            self.fire_schedules(&project);
            self.dispatch(&project, &topdir, &in_use);
        }

        // connectivity test requested from the webui: one nudge, then report
        // the outcome (and the refreshed usage) via heartbeat
        // usage refresh requested from the webui: every account's 5h / 7d now,
        // then the fresh report goes up at once with clear_probe
        if !engine.probe_requested.is_empty() && engine.probe_requested != self.last_probe_request {
            self.last_probe_request = engine.probe_requested.clone();
            println!("[engine] usage refresh requested at {}", engine.probe_requested);
            let now = chrono::Utc::now();
            self.probe_accounts(engine, now, true);
            let accounts = usage::accounts_json(&core_accounts(&self.assigned_accounts()), &in_use, now);
            let next = usage::next_json(&accounts);
            let _ = self.api.post(&format!("/api/engines/{}/heartbeat", self.name), &json!({"clear_probe": true, "accounts": accounts, "next": next}));
        }
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
        let items = self.items.get(project.key()).cloned().unwrap_or_default();
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
                        project.key(), tpl.id, tpl.version
                    ),
                    &claimed,
                )
                .is_err()
            {
                continue; // lost the race (or transient) — next check re-evaluates
            }
            let clone = iter_core::sched::clone_from(tpl);
            match self.api.post(
                &format!("/api/projects/{}/workitems", project.key()),
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


    /// One project's dispatch.  The engine decides WHETHER to ask for work
    /// (account ladder, usage hold, daily budget, maxagents cap, per-agent
    /// caps, dedup triage) and iter_data decides WHAT runs (§4.3: `POST
    /// …/next` picks by priority honouring dependencies, cluster-restart tags,
    /// waits and lock availability, claims the item and takes its locks).
    /// The engine still derives every queued item's visible wait reason
    /// (`blocked by: …` tags, lock holders) from its cached view.
    fn dispatch(&mut self, project: &Project, topdir: &str, in_use: &[String]) {
        let project_name = project.key().to_string();
        let items = self.items.get(&project_name).cloned().unwrap_or_default();
        let by_id: HashMap<String, &WorkItem> = items.iter().map(|i| (i.id.clone(), i)).collect();

        // cluster-restart block (iter_core::cluster): one health verdict per
        // tick, taken only when some open item carries the durable tag.  A
        // tagged PARKED item (what `iter block` leaves) is requeued — tag kept
        // — the moment the cluster is back up; iter_data's next skips tagged
        // queued items and the claim strips the tag.
        let is_cluster_tagged = |i: &WorkItem| cluster::has_tag(&i.tags, CLUSTER_RESTART_TAG);
        let cluster_tagged_open = items.iter().any(|i| (i.state == "queued" || i.state == "parked") && is_cluster_tagged(i));
        let cluster_healthy = if cluster_tagged_open { self.cluster_health(project, &items) } else { true };

        // a project-wide hold: nothing is asked for this tick, and every item
        // that would otherwise be dispatchable is told why
        let mut hold: Option<String> = None;

        // real usage drives both the account ladder and the maxagents gates
        let now = chrono::Utc::now();
        let usage_pct: u8;
        let account: Option<iter_core::Account>;
        if project.accounts.is_empty() {
            // no bills edge: the ambient login's default snapshot
            usage_pct = usage::effective_pct_for("", now);
            account = None;
        } else {
            let map = usage::usage_map(&project.accounts, now);
            // an account with no token in the env file is never picked — the
            // item is not claimed, so it can never be billed to another
            // account's token (2026-09-11)
            let stopped = self.stopped_accounts(&project_name);
            let unpickable = unpickable_accounts(&project.accounts, &stopped);
            match pick_account(&project.accounts, &map, in_use, &unpickable, &usage::resets7d_map(&project.accounts, now)) {
                Some(a) => {
                    usage_pct = map.get(&a.name).copied().unwrap_or(0);
                    account = Some(a.clone());
                }
                None => {
                    let reason = hold_reason(&project.accounts, &stopped, &unpickable);
                    if self.next_said.get(&project_name).map(String::as_str) != Some(reason) {
                        match reason {
                            "accounts switched off" => println!("[engine] {project_name}: every account billing it is switched off — holding"),
                            "no account token" => println!("[engine] {project_name}: no usable account — {} configured, none has a token or is switched on", project.accounts.len()),
                            _ => println!("[engine] {project_name}: all accounts at stop% — holding until a usage window resets"),
                        }
                        self.next_said.insert(project_name.clone(), reason.to_string());
                    }
                    usage_pct = 100;
                    account = None;
                    hold = Some(reason.into());
                }
            }
        }
        if let Some(u) = account.as_ref().and_then(|a| usage::read_usage(&a.name)) {
            if let Some(age) = u.age_sec(now) {
                if age > usage::SNAPSHOT_STALE_WARN_SEC && !self.running.is_empty() {
                    eprintln!("[engine] warning: usage snapshot for '{}' is {age}s old", account.as_ref().map(|a| a.name.as_str()).unwrap_or("default"));
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

        // lease-bound locks: once a minute the sweep removes rows whose holder
        // is not running under the row's lease
        self.sweep_locks(project);

        // current central lock rows — for the visible wait reasons and the
        // deadlock detector only (iter_data's next does the real lock check)
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

        // the cluster is back: requeue every parked tagged item that is NOT
        // still in flight (running here, or holding a live lock row)
        if cluster_healthy {
            let in_flight = |i: &WorkItem| self.running.iter().any(|r| r.current().0 == i.id) || live.iter().any(|r| r.workid == i.id);
            let since = self.cluster_healthy_since.get(&project_name).cloned().unwrap_or_default();
            for i in items.iter().filter(|i| i.state == "parked" && is_cluster_tagged(i) && !in_flight(i) && released_by_recovery(&i.ts.start, &since)) {
                self.requeue_after_cluster_restart(project, i);
            }
        }

        let kids = children_index(&items);
        let deps_satisfied = |item: &WorkItem| -> bool { dependency_status(item, &by_id, &kids) == DepStatus::Satisfied };
        let lock_holders = |item: &WorkItem| -> Vec<(String, String)> { iter_core::lock_holders(item, &live) };

        // classify every queued item once: approval / backoff / cluster /
        // dependency / lock / hold — the reason its "blocked by: …" tag shows
        let mut waits: HashMap<String, Wait> = HashMap::new();
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
        // hold (a shell run spends no model time).  Claimed directly by the
        // engine (a versioned PUT), not through next: next would count it
        // against nothing and could hand back another item instead.
        let mut started: HashSet<String> = HashSet::new();
        for item in items.iter().filter(|i| i.state == "queued" && i.is_test_sweep_run() && !i.needs_approval && deps_satisfied(i)) {
            println!("[engine] test sweep run {} starts on its timer, outside the cap", &item.id[..8.min(item.id.len())]);
            if let Some(claimed) = self.claim_direct(project, item) {
                started.insert(item.id.clone());
                self.start_claimed(project, topdir, claimed, &account_name);
            }
        }
        let in_triage = |i: &WorkItem| self.triaging.iter().any(|(id, _)| id == &i.id);
        for i in items.iter().filter(|i| i.state == "queued" && !started.contains(&i.id)) {
            let mut w = Wait::default();
            if in_triage(i) {
                w.reason = Some(DEDUP_TRIAGE_REASON.into());
            } else if i.needs_approval {
                w.reason = Some("needs approval".into());
            } else if !i.retry_after.is_empty() && i.retry_after > now_iso {
                w.reason = Some(format!("retry after {}", hhmm(&i.retry_after)));
            } else if cluster::blocks(i, cluster_healthy) {
                w.reason = Some(CLUSTER_RESTART_REASON.into());
            } else {
                match dependency_status(i, &by_id, &kids) {
                    DepStatus::Satisfied => {
                        w.holders = lock_holders(i);
                        if let Some((holder, path)) = w.holders.first() {
                            w.reason = Some(if i.run_now {
                                run_now_wait_reason(project, by_id.get(holder).copied(), holder, &self.agents)
                            } else {
                                format!("lock {path}")
                            });
                        } else if let Some(h) = &hold {
                            w.reason = Some(h.clone());
                        }
                    }
                    other => w.reason = other.reason(),
                }
            }
            waits.insert(i.id.clone(), w);
        }
        // the wait-for graph: every loop through declared, deep and lock
        // edges, tagged on each member and noted once
        Self::detect_deadlocks(&self.api, &self.name, &mut self.announced_cycles, project, &items, &by_id, &live, &mut waits);
        // a human already accepted it at the close gate: closing uses no model
        // time, so it waits neither for a cap slot nor out a usage hold
        for id in Self::close_accepted(&self.api, &self.name, &mut self.accept_checked, &mut self.accepting, project, topdir, &items) {
            waits.remove(&id);
            started.insert(id);
        }
        for id in &started {
            waits.remove(id);
        }
        let reconcile_items: Vec<WorkItem> = items.iter().filter(|i| !started.contains(&i.id)).cloned().collect();
        if hold.is_some() {
            self.reconcile_waits(project, &reconcile_items, &waits);
            return;
        }

        // stage-2 dedup triage (crate::dedup): every newly created queued item
        // is judged against its open neighbours BEFORE its first dispatch, on
        // its own thread.  iter_data's next cannot see the triage, so while
        // any judge of this project runs the engine asks for no work here
        // (one judge is seconds to a few minutes; a failed judge stamps the
        // item and dispatch resumes).
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
                let judge = crate::dedup::ModelJudge {
                    api: &api, project: &project_c, topdir: &topdir_c, account: &account, agent_def: &agent_def, workid: &item.id,
                };
                crate::dedup::triage(&api, &project_c, &snapshot, &item, &judge);
            });
            self.triaging.push((i.id.clone(), handle));
            if let Some(w) = waits.get_mut(&i.id) {
                w.reason.get_or_insert_with(|| DEDUP_TRIAGE_REASON.into());
            }
        }
        let triage_pending = items
            .iter()
            .any(|i| self.triaging.iter().any(|(id, _)| id == &i.id) || (crate::dedup::needs_triage(i) && !self.triaged.contains(&i.id)));
        if triage_pending {
            self.reconcile_waits(project, &reconcile_items, &waits);
            return;
        }

        let set_reason = |waits: &mut HashMap<String, Wait>, id: &str, r: String| {
            if let Some(w) = waits.get_mut(id) {
                if w.reason.is_none() {
                    w.reason = Some(r);
                }
            }
        };
        // what the cache says could start: the candidates for a wait reason
        let queued: Vec<&WorkItem> = items
            .iter()
            .filter(|i| i.state == "queued" && !i.needs_approval && !started.contains(&i.id))
            .filter(|i| i.retry_after.is_empty() || i.retry_after <= now_iso)
            .filter(|i| !cluster::blocks(i, cluster_healthy))
            .filter(|i| deps_satisfied(i))
            .collect();

        // Run Now (operator override): with the cap full, one `next` call is
        // still made when a dependency-free run-now item is queued (iter_data
        // is asked for run-now items only via the `run_now` hint).
        let running_now = self.capped_running();
        let run_now_waiting = queued.iter().any(|i| i.run_now && lock_holders(i).is_empty());
        let mut slots = cap.saturating_sub(running_now);
        let mut run_now_only = false;
        if slots == 0 && run_now_waiting {
            println!("[engine] {project_name}: run-now override — asking for one run-now item (running {running_now} / cap {cap})");
            slots = 1;
            run_now_only = true;
        }
        if slots == 0 {
            for i in &queued {
                set_reason(&mut waits, &i.id, format!("usage cap ({running_now}/{cap})"));
            }
            self.reconcile_waits(project, &reconcile_items, &waits);
            return;
        }

        // per-agent-type caps (project override "max", else the agent record's,
        // default 4): an agent at its cap is left out of `agents_allowed`.
        // Shell runs (`exec`) take no model time and are never capped.
        let type_max = |agent: &str| -> usize {
            project
                .agents
                .get(agent)
                .and_then(|o| o.get("max"))
                .and_then(|m| m.as_u64())
                .or_else(|| self.agents.get(agent).and_then(|a| a.get("max")).and_then(|m| m.as_u64()))
                .unwrap_or(4) as usize
        };
        let mut agents_seen: Vec<String> = queued.iter().map(|i| i.agent.clone()).collect();
        agents_seen.extend(self.agents.keys().cloned());
        agents_seen.sort();
        agents_seen.dedup();
        let capped: Vec<String> = agents_seen
            .iter()
            .filter(|a| a.as_str() != "exec")
            .filter(|a| self.running.iter().filter(|r| &r.agent == *a && !r.outside_cap).count() >= type_max(a))
            .cloned()
            .collect();
        for i in queued.iter().filter(|i| capped.contains(&i.agent) && !i.is_shell()) {
            let n = self.running.iter().filter(|r| r.agent == i.agent && !r.outside_cap).count();
            set_reason(&mut waits, &i.id, format!("agent cap ({} {n}/{})", i.agent, type_max(&i.agent)));
        }
        // an agent whose flags grant claude no permission mode cannot write,
        // run a command or read outside its folder in a headless run: it is
        // never started (2026-10-08 — every iter5 agent had empty flags, and
        // each attempt spent ~$1 to end as a "need write permission" question)
        let unarmed: Vec<String> = agents_seen
            .iter()
            .filter(|a| a.as_str() != "exec" && !capped.contains(a))
            .filter(|a| !agent_can_work(project, self.agents.get(a.as_str())))
            .cloned()
            .collect();
        for a in &unarmed {
            if queued.iter().any(|i| &i.agent == a && !i.is_shell()) && self.unarmed_said.insert(format!("{project_name}/{a}")) {
                println!("[engine] {project_name}: agent '{a}' has no permission flags (e.g. --dangerously-skip-permissions) — its items wait until it has");
            }
        }
        for i in queued.iter().filter(|i| unarmed.contains(&i.agent) && !i.is_shell()) {
            set_reason(&mut waits, &i.id, format!("agent '{}' has no permission flags", i.agent));
        }
        let excluded: Vec<&String> = capped.iter().chain(unarmed.iter()).collect();
        let agents_allowed: Option<Vec<String>> =
            if excluded.is_empty() { None } else { Some(agents_seen.iter().filter(|a| !excluded.contains(a)).cloned().collect()) };
        if agents_allowed.as_ref().map(|v| v.is_empty()).unwrap_or(false) {
            self.reconcile_waits(project, &reconcile_items, &waits);
            return;
        }

        while slots > 0 {
            match self.get_next(&project_name, agents_allowed.as_deref(), run_now_only) {
                Ok(Some(item)) => {
                    self.next_said.remove(&project_name);
                    waits.remove(&item.id);
                    started.insert(item.id.clone());
                    self.start_claimed(project, topdir, item, &account_name);
                    slots -= 1;
                }
                Ok(None) => break,
                Err(e) => {
                    if self.next_said.get(&project_name) != Some(&e) {
                        eprintln!("[engine] {project_name}: next failed: {e}");
                        self.next_said.insert(project_name.clone(), e);
                    }
                    break;
                }
            }
        }
        if slots == 0 {
            let n = self.capped_running();
            for i in queued.iter().filter(|i| !started.contains(&i.id)) {
                set_reason(&mut waits, &i.id, format!("usage cap ({n}/{cap})"));
            }
        }
        let reconcile_items: Vec<WorkItem> = items.iter().filter(|i| !started.contains(&i.id)).cloned().collect();
        self.reconcile_waits(project, &reconcile_items, &waits);
    }

    /// `POST /api/projects/{p}/next` (spec §4.3): the server picks, claims
    /// (in-progress, this engine, attempt+1, lease, start ts) and locks the
    /// next item, or says why not.  `Ok(None)` = nothing for this engine now.
    fn get_next(&self, project: &str, agents_allowed: Option<&[String]>, run_now_only: bool) -> Result<Option<WorkItem>, String> {
        let mut body = json!({"engine": self.name, "lease_ttl_sec": iter_core::LOCK_LEASE_TTL_SEC, "max": 1});
        if let Some(a) = agents_allowed {
            body["agents_allowed"] = json!(a);
        }
        if run_now_only {
            body["run_now"] = json!(true);
        }
        let reply = self.api.post(&format!("/api/projects/{project}/next"), &body).map_err(|e| e.to_string())?;
        let raw = match reply.get("item") {
            Some(v) if !v.is_null() => v.clone(),
            _ => match reply.get("items").and_then(|a| a.as_array()).and_then(|a| a.first()) {
                Some(v) => v.clone(),
                None => return Ok(None),
            },
        };
        let item: WorkItem = serde_json::from_value(raw).map_err(|e| format!("next returned an item that does not parse: {e}"))?;
        if item.engine != self.name || item.state != "in-progress" {
            // not a claim for this engine: never run it
            return Err(format!("next returned {} in state '{}' for engine '{}' — not run", &item.id[..8.min(item.id.len())], item.state, item.engine));
        }
        Ok(Some(item))
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
                &format!("/api/projects/{}/workitems/{}?expect_version={}", project.key(), item.id, item.version),
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
            .and_then(|c| self.api.get(&format!("/api/projects/{}/workitems/{}/details", project.key(), c.id)).ok())
            .and_then(|v| v.as_array().cloned())
            .unwrap_or_default();
        let h = cluster::evaluate(cfg, clone, &details, chrono::Utc::now());
        let announced = self.cluster_state.get(project.key()).map(|(ok, why)| (*ok, why.as_str()));
        if h.healthy && announced.map(|(ok, _)| ok) != Some(true) {
            self.cluster_healthy_since.insert(project.key().to_string(), now_utc());
        }
        if announced != Some((h.healthy, h.why.as_str())) {
            println!("[engine] {}: cluster {} — {}", project.key(), if h.healthy { "back up and healthy" } else { "unavailable" }, h.why);
            self.cluster_state.insert(project.key().to_string(), (h.healthy, h.why.clone()));
        }
        h.healthy
    }

    /// The cluster is back: a parked item still tagged `blocked-by-cluster-restart`
    /// goes to queued with the tag kept (the claim strips it, so the row stays
    /// truthful right up to the moment the agent starts) and a "doc" row
    /// naming the release.  A 409 means someone wrote first; next tick re-derives.
    fn requeue_after_cluster_restart(&self, project: &Project, item: &WorkItem) {
        let why = self.cluster_state.get(project.key()).map(|(_, w)| w.clone()).unwrap_or_default();
        let mut updated = serde_json::to_value(item).unwrap();
        updated["state"] = json!("queued");
        updated["retry_after"] = json!("");
        match self.api.put(
            &format!("/api/projects/{}/workitems/{}?expect_version={}", project.key(), item.id, item.version),
            &updated,
        ) {
            Ok(_) => {
                println!("[engine] {} '{}': cluster back up and healthy — requeued (parked -> queued, tag kept until it starts)", &item.id[..8.min(item.id.len())], item.name);
                let _ = self.api.post(
                    &format!("/api/projects/{}/workitems/{}/details", project.key(), item.id),
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
                .put(&format!("/api/projects/{}/workitems/{}?expect_version={}", project.key(), i.id, i.version), &claimed)
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
            .get(project.key())
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
        let due = self.last_sweep.get(project.key()).map(|t| t.elapsed() >= Duration::from_secs(iter_core::LOCK_RENEW_EVERY_SEC)).unwrap_or(true);
        if !due {
            return;
        }
        self.last_sweep.insert(project.key().to_string(), Instant::now());
        let Ok(v) = self.api.post(&format!("/api/projects/{}/locks/sweep", project.key()), &json!({"dry_run": false})) else { return };
        for r in v.get("removed").and_then(|x| x.as_array()).cloned().unwrap_or_default() {
            let s = |k: &str| r.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string();
            let (path, workid) = (s("path"), s("workid"));
            println!(
                "[engine] {}: lock sweep removed {} {path} held by {} ({})",
                project.key(), s("kind"), &workid[..8.min(workid.len())], s("why")
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
                        .post(&format!("/api/projects/{}/locks/release_all", project.key()), &json!({"workid": h}))
                        .ok()
                        .and_then(|v| v.get("released").and_then(|r| r.as_array()).map(|a| a.len()))
                        .unwrap_or(0);
                    println!("[engine] {}: released {released} lock rows of {} (not running) to break {text}", project.key(), &h[..8.min(h.len())]);
                    let note = format!(
                        "deadlock resolved by engine {} at {}: released {released} lock rows of {}, which was not running; cycle {text}",
                        engine_name, now_utc(), &h[h.len().saturating_sub(12)..]
                    );
                    let waiters: Vec<&str> = c.iter().filter(|e| &e.to == h).map(|e| e.from.as_str()).collect();
                    for id in std::iter::once(h.as_str()).chain(waiters) {
                        let _ = api.post(
                            &format!("/api/projects/{}/workitems/{}/details", project.key(), id),
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
                println!("[engine] {}: deadlock {text}", project.key());
                for m in &members {
                    let _ = api.post(
                        &format!("/api/projects/{}/workitems/{}/details", project.key(), m),
                        &json!({"key": "doc", "valuetype": "text", "value": format!(
                            "deadlock seen by engine {} at {}: {text}. No member can start until one of these links is removed; the engine does not edit dependencies.",
                            engine_name, now_utc())}),
                    );
                }
            }
        }
    }


    /// A client-side claim, used ONLY for test-sweep runs (outside the cap,
    /// see dispatch): queued -> in-progress via a versioned write with a new
    /// lease, then every lockdir locked under it; a lost lock race puts the
    /// item back.  Every other item is claimed by iter_data's next.
    fn claim_direct(&mut self, project: &Project, item: &WorkItem) -> Option<WorkItem> {
        let mut claimed = serde_json::to_value(item).ok()?;
        claimed["state"] = json!("in-progress");
        claimed["run_now"] = json!(false);
        claimed["retry_after"] = json!("");
        claimed["blockedby_locks"] = json!([]);
        claimed["tags"] = json!(claim_tags(&item.tags));
        claimed["engine"] = json!(self.name);
        claimed["attempt"] = json!(item.attempt + 1);
        claimed["ts"]["start"] = json!(now_utc());
        let lease = uuid::Uuid::new_v4().to_string();
        claimed["lease"] = json!(lease);
        let resp = self.api.put(&format!("/api/projects/{}/workitems/{}?expect_version={}", project.key(), item.id, item.version), &claimed);
        let claimed_item: WorkItem = match resp.and_then(|v| serde_json::from_value(v).map_err(|e| crate::client::ApiError { status: 0, body: e.to_string() })) {
            Ok(i) => i,
            Err(e) => {
                if e.status != 409 {
                    eprintln!("[engine] claim failed for {}: {e}", item.id);
                }
                return None;
            }
        };
        for d in &item.lockdirs {
            let res = self.api.post(
                &format!("/api/projects/{}/locks/acquire", project.key()),
                &json!({"path": d, "kind": "lock", "engine": self.name, "workid": item.id, "lease": lease, "ttl_sec": iter_core::LOCK_LEASE_TTL_SEC}),
            );
            if res.is_err() {
                let _ = self.api.post(&format!("/api/projects/{}/locks/release_all", project.key()), &json!({"workid": item.id, "lease": lease}));
                let mut back = serde_json::to_value(&claimed_item).ok()?;
                back["state"] = json!("queued");
                back["lease"] = json!("");
                let _ = self.api.put(&format!("/api/projects/{}/workitems/{}?expect_version={}", project.key(), item.id, claimed_item.version), &back);
                return None;
            }
        }
        Some(claimed_item)
    }

    /// Run an item that is already claimed for this engine — with its lease
    /// and its locks taken (by iter_data's next, or claim_direct) — on a
    /// worker thread of its own.  Never claims or locks again.
    fn start_claimed(&mut self, project: &Project, topdir: &str, item: WorkItem, account: &str) {
        println!(
            "[engine] {}: start {} '{}' (agent {}, P{}, account {})",
            project.key(),
            &item.id[..8.min(item.id.len())],
            item.name,
            item.agent,
            item.priority,
            if account.is_empty() { "default" } else { account }
        );
        let api = self.api.clone();
        let engine_name = self.name.clone();
        let project = project.clone();
        let topdir = topdir.to_string();
        let counter = self.running_count.clone();
        counter.fetch_add(1, Ordering::SeqCst);
        let agent_type = item.agent.clone();
        let outside_cap = item.is_test_sweep_run();
        let project_name = project.key().to_string();
        let cur = Arc::new(Mutex::new((item.id.clone(), item.lease.clone())));
        let thread_cur = cur.clone();
        let account = account.to_string();
        let handle = std::thread::spawn(move || {
            crate::work::execute(&api, &engine_name, &project, &topdir, item, &account, &thread_cur);
            counter.fetch_sub(1, Ordering::SeqCst);
        });
        self.running.push(Running { agent: agent_type, outside_cap, project: project_name, cur, handle });
    }
}


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hold_reason_names_the_missing_token() {
        let acct = |n: &str| iter_core::Account { name: n.into(), ..Default::default() };
        let (both, s) = (vec![acct("a"), acct("b")], |v: &[&str]| v.iter().map(|x| x.to_string()).collect::<Vec<_>>());
        assert_eq!(hold_reason(&both, &[], &s(&["a", "b"])), "no account token");
        assert_eq!(hold_reason(&both, &[], &s(&["a"])), "accounts at stop%");
        assert_eq!(hold_reason(&both, &[], &[]), "accounts at stop%");
        // every account switched off names the switch, one off + one tokenless the token
        assert_eq!(hold_reason(&both, &s(&["a", "b"]), &s(&["a", "b"])), "accounts switched off");
        assert_eq!(hold_reason(&both, &s(&["a"]), &s(&["a", "b"])), "no account token");
    }

    /// The cluster's recovery releases only items that last started before it;
    /// one that parked itself on the tag while the cluster was healthy stays.
    #[test]
    fn cluster_recovery_releases_only_items_started_before_it() {
        let since = "2026-10-09T17:00:00Z";
        assert!(released_by_recovery("2026-10-09T03:00:00Z", since), "parked during the outage");
        assert!(released_by_recovery("", since), "tagged before it ever ran");
        assert!(!released_by_recovery("2026-10-09T17:05:00Z", since), "ran on a healthy cluster and parked anyway: no loop");
        assert!(!released_by_recovery("2026-10-09T03:00:00Z", ""), "no healthy verdict yet");
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

/// iter5 engine tests against a stateful stand-in for iter_data (assignments,
/// server-side next, versioned work item writes, details) with the mock
/// provider doing the agent work.
#[cfg(test)]
mod iter5_tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    #[derive(Default)]
    pub(crate) struct St {
        pub items: BTreeMap<String, Value>,
        pub details: HashMap<String, Vec<Value>>,
        pub assignments: Value,
        pub projects: HashMap<String, Value>,
        pub agents: Vec<Value>,
        pub heartbeat_reply: Value,
        pub engine: Value,
        /// extra GET routes: path -> body
        pub routes: HashMap<String, Value>,
    }

    pub(crate) fn item(id: &str, project: &str, name: &str, prio: i64, lockdirs: &[&str]) -> WorkItem {
        let mut w = WorkItem {
            id: id.into(), project: project.into(), name: name.into(), state: "queued".into(), agent: "code".into(),
            priority: prio, version: 1, lockdirs: lockdirs.iter().map(|s| s.to_string()).collect(),
            dedup_checked: "2026-10-02T00:00:00Z".into(), ..Default::default()
        };
        w.ts.receive = format!("2026-10-02T00:00:{:02}Z", prio.clamp(0, 59));
        w
    }

    fn seg(path: &str) -> Vec<String> {
        path.split('?').next().unwrap_or("").trim_matches('/').split('/').map(String::from).collect()
    }

    /// The stand-in.  `next` picks the project's queued item with the lowest
    /// (priority, receive) whose agent is allowed, and claims it the way
    /// iter_data does (in-progress, engine, attempt+1, lease, version+1).
    pub(crate) fn serve(st: Arc<Mutex<St>>) -> crate::client::fake::FakeServer {
        crate::client::fake::serve(move |m, path, body| {
            let mut s = st.lock().unwrap();
            if let Some(v) = s.routes.get(path.split('?').next().unwrap_or("")) {
                if m == "GET" {
                    return Some((200, v.clone()));
                }
            }
            let p = seg(path);
            let ps: Vec<&str> = p.iter().map(|x| x.as_str()).collect();
            let out = match (m, ps.as_slice()) {
                ("GET", ["api", "engines"]) => json!([]),
                ("GET", ["api", "engines", _]) => s.engine.clone(),
                ("GET", ["api", "engines", _, "assignments"]) => s.assignments.clone(),
                ("POST", ["api", "engines", _, "heartbeat"]) => s.heartbeat_reply.clone(),
                ("GET", ["api", "agents"]) => json!(s.agents),
                ("GET", ["api", "agents", a]) => s.agents.iter().find(|x| x["name"] == *a).cloned().unwrap_or(json!({"name": a})),
                ("GET", ["api", "tooling"]) => json!([]),
                ("GET", ["api", "projects", pr]) => s.projects.get(*pr).cloned().unwrap_or(json!({"name": pr})),
                ("GET", ["api", "projects", _, "versions"]) => json!([]),
                ("GET", ["api", "projects", _, "locks"]) => json!([]),
                ("GET", ["api", "projects", _, "spend"]) => json!([]),
                ("GET", ["api", "projects", pr, "workitems"]) => {
                    json!(s.items.values().filter(|i| i["project"] == *pr).cloned().collect::<Vec<_>>())
                }
                ("GET", ["api", "projects", _, "workitems", id]) => match s.items.get(*id) {
                    Some(i) => i.clone(),
                    None => return Some((404, json!({"error": "no such item"}))),
                },
                ("PUT", ["api", "projects", _, "workitems", id]) => {
                    let expect: Option<u64> = path.split("expect_version=").nth(1).and_then(|v| v.parse().ok());
                    let cur = s.items.get(*id).and_then(|i| i["version"].as_u64()).unwrap_or(0);
                    if expect.is_some() && expect != Some(cur) {
                        return Some((409, json!({"error": "version conflict"})));
                    }
                    let mut v = body.clone();
                    v["version"] = json!(cur + 1);
                    s.items.insert(id.to_string(), v.clone());
                    v
                }
                ("GET", ["api", "projects", _, "workitems", id, "details"]) => json!(s.details.get(*id).cloned().unwrap_or_default()),
                ("POST", ["api", "projects", _, "workitems", id, "details"]) => {
                    let rows = s.details.entry(id.to_string()).or_default();
                    let mut row = body.clone();
                    row["order"] = json!(rows.len());
                    rows.push(row.clone());
                    row
                }
                ("POST", ["api", "projects", pr, "next"]) => {
                    let allowed: Option<Vec<String>> = body.get("agents_allowed").and_then(|a| serde_json::from_value(a.clone()).ok());
                    let engine = body["engine"].as_str().unwrap_or("").to_string();
                    let pick = s
                        .items
                        .values()
                        .filter(|i| i["project"] == *pr && i["state"] == "queued")
                        .filter(|i| allowed.as_ref().map(|a| a.iter().any(|x| i["agent"] == x.as_str())).unwrap_or(true))
                        .min_by_key(|i| (i["priority"].as_i64().unwrap_or(0), i["ts"]["receive"].as_str().unwrap_or("").to_string()))
                        .cloned();
                    match pick {
                        Some(mut i) => {
                            i["state"] = json!("in-progress");
                            i["engine"] = json!(engine);
                            i["attempt"] = json!(i["attempt"].as_u64().unwrap_or(0) + 1);
                            i["lease"] = json!(format!("lease-{}", i["id"].as_str().unwrap_or("")));
                            i["version"] = json!(i["version"].as_u64().unwrap_or(0) + 1);
                            i["ts"]["start"] = json!(now_utc());
                            s.items.insert(i["id"].as_str().unwrap().to_string(), i.clone());
                            json!({"item": i, "reason": ""})
                        }
                        None => json!({"item": null, "reason": "none-queued"}),
                    }
                }
                _ => json!({}),
            };
            Some((200, out))
        })
    }

    pub(crate) fn topdir(tag: &str) -> String {
        let d = std::env::temp_dir().join(format!("iter5-engine-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join("src")).unwrap();
        d.to_string_lossy().into_owned()
    }

    fn state(assignments: Value) -> St {
        St {
            assignments,
            engine: json!({"name": "E1", "ticksec": 1, "probe_stale_min": 0}),
            heartbeat_reply: json!({"files_waiting": [], "build_waiting": []}),
            agents: vec![json!({"name": "code", "promptbody": "# code agent", "flags": "--dangerously-skip-permissions", "closegate": {"verify": "haiku"}})],
            ..Default::default()
        }
    }

    fn mock_account(name: &str, envar: &str, order: i64) -> Value {
        json!({"name": name, "provider": "mock", "token_envar": envar, "order": order, "switch": 80, "stop": 95, "model": ""})
    }

    fn engine_of(st: &Arc<Mutex<St>>) -> Engine {
        serde_json::from_value(st.lock().unwrap().engine.clone()).unwrap()
    }

    fn rt_for(srv: &crate::client::fake::FakeServer) -> EngineRuntime {
        let mut rt = EngineRuntime::new(srv.api(), "E1".into(), "/x/.env".into());
        rt.load_assignments();
        rt
    }

    fn state_of(st: &Arc<Mutex<St>>, id: &str) -> String {
        st.lock().unwrap().items[id]["state"].as_str().unwrap_or("").to_string()
    }

    /// One engine, two served projects: each project's item is handed out by
    /// iter_data's next (never claimed by the engine), run by the mock
    /// provider in that project's checkout, and closed complete through the
    /// close gate (mock verifier: complete).
    #[test]
    fn assignments_drive_a_multi_project_tick_through_next() {
        crate::envstore::set_for_test("ENGT_MP_TOKEN", "tok");
        let (t1, t2) = (topdir("mp1"), topdir("mp2"));
        let asg = json!({"engine": "E1", "projects": [
            {"project": "mp1", "topdir": t1, "read_only": false, "state": "Running", "accounts": [mock_account("engt-mp", "ENGT_MP_TOKEN", 1)]},
            {"project": "mp2", "topdir": t2, "read_only": false, "state": "Running", "accounts": [mock_account("engt-mp", "ENGT_MP_TOKEN", 1)]}],
            "accounts": [{"name": "engt-mp", "provider": "mock", "token_envar": "ENGT_MP_TOKEN"}]});
        let st = Arc::new(Mutex::new(state(asg)));
        for (id, p) in [("mp1-item-0001", "mp1"), ("mp2-item-0001", "mp2")] {
            let w = item(id, p, &format!("work in {p}"), 5, &[]);
            let mut s = st.lock().unwrap();
            s.items.insert(id.into(), serde_json::to_value(&w).unwrap());
            s.details.insert(id.into(), vec![json!({"order": 0, "key": "request", "value": format!("do it\nmock: write out.txt <<<{p}>>>\nmock: say wrote {p}")})]);
        }
        let srv = serve(st.clone());
        let mut rt = rt_for(&srv);
        rt.tick(&engine_of(&st));
        rt.drain();

        for (id, p, t) in [("mp1-item-0001", "mp1", &t1), ("mp2-item-0001", "mp2", &t2)] {
            assert_eq!(state_of(&st, id), "complete", "{id}: {:?}", st.lock().unwrap().items[id]);
            assert_eq!(std::fs::read_to_string(std::path::Path::new(t).join("out.txt")).unwrap(), format!("{p}\n"));
            let rows = st.lock().unwrap().details[id].clone();
            assert!(rows.iter().any(|r| r["key"] == "response" && r["value"] == format!("wrote {p}")), "{rows:?}");
            assert!(rows.iter().all(|r| r["key"] != "question"), "the mock verifier passed it: {rows:?}");
        }
        // both projects asked next, naming this engine; no client-side claim or lock
        let nexts = srv.calls.lock().unwrap().iter().filter(|(m, p, _)| m == "POST" && p.ends_with("/next")).map(|(_, p, b)| (p.clone(), b.clone())).collect::<Vec<_>>();
        assert!(nexts.iter().any(|(p, _)| p == "/api/projects/mp1/next") && nexts.iter().any(|(p, _)| p == "/api/projects/mp2/next"), "{nexts:?}");
        for (_, b) in &nexts {
            assert_eq!((b["engine"].as_str(), b["max"].as_u64(), b["lease_ttl_sec"].as_u64()), (Some("E1"), Some(1), Some(iter_core::LOCK_LEASE_TTL_SEC as u64)));
            assert!(b.get("agents_allowed").is_none(), "no agent is capped");
        }
        assert!(srv.calls_to("PUT", "/workitems/").iter().all(|b| b["state"] != "in-progress"), "the engine never claims an item next handed it");
        assert!(srv.calls_to("POST", "/locks/acquire").is_empty(), "next already took the locks");
        // the heartbeat names the served projects
        assert_eq!(srv.calls_to("POST", "/heartbeat")[0]["projects"], json!(["mp1", "mp2"]));
    }

    /// The bills edge's switch/stop: the first account over its switch% hands
    /// work to the second; every account over its stop% holds the project —
    /// next is never called and the queued item says why.
    #[test]
    fn account_switch_and_stop_come_from_the_bills_edge() {
        crate::envstore::set_for_test("ENGT_SW_A_TOKEN", "a");
        crate::envstore::set_for_test("ENGT_SW_B_TOKEN", "b");
        crate::envstore::set_for_test("ITER_MOCK_USAGE_ENGT_SW_A", "85,10");
        crate::envstore::set_for_test("ITER_MOCK_USAGE_ENGT_SW_B", "5,5");
        let t = topdir("sw");
        let accts = json!([mock_account("engt-sw-a", "ENGT_SW_A_TOKEN", 1), mock_account("engt-sw-b", "ENGT_SW_B_TOKEN", 2)]);
        let st = Arc::new(Mutex::new(state(json!({"engine": "E1", "projects": [{"project": "sw", "topdir": t, "state": "Running", "accounts": accts}]}))));
        st.lock().unwrap().engine["probe_stale_min"] = json!(30); // idle refresh asks the mock provider
        st.lock().unwrap().items.insert("sw-item-0001".into(), serde_json::to_value(item("sw-item-0001", "sw", "x", 5, &[])).unwrap());
        let srv = serve(st.clone());
        let mut rt = rt_for(&srv);
        rt.tick(&engine_of(&st));
        rt.drain();
        // tick 1 heartbeats before the idle refresh: tick 2 sees the snapshots
        rt.tick(&engine_of(&st));
        let hb = srv.calls_to("POST", "/heartbeat");
        assert_eq!(hb.last().unwrap()["account"], "engt-sw-b", "A is over its switch% (85 >= 80)");
        assert_eq!(state_of(&st, "sw-item-0001"), "complete");
        rt.drain();

        // both at/over stop%: hold, no next, the reason on the item
        for (k, v) in [("engt-sw-a", 96.0), ("engt-sw-b", 99.0)] {
            crate::usage::write_snapshot(k, &crate::usage::Usage { ts: Some(chrono::Utc::now()), five_hour_pct: v, source: "mock".into(), ..Default::default() }).unwrap();
        }
        st.lock().unwrap().items.insert("sw-item-0002".into(), serde_json::to_value(item("sw-item-0002", "sw", "y", 5, &[])).unwrap());
        rt.seen_seq.clear();
        let before = srv.calls_to("POST", "/next").len();
        rt.tick(&engine_of(&st));
        assert_eq!(srv.calls_to("POST", "/next").len(), before, "held: next is not called");
        let last = srv.calls_to("POST", "/heartbeat").last().cloned().unwrap();
        assert_eq!((last["account"].as_str(), last["hold"].as_str()), (Some(""), Some("all accounts at stop%")));
        let tags = st.lock().unwrap().items["sw-item-0002"]["tags"].clone();
        assert!(tags.to_string().contains("blocked by: accounts at stop%"), "{tags}");
    }

    /// A usage refresh from the webui (2026-10-08): every account is asked
    /// again whatever its snapshot's age, and the fresh report goes up once
    /// with clear_probe; the same stamp is not answered twice.
    #[test]
    fn a_usage_refresh_reprobes_every_account_once() {
        crate::envstore::set_for_test("ENGT_PR_A_TOKEN", "a");
        crate::envstore::set_for_test("ENGT_PR_B_TOKEN", "b");
        crate::envstore::set_for_test("ITER_MOCK_USAGE_ENGT_PR_A", "11,3");
        crate::envstore::set_for_test("ITER_MOCK_USAGE_ENGT_PR_B", "22,4");
        let t = topdir("pr");
        let accts = json!([mock_account("engt-pr-a", "ENGT_PR_A_TOKEN", 1), mock_account("engt-pr-b", "ENGT_PR_B_TOKEN", 2)]);
        let st = Arc::new(Mutex::new(state(json!({"engine": "E1", "projects": [{"project": "pr", "topdir": t, "state": "Stopped", "accounts": accts}]}))));
        let srv = serve(st.clone());
        let mut rt = rt_for(&srv);
        st.lock().unwrap().engine["probe_requested"] = json!("2026-10-08T12:00:00Z");
        rt.tick(&engine_of(&st));
        let cleared: Vec<Value> = srv.calls_to("POST", "/heartbeat").into_iter().filter(|b| b["clear_probe"] == true).collect();
        assert_eq!(cleared.len(), 1, "one report answers the request");
        let pct = |n: &str| cleared[0]["accounts"].as_array().unwrap().iter().find(|a| a["name"] == n).map(|a| a["five_hour_pct"].clone()).unwrap();
        assert_eq!((pct("engt-pr-a"), pct("engt-pr-b")), (json!(11.0), json!(22.0)));
        // the settings stay on the edge: the report repeats none of them
        assert!(cleared[0]["accounts"][0].get("stop").is_none() && cleared[0]["accounts"][0].get("switch").is_none());
        rt.tick(&engine_of(&st));
        assert_eq!(srv.calls_to("POST", "/heartbeat").into_iter().filter(|b| b["clear_probe"] == true).count(), 1, "the same stamp is answered once");
    }

    /// An account switched off (2026-10-08) is skipped by the ladder; with
    /// every account off the project holds — never the ambient login.
    #[test]
    fn switched_off_accounts_are_never_picked() {
        crate::envstore::set_for_test("ENGT_OFF_A_TOKEN", "a");
        crate::envstore::set_for_test("ENGT_OFF_B_TOKEN", "b");
        let t = topdir("off");
        let mut a = mock_account("engt-off-a", "ENGT_OFF_A_TOKEN", 1);
        a["stopped"] = json!(true);
        let accts = json!([a, mock_account("engt-off-b", "ENGT_OFF_B_TOKEN", 2)]);
        let st = Arc::new(Mutex::new(state(json!({"engine": "E1", "projects": [{"project": "off", "topdir": t, "state": "Running", "accounts": accts}]}))));
        st.lock().unwrap().items.insert("off-item-0001".into(), serde_json::to_value(item("off-item-0001", "off", "x", 5, &[])).unwrap());
        let srv = serve(st.clone());
        let mut rt = rt_for(&srv);
        rt.tick(&engine_of(&st));
        rt.drain();
        assert_eq!(srv.calls_to("POST", "/heartbeat").last().unwrap()["account"], "engt-off-b", "A comes first by order but is off");
        assert_eq!(state_of(&st, "off-item-0001"), "complete");

        // both off: hold, no next, the reason on the item and the heartbeat
        st.lock().unwrap().assignments["projects"][0]["accounts"][1]["stopped"] = json!(true);
        st.lock().unwrap().items.insert("off-item-0002".into(), serde_json::to_value(item("off-item-0002", "off", "y", 5, &[])).unwrap());
        rt.load_assignments();
        rt.seen_seq.clear();
        let before = srv.calls_to("POST", "/next").len();
        rt.tick(&engine_of(&st));
        assert_eq!(srv.calls_to("POST", "/next").len(), before, "held: next is not called");
        let last = srv.calls_to("POST", "/heartbeat").last().cloned().unwrap();
        assert_eq!((last["account"].as_str(), last["hold"].as_str()), (Some(""), Some("accounts switched off")));
        let tags = st.lock().unwrap().items["off-item-0002"]["tags"].clone();
        assert!(tags.to_string().contains("blocked by: accounts switched off"), "{tags}");
    }

    /// Per-agent caps travel to iter_data as `agents_allowed`; an unknown
    /// provider fails the attempt (queued behind the backoff) with the error.
    #[test]
    fn capped_agents_are_left_out_and_unknown_providers_fail_the_attempt() {
        crate::envstore::set_for_test("ENGT_UP_TOKEN", "x");
        let t = topdir("up");
        let st = Arc::new(Mutex::new(state(json!({"engine": "E1", "projects": [{"project": "up", "topdir": t, "state": "Running",
            "accounts": [{"name": "engt-up", "provider": "openai", "token_envar": "ENGT_UP_TOKEN", "order": 1}]}]}))));
        {
            let mut s = st.lock().unwrap();
            s.projects.insert("up".into(), json!({"name": "up", "agents": {"plan": {"max": 0}}, "failure": {"maxattempts": 5}}));
            s.agents.push(json!({"name": "plan", "promptbody": "# plan"}));
            s.items.insert("up-item-0001".into(), serde_json::to_value(item("up-item-0001", "up", "x", 5, &[])).unwrap());
            let mut p = item("up-plan-0001", "up", "a plan", 1, &[]);
            p.agent = "plan".into();
            s.items.insert("up-plan-0001".into(), serde_json::to_value(p).unwrap());
        }
        let srv = serve(st.clone());
        let mut rt = rt_for(&srv);
        rt.tick(&engine_of(&st));
        rt.drain();
        let body = srv.calls_to("POST", "/next")[0].clone();
        let allowed: Vec<String> = serde_json::from_value(body["agents_allowed"].clone()).unwrap();
        assert!(allowed.contains(&"code".to_string()) && !allowed.contains(&"plan".to_string()), "{allowed:?}");
        assert_eq!(state_of(&st, "up-plan-0001"), "queued", "plan is capped at 0");
        let it = st.lock().unwrap().items["up-item-0001"].clone();
        assert_eq!(it["state"], "queued", "{it}");
        assert!(it["lasterror"].as_str().unwrap().contains("unknown provider 'openai'"), "{it}");
        assert!(!it["retry_after"].as_str().unwrap().is_empty());
        assert!(st.lock().unwrap().items["up-plan-0001"]["tags"].to_string().contains("agent cap (plan 0/0)"));
    }

    /// An agent whose flags grant no permission mode is never started
    /// (2026-10-08): it is left out of agents_allowed, its item stays queued
    /// with the reason, and nothing is spent; a project override or
    /// `readonly: true` arms it.
    #[test]
    fn an_agent_without_permission_flags_is_not_started() {
        crate::envstore::set_for_test("ENGT_PF_TOKEN", "x");
        let t = topdir("pf");
        let st = Arc::new(Mutex::new(state(json!({"engine": "E1", "projects": [{"project": "pf", "topdir": t, "state": "Running",
            "accounts": [mock_account("engt-pf", "ENGT_PF_TOKEN", 1)]}]}))));
        {
            let mut s = st.lock().unwrap();
            s.agents.push(json!({"name": "refactor", "promptbody": "# refactor", "flags": ""}));
            s.agents.push(json!({"name": "explain", "promptbody": "# explain", "readonly": true}));
            let mut r = item("pf-refa-0001", "pf", "tidy", 1, &[]);
            r.agent = "refactor".into();
            s.items.insert("pf-refa-0001".into(), serde_json::to_value(r).unwrap());
        }
        let srv = serve(st.clone());
        let mut rt = rt_for(&srv);
        rt.tick(&engine_of(&st));
        rt.drain();
        let allowed: Vec<String> = serde_json::from_value(srv.calls_to("POST", "/next")[0]["agents_allowed"].clone()).unwrap();
        assert!(!allowed.contains(&"refactor".to_string()) && allowed.contains(&"code".to_string()) && allowed.contains(&"explain".to_string()), "{allowed:?}");
        assert_eq!(state_of(&st, "pf-refa-0001"), "queued");
        assert!(st.lock().unwrap().items["pf-refa-0001"]["tags"].to_string().contains("agent 'refactor' has no permission flags"));
        // the project's override arms it: next time it may run
        st.lock().unwrap().projects.insert("pf".into(), json!({"name": "pf", "agents": {"refactor": {"flags": "--permission-mode bypassPermissions"}}}));
        rt.seen_seq.clear();
        rt.tick(&engine_of(&st));
        rt.drain();
        assert_eq!(state_of(&st, "pf-refa-0001"), "complete");
    }

    /// `mock: gate incomplete` makes the mock verifier say incomplete: the
    /// item bounces back to queued with the close-gate reason.
    #[test]
    fn mock_gate_incomplete_bounces_the_item() {
        crate::envstore::set_for_test("ENGT_GI_TOKEN", "x");
        let t = topdir("gi");
        let st = Arc::new(Mutex::new(state(json!({"engine": "E1", "projects": [{"project": "gi", "topdir": t, "state": "Running",
            "accounts": [mock_account("engt-gi", "ENGT_GI_TOKEN", 1)]}]}))));
        {
            let mut s = st.lock().unwrap();
            s.items.insert("gi-item-0001".into(), serde_json::to_value(item("gi-item-0001", "gi", "x", 5, &[])).unwrap());
            s.details.insert("gi-item-0001".into(), vec![json!({"order": 0, "key": "request", "value": "do\nmock: gate incomplete"})]);
        }
        let srv = serve(st.clone());
        let mut rt = rt_for(&srv);
        rt.tick(&engine_of(&st));
        rt.drain();
        let it = st.lock().unwrap().items["gi-item-0001"].clone();
        assert_eq!((it["state"].as_str(), it["gate_bounces"].as_u64()), (Some("queued"), Some(1)), "{it}");
        assert!(it["lasterror"].as_str().unwrap().starts_with("close gate: verifier:"), "{it}");
        let rows = st.lock().unwrap().details["gi-item-0001"].clone();
        assert!(rows.iter().any(|r| r["key"] == "verify"), "{rows:?}");
    }

    /// Session chaining still works with server-side next: the first item
    /// comes from next, its queued neighbour (same lockdirs) is claimed by the
    /// chain's versioned PUT and runs in the same mock session.
    #[test]
    fn session_chaining_still_claims_the_neighbour() {
        crate::envstore::set_for_test("ENGT_CH_TOKEN", "x");
        let t = topdir("ch");
        let st = Arc::new(Mutex::new(state(json!({"engine": "E1", "projects": [{"project": "ch", "topdir": t, "state": "Running",
            "accounts": [mock_account("engt-ch", "ENGT_CH_TOKEN", 1)]}]}))));
        {
            let mut s = st.lock().unwrap();
            s.projects.insert("ch".into(), json!({"name": "ch", "session_chain_max": 3}));
            for (id, prio, f) in [("ch-item-0001", 1, "a.txt"), ("ch-item-0002", 2, "b.txt")] {
                s.items.insert(id.into(), serde_json::to_value(item(id, "ch", id, prio, &["{topdir}/src"])).unwrap());
                s.details.insert(id.into(), vec![json!({"order": 0, "key": "request", "value": format!("mock: write {f} <<<{id}>>>")})]);
            }
        }
        let srv = serve(st.clone());
        let mut rt = rt_for(&srv);
        // cap 1: only one item can come from next this tick
        st.lock().unwrap().projects.get_mut("ch").unwrap()["maxagents"] = json!({"else": 1});
        rt.tick(&engine_of(&st));
        rt.drain();
        assert_eq!(state_of(&st, "ch-item-0001"), "complete");
        assert_eq!(state_of(&st, "ch-item-0002"), "complete");
        assert_eq!(srv.calls_to("POST", "/next").len(), 1, "the neighbour did not come from next");
        let chain_claims: Vec<Value> = srv.calls_to("PUT", "/workitems/ch-item-0002").into_iter().filter(|b| b["state"] == "in-progress").collect();
        assert_eq!(chain_claims.len(), 1, "claimed by the chain");
        assert!(std::path::Path::new(&t).join("src/a.txt").exists() && std::path::Path::new(&t).join("src/b.txt").exists());
    }

    /// A project whose assignment says Stopped is not dispatched; a project
    /// no longer in the assignments is not touched at all.
    #[test]
    fn stopped_or_unassigned_projects_get_no_next() {
        let t = topdir("stp");
        let st = Arc::new(Mutex::new(state(json!({"engine": "E1", "projects": [{"project": "stp", "topdir": t, "state": "Stopped", "accounts": []}]}))));
        st.lock().unwrap().items.insert("stp-item-0001".into(), serde_json::to_value(item("stp-item-0001", "stp", "x", 5, &[])).unwrap());
        let srv = serve(st.clone());
        let mut rt = rt_for(&srv);
        rt.tick(&engine_of(&st));
        assert!(srv.calls_to("POST", "/next").is_empty());
        st.lock().unwrap().assignments = json!({"engine": "E1", "projects": []});
        rt.load_assignments();
        rt.tick(&engine_of(&st));
        assert!(srv.calls_to("GET", "/api/projects/stp").len() <= 3, "only the first tick read it");
        assert!(srv.calls_to("POST", "/next").is_empty());
    }

    /// A new item is triaged (mock judge) before the engine asks next for
    /// anything in its project; a project with no checkout yet is never
    /// dispatched.
    #[test]
    fn dedup_triage_holds_next_and_a_missing_checkout_skips_dispatch() {
        crate::envstore::set_for_test("ENGT_DT_TOKEN", "x");
        let t = topdir("dt");
        let missing = format!("{t}-not-built");
        let st = Arc::new(Mutex::new(state(json!({"engine": "E1", "projects": [
            {"project": "dt", "topdir": t, "state": "Running", "accounts": [mock_account("engt-dt", "ENGT_DT_TOKEN", 1)]},
            {"project": "nb", "topdir": missing, "state": "Running", "accounts": []}]}))));
        {
            let mut s = st.lock().unwrap();
            let mut w = item("dt-item-0001", "dt", "fresh", 5, &["{topdir}/src"]);
            w.dedup_checked = String::new();
            w.ts.receive = "2026-10-02T10:00:00Z".into();
            s.items.insert(w.id.clone(), serde_json::to_value(&w).unwrap());
            s.items.insert("nb-item-0001".into(), serde_json::to_value(item("nb-item-0001", "nb", "x", 5, &[])).unwrap());
        }
        let srv = serve(st.clone());
        let mut rt = rt_for(&srv);
        rt.tick(&engine_of(&st));
        assert!(srv.calls_to("POST", "/next").is_empty(), "triage first");
        rt.drain();
        assert!(!st.lock().unwrap().items["dt-item-0001"]["dedup_checked"].as_str().unwrap().is_empty(), "stamped by the triage");
        rt.tick(&engine_of(&st));
        rt.drain();
        let nexts: Vec<String> = srv.calls.lock().unwrap().iter().filter(|(m, p, _)| m == "POST" && p.ends_with("/next")).map(|(_, p, _)| p.clone()).collect();
        assert!(!nexts.is_empty() && nexts.iter().all(|p| p == "/api/projects/dt/next"), "nb has no checkout: {nexts:?}");
        assert_eq!(state_of(&st, "dt-item-0001"), "complete");
    }
}
