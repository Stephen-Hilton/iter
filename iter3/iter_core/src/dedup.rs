//! Repeat detection (built 2026-09-10, spec: iter3/plans/!iter_dedup_spec.md).
//!
//! Between 05:57Z and 07:23Z on 2026-09-10 one unchanged condition in pdy-dev
//! filed thirteen priority-1 items, nine of them word-for-word twins, because
//! nothing looked for an existing item before creating one.  This module holds
//! the rules both halves share:
//!
//! - **stage 1** (iter_data, at create, no model): an item whose tags carry
//!   both `check:<label>` and `container:<name>` is a repeat of any OPEN item
//!   with the same two tags — no second row, the survivor is told;
//! - **stage 2** (iter_engine, before dispatch, Sonnet): a new item is judged
//!   against a deterministically narrowed set of open neighbours and merged
//!   into one only on a confident `same_fault`.
//!
//! A repeat is evidence, never clutter: the survivor gets a `doc` row carrying
//! the repeat's request text, `repeats` + 1, its priority halved (lower =
//! sooner; floor 1) and the `repeated` tag once `repeats` reaches the
//! project's `dedup.repeated_threshold`.

use crate::{Tag, WorkItem, paths_overlap};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// The identity of a repeat: `check:<rule label>` + `container:<name>`.
pub const CHECK_TAG_PREFIX: &str = "check:";
pub const CONTAINER_TAG_PREFIX: &str = "container:";
/// On the closed duplicate: `dup of: <last 12 of the survivor id>` — the
/// working item is one click away from any list.
pub const DUP_OF_TAG_PREFIX: &str = "dup of: ";
/// On the survivor once `repeats` reaches the threshold.
pub const REPEATED_TAG: &str = "repeated";
pub const DEDUP_TAG_COLOR: &str = "#8e44ad";
/// Stage 2 candidate cap: newest first, at most this many.
pub const STAGE2_CAP: usize = 20;
/// The engine-internal agent type that judges stage 2 (run like `explain`).
pub const JUDGE_AGENT: &str = "dedup";
/// Items received before this were filed before the feature existed; the
/// engine never triages them ("do not touch existing items", spec 2026-09-10).
pub const DEDUP_EPOCH: &str = "2026-09-10T17:30:00Z";

/// Project setting `dedup` (webui project settings, JSON).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DedupConfig {
    /// `repeats` at which the survivor gets the `repeated` tag (default 3)
    #[serde(default = "default_threshold")]
    pub repeated_threshold: u32,
}
fn default_threshold() -> u32 { 3 }
impl Default for DedupConfig {
    fn default() -> Self { Self { repeated_threshold: default_threshold() } }
}

fn tag_with_prefix(tags: &[Tag], prefix: &str) -> Option<String> {
    tags.iter().find(|t| t.text.starts_with(prefix) && t.text.len() > prefix.len()).map(|t| t.text.clone())
}
fn json_tags(v: &Value) -> Vec<Tag> {
    v.get("tags").and_then(|t| serde_json::from_value(t.clone()).ok()).unwrap_or_default()
}

/// The stage-1 key (`check:` text, `container:` text) when BOTH are present.
pub fn key_of(tags: &[Tag]) -> Option<(String, String)> {
    Some((tag_with_prefix(tags, CHECK_TAG_PREFIX)?, tag_with_prefix(tags, CONTAINER_TAG_PREFIX)?))
}
/// Same, from a JSON row.
pub fn key_of_row(row: &Value) -> Option<(String, String)> {
    key_of(&json_tags(row))
}

/// Open = still someone's work: not closed, and not a schedule template.
pub fn is_open_row(row: &Value) -> bool {
    let s = row.get("state").and_then(|s| s.as_str()).unwrap_or("");
    is_open_state(s)
}
pub fn is_open_state(state: &str) -> bool {
    !matches!(state, "complete" | "failed" | "scheduled")
}

/// Halving rule (Stephen, 2026-09-10): smaller is more urgent, so a repeat
/// halves the number, rounding down, floor 1 — 77, 38, 19, 9, 4, 2, 1, 1.
/// P0 is already the front of the queue and stays 0.
pub fn halve_priority(p: i64) -> i64 {
    if p <= 1 { p } else { p / 2 }
}

