//! Cluster-restart block (built 2026-09-09; plan:
//! `iter3/plans/cluster_restart_block.buildplan.md`).
//!
//! The statement this module makes true: *if you require the cluster for
//! your work and it is between 02:00 and 06:00 PT, tag yourself as "blocked
//! by: cluster restart" and exit without incrementing your attempt; the
//! engine will block your work item from restarting until the cluster is
//! back up and healthy; once it is, you are no longer blocked; when your
//! work item goes from queued to in-progress the tag is stripped.*
//!
//! Two tags, on purpose.  The DURABLE tag the agent (or a human) sets is
//! `blocked-by-cluster-restart` — outside the engine-owned `blocked by: `
//! prefix, so `reconcile_waits` never deletes it.  The DISPLAYED tag,
//! `blocked by: cluster restart`, is derived by the engine every tick from
//! the durable tag plus the cluster's health, exactly like every other wait
//! reason.  The claim that moves the item to in-progress strips both
//! (`claim_tags`).

use crate::{Tag, WorkItem};
use chrono::{DateTime, Duration, NaiveTime, Utc};
use chrono_tz::Tz;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// The durable, agent-owned tag.  Never starts with `BLOCKED_TAG_PREFIX`.
pub const CLUSTER_RESTART_TAG: &str = "blocked-by-cluster-restart";
pub const CLUSTER_RESTART_TAG_COLOR: &str = "#c47a1f";
/// The wait reason the engine derives from it; renders as the engine-owned
/// tag `blocked by: cluster restart`.
pub const CLUSTER_RESTART_REASON: &str = "cluster restart";
/// `lasterror` the block verb writes — the one field the next run reads back
/// (the prompt's "Previous attempt" section).
pub const CLUSTER_RESTART_LASTERROR_PREFIX: &str = "blocked by: cluster restart — ";
/// Detail-row key the restart window posts on its own clone:
/// `{"cluster":"corridor-dev1","bringup_exit":N,"status_exit":M,"at":"<ISO>"}`.
pub const CLUSTERHEALTH_KEY: &str = "clusterhealth";
/// A window older than this says nothing about the cluster now; the clock
/// rule takes over.
pub const HEALTH_MAX_AGE_SEC: i64 = 24 * 3600;

/// Per-project settings (`project.cluster_restart`).  All optional: with no
/// `schedule` the clock rule alone decides.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ClusterRestart {
    /// id of the scheduled TEMPLATE whose clones run the nightly restart
    /// window (pdy-dev: `9da12551-bb18-4adf-8fae-a4ca138fb317`); "" = no
    /// window is known, use the clock only
    #[serde(default)]
    pub schedule: String,
    /// IANA zone the window is stated in
    #[serde(default = "default_tz")]
    pub tz: String,
    /// window start "HH:MM" (inclusive)
    #[serde(default = "default_from")]
    pub from: String,
    /// window end "HH:MM" (exclusive)
    #[serde(default = "default_until")]
    pub until: String,
}
fn default_tz() -> String { "America/Los_Angeles".into() }
fn default_from() -> String { "02:00".into() }
fn default_until() -> String { "06:00".into() }
impl Default for ClusterRestart {
    fn default() -> Self {
        Self { schedule: String::new(), tz: default_tz(), from: default_from(), until: default_until() }
    }
}

/// The verdict and the one line that explains it (printed on change).
#[derive(Debug, Clone, PartialEq)]
pub struct Health {
    pub healthy: bool,
    pub why: String,
}

pub fn has_tag(tags: &[Tag], text: &str) -> bool {
    tags.iter().any(|t| t.text == text)
}

/// The tags an item keeps when a claim moves it queued -> in-progress: every
/// engine-owned `blocked by: …` tag and the durable cluster-restart tag come
/// off (the wait is over); everything else is untouched.  ONE home for the
/// rule (PDY-TECH-046): the dispatch claim and the session-chaining claim
/// both call it.
pub fn claim_tags(tags: &[Tag]) -> Vec<Tag> {
    tags.iter()
        .filter(|t| !t.text.starts_with(crate::BLOCKED_TAG_PREFIX) && t.text != CLUSTER_RESTART_TAG)
        .cloned()
        .collect()
}

/// Is this item held by the cluster-restart rule right now?
pub fn blocks(item: &WorkItem, cluster_healthy: bool) -> bool {
    !cluster_healthy && has_tag(&item.tags, CLUSTER_RESTART_TAG)
}

