//! Server-side get_next (iter5 spec §4.3): `POST /api/projects/{p}/next`.
//!
//! The selection rules are the iter4 engine's `dispatch` pick, moved here so
//! every engine of a multi-project machine asks the server for one specific
//! project:
//! - only `queued` items, not `needs_approval`, past `retry_after`, not held
//!   by the cluster-restart tag while the cluster is unavailable, not waiting
//!   for their stage-2 dedup triage, with `dependency_status == Satisfied`
//!   (the deep rule, cycles and sibling tie-break included);
//! - the agent must be enabled on the project (an active `runs` edge) when an
//!   agent record exists for it, and in `agents_allowed` when the engine sends
//!   one (shell items — `exec`, test runs — are outside the agent caps and are
//!   not filtered by it);
//! - order: test-sweep runs first (they run outside the cap), then `run_now`
//!   items, then (priority, receive time);
//! - an item whose lockdirs overlap a LIVE lock row is skipped; the first such
//!   item reserves its paths (`reserve` rows, 600 s) so new overlapping work
//!   is not admitted; an item overlapping another queued item's reservation
//!   needs a strictly better (lower) priority than the reserver.
//!
//! The claim is the engine's: one versioned write (in-progress, engine,
//! attempt+1, a new lease, start ts, run_now/retry_after/blockedby_locks
//! cleared, claim tags), then a lock row per lockdir under that lease. If a
//! lock cannot be taken the claim is rolled back (rows released, the item's
//! previous record restored) and the next candidate is tried.
//!
//! Engine-side caps (max agents, budget, account choice) stay in the engine,
//! which calls this only when it may start something.

use crate::api::{ApiError, AppState, AuthUser, NOSK, forbidden, iso_in, notfound};
use crate::storage::{StorageError, body_str};
use axum::extract::{Path, State};
use axum::Json;
use iter_core::cluster::{self, CLUSTER_RESTART_TAG};
use iter_core::{DepStatus, LockRow, WorkItem, children_index, claim_tags, dependency_status, now_utc, paths_overlap};
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

/// How long a scope reservation lives (engine iter4: 600 s).
const RESERVE_TTL_SEC: i64 = 600;

#[derive(serde::Deserialize, Default)]
pub struct NextReq {
    pub engine: String,
    /// lock-row lifetime for this run; 0 = LOCK_LEASE_TTL_SEC (also the cap)
    #[serde(default)]
    pub lease_ttl_sec: i64,
    /// agent types the engine still has room for (absent = any)
    #[serde(default)]
    pub agents_allowed: Option<Vec<String>>,
    /// only 1 is supported
    #[serde(default)]
    #[allow(dead_code)]
    pub max: Option<u32>,
}

/// Stage-2 dedup triage hold (the engine's `dedup::needs_triage`): a newly
/// created item is judged against its neighbours before its first dispatch.
pub fn needs_triage(i: &WorkItem) -> bool {
    i.state == "queued"
        && i.dedup_checked.is_empty()
        && i.attempt == 0
        && i.source_schedule.is_empty()
        && !i.is_shell()
        && i.ts.receive.as_str() >= iter_core::dedup::DEDUP_EPOCH
}

/// What the pick found before any claim.
#[derive(Debug, Default)]
pub struct Selection<'a> {
    /// claimable now, in order
    pub ready: Vec<&'a WorkItem>,
    /// the best dispatchable-but-scope-blocked item: it reserves its paths
    pub reserver: Option<&'a WorkItem>,
    pub any_queued: bool,
    /// some otherwise-ready item waits on a lock or a reservation
    pub any_locked: bool,
}

