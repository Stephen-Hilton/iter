//! The tick loop: sync metadata (seq-gated with a periodic full-refresh
//! fallback), heartbeat, pick queued work, take central locks, run, close.

use crate::client::Api;
use iter_core::cluster::{self, CLUSTER_RESTART_REASON, CLUSTER_RESTART_TAG};
use iter_core::{BLOCKED_TAG_COLOR, BLOCKED_TAG_PREFIX, DepStatus, Engine, Project, WorkItem, children_index, claim_tags, dependency_status, now_utc, paths_overlap, pick_account};
use serde_json::{Value, json};
use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;
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
    /// running work: (workid, agent_type, handle)
    running: Vec<(String, String, std::thread::JoinHandle<()>)>,
    /// ELI5 runs in flight (spec: Explain / ELI5): outside the cap, never in
    /// `running`, so they neither count toward maxagents nor delay a drain
    explaining: Vec<(String, std::thread::JoinHandle<()>)>,
    /// stage-2 dedup triages in flight (crate::dedup, 2026-09-10): the item
    /// is held out of dispatch while its judge runs on its own thread
    triaging: Vec<(String, std::thread::JoinHandle<()>)>,
    /// items whose triage finished this process (the cache may not show the
    /// stamp for a tick; never start a second judge on the same item)
    triaged: std::collections::HashSet<String>,
    running_count: Arc<AtomicUsize>,
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
}

use crate::usage;

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

fn expand_topdir(topdir: &str) -> String {
    if let Some(rest) = topdir.strip_prefix("~/") {
        if let Ok(home) = std::env::var("HOME") {
            return format!("{home}/{rest}");
        }
    }
    topdir.to_string()
}