fn parse_hhmm(s: &str) -> Option<NaiveTime> {
    let (h, m) = s.trim().split_once(':')?;
    NaiveTime::from_hms_opt(h.parse().ok()?, m.parse().ok()?, 0)
}

fn tz_of(cfg: &ClusterRestart) -> Tz {
    cfg.tz.trim().parse().unwrap_or(chrono_tz::America::Los_Angeles)
}

/// Is `now` inside the configured window, by the wall clock in `cfg.tz`?
/// A window that wraps midnight ("23:00" to "01:00") is handled.
pub fn in_window(cfg: &ClusterRestart, now: DateTime<Utc>) -> bool {
    let (Some(from), Some(until)) = (parse_hhmm(&cfg.from), parse_hhmm(&cfg.until)) else { return false };
    let local = now.with_timezone(&tz_of(cfg)).time();
    if from <= until { local >= from && local < until } else { local >= from || local < until }
}

/// The newest clone of the restart template (by receive time), if any.
pub fn newest_clone<'a>(cfg: &ClusterRestart, items: &'a [WorkItem]) -> Option<&'a WorkItem> {
    if cfg.schedule.trim().is_empty() {
        return None;
    }
    items.iter().filter(|i| i.source_schedule == cfg.schedule).max_by(|a, b| a.ts.receive.cmp(&b.ts.receive))
}

/// The newest `clusterhealth` row's (bringup_exit, status_exit), if the row
/// exists.  A missing `bringup_exit` reads as 0; a missing `status_exit` is
/// reported as None (the row says nothing usable).
fn health_row(details: &[Value]) -> Option<(i64, Option<i64>)> {
    let row = details.iter().rev().find(|d| d.get("key").and_then(|k| k.as_str()) == Some(CLUSTERHEALTH_KEY))?;
    let value = match row.get("value") {
        Some(Value::String(s)) => serde_json::from_str::<Value>(s).unwrap_or(Value::Null),
        Some(v) => v.clone(),
        None => Value::Null,
    };
    Some((
        value.get("bringup_exit").and_then(|v| v.as_i64()).unwrap_or(0),
        value.get("status_exit").and_then(|v| v.as_i64()),
    ))
}

/// "Back up and healthy", concretely.  `clone` is the newest restart-window
/// clone (`newest_clone`) and `clone_details` its detail rows.
///
/// 1. A window that ran within the last 24 h decides: complete + a
///    `clusterhealth` row with both exit codes 0 = healthy; complete + a red
///    row = unhealthy (a red night stays blocked until the next green window,
///    or until 24 h pass); still open (the window is running now) = unhealthy;
///    failed = unhealthy.
/// 2. Otherwise — no clone, no clone in 24 h, or a complete clone that posted
///    no row — the clock in `cfg.tz` decides: inside `from`–`until` is
///    unhealthy, outside is healthy.
pub fn evaluate(cfg: &ClusterRestart, clone: Option<&WorkItem>, clone_details: &[Value], now: DateTime<Utc>) -> Health {
    let recent = |iso: &str| -> bool {
        crate::sched::parse_iso(iso).map(|t| now - t <= Duration::seconds(HEALTH_MAX_AGE_SEC)).unwrap_or(false)
    };
    let short = |id: &str| id[..8.min(id.len())].to_string();
    if let Some(c) = clone {
        if c.state == "complete" && recent(&c.ts.complete) {
            match health_row(clone_details) {
                Some((bring, Some(status))) if bring == 0 && status == 0 => {
                    return Health { healthy: true, why: format!("restart window {} closed complete at {} with clusterhealth exits 0/0", short(&c.id), c.ts.complete) };
                }
                Some((bring, status)) => {
                    return Health {
                        healthy: false,
                        why: format!(
                            "restart window {} at {} recorded a red status (bringup_exit {bring}, status_exit {}) — blocked until a green window or 24 h pass",
                            short(&c.id),
                            c.ts.complete,
                            status.map(|s| s.to_string()).unwrap_or_else(|| "missing".into())
                        ),
                    };
                }
                None => {} // no measurement: fall through to the clock
            }
        } else if c.state == "failed" && recent(&c.ts.receive) {
            return Health { healthy: false, why: format!("restart window {} failed (received {})", short(&c.id), c.ts.receive) };
        } else if crate::sched::is_open_state(&c.state) && recent(&c.ts.receive) {
            return Health { healthy: false, why: format!("restart window {} is {} (received {})", short(&c.id), c.state, c.ts.receive) };
        }
    }
    if in_window(cfg, now) {
        Health { healthy: false, why: format!("clock: inside the {}–{} {} restart window", cfg.from, cfg.until, cfg.tz) }
    } else {
        Health { healthy: true, why: format!("clock: outside the {}–{} {} restart window", cfg.from, cfg.until, cfg.tz) }
    }
}