/// The pick, pure: everything but the claim.
pub fn select<'a>(
    items: &'a [WorkItem],
    lock_rows: &[LockRow],
    now_iso: &str,
    cluster_healthy: bool,
    agents_allowed: Option<&[String]>,
    disabled_agents: &HashSet<String>,
) -> Selection<'a> {
    let by_id: HashMap<String, &WorkItem> = items.iter().map(|i| (i.id.clone(), i)).collect();
    let kids = children_index(items);
    let live = iter_core::live_lock_rows(lock_rows, &by_id, now_iso);
    let mut sel = Selection { any_queued: items.iter().any(|i| i.state == "queued"), ..Default::default() };
    let mut queued: Vec<&WorkItem> = items
        .iter()
        .filter(|i| i.state == "queued" && !i.needs_approval)
        .filter(|i| i.retry_after.is_empty() || i.retry_after.as_str() <= now_iso)
        .filter(|i| !cluster::blocks(i, cluster_healthy))
        .filter(|i| !needs_triage(i))
        .filter(|i| !disabled_agents.contains(&i.agent))
        .filter(|i| i.is_shell() || i.is_test_sweep_run() || agents_allowed.map(|a| a.contains(&i.agent)).unwrap_or(true))
        .filter(|i| dependency_status(i, &by_id, &kids) == DepStatus::Satisfied)
        .collect();
    queued.sort_by(|a, b| {
        let key = |i: &WorkItem| (!i.is_test_sweep_run(), !i.run_now, i.priority, i.ts.receive.clone(), i.id.clone());
        key(a).cmp(&key(b))
    });
    let scope_blocked = |i: &WorkItem| !iter_core::lock_holders(i, &live).is_empty();
    sel.reserver = queued.iter().copied().find(|i| scope_blocked(i));
    // live reservations held by a still-queued item: (path, priority, holder)
    let reserved: Vec<(String, i64, String)> = lock_rows
        .iter()
        .filter(|r| r.kind == "reserve" && (r.expires.is_empty() || r.expires.as_str() >= now_iso))
        .filter_map(|r| by_id.get(&r.workid).filter(|i| i.state == "queued").map(|i| (r.path.clone(), i.priority, r.workid.clone())))
        .chain(sel.reserver.iter().flat_map(|r| r.lockdirs.iter().map(|d| (d.clone(), r.priority, r.id.clone()))))
        .collect();
    for item in queued {
        if scope_blocked(item) {
            sel.any_locked = true;
            continue;
        }
        let gated = item.lockdirs.iter().any(|d| {
            reserved.iter().any(|(path, rprio, rid)| rid != &item.id && paths_overlap(d, path) && item.priority >= *rprio)
        });
        if gated {
            sel.any_locked = true;
            continue;
        }
        sel.ready.push(item);
    }
    sel
}

fn not_served(detail: String) -> Json<Value> {
    Json(json!({"item": null, "reason": "not-served", "detail": detail}))
}

