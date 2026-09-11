//! Stage 2 of repeat detection (built 2026-09-10, spec:
//! iter3/plans/!iter_dedup_spec.md; rules in `iter_core::dedup`).
//!
//! Stage 1 (iter_data, at create) sees only an exact `check:` + `container:`
//! twin.  What it cannot see — the same fault under a different rule label,
//! an agent-written defect with no label, two agents describing one problem
//! in their own words — the engine triages here, once per newly created
//! queued item, BEFORE it is dispatched:
//!
//! 1. narrow deterministically (`iter_core::dedup::stage2_candidates`): open
//!    items sharing the container, the check, or the lock scope; cap 20,
//!    newest first.  No candidates: no model call;
//! 2. judge with one short Sonnet session (the `dedup` agent, run the way
//!    `explain` is: read-only tools, short timeout), asked for JSON only;
//! 3. act on confidence alone (`iter_core::dedup::decide`): merge into one
//!    `same_fault` survivor via iter_data's `duplicate_of`; `unsure` leaves
//!    "may overlap" notes on both; `different` changes nothing.
//!
//! A judge that fails, times out, or returns prose never holds an item back:
//! the item is stamped `dedup_checked` and dispatched as today.  The judge
//! sits behind the `Judge` trait so the tests script its verdicts and no
//! test ever calls a model.

use crate::client::Api;
use iter_core::dedup::{self, Decision, Judgement, DEDUP_EPOCH, JUDGE_AGENT, STAGE2_CAP};
use iter_core::{Project, WorkItem, now_utc};
use serde_json::{Value, json};

/// What the judge reads about one item.
#[derive(Debug, Clone, PartialEq)]
pub struct ItemText {
    pub id: String,
    pub name: String,
    pub state: String,
    pub request: String,
}

/// The one model-facing seam: tests supply a stand-in with scripted verdicts.
pub trait Judge {
    fn judge(&self, new: &ItemText, candidates: &[ItemText]) -> Result<Vec<Judgement>, String>;
}

/// Which queued items stage 2 looks at: newly created (first attempt, not a
/// schedule clone, not a shell exec), not yet stamped, and filed after the
/// feature landed — items already in a queue on 2026-09-10 are never touched.
pub fn needs_triage(i: &WorkItem) -> bool {
    i.state == "queued"
        && i.dedup_checked.is_empty()
        && i.attempt == 0
        && i.source_schedule.is_empty()
        && i.agent != "exec"
        && i.ts.receive.as_str() >= DEDUP_EPOCH
}

/// The real judge: one Sonnet session per new item, run like `explain`
/// (read-only tools, bounded turns, the project's account token), with its
/// spend recorded on the new item.  Model and timeout come from the project's
/// `dedup` agent record when one exists (default sonnet / 180 s).
pub struct ClaudeJudge<'a> {
    pub api: &'a Api,
    pub project: &'a Project,
    pub topdir: &'a str,
    pub account: &'a str,
    pub agent_def: &'a Value,
    pub workid: &'a str,
}

impl Judge for ClaudeJudge<'_> {
    fn judge(&self, new: &ItemText, candidates: &[ItemText]) -> Result<Vec<Judgement>, String> {
        let prompt = judge_prompt(&self.project.name, new, candidates);
        let model = self.agent_def.get("model").and_then(|m| m.as_str()).unwrap_or("sonnet").trim().to_string();
        let timeout = self.agent_def.get("timeoutsec").and_then(|t| t.as_u64()).unwrap_or(180).clamp(30, 900);
        let extra = vec![
            "--allowedTools".to_string(),
            "Read,Glob,Grep".to_string(),
            "--disallowedTools".to_string(),
            "Bash,Edit,Write,MultiEdit,NotebookEdit,WebFetch,WebSearch,Agent".to_string(),
            "--max-turns".to_string(),
            "12".to_string(),
        ];
        let raw = crate::work::spawn_claude(self.project, self.topdir, self.account, &prompt, &model, &extra, timeout)?;
        let (_, out) = crate::work::parse_claude_stream(self.account, &raw);
        if out.cost_usd > 0.0 || out.input_tokens > 0 {
            let details = format!("/api/projects/{}/workitems/{}/details", self.project.name, self.workid);
            let _ = self.api.post(&details, &json!({"key": "spend", "valuetype": "json", "value": {"usd": out.cost_usd,
                "input_tokens": out.input_tokens, "output_tokens": out.output_tokens,
                "cache_read_tokens": out.cache_read_tokens, "cache_create_tokens": out.cache_create_tokens,
                "turns": out.num_turns, "agent": JUDGE_AGENT}}));
            let _ = self.api.post(&format!("/api/projects/{}/spend", self.project.name),
                &json!({"usd": out.cost_usd, "input_tokens": out.input_tokens, "output_tokens": out.output_tokens,
                    "cache_read_tokens": out.cache_read_tokens, "cache_create_tokens": out.cache_create_tokens, "workid": self.workid}));
        }
        if out.subtype != "success" {
            return Err(format!("judge session ended with '{}'", out.subtype));
        }
        dedup::parse_judgements(&out.text)
    }
}

