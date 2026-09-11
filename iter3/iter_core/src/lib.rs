//! iter_core — shared types for iter V3 (spec: src/features/iter.v3.md).
//! These are the wire shapes both iter_data and iter_engine speak; storage
//! backends persist them as JSON bodies, so unknown fields round-trip.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub mod cluster;
pub mod dedup;
pub mod sched;
pub mod widget;

/// The claim's tag rule, at the root so both claim sites name one thing
/// (iter_core::claim_tags — see `cluster::claim_tags`).
pub use cluster::claim_tags;

/// Logical table names — storage backends map these to physical names
/// (DynamoDB: prefix + name; SQLite: table name).
pub const TABLES: &[&str] = &[
    "agent",
    "project",
    "engine",
    "workitem",
    "workitem_detail",
    "project_prepostwork",
    "webui_user",
    "webui",
    "versions",
    "lock",
    "project_structure",
    "agent_tooling",
    "spend",
];

/// Workitem states (glossary in iter.v3.md).
pub const STATES: &[&str] = &[
    "in-progress",
    "queued",
    "question",
    "parked",
    "paused",
    "failed",
    "complete",
    "scheduled",
];

/// `null` reads as the default (fixed 2026-09-10): the webui's settings form
/// sends `null` for an emptied JSON box, and `#[serde(default)]` alone only
/// covers a MISSING key — "project does not parse: invalid type: null,
/// expected struct DedupConfig" was the symptom.
pub fn null_is_default<'de, D, T>(d: D) -> Result<T, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Default + Deserialize<'de>,
{
    Ok(Option::<T>::deserialize(d)?.unwrap_or_default())
}
/// same, for `default_context` (whose default is the pattern list, not empty)
fn null_is_default_context<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Vec<String>, D::Error> {
    Ok(Option::<Vec<String>>::deserialize(d)?.unwrap_or_else(default_context))
}

pub fn now_utc() -> String {
    chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AgentDef {
    pub name: String,
    #[serde(default)]
    pub desc: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub childstate: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeoutsec: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub flags: Option<String>,
    #[serde(default)]
    pub promptbody: String,
    /// completion contract the engine enforces at close (spec: Close Gate)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub closegate: Option<CloseGate>,
    /// which lockdirs an item of this agent type may carry (spec: Lock Shape,
    /// 2026-09-07); absent = anything goes
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lockshape: Option<LockShape>,
}

/// Per-agent lock shape (decided 2026-09-07, after a pdy-dev plan item locked
/// three top-level areas for an hour): what an item of this agent type may
/// lock, checked by iter_data on create and on every PUT that changes
/// `lockdirs`, and by `iter add` before it posts.  Overridable per project
/// via `project.agents[agent].lockshape`, key by key (see `lockshape_for`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct LockShape {
    /// every lockdir must equal or sit under one of these; `*` matches one
    /// path segment, `**` any number, `{test_dir}` the project's test dir
    /// name.  Empty = any path.
    #[serde(default)]
    pub allow: Vec<String>,
    /// what a lockdir outside `allow` does: "refuse" (default) | "warn"
    #[serde(default)]
    pub outside: String,
    /// warn when a lockdir overlaps more than this many OTHER open items
    /// (0 = off).  With `outside: refuse` the overlap count is quoted in the
    /// refusal so the reader sees the cost.
    #[serde(default)]
    pub max_overlap: u32,
    /// this agent writes nothing: any lockdir is refused
    #[serde(default)]
    pub none: bool,
}

impl LockShape {
    pub fn refuses_outside(&self) -> bool {
        self.outside.trim() != "warn"
    }
}

/// Resolve the effective lock shape: the agent record's, then the project's
/// per-agent override merged key by key.  None when neither says anything.
pub fn lockshape_for(agent_def: &serde_json::Value, project_override: &serde_json::Value) -> Option<LockShape> {
    let mut merged = agent_def.get("lockshape").cloned().unwrap_or(serde_json::Value::Null);
    if !merged.is_object() {
        merged = serde_json::Value::Null;
    }
    if let Some(ovr) = project_override.get("lockshape").and_then(|v| v.as_object()) {
        if !merged.is_object() {
            merged = serde_json::json!({});
        }
        for (k, v) in ovr {
            merged[k] = v.clone();
        }
    }
    if !merged.is_object() {
        return None;
    }
    serde_json::from_value(merged).ok()
}

/// One finding of `check_lockshape`: a refusal (the write must not happen)
/// or a warning (it may, but the reader should know).
#[derive(Debug, Clone, PartialEq)]
pub struct LockFinding {
    pub refuse: bool,
    pub msg: String,
}