pub async fn get_next(user: AuthUser, State(st): State<Arc<AppState>>, Path(name): Path<String>, Json(req): Json<NextReq>) -> Result<Json<Value>, ApiError> {
    user.require_writer()?;
    let store = st.store.as_ref();
    if req.engine.trim().is_empty() {
        return Err(crate::api::bad("engine required"));
    }
    let edges = crate::settings::load_edges(store).await?;
    // an engine token may only ask for its own engines
    if user.role == "engine" {
        let mine = crate::settings::engines_of_user(store, &edges, &user.sub).await?;
        if !mine.contains(&req.engine) {
            return Err(forbidden());
        }
    }
    let Some(serves) = crate::settings::serves_edge(&edges, &req.engine, &name) else {
        return Ok(not_served(format!("engine '{}' has no active serves edge to '{name}'", req.engine)));
    };
    if serves.setting("read_only").and_then(|v| v.as_bool()).unwrap_or(false) {
        return Ok(not_served(format!("engine '{}' serves '{name}' read-only", req.engine)));
    }
    let project = store.get("project", &name, NOSK).await?.ok_or_else(notfound)?;
    let pstate = Some(body_str(&project, "state")).filter(|s| !s.is_empty()).unwrap_or_else(|| "Running".into());
    if pstate != "Running" {
        return Ok(Json(json!({"item": null, "reason": "project-stopped", "detail": format!("project state {pstate}")})));
    }
    let raw: Vec<Value> = store.query("workitem", &name).await?;
    let raw_by_id: HashMap<String, Value> = raw.iter().map(|r| (body_str(r, "id"), r.clone())).collect();
    let items: Vec<WorkItem> = raw.into_iter().filter_map(|r| serde_json::from_value(r).ok()).collect();
    let now_iso = now_utc();
    // the lock sweep first (the engine ran it once a minute): a row that does
    // not count would otherwise make the claim's acquire fail
    let mut lock_rows: Vec<LockRow> = Vec::new();
    {
        let by_id: HashMap<String, &WorkItem> = items.iter().map(|i| (i.id.clone(), i)).collect();
        let mut swept = false;
        for r in store.query("lock", &name).await? {
            let Ok(lr) = serde_json::from_value::<LockRow>(r) else { continue };
            if iter_core::waitgraph::sweep_reason(&lr, by_id.get(&lr.workid).copied(), &now_iso).is_some() {
                store.delete("lock", &name, &iter_core::lock_sk(&lr.kind, &lr.path)).await?;
                swept = true;
                continue;
            }
            lock_rows.push(lr);
        }
        if swept {
            store.bump_seq(&name, "lock").await?;
        }
    }

    // cluster-restart block: one health verdict, only when a queued item carries the tag
    let cluster_healthy = if items.iter().any(|i| i.state == "queued" && cluster::has_tag(&i.tags, CLUSTER_RESTART_TAG)) {
        let cfg: iter_core::Project = serde_json::from_value(project.clone()).unwrap_or_default();
        let clone = cluster::newest_clone(&cfg.cluster_restart, &items);
        let details = match clone {
            Some(c) => store.query("workitem_detail", &c.id).await?,
            None => vec![],
        };
        cluster::evaluate(&cfg.cluster_restart, clone, &details, chrono::Utc::now()).healthy
    } else {
        true
    };

    // agents with a record but no active runs edge are disabled on this project
    let enabled = crate::settings::agents_enabled(&edges, &name);
    let disabled: HashSet<String> = store
        .scan("agent")
        .await?
        .iter()
        .map(|a| body_str(a, "name"))
        .filter(|a| !a.is_empty() && !enabled.contains_key(a))
        .collect();

    let sel = select(&items, &lock_rows, &now_iso, cluster_healthy, req.agents_allowed.as_deref(), &disabled);
    if let Some(r) = sel.reserver {
        reserve(&st, &name, &req.engine, r).await?;
    }
    let ttl = if req.lease_ttl_sec > 0 { req.lease_ttl_sec.min(iter_core::LOCK_LEASE_TTL_SEC) } else { iter_core::LOCK_LEASE_TTL_SEC };
    let mut lost_lock = false;
    for item in &sel.ready {
        let Some(orig) = raw_by_id.get(&item.id) else { continue };
        match claim(&st, &name, &req.engine, item, orig, ttl).await? {
            Claim::Won(v) => return Ok(Json(json!({"item": v, "reason": null}))),
            Claim::Lost => continue,
            Claim::Locked => {
                lost_lock = true;
                continue;
            }
        }
    }
    let reason = if !sel.any_queued {
        "none-queued"
    } else if sel.any_locked || lost_lock {
        "locked"
    } else {
        "blocked"
    };
    Ok(Json(json!({"item": null, "reason": reason})))
}