fn clip(s: &str, max: usize) -> String {
    if s.chars().count() <= max { s.to_string() } else { format!("{}…", s.chars().take(max).collect::<String>()) }
}

/// The judge's single-turn prompt: the new item and every candidate's id,
/// name and request text; a one-line summary per candidate first, then one
/// verdict each; JSON only, so the engine parses a verdict and never prose.
pub fn judge_prompt(project: &str, new: &ItemText, candidates: &[ItemText]) -> String {
    let mut s = String::new();
    s.push_str(&format!(
        "# Is the new work item a repeat of an open one?\n\n\
You are the iter engine's dedup judge for project \"{project}\". A work item was just filed. Below it are open \
items that share its container, its check rule, or its lock scope. For EACH candidate decide whether it \
describes the SAME underlying fault as the new item — working both would be doing one job twice — a \
DIFFERENT fault, or you are UNSURE.\n\n\
Rules:\n\
- `same_fault` only when you are confident: the same thing is failing for the same reason. The same rule \
worded differently, a different stage of the same release, or extra run-specific detail (timestamps, run ids, \
counts) do NOT make two reports different.\n\
- The same container with a different symptom or cause is `different`. A different container is almost \
always `different`.\n\
- When the texts are too thin to tell, say `unsure`. Never guess `same_fault`.\n\
- You may read the repository (Read, Glob, Grep) if a file name in the texts would settle it; usually the texts are enough.\n\n\
Output ONLY this JSON object, no prose before or after, every candidate id exactly once:\n\
{{\"candidates\":[{{\"id\":\"<candidate id>\",\"summary\":\"<one line: what that item is about>\",\"verdict\":\"same_fault|different|unsure\",\"reason\":\"<one sentence>\"}}]}}\n\n"
    ));
    s.push_str(&format!("## The new item\nId: {}\nName: {}\n\n{}\n\n", new.id, new.name, clip(&new.request, 6000)));
    s.push_str("## Candidates (open items)\n");
    for c in candidates {
        s.push_str(&format!("\n### {} — {} [{}]\n{}\n", c.id, c.name, c.state, clip(&c.request, 3000)));
    }
    s
}

fn text_of(api: &Api, project: &Project, item: &WorkItem) -> ItemText {
    let details = crate::work::fetch_details(api, project, item);
    ItemText { id: item.id.clone(), name: item.name.clone(), state: item.state.clone(), request: crate::work::request_text(&details, item) }
}

/// The triage plan for one new item, with no side effects: narrow, ask the
/// judge, decide.  Pure over the judge, so the tests drive it with scripted
/// verdicts.  A judge error is `Nothing` (dispatch as today) after a log line.
pub fn plan(new: &WorkItem, items: &[WorkItem], judge: &dyn Judge, texts: &dyn Fn(&WorkItem) -> ItemText) -> Decision {
    let cands = dedup::stage2_candidates(new, items, STAGE2_CAP);
    if cands.is_empty() {
        return Decision::Nothing;
    }
    let new_t = texts(new);
    let cand_t: Vec<ItemText> = cands.iter().map(|c| texts(c)).collect();
    match judge.judge(&new_t, &cand_t) {
        Ok(js) => dedup::decide(new, &cands, &js),
        Err(e) => {
            eprintln!("[engine] dedup judge failed on {} '{}': {e} — dispatching as usual", short(&new.id), new.name);
            Decision::Nothing
        }
    }
}

/// One item's triage, on its own thread: plan, act through iter_data, stamp.
pub fn triage(api: &Api, project: &Project, items: &[WorkItem], item: &WorkItem, judge: &dyn Judge) -> Decision {
    let n = dedup::stage2_candidates(item, items, STAGE2_CAP).len();
    if n > 0 {
        println!("[engine] dedup triage: {} '{}' — {n} open item(s) share its container, check or scope; asking the judge", short(&item.id), item.name);
    }
    let decision = plan(item, items, judge, &|i| text_of(api, project, i));
    apply(api, project, item, &decision);
    decision
}