/// What one observed repeat did to the survivor.
#[derive(Debug, Clone, PartialEq)]
pub struct RepeatOutcome {
    pub repeats: u64,
    pub priority_from: i64,
    pub priority_to: i64,
    /// the `repeated` tag was added by THIS repeat
    pub newly_repeated: bool,
}

/// Apply one repeat to a survivor's JSON row (record only — the caller
/// writes it and appends the "seen again" doc row): `repeats` + 1, priority
/// halved, `repeated` tag at the threshold.  Version is left to the writer.
pub fn apply_repeat(row: &mut Value, threshold: u32) -> RepeatOutcome {
    let repeats = row.get("repeats").and_then(|r| r.as_u64()).unwrap_or(0) + 1;
    row["repeats"] = json!(repeats);
    let from = row.get("priority").and_then(|p| p.as_i64()).unwrap_or(crate::PRIO_BAND_HUMAN.0);
    let to = halve_priority(from);
    row["priority"] = json!(to);
    let mut newly = false;
    if repeats >= threshold.max(1) as u64 {
        let mut tags: Vec<Value> = row.get("tags").and_then(|t| t.as_array()).cloned().unwrap_or_default();
        if !tags.iter().any(|t| t.get("text").and_then(|x| x.as_str()) == Some(REPEATED_TAG)) {
            tags.push(json!({"text": REPEATED_TAG, "color": DEDUP_TAG_COLOR}));
            newly = true;
        }
        row["tags"] = json!(tags);
    }
    RepeatOutcome { repeats, priority_from: from, priority_to: to, newly_repeated: newly }
}

/// The tag the closed duplicate carries.
pub fn dup_of_tag(survivor_id: &str) -> Tag {
    let n = survivor_id.len();
    Tag { text: format!("{DUP_OF_TAG_PREFIX}{}", &survivor_id[n.saturating_sub(12)..]), color: DEDUP_TAG_COLOR.into() }
}

/// The "seen again" doc row text on the survivor.  `via` names how the
/// repeat arrived (a create request, or a merged duplicate's id).
pub fn seen_again_note(ts: &str, source: &str, via: &str, request: &str) -> String {
    let mut s = format!("seen again {ts} by {}{}", if source.is_empty() { "unknown" } else { source }, if via.is_empty() { String::new() } else { format!(" ({via})") });
    if !request.trim().is_empty() {
        s.push_str(":\n\n");
        s.push_str(request.trim());
    }
    s
}

// ---------- stage 2 ----------

/// Narrow deterministically: open items in the same project (never the new
/// item itself, never a template) that share the new item's `container:`
/// tag, or its `check:` tag, or whose lockdirs overlap its own.  Newest
/// first, at most `cap`.
pub fn stage2_candidates<'a>(new: &WorkItem, items: &'a [WorkItem], cap: usize) -> Vec<&'a WorkItem> {
    let check = tag_with_prefix(&new.tags, CHECK_TAG_PREFIX);
    let container = tag_with_prefix(&new.tags, CONTAINER_TAG_PREFIX);
    let mut out: Vec<&WorkItem> = items
        .iter()
        .filter(|i| i.id != new.id && is_open_state(&i.state))
        .filter(|i| {
            let shares_tag = |want: &Option<String>| want.as_ref().map(|w| i.tags.iter().any(|t| &t.text == w)).unwrap_or(false);
            shares_tag(&container)
                || shares_tag(&check)
                || new.lockdirs.iter().any(|a| i.lockdirs.iter().any(|b| paths_overlap(a, b)))
        })
        .collect();
    out.sort_by(|a, b| b.ts.receive.cmp(&a.ts.receive).then_with(|| b.id.cmp(&a.id)));
    out.truncate(cap);
    out
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Verdict {
    SameFault,
    Different,
    Unsure,
}

/// One candidate's line from the judge.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Judgement {
    pub id: String,
    #[serde(default)]
    pub summary: String,
    pub verdict: Verdict,
    #[serde(default)]
    pub reason: String,
}