/// Does `lockdir` equal or sit under `pattern`?  Segment-wise: `*` matches
/// one segment, `**` any run of segments (including none); `{test_dir}` is
/// replaced by the project's test dir name before matching.  Both sides are
/// compared after trimming trailing slashes.
pub fn lock_pattern_matches(pattern: &str, lockdir: &str, test_dir: &str) -> bool {
    let pattern = pattern.replace("{test_dir}", test_dir);
    let pat: Vec<&str> = pattern.trim_end_matches('/').split('/').filter(|s| !s.is_empty() || pattern.starts_with('/')).collect();
    let dir: Vec<&str> = lockdir.trim_end_matches('/').split('/').filter(|s| !s.is_empty() || lockdir.starts_with('/')).collect();
    fn go(pat: &[&str], dir: &[&str]) -> bool {
        match pat.first() {
            // pattern exhausted: the lockdir may go deeper (equal-or-under)
            None => true,
            Some(&"**") => (0..=dir.len()).any(|k| go(&pat[1..], &dir[k..])),
            Some(p) => match dir.first() {
                Some(d) if *p == "*" || p == d => go(&pat[1..], &dir[1..]),
                _ => false,
            },
        }
    }
    go(&pat, &dir)
}

/// How many of `others` (id, lockdirs) have a lockdir overlapping `lockdir`.
pub fn overlap_count(lockdir: &str, others: &[(String, Vec<String>)]) -> usize {
    others.iter().filter(|(_, dirs)| dirs.iter().any(|d| paths_overlap(d, lockdir))).count()
}

/// Check an item's lockdirs against its agent's lock shape.  `others` are
/// the project's OTHER open items (id, lockdirs) — never the item itself.
pub fn check_lockshape(
    agent: &str,
    shape: &LockShape,
    lockdirs: &[String],
    others: &[(String, Vec<String>)],
    test_dir: &str,
) -> Vec<LockFinding> {
    let mut out = Vec::new();
    if shape.none {
        for d in lockdirs {
            out.push(LockFinding {
                refuse: true,
                msg: format!("refused: codepath {d} — the {agent} agent writes nothing and takes no lock (lockshape.none)"),
            });
        }
        return out;
    }
    for d in lockdirs {
        let overlaps = overlap_count(d, others);
        let outside = !shape.allow.is_empty() && !shape.allow.iter().any(|p| lock_pattern_matches(p, d, test_dir));
        if outside {
            let allowed = shape.allow.join(", ");
            let cost = if overlaps > 0 { format!(" and overlaps {overlaps} open item(s)") } else { String::new() };
            out.push(LockFinding {
                refuse: shape.refuses_outside(),
                msg: format!(
                    "{}: codepath {d} is outside the {agent} agent's lock shape{cost}; a {agent} item locks the directory it writes ({allowed}), not the tree it reads",
                    if shape.refuses_outside() { "refused" } else { "warning" }
                ),
            });
            continue;
        }
        if shape.max_overlap > 0 && overlaps > shape.max_overlap as usize {
            out.push(LockFinding {
                refuse: false,
                msg: format!("warning: codepath {d} overlaps {overlaps} open item(s) (lockshape.max_overlap {}); they all wait while this item runs — narrow it if the work does not own the whole tree", shape.max_overlap),
            });
        }
    }
    out
}

/// Tag text prefix the engine uses for the one synthesized "why is this
/// queued item not running" tag (spec: lock waits are visible, 2026-09-07).
/// The engine owns every tag starting with it; humans' tags are untouched.
pub const BLOCKED_TAG_PREFIX: &str = "blocked by: ";
pub const BLOCKED_TAG_COLOR: &str = "#c47a1f";

/// Usecase membership (decided 2026-09-08): an item born under a usecase
/// carries `usecase:<name>`; children inherit it at birth, all the way down.
/// Engine-owned prefix like the blocked tag — the webui groups by it.
pub const USECASE_TAG_PREFIX: &str = "usecase:";
pub const USECASE_TAG_COLOR: &str = "#3b6fb6";

/// Priority is 0–99, LOWER = sooner (widened from 0–10 on 2026-09-08; the
/// migration multiplied existing numbers by 10, P0 stayed P0).  Bands:
///   0–9   do now
///   10–39 usecases — ONE number per usecase, inherited by everything under it
///   40–49 human default (new human-filed roots)
///   50–99 maintenance, schedules, agent-filed roots with no lineage
/// A root item that names no priority takes the lowest number in its band
/// that no OPEN item uses, so two lineages never compete on one number.
pub const PRIO_BAND_DO_NOW: (i64, i64) = (0, 9);
pub const PRIO_BAND_USECASE: (i64, i64) = (10, 39);
pub const PRIO_BAND_HUMAN: (i64, i64) = (40, 49);
pub const PRIO_BAND_MAINT: (i64, i64) = (50, 99);
pub const PRIO_MAX: i64 = 99;

/// Lowest number in `[lo, hi]` absent from `used`; `lo` when the band is full.
pub fn pick_unused_priority(band: (i64, i64), used: &[i64]) -> i64 {
    (band.0..=band.1).find(|p| !used.contains(p)).unwrap_or(band.0)
}

/// The `usecase:` tags on an item (text only, deduped, in order).
pub fn usecase_tags(tags: &[Tag]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for t in tags {
        if t.text.starts_with(USECASE_TAG_PREFIX) && !out.contains(&t.text) {
            out.push(t.text.clone());
        }
    }
    out
}