fn doc(api: &Api, project: &Project, id: &str, text: &str) {
    if let Err(e) = api.post(
        &format!("/api/projects/{}/workitems/{}/details", project.name, id),
        &json!({"key": "doc", "valuetype": "text", "value": text}),
    ) {
        eprintln!("[engine] could not append a dedup note to {}: {e}", short(id));
    }
}

/// Carry a decision out: the merge is one iter_data call (it closes the
/// duplicate and books the survivor's repeat atomically enough); overlap
/// notes are doc rows on both sides; everything else stamps the item.
pub fn apply(api: &Api, project: &Project, item: &WorkItem, decision: &Decision) {
    match decision {
        Decision::Merge { survivor, reason, others, overlaps } => {
            let r = api.post(
                &format!("/api/projects/{}/workitems/{}/duplicate_of", project.name, item.id),
                &json!({"survivor": survivor, "reason": reason, "others": others}),
            );
            match r {
                Ok(_) => println!(
                    "[engine] {} '{}' is a duplicate of {} — closed complete (dup of: {}); the survivor is seen again{}: {reason}",
                    short(&item.id), item.name, short(survivor), &survivor[survivor.len().saturating_sub(12)..],
                    if others.is_empty() { String::new() } else { format!(" (also the same fault: {})", others.iter().map(|o| short(o)).collect::<Vec<_>>().join(", ")) }
                ),
                Err(e) => {
                    eprintln!("[engine] merge of {} into {} refused: {e} — dispatching as usual", short(&item.id), short(survivor));
                    stamp(api, project, item);
                }
            }
            for (id, why) in overlaps {
                doc(api, project, id, &dedup::overlap_note(survivor, why));
            }
        }
        Decision::Overlap(notes) => {
            for (id, why) in notes {
                doc(api, project, &item.id, &dedup::overlap_note(id, why));
                doc(api, project, id, &dedup::overlap_note(&item.id, why));
                println!("[engine] {} '{}' may overlap with {} — both noted, both stay open", short(&item.id), item.name, short(id));
            }
            stamp(api, project, item);
        }
        Decision::Nothing => stamp(api, project, item),
    }
}

/// `dedup_checked = now` on the item (a versioned write, re-read on a lost
/// race) so it is dispatched from the next tick and never triaged twice.
fn stamp(api: &Api, project: &Project, item: &WorkItem) {
    for attempt in 0..4 {
        let row = if attempt == 0 {
            serde_json::to_value(item).unwrap()
        } else {
            match api.get(&format!("/api/projects/{}/workitems/{}", project.name, item.id)) {
                Ok(v) => v,
                Err(_) => return,
            }
        };
        let state = row.get("state").and_then(|s| s.as_str()).unwrap_or("");
        if !dedup::is_open_state(state) {
            return; // closed underneath us (a merge, a human): nothing to stamp
        }
        let version = row.get("version").and_then(|v| v.as_u64()).unwrap_or(item.version);
        let mut updated = row.clone();
        updated["dedup_checked"] = json!(now_utc());
        match api.put(&format!("/api/projects/{}/workitems/{}?expect_version={version}", project.name, item.id), &updated) {
            Ok(_) => return,
            Err(e) if e.status == 409 => continue,
            Err(e) => {
                eprintln!("[engine] could not stamp dedup_checked on {}: {e}", short(&item.id));
                return;
            }
        }
    }
}

fn short(id: &str) -> &str {
    &id[..8.min(id.len())]
}

#[cfg(test)]
mod tests {
    use super::*;
    use iter_core::Tag;
    use iter_core::dedup::Verdict;
    use std::cell::RefCell;

    fn item(id: &str, state: &str, tags: &[&str], lockdirs: &[&str], receive: &str) -> WorkItem {
        WorkItem {
            id: id.into(), name: format!("item {id}"), state: state.into(),
            tags: tags.iter().map(|t| Tag { text: t.to_string(), color: String::new() }).collect(),
            lockdirs: lockdirs.iter().map(|s| s.to_string()).collect(),
            ts: iter_core::WorkItemTs { receive: receive.into(), ..Default::default() },
            ..Default::default()
        }
    }
    fn texts(i: &WorkItem) -> ItemText {
        ItemText { id: i.id.clone(), name: i.name.clone(), state: i.state.clone(), request: format!("request of {}", i.id) }
    }