/// Parse the judge's output: a JSON object `{"candidates":[…]}` or a bare
/// array, optionally wrapped in a code fence or prose.  Unknown ids are
/// dropped by the caller; malformed output is an error (never a merge).
pub fn parse_judgements(text: &str) -> Result<Vec<Judgement>, String> {
    let t = text.trim();
    let start = t.find(|c| c == '{' || c == '[').ok_or("judge output holds no JSON")?;
    let end = t.rfind(|c| c == '}' || c == ']').ok_or("judge output holds no JSON")?;
    if end < start {
        return Err("judge output holds no JSON".into());
    }
    let v: Value = serde_json::from_str(&t[start..=end]).map_err(|e| format!("judge output is not JSON: {e}"))?;
    let arr = match &v {
        Value::Array(a) => a.clone(),
        Value::Object(o) => o.get("candidates").and_then(|c| c.as_array()).cloned().ok_or("judge output has no \"candidates\" array")?,
        _ => return Err("judge output is neither an object nor an array".into()),
    };
    arr.into_iter().map(|j| serde_json::from_value::<Judgement>(j).map_err(|e| format!("bad judgement: {e}"))).collect()
}

/// What the engine does with a verdict set.
#[derive(Debug, Clone, PartialEq)]
pub enum Decision {
    /// close `new` as a duplicate of `survivor`; `others` were judged the same
    /// fault too (noted on the survivor); `overlaps` are unsure notes to leave
    Merge { survivor: String, reason: String, others: Vec<String>, overlaps: Vec<(String, String)> },
    /// merge nothing; "may overlap with <id>: <reason>" on both sides
    Overlap(Vec<(String, String)>),
    /// dispatch as today
    Nothing,
}

/// Act only on confidence: `same_fault` on exactly one eligible candidate
/// merges into it; on several, into the oldest open one, the rest noted;
/// `unsure` leaves notes on both and merges nothing; `different` everywhere
/// changes nothing.  A survivor is never in-progress or closed, and a
/// duplicate is never in-progress — such a `same_fault` becomes an overlap
/// note instead.  Ids the judge invents are ignored.
pub fn decide(new: &WorkItem, candidates: &[&WorkItem], judgements: &[Judgement]) -> Decision {
    let mut same: Vec<(&WorkItem, String)> = Vec::new();
    let mut overlaps: Vec<(String, String)> = Vec::new();
    for j in judgements {
        let Some(c) = candidates.iter().find(|c| c.id == j.id) else { continue };
        match j.verdict {
            Verdict::Different => {}
            Verdict::Unsure => overlaps.push((c.id.clone(), j.reason.clone())),
            Verdict::SameFault => {
                if c.state == "in-progress" || !is_open_state(&c.state) || new.state == "in-progress" {
                    overlaps.push((c.id.clone(), format!("{} (judged the same fault, but {} is {}; nothing merged)", j.reason, if new.state == "in-progress" { "the new item" } else { "that item" }, if new.state == "in-progress" { "in progress" } else { &c.state })));
                } else {
                    same.push((c, j.reason.clone()));
                }
            }
        }
    }
    if same.is_empty() {
        return if overlaps.is_empty() { Decision::Nothing } else { Decision::Overlap(overlaps) };
    }
    // oldest open one survives (receive ts, then id for determinism)
    same.sort_by(|a, b| a.0.ts.receive.cmp(&b.0.ts.receive).then_with(|| a.0.id.cmp(&b.0.id)));
    let (survivor, reason) = same[0].clone();
    let others = same.iter().skip(1).map(|(c, _)| c.id.clone()).collect();
    Decision::Merge { survivor: survivor.id.clone(), reason, others, overlaps }
}