/// What `iter block --cluster-restart` writes, as one versioned PUT on the
/// calling item: the durable tag (if absent), `state: parked`, the
/// `lasterror` the next run reads back, and the attempt counter put back
/// where the claim found it (the claim incremented it before the agent ran a
/// single turn — this is the whole of "exit without incrementing").
pub fn apply_block(item: &mut Value, reason: &str) {
    let mut tags: Vec<Value> = item.get("tags").and_then(|t| t.as_array()).cloned().unwrap_or_default();
    if !tags.iter().any(|t| t.get("text").and_then(|x| x.as_str()) == Some(CLUSTER_RESTART_TAG)) {
        tags.push(json!({"text": CLUSTER_RESTART_TAG, "color": CLUSTER_RESTART_TAG_COLOR}));
    }
    item["tags"] = json!(tags);
    item["state"] = json!("parked");
    item["lasterror"] = json!(format!("{CLUSTER_RESTART_LASTERROR_PREFIX}{}", reason.trim()));
    let attempt = item.get("attempt").and_then(|a| a.as_u64()).unwrap_or(0);
    item["attempt"] = json!(attempt.saturating_sub(1));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tag(text: &str) -> Tag {
        Tag { text: text.into(), color: String::new() }
    }

    /// Rule 1 (plan §5.1): the claim drops the durable tag and every engine
    /// `blocked by: ` tag and keeps everything else.
    #[test]
    fn claim_tags_drops_both_block_tags_and_keeps_the_rest() {
        let tags = vec![tag("usecase:chips"), tag(CLUSTER_RESTART_TAG), tag("blocked by: lock {topdir}/src"), tag("human-note")];
        let kept: Vec<String> = claim_tags(&tags).into_iter().map(|t| t.text).collect();
        assert_eq!(kept, vec!["usecase:chips".to_string(), "human-note".to_string()]);
    }

    /// Rule 2 (plan §5.2): a tagged item is held only while the cluster is
    /// unhealthy; an untagged item never is.
    #[test]
    fn tagged_item_is_held_only_while_unhealthy() {
        let tagged = WorkItem { state: "queued".into(), tags: vec![tag(CLUSTER_RESTART_TAG)], ..Default::default() };
        let plain = WorkItem { state: "queued".into(), ..Default::default() };
        assert!(blocks(&tagged, false));
        assert!(!blocks(&tagged, true));
        assert!(!blocks(&plain, false));
    }

    fn clone_at(state: &str, receive: &str, complete: &str) -> WorkItem {
        WorkItem {
            id: "9da12551-clone".into(),
            state: state.into(),
            source_schedule: "tpl".into(),
            ts: crate::WorkItemTs { receive: receive.into(), start: String::new(), complete: complete.into() },
            ..Default::default()
        }
    }
    fn cfg() -> ClusterRestart {
        ClusterRestart { schedule: "tpl".into(), ..Default::default() }
    }
    fn row(bringup: i64, status: i64) -> Vec<Value> {
        vec![json!({"key": "clusterhealth", "valuetype": "json", "value": {"cluster": "corridor-dev1", "bringup_exit": bringup, "status_exit": status}})]
    }
    fn at(iso: &str) -> DateTime<Utc> {
        crate::sched::parse_iso(iso).unwrap()
    }

    /// Rule 3 (plan §5.3): "closed complete" alone is not healthy — the
    /// clusterhealth row's status_exit decides.
    #[test]
    fn complete_window_with_red_status_is_unhealthy() {
        // 13:00Z = 06:00 PDT, outside the clock window: only the row can block
        let now = at("2026-09-09T13:00:00Z");
        let c = clone_at("complete", "2026-09-09T09:00:00Z", "2026-09-09T12:00:00Z");
        assert!(!evaluate(&cfg(), Some(&c), &row(0, 3), now).healthy);
        assert!(evaluate(&cfg(), Some(&c), &row(0, 0), now).healthy);
        assert!(!evaluate(&cfg(), Some(&c), &row(1, 0), now).healthy, "a failed bring-up is red too");
        // a row whose value arrived as a JSON string still parses
        let as_text = vec![json!({"key": "clusterhealth", "valuetype": "text", "value": "{\"status_exit\":3}"})];
        assert!(!evaluate(&cfg(), Some(&c), &as_text, now).healthy);
        // a window that is still running is unhealthy; one that failed is unhealthy
        assert!(!evaluate(&cfg(), Some(&clone_at("in-progress", "2026-09-09T09:00:00Z", "")), &[], now).healthy);
        assert!(!evaluate(&cfg(), Some(&clone_at("failed", "2026-09-09T09:00:00Z", "")), &[], now).healthy);
        // a complete window that posted no row says nothing: the clock decides (06:00 PDT = outside)
        assert!(evaluate(&cfg(), Some(&c), &[], now).healthy);
        // a red window older than 24 h no longer counts: the clock decides
        let old = clone_at("complete", "2026-09-07T09:00:00Z", "2026-09-07T12:00:00Z");
        assert!(evaluate(&cfg(), Some(&old), &row(0, 3), now).healthy);
    }

    /// Rule 4 (plan §5.4): with no window in 24 h the wall clock in
    /// America/Los_Angeles decides — 03:00 PDT is inside, 06:01 PDT is outside.
    /// (A UTC comparison, 7 hours out in September, would invert both.)
    #[test]
    fn clock_rule_uses_the_pacific_wall_clock() {
        let c = cfg();
        assert!(!evaluate(&c, None, &[], at("2026-09-09T10:00:00Z")).healthy, "03:00 PDT");
        assert!(evaluate(&c, None, &[], at("2026-09-09T13:01:00Z")).healthy, "06:01 PDT");
        assert!(!evaluate(&c, None, &[], at("2026-09-09T09:00:00Z")).healthy, "02:00 PDT (inclusive start)");
        assert!(evaluate(&c, None, &[], at("2026-09-09T13:00:00Z")).healthy, "06:00 PDT (exclusive end)");
        // the same instants in winter (PST): 10:00Z is 02:00 PST — still inside
        assert!(!evaluate(&c, None, &[], at("2026-01-09T10:00:00Z")).healthy);
        // a wrapped window and an explicit zone
        let wrap = ClusterRestart { tz: "UTC".into(), from: "23:00".into(), until: "01:00".into(), ..Default::default() };
        assert!(in_window(&wrap, at("2026-09-09T23:30:00Z")));
        assert!(in_window(&wrap, at("2026-09-09T00:30:00Z")));
        assert!(!in_window(&wrap, at("2026-09-09T12:00:00Z")));
    }

    /// Rule 5 (plan §5.5): the block leaves `attempt` at its pre-claim value
    /// (the claim added one; the block takes it back), parks the item, adds
    /// the durable tag once, and writes the lasterror the next run reads.
    #[test]
    fn block_puts_the_attempt_back_and_tags_once() {
        let mut item = json!({"state": "in-progress", "attempt": 3, "tags": [{"text": "usecase:chips", "color": "#3b6fb6"}], "lasterror": ""});
        apply_block(&mut item, "  need corridor-dev1 for the smoke test ");
        assert_eq!(item["attempt"], json!(2));
        assert_eq!(item["state"], json!("parked"));
        assert_eq!(item["lasterror"], json!("blocked by: cluster restart — need corridor-dev1 for the smoke test"));
        let texts: Vec<&str> = item["tags"].as_array().unwrap().iter().map(|t| t["text"].as_str().unwrap()).collect();
        assert_eq!(texts, vec!["usecase:chips", CLUSTER_RESTART_TAG]);
        // idempotent on the tag, floored at 0 on the counter
        apply_block(&mut item, "again");
        apply_block(&mut item, "again");
        assert_eq!(item["attempt"], json!(0));
        assert_eq!(item["tags"].as_array().unwrap().len(), 2);
        // the durable tag never collides with the engine-owned prefix
        assert!(!CLUSTER_RESTART_TAG.starts_with(crate::BLOCKED_TAG_PREFIX));
    }

    #[test]
    fn newest_clone_picks_the_latest_receive_of_the_configured_template() {
        let items = vec![
            clone_at("complete", "2026-09-08T09:00:00Z", "2026-09-08T12:00:00Z"),
            clone_at("complete", "2026-09-09T09:00:00Z", "2026-09-09T12:00:00Z"),
            WorkItem { source_schedule: "other".into(), ts: crate::WorkItemTs { receive: "2026-09-10T09:00:00Z".into(), ..Default::default() }, ..Default::default() },
        ];
        assert_eq!(newest_clone(&cfg(), &items).map(|c| c.ts.receive.as_str()), Some("2026-09-09T09:00:00Z"));
        assert!(newest_clone(&ClusterRestart::default(), &items).is_none(), "no schedule configured = no window known");
    }
}