impl EngineRuntime {
    pub fn new(api: Api, name: String) -> Self {
        Self {
            api,
            name,
            seen_seq: HashMap::new(),
            last_full_refresh: Instant::now() - Duration::from_secs(86400 * 365),
            projects: HashMap::new(),
            items: HashMap::new(),
            agents: HashMap::new(),
            deferred: HashMap::new(),
            running: Vec::new(),
            explaining: Vec::new(),
            triaging: Vec::new(),
            triaged: std::collections::HashSet::new(),
            running_count: Arc::new(AtomicUsize::new(0)),
            max_ticks: None,
            last_test_handled: String::new(),
            holding: false,
            last_probe: HashMap::new(),
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
                    {
                        std::thread::sleep(Duration::from_millis(200));
                    }
                    return;
                }
            }
            std::thread::sleep(Duration::from_secs(engine.ticksec.max(1)));
        }
    }

    /// `claude -p "."` on haiku for the active account; the result and the
    /// refreshed usage snapshot go back on the engine record.
    fn run_test(&mut self, engine: &Engine, account: &str) {
        // token: the active account's env var, from whichever project defines it
        let token = self
            .projects
            .values()
            .flat_map(|p| p.accounts.iter())
            .find(|a| a.name == account)
            .and_then(|a| std::env::var(&a.token_envar).ok())
            .map(|t| t.trim().to_string())
            .filter(|t| !t.is_empty());
        let cwd = engine
            .projects
            .values()
            .next()
            .and_then(|d| d.dirs.get("topdir"))
            .map(|t| expand_topdir(t))
            .filter(|t| std::path::Path::new(t).is_dir())
            .unwrap_or_else(|| std::env::temp_dir().to_string_lossy().to_string());
        println!("[engine] connectivity test requested at {} (account '{}')", engine.test_requested, if account.is_empty() { "default" } else { account });
        let result = match crate::work::nudge(token, account, &cwd) {
            Ok((out, ms)) => json!({
                "requested": engine.test_requested, "ts": now_utc(), "ok": out.subtype == "success",
                "ms": ms, "model": "haiku", "account": account,
                "text": out.text.chars().take(200).collect::<String>(), "subtype": out.subtype,
            }),
            Err(e) => json!({
                "requested": engine.test_requested, "ts": now_utc(), "ok": false, "model": "haiku",
                "account": account, "error": e.chars().take(500).collect::<String>(),
            }),
        };
        println!("[engine] connectivity test {}", if result["ok"].as_bool().unwrap_or(false) { "OK" } else { "FAILED" });
        let _ = self.api.post(
            &format!("/api/engines/{}/heartbeat", self.name),
            &json!({"test_result": result, "clear_test": true, "account": account,
                    "usage": if self.holding { Value::Null } else { usage::snapshot_json(account, chrono::Utc::now()).unwrap_or(Value::Null) }}),
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
        for project_name in engine.projects.keys() {
            if let Some(p) = self.projects.get(project_name) {
                for a in &p.accounts {
                    any_accounts = true;
                    if targets.iter().any(|(n, _)| n == &a.name) {
                        continue;
                    }
                    let tok = std::env::var(&a.token_envar).ok().map(|t| t.trim().to_string()).filter(|t| !t.is_empty());
                    if let Some(tok) = tok {
                        targets.push((a.name.clone(), tok));
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

    fn prune_running(&mut self) -> usize {
        self.running.retain(|(_, _, h)| !h.is_finished());
        self.running.len()
    }

    fn tick(&mut self, engine: &Engine) {
        self.prune_running();

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
                if let Some(acct) = pick_account(&p.accounts, &map, &in_use) {
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
        let accounts = usage::accounts_json(&all_accounts, &in_use, now);
        let next = usage::next_json(&accounts);
        let _ = self.api.post(
            &format!("/api/engines/{}/heartbeat", self.name),
            &json!({"state": "Running", "account": chosen_account,
                    "hold": if self.holding { "all accounts at stop%" } else { "" },
                    "usage": if self.holding { Value::Null } else { usage::snapshot_json(&chosen_account, now).unwrap_or(Value::Null) },
                    "accounts": accounts, "next": next}),
        );

        // connectivity test requested from the webui: one haiku nudge, then
        // report the outcome (and the refreshed usage) via heartbeat
        if !engine.test_requested.is_empty() && engine.test_requested != self.last_test_handled {
            self.last_test_handled = engine.test_requested.clone();
            self.run_test(engine, &chosen_account);
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
            self.fire_schedules(&project);
            let Some(dirs) = engine.projects.get(project_name) else { continue };
            let topdir = expand_topdir(dirs.dirs.get("topdir").map(String::as_str).unwrap_or("."));
            self.dispatch(engine, &project, &topdir, &in_use);
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
            match pick_account(&project.accounts, &map, in_use) {
                Some(a) => {
                    usage_pct = map.get(&a.name).copied().unwrap_or(0);
                    account = Some(a.clone());
                }
                None => {
                    // all accounts at/over their stop%: stop all activity and
                    // monitor for the usage refresh (expiry zeroes windows)
                    println!(
                        "[engine] {project_name}: all accounts at stop% — holding until a usage window resets"
                    );
                    usage_pct = 100;
                    account = None;
                    hold = Some("accounts at stop%".into());
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

        // current central lock rows (locks + reservations)
        let lock_rows: Vec<Value> = self
            .api
            .get(&format!("/api/projects/{project_name}/locks"))
            .ok()
            .and_then(|v| v.as_array().cloned())
            .unwrap_or_default();
        // an expired row is free (acquire treats it so); never let a leftover
        // row from a closed item hold a waiter (seen live 2026-09-08)
        let locked_paths: Vec<(String, String, String)> = lock_rows
            .iter()
            .filter(|r| r.get("expires").and_then(|e| e.as_str()).map_or(true, |e| e.is_empty() || e >= now_iso.as_str()))
            .map(|r| {
                (
                    r.get("path").and_then(|p| p.as_str()).unwrap_or("").to_string(),
                    r.get("kind").and_then(|k| k.as_str()).unwrap_or("lock").to_string(),
                    r.get("workid").and_then(|w| w.as_str()).unwrap_or("").to_string(),
                )
            })
            .collect();

        // the cluster is back: requeue every parked tagged item that is NOT still
        // in flight.  `iter block` parks the item mid-run, seconds to minutes
        // before its turn ends; requeueing inside that gap would let the run
        // close through the gate (seen in e2e 2026-09-09).  In flight = running
        // on this engine, or still holding a live central lock row (another
        // engine's run releases its rows only when the run is over).
        if cluster_healthy {
            let in_flight = |i: &WorkItem| {
                self.running.iter().any(|(w, _, _)| w == &i.id)
                    || locked_paths.iter().any(|(_, kind, w)| kind == "lock" && w == &i.id)
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
        // the running items whose "lock" rows overlap this item's lockdirs:
        // (holder workid, the locked path) — the holder is a dependency the
        // engine knows about and, since 2026-09-07, records (`blockedby_locks`)
        let lock_holders = |item: &WorkItem| -> Vec<(String, String)> {
            let mut v: Vec<(String, String)> = Vec::new();
            for d in &item.lockdirs {
                for (path, kind, workid) in &locked_paths {
                    if workid != &item.id && kind == "lock" && paths_overlap(d, path) && !v.iter().any(|(w, _)| w == workid) {
                        v.push((workid.clone(), path.clone()));
                    }
                }
            }
            v
        };
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
                        if let Some((_, path)) = w.holders.first() {
                            w.reason = Some(format!("lock {path}"));
                        } else if let Some(h) = &hold {
                            w.reason = Some(h.clone());
                        }
                    }
                    // a declared loop can never start: say so on the item
                    // (renders "blocked by: dependency cycle with <id12>")
                    DepStatus::Cycle(other) => {
                        w.reason = Some(format!("dependency cycle with {}", &other[other.len().saturating_sub(12)..]));
                    }
                    DepStatus::Waiting(_) | DepStatus::Failed(_) => {}
                }
            }
            waits.insert(i.id.clone(), w);
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
        let mut started_now: Vec<String> = Vec::new();
        let run_now: Vec<&WorkItem> = items
            .iter()
            .filter(|i| i.run_now && i.state == "queued" && !i.needs_approval && !cluster::blocks(i, cluster_healthy) && deps_satisfied(i) && !scope_blocked(i))
            .collect();
        for item in run_now {
            println!(
                "[engine] run-now override: starting {} '{}' (running {} / cap {})",
                &item.id[..8.min(item.id.len())], item.name, self.running.len(), cap
            );
            if self.start_item(engine, project, topdir, item, &account_name) {
                started_now.push(item.id.clone());
                waits.remove(&item.id);
            }
        }
        let running_now = self.running.len();
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
        let reserved: Vec<(String, i64, String)> = locked_paths
            .iter()
            .filter(|(_, kind, _)| kind == "reserve")
            .filter_map(|(path, _, workid)| {
                by_id.get(workid).map(|i| (path.clone(), i.priority, workid.clone()))
            })
            .collect();

        let mut slots = cap.saturating_sub(running_now);
        for item in queued {
            if slots == 0 {
                set_reason(&mut waits, &item.id, format!("usage cap ({}/{cap})", self.running.len()));
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
                self.running.iter().filter(|(_, a, _)| a == &item.agent).count();
            let type_max = project
                .agents
                .get(&item.agent)
                .and_then(|o| o.get("max"))
                .and_then(|m| m.as_u64())
                .or_else(|| {
                    self.agents.get(&item.agent).and_then(|a| a.get("max")).and_then(|m| m.as_u64())
                })
                .unwrap_or(4) as usize;
            if item.agent != "exec" && type_running >= type_max {
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

        // central locks for every lockdir
        let ttl = 3600 + 300;
        let mut acquired: Vec<String> = Vec::new();
        for d in &item.lockdirs {
            let res = self.api.post(
                &format!("/api/projects/{}/locks/acquire", project.name),
                &json!({"path": d, "kind": "lock", "engine": self.name,
                        "workid": item.id, "ttl_sec": ttl}),
            );
            if res.is_err() {
                for p in &acquired {
                    let _ = self.api.post(
                        &format!("/api/projects/{}/locks/release", project.name),
                        &json!({"path": p, "workid": item.id}),
                    );
                }
                // lost the lock race: put it back to queued
                let mut back = serde_json::to_value(&claimed_item).unwrap();
                back["state"] = json!("queued");
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
            acquired.push(d.clone());
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
        let workid = run_item.id.clone();
        let account = account.to_string();
        let handle = std::thread::spawn(move || {
            crate::work::execute(&api, &engine_name, &project, &topdir, run_item, &account);
            counter.fetch_sub(1, Ordering::SeqCst);
        });
        self.running.push((workid, agent_type, handle));
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