/// The scope reservation the iter4 engine wrote: best scope-blocked item
/// reserves its paths. A refusal (already reserved by someone) is fine.
async fn reserve(st: &Arc<AppState>, project: &str, engine: &str, r: &WorkItem) -> Result<(), StorageError> {
    let now = now_utc();
    for d in &r.lockdirs {
        let row = LockRow {
            project: project.into(),
            path: d.clone(),
            kind: "reserve".into(),
            engine: engine.into(),
            workid: r.id.clone(),
            acquired: now.clone(),
            expires: iso_in(RESERVE_TTL_SEC),
            lease: String::new(),
        };
        let body = serde_json::to_value(&row).unwrap();
        match st.store.acquire_lock("lock", project, &iter_core::lock_sk("reserve", d), &body, &now, &r.id).await {
            Ok(()) | Err(StorageError::Conflict(_)) => {}
            Err(e) => return Err(e),
        }
    }
    if !r.lockdirs.is_empty() {
        st.store.bump_seq(project, "lock").await?;
    }
    Ok(())
}

pub(crate) enum Claim {
    Won(Value),
    /// someone else wrote the item first
    Lost,
    /// claimed, but a lock was taken meanwhile: rolled back
    Locked,
}

pub(crate) async fn claim(st: &Arc<AppState>, project: &str, engine: &str, item: &WorkItem, orig: &Value, ttl: i64) -> Result<Claim, ApiError> {
    let store = st.store.as_ref();
    let now = now_utc();
    let lease = uuid::Uuid::new_v4().to_string();
    let mut claimed = orig.clone();
    claimed["state"] = json!("in-progress");
    claimed["run_now"] = json!(false);
    claimed["retry_after"] = json!("");
    claimed["blockedby_locks"] = json!([]);
    claimed["tags"] = json!(claim_tags(&item.tags));
    claimed["engine"] = json!(engine);
    claimed["attempt"] = json!(item.attempt + 1);
    if !claimed.get("ts").map(|t| t.is_object()).unwrap_or(false) {
        claimed["ts"] = json!({});
    }
    claimed["ts"]["start"] = json!(now);
    claimed["lease"] = json!(lease);
    claimed["version"] = json!(item.version + 1);
    match store.put_versioned("workitem", project, &item.id, &claimed, item.version).await {
        Ok(()) => {}
        Err(StorageError::Conflict(_)) => return Ok(Claim::Lost),
        Err(e) => return Err(e.into()),
    }
    store.bump_seq(project, "workitem").await?;
    // locks under the run's lease
    let mut taken = false;
    for d in &item.lockdirs {
        let row = LockRow {
            project: project.into(),
            path: d.clone(),
            kind: "lock".into(),
            engine: engine.into(),
            workid: item.id.clone(),
            acquired: now.clone(),
            expires: iso_in(ttl),
            lease: lease.clone(),
        };
        let body = serde_json::to_value(&row).unwrap();
        match store.acquire_lock("lock", project, &iter_core::lock_sk("lock", d), &body, &now, &item.id).await {
            Ok(()) => taken = true,
            Err(StorageError::Conflict(_)) => {
                rollback(st, project, item, orig, &lease).await?;
                return Ok(Claim::Locked);
            }
            Err(e) => {
                rollback(st, project, item, orig, &lease).await?;
                return Err(e.into());
            }
        }
    }
    if taken {
        store.bump_seq(project, "lock").await?;
    }
    Ok(Claim::Won(claimed))
}

/// Undo a claim: release the lease's rows, restore the previous record (one
/// version later). A conflict here means someone already moved the item on.
async fn rollback(st: &Arc<AppState>, project: &str, item: &WorkItem, orig: &Value, lease: &str) -> Result<(), ApiError> {
    crate::api::release_rows(st, project, |r| r.workid == item.id && r.lease == lease).await?;
    let mut back = orig.clone();
    back["version"] = json!(item.version + 2);
    match st.store.put_versioned("workitem", project, &item.id, &back, item.version + 1).await {
        Ok(()) | Err(StorageError::Conflict(_)) => {}
        Err(e) => return Err(e.into()),
    }
    st.store.bump_seq(project, "workitem").await?;
    Ok(())
}