    /// Scripted judge: a fixed verdict for every candidate, or an error;
    /// records what it was asked so a test can assert the narrowing.
    struct Scripted {
        verdict: Result<Verdict, String>,
        asked: RefCell<Vec<Vec<String>>>,
    }
    impl Scripted {
        fn says(v: Verdict) -> Self { Self { verdict: Ok(v), asked: RefCell::new(vec![]) } }
        fn fails() -> Self { Self { verdict: Err("timed out".into()), asked: RefCell::new(vec![]) } }
    }
    impl Judge for Scripted {
        fn judge(&self, _new: &ItemText, candidates: &[ItemText]) -> Result<Vec<Judgement>, String> {
            self.asked.borrow_mut().push(candidates.iter().map(|c| c.id.clone()).collect());
            let v = self.verdict.clone()?;
            Ok(candidates.iter().map(|c| Judgement { id: c.id.clone(), summary: String::new(), verdict: v, reason: format!("scripted for {}", c.id) }).collect())
        }
    }

    /// Case 8 (engine half): same_fault on one candidate merges the newer
    /// into the older; unsure leaves overlap notes; different is untouched;
    /// a judge error dispatches as today.  No model, no API.
    #[test]
    fn plan_follows_the_scripted_verdict() {
        let new = item("new", "queued", &["container:clearing"], &[], "2026-09-10T13:00:00Z");
        let old = item("old", "queued", &["container:clearing"], &[], "2026-09-10T12:30:00Z");
        let unrelated = item("far", "queued", &["container:authority"], &["{topdir}/elsewhere"], "2026-09-10T12:45:00Z");
        let items = vec![new.clone(), old.clone(), unrelated.clone()];

        let same = Scripted::says(Verdict::SameFault);
        assert_eq!(
            plan(&new, &items, &same, &texts),
            Decision::Merge { survivor: "old".into(), reason: "scripted for old".into(), others: vec![], overlaps: vec![] }
        );
        assert_eq!(same.asked.borrow()[0], vec!["old".to_string()], "the unrelated item was never shown to the judge");

        let unsure = Scripted::says(Verdict::Unsure);
        assert_eq!(plan(&new, &items, &unsure, &texts), Decision::Overlap(vec![("old".into(), "scripted for old".into())]));

        let different = Scripted::says(Verdict::Different);
        assert_eq!(plan(&new, &items, &different, &texts), Decision::Nothing);

        let broken = Scripted::fails();
        assert_eq!(plan(&new, &items, &broken, &texts), Decision::Nothing, "a judge failure never blocks dispatch");
        assert_eq!(broken.asked.borrow().len(), 1);
    }

    /// Case 7 (engine half): no candidates → no judge call at all.
    #[test]
    fn no_candidates_means_no_model_call() {
        let new = item("new", "queued", &["container:clearing"], &["{topdir}/a"], "2026-09-10T13:00:00Z");
        let items = vec![new.clone(), item("far", "queued", &["container:authority"], &["{topdir}/b"], "2026-09-10T12:00:00Z")];
        let j = Scripted::says(Verdict::SameFault);
        assert_eq!(plan(&new, &items, &j, &texts), Decision::Nothing);
        assert!(j.asked.borrow().is_empty(), "judge must not be called");
    }

    /// Which queued items are triaged: new, first attempt, not a schedule
    /// clone or exec, not yet stamped, filed after the feature landed.
    #[test]
    fn needs_triage_rules() {
        let mut i = item("x", "queued", &[], &[], "2026-09-10T18:00:00Z");
        i.agent = "code".into();
        assert!(needs_triage(&i));
        let mut stamped = i.clone();
        stamped.dedup_checked = "2026-09-10T18:01:00Z".into();
        assert!(!needs_triage(&stamped));
        let mut retry = i.clone();
        retry.attempt = 1;
        assert!(!needs_triage(&retry));
        let mut clone = i.clone();
        clone.source_schedule = "tpl".into();
        assert!(!needs_triage(&clone));
        let mut exec = i.clone();
        exec.agent = "exec".into();
        assert!(!needs_triage(&exec));
        let mut old = i.clone();
        old.ts.receive = "2026-09-10T17:00:00Z".into();
        assert!(!needs_triage(&old), "items in a queue before the feature landed are never touched");
        let mut running = i.clone();
        running.state = "in-progress".into();
        assert!(!needs_triage(&running));
    }

    #[test]
    fn prompt_names_every_candidate_and_asks_for_json() {
        let new = ItemText { id: "n1".into(), name: "new".into(), state: "queued".into(), request: "r".into() };
        let c = vec![ItemText { id: "c1".into(), name: "cand".into(), state: "parked".into(), request: "x".repeat(5000) }];
        let p = judge_prompt("proj", &new, &c);
        assert!(p.contains("### c1 — cand [parked]") && p.contains("\"candidates\"") && p.contains("Id: n1"));
        assert!(p.contains('…'), "long request text is clipped");
    }
}