/// Per-agent close gate (decided 2026-09-03): what must be true before an
/// item may close complete.  Every key is overridable per project via
/// `project.agents[agent].closegate`; see `close_gate_for`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CloseGate {
    /// verifier model alias (haiku | sonnet | opus | ...); "" disables the LLM half
    #[serde(default = "default_verify")]
    pub verify: String,
    /// the item must have created >=1 workitem with createdby == its id
    #[serde(default)]
    pub requires_children: bool,
    /// the enforced git postwork must have produced a new commit
    #[serde(default)]
    pub requires_commit: bool,
    /// bounces back to queued before the item goes to question
    #[serde(default = "default_max_bounces")]
    pub max_bounces: u32,
    /// turn cap for the verifier session
    #[serde(default = "default_verify_max_turns")]
    pub verify_max_turns: u32,
}
fn default_verify() -> String { "haiku".into() }
fn default_max_bounces() -> u32 { 1 }
fn default_verify_max_turns() -> u32 { 8 }
impl Default for CloseGate {
    fn default() -> Self {
        Self {
            verify: default_verify(),
            requires_children: false,
            requires_commit: false,
            max_bounces: default_max_bounces(),
            verify_max_turns: default_verify_max_turns(),
        }
    }
}

/// Resolve the effective close gate: agent-def defaults, then the project's
/// per-agent override merged key-by-key (an override may set just one key).
pub fn close_gate_for(agent_def: &serde_json::Value, project_override: &serde_json::Value) -> CloseGate {
    let mut merged = agent_def.get("closegate").cloned().unwrap_or(serde_json::json!({}));
    if !merged.is_object() {
        merged = serde_json::json!({});
    }
    if let Some(ovr) = project_override.get("closegate").and_then(|v| v.as_object()) {
        for (k, v) in ovr {
            merged[k] = v.clone();
        }
    }
    serde_json::from_value(merged).unwrap_or_default()
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Account {
    pub name: String,
    pub token_envar: String,
    #[serde(default)]
    pub order: i64,
    /// switch to next account at this 5hr/7d usage % (first pass)
    #[serde(default)]
    pub switch: u8,
    /// hard stop for this account at this usage % (second pass)
    #[serde(default)]
    pub stop: u8,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct FailurePolicy {
    #[serde(default = "default_maxattempts")]
    pub maxattempts: u32,
    #[serde(default = "default_first_retry")]
    pub first_retry_second: u64,
    #[serde(default = "default_backoff")]
    pub retry_backoff_exponent: u32,
}
fn default_maxattempts() -> u32 { 5 }
fn default_first_retry() -> u64 { 10 }
fn default_backoff() -> u32 { 2 }

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Project {
    pub name: String,
    #[serde(default)]
    pub desc: String,
    /// Aspirational: Running | Draining | Stopped
    #[serde(default = "default_running")]
    pub state: String,
    #[serde(default)]
    pub gitrepo: String,
    /// ordered usage-gates, e.g. {">98%": 0, "else": 4}
    #[serde(default, deserialize_with = "null_is_default")]
    pub maxagents: BTreeMap<String, u32>,
    /// null/absent = unlimited; 0 = spend nothing; >0 = $/day cap
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub maxdailycost: Option<f64>,
    /// per-project agent overrides keyed by agent name
    #[serde(default, deserialize_with = "null_is_default")]
    pub agents: BTreeMap<String, serde_json::Value>,
    #[serde(default, deserialize_with = "null_is_default")]
    pub failure: FailurePolicy,
    /// links to iter3_engine records by name
    #[serde(default, deserialize_with = "null_is_default")]
    pub engines: Vec<String>,
    #[serde(default, deserialize_with = "null_is_default")]
    pub accounts: Vec<Account>,
    /// the project head file (structureV2): its frontmatter names the global
    /// context files, interface/usecase dirs and scan dirs the engine surfaces
    /// to every agent. Default "{topdir}/main.iter.md".
    #[serde(default = "default_mainfile")]
    pub mainfile: String,
    /// context patterns every NEW item inherits when it names none
    /// ({marker} = nearest *.iter.md above the codepath, {ancestor_markers} =
    /// the markers of every ancestor directory up to topdir)
    #[serde(default = "default_context", deserialize_with = "null_is_default_context")]
    pub default_context: Vec<String>,
    /// decided 2026-09-08: a non-green testgroup run files a `code` item to
    /// investigate and fix (a group's own `auto_fix` flag overrides per group)
    #[serde(default)]
    pub fix_on_test_failure: bool,
    /// decided 2026-09-08: how many queued neighbours (same lockdirs, same
    /// usecase) one claude session may take on after its item closes complete;
    /// 0 or 1 = never chain
    #[serde(default = "default_session_chain_max")]
    pub session_chain_max: u32,
    /// cluster-restart block (built 2026-09-09, see `cluster`): the nightly
    /// restart template and the window an item tagged
    /// `blocked-by-cluster-restart` waits out
    #[serde(default, deserialize_with = "null_is_default")]
    pub cluster_restart: cluster::ClusterRestart,
    /// decided 2026-09-09: tags the webui's "Add tag…" picker offers FIRST,
    /// before the tags already in use on the project, so a project can name
    /// the tags its agents and scripts react to (pdy-dev:
    /// ["blocked-by-cluster-restart","blocked-until-cluster-restart"]) without
    /// every project inheriting them. Text only; a pinned tag that is also in
    /// use keeps that colour. Never an engine-owned `blocked by: ` tag.
    #[serde(default, deserialize_with = "null_is_default")]
    pub pinned_tags: Vec<String>,
    /// repeat detection (built 2026-09-10, see `dedup`): `repeated_threshold`
    /// = the `repeats` count at which a survivor gets the `repeated` tag
    #[serde(default, deserialize_with = "null_is_default")]
    pub dedup: dedup::DedupConfig,
    /// 100 once the 0–99 priority migration ran on this project (absent =
    /// still on the 0–10 scale); read by the migration endpoint only
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub priority_scale: Option<u32>,
}
fn default_session_chain_max() -> u32 { 3 }
fn default_mainfile() -> String { "{topdir}/main.iter.md".into() }
fn default_context() -> Vec<String> { vec!["{marker}".into(), "{ancestor_markers}".into()] }

/// Agent tooling (decided 2026-09-04): the text V2 kept in .iter/ beside the
/// agents, now central so every engine assembles the same prompt.
///   kind = shared     -> appended to every agent prompt (V2 _shared.md)
///          capability -> indexed in every prompt; full text via `iter capability <name>`
///          source     -> "source instructions" by requester: user | agent | error
///          prepost    -> a prose pre/postwork step run as its own agent turn
///          critic     -> the `iter critreview` persona (model/flags in the row)
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AgentTooling {
    pub name: String,
    #[serde(default)]
    pub kind: String,
    #[serde(default)]
    pub desc: String,
    #[serde(default)]
    pub body: String,
    #[serde(default)]
    pub model: String,
    #[serde(default)]
    pub flags: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeoutsec: Option<u64>,
}
pub const TOOLING_KINDS: &[&str] = &["shared", "capability", "source", "prepost", "critic"];
fn default_running() -> String { "Running".into() }

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct EngineProjectDirs {
    #[serde(default)]
    pub dirs: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Engine {
    pub name: String,
    #[serde(default)]
    pub host: String,
    /// Actual: Running | Draining | Stopped
    #[serde(default = "default_stopped")]
    pub state: String,
    #[serde(default)]
    pub last_seen: String,
    #[serde(default = "default_ticksec")]
    pub ticksec: u64,
    /// unconditional full reload cadence — the seq fallback
    #[serde(default = "default_full_refresh")]
    pub full_refresh_minutes: u64,
    /// Claude account currently in use (visible to other engines)
    #[serde(default)]
    pub account: String,
    #[serde(default)]
    pub queuelock: BTreeMap<String, u64>,
    /// per-project machine paths, keyed by project name
    #[serde(default)]
    pub projects: BTreeMap<String, EngineProjectDirs>,
    /// engine-owned: latest usage snapshot for the active account
    /// {account, five_hour_pct, seven_day_pct, ..., ts} (heartbeat)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<serde_json::Value>,
    /// webui-owned: ISO ts of a pending connectivity test ("" = none)
    #[serde(default)]
    pub test_requested: String,
    /// engine-owned: outcome of the last connectivity test
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub test_result: Option<serde_json::Value>,
    /// idle usage probe: when the active account's snapshot is older than this
    /// many minutes and nothing is running, nudge haiku once (0 = off)
    #[serde(default = "default_probe_stale_min")]
    pub probe_stale_min: u64,
}
fn default_probe_stale_min() -> u64 { 20 }

/// Failure backoff (V2 retry_backoff_sec, now per project): seconds to wait
/// before attempt `attempt + 1` = first_retry_second * exponent^(attempt-1).
pub fn retry_delay_sec(policy: &FailurePolicy, attempt: u32) -> u64 {
    let base = policy.first_retry_second.max(1);
    let exp = policy.retry_backoff_exponent.max(1) as u64;
    let mut d = base;
    for _ in 1..attempt.max(1) {
        d = d.saturating_mul(exp).min(24 * 3600);
    }
    d
}
fn default_stopped() -> String { "Stopped".into() }
fn default_ticksec() -> u64 { 5 }
fn default_full_refresh() -> u64 { 360 }

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct WorkItemTs {
    #[serde(default)]
    pub receive: String,
    #[serde(default)]
    pub start: String,
    #[serde(default)]
    pub complete: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Tag {
    pub text: String,
    #[serde(default)]
    pub color: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct WorkItem {
    pub id: String,
    pub project: String,
    /// bumped by iter_data on every write; writers pass expect_version
    #[serde(default)]
    pub version: u64,
    #[serde(default)]
    pub name: String,
    #[serde(default = "default_queued")]
    pub state: String,
    /// agent name, or "exec" for shell items
    #[serde(default)]
    pub agent: String,
    /// for agent == "exec": the shell command to run
    #[serde(default)]
    pub exec_shell: String,
    /// 0–99, lower = sooner; P0 most urgent; default 40 (see PRIO_BAND_*);
    /// children inherit the creator's number exactly
    #[serde(default = "default_priority")]
    pub priority: i64,
    #[serde(default)]
    pub lockdirs: Vec<String>,
    #[serde(default)]
    pub createdby: String,
    #[serde(default)]
    pub requestedby: String,
    #[serde(default)]
    pub blockedby: Vec<String>,
    /// opt out of DEEP dependencies (workitem_dependency.md): by default a
    /// blocker is satisfied only when it AND everything it created (createdby,
    /// transitively) closed complete; shallow = the blocker alone
    #[serde(default)]
    pub blockedby_shallow: bool,
    /// engine-owned (2026-09-07): the running items whose central lock rows
    /// overlap this queued item's lockdirs — a dependency the engine knows
    /// about, written so the webui nests the waiter under the holder.  Set
    /// while the wait lasts, cleared the tick after the lock goes; never
    /// part of `dependency_status` (a lock ends with the holder's RUN, long
    /// before the deep rule would release it)
    #[serde(default)]
    pub blockedby_locks: Vec<String>,
    #[serde(default)]
    pub attempt: u32,
    /// close-gate bounces so far (spec: Close Gate); reset by a human requeue
    #[serde(default)]
    pub gate_bounces: u32,
    #[serde(default)]
    pub prework: Vec<String>,
    #[serde(default)]
    pub postwork: Vec<String>,
    #[serde(default)]
    pub ts: WorkItemTs,
    #[serde(default)]
    pub tags: Vec<Tag>,
    /// schedule spec — present only on "scheduled" templates (see sched module)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sched: Option<sched::Sched>,
    /// provenance: the template id this run was cloned from
    #[serde(default)]
    pub source_schedule: String,
    /// engine currently running it (set on pick)
    #[serde(default)]
    pub engine: String,
    #[serde(default)]
    pub lasterror: String,
    /// approval-workitem support: signed workid, verified by iter_data
    #[serde(default)]
    pub approval_code: String,
    #[serde(default)]
    pub needs_approval: bool,
    /// operator override (spec: Run Now): start on the next tick once deps
    /// and locks allow, even when the maxagents cap is full; cleared on claim
    #[serde(default)]
    pub run_now: bool,
    /// context file patterns surfaced to the agent (V2 `context`); empty =
    /// the project's default_context. {topdir} {codepath} {marker}
    /// {ancestor_markers} and globs resolve at run time.
    #[serde(default)]
    pub context: Vec<String>,
    /// per-item model override (opus | sonnet | haiku | fable); "" = agent default
    #[serde(default)]
    pub model: String,
    /// webui-owned: halt this item mid-run (workitem_stop.md); the engine kills
    /// the session, parks the item with a note, and clears the flag
    #[serde(default)]
    pub stop_requested: bool,
    /// engine-owned: not before this ISO ts (failure backoff); cleared on claim
    #[serde(default)]
    pub retry_after: String,
    /// webui-owned: ISO ts of a pending ELI5 request ("" = none). The engine
    /// runs the read-only `explain` agent on it immediately, outside the
    /// agent cap and the queue, appends an "explained" detail row, and
    /// clears the flag. Allowed on closed items.
    #[serde(default)]
    pub explain_requested: String,
    /// iter_data-owned: the one engine that runs this ELI5 — picked at random
    /// among live engines serving the project when the button is pressed,
    /// else claimed by the first engine to see the flag; cleared with it
    #[serde(default)]
    pub explain_engine: String,
    /// how many times this item's fault was reported again (see `dedup`):
    /// a stage-1 twin refused at create, or a duplicate merged into it
    #[serde(default)]
    pub repeats: u64,
    /// engine-owned: ISO ts of the stage-2 dedup triage (`dedup` module);
    /// "" = not yet — a queued item without it is held one tick for the judge
    #[serde(default)]
    pub dedup_checked: String,
}
fn default_queued() -> String { "queued".into() }
fn default_priority() -> i64 { PRIO_BAND_HUMAN.0 }

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct WorkItemDetail {
    pub id: String,
    pub order: i64,
    pub key: String,
    #[serde(default)]
    pub valuetype: String,
    #[serde(default)]
    pub value: serde_json::Value,
    /// provenance, stamped by iter_data on every write: JWT principal + UTC ts
    #[serde(default)]
    pub by: String,
    #[serde(default)]
    pub ts: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct PrePostWork {
    pub projectname: String,
    pub name: String,
    #[serde(default)]
    pub shell: String,
    #[serde(default = "default_ppw_timeout")]
    pub timeoutsec: u64,
    #[serde(default)]
    pub failhalt: bool,
}
fn default_ppw_timeout() -> u64 { 30 }

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct WebuiUser {
    pub user: String,
    #[serde(default)]
    pub email: String,
    /// user | engine | admin | viewer (read-only: no queue writes; may edit
    /// their own profile)
    #[serde(default = "default_role")]
    pub role: String,
    #[serde(default)]
    pub pwhash: String,
    #[serde(default = "default_tokenver")]
    pub tokenver: u64,
    #[serde(default)]
    pub css: String,
    /// IANA zone the webui renders every (UTC-stored) timestamp in for this
    /// user, e.g. "America/Los_Angeles"; "" = the browser's zone
    #[serde(default)]
    pub timezone: String,
    #[serde(default)]
    pub pubkey: String,
    #[serde(default)]
    pub settings: BTreeMap<String, serde_json::Value>,
    #[serde(default)]
    pub authz: BTreeMap<String, String>,
}
fn default_role() -> String { "user".into() }
fn default_tokenver() -> u64 { 1 }

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct VersionRow {
    pub projectname: String,
    pub table: String,
    #[serde(default)]
    pub seq: u64,
    #[serde(default)]
    pub updated: String,
}

/// kind: "lock" (held by a running workitem) | "reserve" (scope_reservation barrier)
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct LockRow {
    pub project: String,
    pub path: String,
    #[serde(default = "default_lock_kind")]
    pub kind: String,
    #[serde(default)]
    pub engine: String,
    #[serde(default)]
    pub workid: String,
    #[serde(default)]
    pub acquired: String,
    #[serde(default)]
    pub expires: String,
}
fn default_lock_kind() -> String { "lock".into() }

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ProjectStructure {
    pub projectname: String,
    #[serde(default)]
    pub hash: String,
    #[serde(default)]
    pub updated: String,
    #[serde(default)]
    pub snapshot: serde_json::Value,
}

/// Dependency gate (workitem_dependency.md, V2 semantics kept in V3).
#[derive(Debug, Clone, PartialEq)]
pub enum DepStatus {
    Satisfied,
    /// still waiting on this item (the blocker itself, or one of its descendants)
    Waiting(String),
    /// this blocker (or a descendant) closed failed: the dependent stays queued
    /// underneath it until the failed item is reopened and completes
    Failed(String),
}

/// creator id -> items it created (createdby)
pub fn children_index(items: &[WorkItem]) -> std::collections::HashMap<String, Vec<&WorkItem>> {
    let mut idx: std::collections::HashMap<String, Vec<&WorkItem>> = std::collections::HashMap::new();
    for i in items {
        if !i.createdby.is_empty() {
            idx.entry(i.createdby.clone()).or_default().push(i);
        }
    }
    idx
}

/// Deep by default: every blocker must be complete and so must every item it
/// created, transitively. Unknown ids (deleted) count as satisfied. Cycle-safe.
pub fn dependency_status(
    item: &WorkItem,
    by_id: &std::collections::HashMap<String, &WorkItem>,
    children: &std::collections::HashMap<String, Vec<&WorkItem>>,
) -> DepStatus {
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    for dep in &item.blockedby {
        let Some(d) = by_id.get(dep) else { continue };
        if d.state == "failed" {
            return DepStatus::Failed(d.id.clone());
        }
        if d.state != "complete" {
            return DepStatus::Waiting(d.id.clone());
        }
        if item.blockedby_shallow {
            continue;
        }
        let mut stack: Vec<&WorkItem> = children.get(dep).map(|v| v.clone()).unwrap_or_default();
        while let Some(c) = stack.pop() {
            if c.id == item.id || !seen.insert(c.id.clone()) {
                continue;
            }
            if c.state == "failed" {
                return DepStatus::Failed(c.id.clone());
            }
            if c.state != "complete" {
                return DepStatus::Waiting(c.id.clone());
            }
            if let Some(more) = children.get(&c.id) {
                stack.extend(more.iter().copied());
            }
        }
    }
    DepStatus::Satisfied
}

/// Two lock paths overlap when one is an ancestor of (or equal to) the other.
/// Paths are compared after trimming trailing slashes; `{topdir}` prefixes
/// compare literally, which is correct because both sides use the same token.
pub fn paths_overlap(a: &str, b: &str) -> bool {
    let a = a.trim_end_matches('/');
    let b = b.trim_end_matches('/');
    if a == b {
        return true;
    }
    let (shorter, longer) = if a.len() <= b.len() { (a, b) } else { (b, a) };
    longer.starts_with(shorter) && longer.as_bytes().get(shorter.len()) == Some(&b'/')
}

/// Account-ladder pick: exclusion-with-fallback (decided 2026-09-01).
/// `usage` maps account name -> max(5hr%, 7d%); missing = 0 (unknown usage
/// never blocks). `in_use` are accounts other Running engines currently hold.
/// Pass 1 uses `switch` thresholds, pass 2 `stop`; None = all accounts stopped.
pub fn pick_account<'a>(
    accounts: &'a [Account],
    usage: &BTreeMap<String, u8>,
    in_use: &[String],
) -> Option<&'a Account> {
    for threshold in ["switch", "stop"] {
        for exclusion_active in [true, false] {
            let mut sorted: Vec<&Account> = accounts
                .iter()
                .filter(|a| !exclusion_active || !in_use.contains(&a.name))
                .collect();
            sorted.sort_by_key(|a| a.order);
            for acct in sorted {
                let used = usage.get(&acct.name).copied().unwrap_or(0);
                let limit = if threshold == "switch" { acct.switch } else { acct.stop };
                if used < limit || limit == 0 {
                    return Some(acct);
                }
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn overlap_ancestor() {
        assert!(paths_overlap("{topdir}/src/", "{topdir}/src/deep/child/"));
        assert!(paths_overlap("{topdir}/src/deep/", "{topdir}/src/"));
        assert!(paths_overlap("{topdir}/src", "{topdir}/src/"));
        assert!(!paths_overlap("{topdir}/src/a/", "{topdir}/src/ab/"));
        assert!(!paths_overlap("{topdir}/a/", "{topdir}/b/"));
    }

    fn acct(name: &str, order: i64, switch: u8, stop: u8) -> Account {
        Account { name: name.into(), token_envar: format!("{}_TOKEN", name.to_uppercase()), order, switch, stop }
    }

    fn wi(id: &str, state: &str, createdby: &str, blockedby: &[&str]) -> WorkItem {
        WorkItem { id: id.into(), state: state.into(), createdby: createdby.into(),
            blockedby: blockedby.iter().map(|s| s.to_string()).collect(), ..Default::default() }
    }

    #[test]
    fn deep_dependencies_wait_for_descendants_and_park_on_failure() {
        let items = vec![
            wi("plan", "complete", "user", &[]),
            wi("child", "queued", "plan", &[]),
            wi("grandchild", "complete", "child", &[]),
            wi("dep", "queued", "user", &["plan"]),
        ];
        let by_id: std::collections::HashMap<String, &WorkItem> = items.iter().map(|i| (i.id.clone(), i)).collect();
        let kids = children_index(&items);
        assert_eq!(dependency_status(&items[3], &by_id, &kids), DepStatus::Waiting("child".into()));
        let mut shallow = items[3].clone();
        shallow.blockedby_shallow = true;
        assert_eq!(dependency_status(&shallow, &by_id, &kids), DepStatus::Satisfied);
        // descendant failed -> Failed; unknown blocker -> satisfied
        let mut failed = items.clone();
        failed[1].state = "failed".into();
        let by2: std::collections::HashMap<String, &WorkItem> = failed.iter().map(|i| (i.id.clone(), i)).collect();
        let kids2 = children_index(&failed);
        assert_eq!(dependency_status(&failed[3], &by2, &kids2), DepStatus::Failed("child".into()));
        let ghost = wi("g", "queued", "", &["nope"]);
        assert_eq!(dependency_status(&ghost, &by_id, &kids), DepStatus::Satisfied);
    }

    #[test]
    fn retry_backoff_grows() {
        let p = FailurePolicy { maxattempts: 5, first_retry_second: 10, retry_backoff_exponent: 2 };
        assert_eq!(retry_delay_sec(&p, 1), 10);
        assert_eq!(retry_delay_sec(&p, 2), 20);
        assert_eq!(retry_delay_sec(&p, 4), 80);
    }

    #[test]
    fn lock_patterns_match_equal_or_under_with_globs() {
        assert!(lock_pattern_matches("{topdir}/devops/plan", "{topdir}/devops/plan/", "tests"));
        assert!(lock_pattern_matches("{topdir}/devops/plan", "{topdir}/devops/plan/corridor", "tests"));
        assert!(!lock_pattern_matches("{topdir}/devops/plan", "{topdir}/devops", "tests"), "an ancestor is not under the pattern");
        assert!(!lock_pattern_matches("{topdir}/devops/plan", "{topdir}/devops/planner", "tests"));
        assert!(lock_pattern_matches("{topdir}/devops/deploy/*", "{topdir}/devops/deploy/corridor", "tests"));
        assert!(!lock_pattern_matches("{topdir}/devops/deploy/*", "{topdir}/devops/deploy", "tests"));
        assert!(lock_pattern_matches("{topdir}/**/{test_dir}", "{topdir}/core/repos/x/tests", "tests"));
    }

    /// The webui sends `null` for an emptied settings box; every defaulted
    /// object/list field must read that as its default, not refuse the save.
    #[test]
    fn project_settings_accept_null_as_default() {
        let p: Project = serde_json::from_value(serde_json::json!({
            "name": "x", "state": "Running",
            "dedup": null, "cluster_restart": null, "failure": null, "pinned_tags": null,
            "maxagents": null, "agents": null, "engines": null, "accounts": null, "default_context": null
        })).expect("null settings parse");
        assert_eq!(p.dedup, dedup::DedupConfig::default());
        assert_eq!(p.dedup.repeated_threshold, 3);
        assert!(p.pinned_tags.is_empty() && p.engines.is_empty() && p.maxagents.is_empty());
        assert_eq!(p.default_context, default_context(), "null default_context = the pattern list, not empty");
        let p: Project = serde_json::from_value(serde_json::json!({"name": "x", "state": "Running", "dedup": {"repeated_threshold": 5}})).unwrap();
        assert_eq!(p.dedup.repeated_threshold, 5);
    }

    #[test]
    fn unused_priority_is_lowest_free_in_band() {
        assert_eq!(pick_unused_priority(PRIO_BAND_USECASE, &[]), 10);
        assert_eq!(pick_unused_priority(PRIO_BAND_USECASE, &[10, 11, 13]), 12);
        assert_eq!(pick_unused_priority(PRIO_BAND_HUMAN, &(40..=49).collect::<Vec<_>>()), 40);
        let tags = vec![
            Tag { text: "usecase:signup".into(), color: "".into() },
            Tag { text: "other".into(), color: "".into() },
            Tag { text: "usecase:signup".into(), color: "".into() },
        ];
        assert_eq!(usecase_tags(&tags), vec!["usecase:signup".to_string()]);
        assert!(lock_pattern_matches("{topdir}/**/{test_dir}", "{topdir}/core/repos/x/tests/unit", "tests"));
        assert!(!lock_pattern_matches("{topdir}/**/{test_dir}", "{topdir}/core/repos/x", "tests"));
    }

    #[test]
    fn lockshape_refuses_outside_warns_on_overlap_and_merges_overrides() {
        let others: Vec<(String, Vec<String>)> = (0..11)
            .map(|n| (format!("i{n}"), vec![format!("{{topdir}}/devops/thing{n}/")]))
            .collect();
        let plan = LockShape { allow: vec!["{topdir}/devops/plan".into()], outside: "".into(), max_overlap: 0, none: false };
        let f = check_lockshape("plan", &plan, &["{topdir}/devops".to_string()], &others, "tests");
        assert_eq!(f.len(), 1);
        assert!(f[0].refuse);
        assert!(f[0].msg.contains("refused: codepath {topdir}/devops") && f[0].msg.contains("overlaps 11 open item(s)") && f[0].msg.contains("devops/plan"), "{}", f[0].msg);
        assert!(check_lockshape("plan", &plan, &["{topdir}/devops/plan/x".to_string()], &others, "tests").is_empty());
        // code: anything goes, but a wide lock warns with the count
        let code = LockShape { allow: vec![], outside: "".into(), max_overlap: 3, none: false };
        let f = check_lockshape("code", &code, &["{topdir}/devops".to_string()], &others, "tests");
        assert_eq!(f.len(), 1);
        assert!(!f[0].refuse && f[0].msg.contains("overlaps 11 open item(s)"));
        assert!(check_lockshape("code", &code, &["{topdir}/devops/thing1/".to_string()], &others, "tests").is_empty());
        // warn-only shape
        let soft = LockShape { allow: vec!["{topdir}/a".into()], outside: "warn".into(), max_overlap: 0, none: false };
        assert!(!check_lockshape("x", &soft, &["{topdir}/b".to_string()], &[], "tests")[0].refuse);
        // explain: no lock at all
        let none = LockShape { none: true, ..Default::default() };
        assert!(check_lockshape("explain", &none, &["{topdir}/x".to_string()], &[], "tests")[0].refuse);
        assert!(check_lockshape("explain", &none, &[], &[], "tests").is_empty());
        // merge: agent default + project override key by key; absent everywhere -> None
        let def = serde_json::json!({"lockshape": {"allow": ["{topdir}/devops/plan"]}});
        let ovr = serde_json::json!({"lockshape": {"outside": "warn"}});
        let s = lockshape_for(&def, &ovr).unwrap();
        assert_eq!(s.allow, vec!["{topdir}/devops/plan".to_string()]);
        assert!(!s.refuses_outside());
        assert!(lockshape_for(&serde_json::json!({}), &serde_json::Value::Null).is_none());
        assert!(lockshape_for(&serde_json::json!({}), &ovr).is_some(), "a project override alone defines a shape");
    }

    #[test]
    fn close_gate_merges_override_keywise() {
        let def = serde_json::json!({"closegate": {"verify": "sonnet", "requires_children": true}});
        let ovr = serde_json::json!({"closegate": {"max_bounces": 2}});
        let g = close_gate_for(&def, &ovr);
        assert_eq!(g.verify, "sonnet");
        assert!(g.requires_children);
        assert_eq!(g.max_bounces, 2);
        // absent everywhere -> defaults (haiku, one bounce)
        let g = close_gate_for(&serde_json::json!({}), &serde_json::Value::Null);
        assert_eq!(g, CloseGate::default());
        assert_eq!(g.verify, "haiku");
        // override can disable the verifier
        let g = close_gate_for(&def, &serde_json::json!({"closegate": {"verify": ""}}));
        assert_eq!(g.verify, "");
    }

    #[test]
    fn ladder_prefers_unused_account() {
        let accounts = vec![acct("Dev1", 1, 80, 99), acct("Dev2", 2, 80, 99)];
        let usage = BTreeMap::new();
        let picked = pick_account(&accounts, &usage, &["Dev1".into()]).unwrap();
        assert_eq!(picked.name, "Dev2");
    }

    #[test]
    fn ladder_falls_back_to_shared_when_exclusion_empties() {
        let accounts = vec![acct("Dev1", 1, 80, 99)];
        let usage = BTreeMap::new();
        let picked = pick_account(&accounts, &usage, &["Dev1".into()]).unwrap();
        assert_eq!(picked.name, "Dev1");
    }

    #[test]
    fn ladder_switch_then_stop_then_none() {
        let accounts = vec![acct("Dev1", 1, 80, 99), acct("Dev2", 2, 80, 99)];
        let mut usage = BTreeMap::new();
        usage.insert("Dev1".to_string(), 85u8);
        // Dev1 over switch, Dev2 under: pick Dev2
        assert_eq!(pick_account(&accounts, &usage, &[]).unwrap().name, "Dev2");
        usage.insert("Dev2".to_string(), 90u8);
        // both over switch, both under stop: pass 2 picks Dev1 (order)
        assert_eq!(pick_account(&accounts, &usage, &[]).unwrap().name, "Dev1");
        usage.insert("Dev1".to_string(), 99u8);
        usage.insert("Dev2".to_string(), 99u8);
        // both at stop: nothing
        assert!(pick_account(&accounts, &usage, &[]).is_none());
    }
}