/// The overlap note left on both items.
pub fn overlap_note(other_id: &str, reason: &str) -> String {
    format!("may overlap with {other_id}: {}", if reason.trim().is_empty() { "the dedup judge was unsure" } else { reason.trim() })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tag(t: &str) -> Tag { Tag { text: t.into(), color: String::new() } }
    fn item(id: &str, state: &str, tags: &[&str], lockdirs: &[&str], receive: &str) -> WorkItem {
        WorkItem {
            id: id.into(), state: state.into(), tags: tags.iter().map(|t| tag(t)).collect(),
            lockdirs: lockdirs.iter().map(|s| s.to_string()).collect(),
            ts: crate::WorkItemTs { receive: receive.into(), ..Default::default() },
            ..Default::default()
        }
    }
    fn j(id: &str, v: Verdict, reason: &str) -> Judgement {
        Judgement { id: id.into(), summary: String::new(), verdict: v, reason: reason.into() }
    }

    #[test]
    fn key_needs_both_tags() {
        assert_eq!(key_of(&[tag("check:a"), tag("container:b")]), Some(("check:a".into(), "container:b".into())));
        assert_eq!(key_of(&[tag("check:a")]), None);
        assert_eq!(key_of(&[tag("container:b"), tag("check:")]), None); // empty label is no key
    }

    /// Case 5: 77 → 38 → 19 → 9 → 4 → 2 → 1 → 1; priority 1 stays 1.
    #[test]
    fn halving_rounds_down_floor_one() {
        let mut p = 77;
        let mut seen = vec![p];
        for _ in 0..7 {
            p = halve_priority(p);
            seen.push(p);
        }
        assert_eq!(seen, vec![77, 38, 19, 9, 4, 2, 1, 1]);
        assert_eq!(halve_priority(1), 1);
        assert_eq!(halve_priority(0), 0);
        let mut row = json!({"priority": 77, "tags": []});
        let out = apply_repeat(&mut row, 3);
        assert_eq!((out.priority_from, out.priority_to, out.repeats), (77, 38, 1));
        assert_eq!(row["priority"], json!(38));
        assert_eq!(row["repeats"], json!(1));
    }

    /// Case 6: the `repeated` tag appears at the threshold and not before.
    #[test]
    fn repeated_tag_at_threshold_only() {
        let mut row = json!({"priority": 50, "tags": [{"text":"container:x","color":""}]});
        let has = |r: &Value| r["tags"].as_array().unwrap().iter().any(|t| t["text"] == REPEATED_TAG);
        let o1 = apply_repeat(&mut row, 3);
        assert!(!has(&row) && !o1.newly_repeated);
        let o2 = apply_repeat(&mut row, 3);
        assert!(!has(&row) && !o2.newly_repeated);
        let o3 = apply_repeat(&mut row, 3);
        assert!(has(&row) && o3.newly_repeated && o3.repeats == 3);
        let o4 = apply_repeat(&mut row, 3);
        assert!(has(&row) && !o4.newly_repeated, "added once, never twice");
        assert_eq!(row["tags"].as_array().unwrap().len(), 2);
    }

    /// Case 7: narrowing — nothing shared is not a candidate; container,
    /// check and lockdir overlap each make one; closed/template/self never;
    /// newest first; the cap holds.
    #[test]
    fn stage2_narrowing() {
        let new = item("new", "queued", &["check:c1", "container:k1"], &["{topdir}/svc/a"], "2026-09-10T10:00:00Z");
        let items = vec![
            item("none", "queued", &["check:zz", "container:zz"], &["{topdir}/other"], "2026-09-10T09:00:00Z"),
            item("bycont", "queued", &["container:k1"], &[], "2026-09-10T08:00:00Z"),
            item("bycheck", "question", &["check:c1"], &[], "2026-09-10T09:30:00Z"),
            item("bylock", "parked", &[], &["{topdir}/svc"], "2026-09-10T07:00:00Z"),
            item("closed", "complete", &["container:k1"], &[], "2026-09-10T09:59:00Z"),
            item("tpl", "scheduled", &["container:k1"], &[], "2026-09-10T09:58:00Z"),
            new.clone(),
        ];
        let c: Vec<&str> = stage2_candidates(&new, &items, STAGE2_CAP).iter().map(|i| i.id.as_str()).collect();
        assert_eq!(c, vec!["bycheck", "bycont", "bylock"]);
        // cap
        let many: Vec<WorkItem> = (0..30).map(|n| item(&format!("m{n}"), "queued", &["container:k1"], &[], &format!("2026-09-10T00:00:{n:02}Z"))).collect();
        assert_eq!(stage2_candidates(&new, &many, STAGE2_CAP).len(), STAGE2_CAP);
        assert_eq!(stage2_candidates(&new, &many, STAGE2_CAP)[0].id, "m29", "newest first");
    }

    #[test]
    fn judge_output_parses_fenced_or_bare() {
        let fenced = "Here you go:\n```json\n{\"candidates\":[{\"id\":\"a\",\"summary\":\"s\",\"verdict\":\"same_fault\",\"reason\":\"r\"}]}\n```";
        let js = parse_judgements(fenced).unwrap();
        assert_eq!(js.len(), 1);
        assert_eq!(js[0].verdict, Verdict::SameFault);
        let bare = "[{\"id\":\"b\",\"verdict\":\"unsure\"}]";
        assert_eq!(parse_judgements(bare).unwrap()[0].verdict, Verdict::Unsure);
        assert!(parse_judgements("no json here").is_err());
        assert!(parse_judgements("{\"candidates\":[{\"id\":\"x\",\"verdict\":\"maybe\"}]}").is_err(), "unknown verdict is an error, never a merge");
    }

    /// Case 8 (decision half): one same_fault merges newer into older;
    /// several merge into the oldest and note the rest; unsure leaves notes;
    /// different is untouched; invented ids are ignored.
    #[test]
    fn decide_on_confidence_only() {
        let new = item("new", "queued", &[], &[], "2026-09-10T10:00:00Z");
        let old = item("old", "queued", &[], &[], "2026-09-10T08:00:00Z");
        let older = item("older", "parked", &[], &[], "2026-09-10T07:00:00Z");
        let cands = vec![&old, &older];
        assert_eq!(
            decide(&new, &cands, &[j("old", Verdict::SameFault, "same container, same red"), j("older", Verdict::Different, "")]),
            Decision::Merge { survivor: "old".into(), reason: "same container, same red".into(), others: vec![], overlaps: vec![] }
        );
        assert_eq!(
            decide(&new, &cands, &[j("old", Verdict::SameFault, "a"), j("older", Verdict::SameFault, "b")]),
            Decision::Merge { survivor: "older".into(), reason: "b".into(), others: vec!["old".into()], overlaps: vec![] }
        );
        assert_eq!(
            decide(&new, &cands, &[j("old", Verdict::Unsure, "maybe"), j("older", Verdict::Different, "")]),
            Decision::Overlap(vec![("old".into(), "maybe".into())])
        );
        assert_eq!(decide(&new, &cands, &[j("old", Verdict::Different, ""), j("older", Verdict::Different, "")]), Decision::Nothing);
        assert_eq!(decide(&new, &cands, &[j("ghost", Verdict::SameFault, "invented")]), Decision::Nothing);
    }

    /// Case 9 (decision half): never merge into an in-progress or closed
    /// survivor; never close an in-progress duplicate.
    #[test]
    fn never_merges_into_in_progress_or_closed() {
        let new = item("new", "queued", &[], &[], "2026-09-10T10:00:00Z");
        let running = item("run", "in-progress", &[], &[], "2026-09-10T08:00:00Z");
        let closed = item("done", "complete", &[], &[], "2026-09-10T07:00:00Z");
        let d = decide(&new, &[&running, &closed], &[j("run", Verdict::SameFault, "x"), j("done", Verdict::SameFault, "y")]);
        match d {
            Decision::Overlap(notes) => {
                assert_eq!(notes.len(), 2);
                assert!(notes[0].1.contains("in-progress") && notes[1].1.contains("complete"));
            }
            other => panic!("expected overlap notes, got {other:?}"),
        }
        // a duplicate that is already running is never closed
        let started = item("new", "in-progress", &[], &[], "2026-09-10T10:00:00Z");
        let old = item("old", "queued", &[], &[], "2026-09-10T08:00:00Z");
        assert!(matches!(decide(&started, &[&old], &[j("old", Verdict::SameFault, "x")]), Decision::Overlap(_)));
    }

    #[test]
    fn dup_tag_and_notes() {
        assert_eq!(dup_of_tag("f2a3c1e4-9b1c-4d3e-8f7a-f89259cb05e0").text, "dup of: f89259cb05e0");
        let n = seen_again_note("2026-09-10T08:00:00Z", "agent:code", "duplicate abc merged here", "the request");
        assert!(n.starts_with("seen again 2026-09-10T08:00:00Z by agent:code (duplicate abc merged here):\n\nthe request"));
        assert_eq!(overlap_note("x", ""), "may overlap with x: the dedup judge was unsure");
    }
}
