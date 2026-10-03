//! Close gate (spec: Close Gate, decided 2026-09-03): the checks an agent
//! item must pass before the engine may close it complete.  Pure helpers
//! live here (prompt text, verdict parsing, widget shapes, detail-row
//! inspection); the spawning and state writes stay in work.rs.

use serde_json::{Value, json};

/// Marker the verifier prompt always carries — lets a test double (and a
/// human reading logs) tell a verifier session from a worker session.
pub const VERIFIER_MARKER: &str = "iter close-gate verifier";
/// Marker on the question widget the gate writes when it gives up.
pub const GATE_WIDGET_KIND: &str = "close";

/// Paragraph appended to every worker prompt.
pub const WORKER_CLOSE_GATE_PROMPT: &str = "# Close gate\n\
When you finish, your final message must state plainly what you delivered, and list every obligation \
from this workitem that you did NOT complete, each on its own line starting with \"NOT DONE:\". \
A verifier compares your final message against the request before this item can close. \
An unfinished item is bounced back to you (or to a human) with the open obligations; it is never closed \
as complete.  Do not end your turn while waiting on something to finish — finish it, or say NOT DONE.\n";

/// What the verifier asks the human and recommends (close-gate question
/// shape, 2026-09-12: the widget used to show the item's name as its title
/// and clip the verifier's doubt out of the summary).  Empty strings when an
/// older verifier leaves them out.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Advice {
    /// ONE sentence ending in '?': the single fact a human must settle
    pub question: String,
    /// "accept" | "continue" ("" = none given)
    pub recommendation: String,
    /// one clause: why that recommendation
    pub why: String,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Verdict {
    Complete,
    Incomplete { open: Vec<String>, reason: String, advice: Advice },
    Unclear { reason: String, advice: Advice },
    /// the verifier answered but its output held no parseable verdict json —
    /// retried once before a human is asked (R4)
    Unparsed { reason: String },
    /// the verifier process could not run (spawn failure, non-zero exit,
    /// timeout) twice in a row — a tooling failure, not a verdict about the
    /// work (2026-09-07: it used to park the item as a human question)
    Unavailable { reason: String },
}

/// The workitems whose `createdby` is `id` — one list read serves both the
/// evidence count (always taken, 2026-09-07: it used to be 0 unless the gate
/// required children, and the verifier read that 0 as engine fact) and the
/// re-run's "already created" section.
pub fn children_of(items: &[Value], id: &str) -> Vec<Value> {
    items.iter().filter(|w| w.get("createdby").and_then(|c| c.as_str()) == Some(id)).cloned().collect()
}

/// What the engine measured around the run; shown to the verifier and kept
/// in the "verify" detail row so a bounce is explainable in the UI.
#[derive(Debug, Clone, Default)]
pub struct Evidence {
    pub result_subtype: String,
    pub num_turns: u64,
    pub head_before: String,
    pub head_after: String,
    pub diffstat: String,
    pub children: usize,
    pub open_reviews: usize,
    /// scoped end-of-run commit (2026-09-12): what the diffstat was limited
    /// to — "lock scope (k paths)" or "whole tree (item has no lockdirs)"
    pub commit_scope: String,
    /// paths another item left dirty in the shared checkout, never listed
    pub outside_scope_dirty: usize,
    /// (short hash, subject) of this item's own commits over its WHOLE life
    /// — every attempt, not just this one (2026-09-16: attempt 1's real work
    /// was invisible to attempt 2's verifier) — matched by `commits_with_id`
    pub commits: Vec<(String, String)>,
    /// (path, short hash): files this item's own commits changed OUTSIDE its
    /// lock scope — a ledger row, the container's techreq; its work, and
    /// evidence (2026-09-14..26: they were reported as "left uncommitted")
    pub outside_scope_committed: Vec<(String, String)>,
    /// (short hash, subject): other items' commits inside this scope during
    /// the run — listed apart so they are never read as this item's work
    pub other_scope_commits: Vec<(String, String)>,
    /// `doc` rows this attempt appended to the item (2026-09-29: a "record the
    /// answer as a note, change no files" item was bounced as unproven —
    /// the verifier saw no commit and never the note itself)
    pub notes: Vec<String>,
}

/// The git log format `commits_with_id` reads: hash, subject and body,
/// fields split by 0x1f, records by 0x1e.
pub const COMMIT_LOG_FORMAT: &str = "%h%x1f%s%x1f%b%x1e";

/// The commits in a `git log --format=COMMIT_LOG_FORMAT` listing that carry
/// this item's id: the full id anywhere in the subject or body, or a subject
/// ending in `(<first 8>)` (the engine's own commit) or `(<last 12>)` (the
/// short form the webui shows).  Several agents commit to one checkout
/// concurrently, so the listing holds siblings' commits too.
pub fn commits_with_id(log: &str, id: &str) -> Vec<(String, String)> {
    let id8 = format!("({})", &id[..8.min(id.len())]);
    let id12 = format!("({})", &id[id.len().saturating_sub(12)..]);
    log.split('\x1e')
        .filter_map(|rec| {
            let mut f = rec.trim_start_matches('\n').splitn(3, '\x1f');
            let (h, subj, body) = (f.next()?.trim(), f.next()?.trim(), f.next().unwrap_or(""));
            if h.is_empty() {
                return None;
            }
            let mine = subj.contains(id) || body.contains(id) || subj.ends_with(&id8) || subj.ends_with(&id12);
            mine.then(|| (h.to_string(), subj.to_string()))
        })
        .collect()
}

impl Evidence {
    pub fn committed(&self) -> bool {
        !self.head_after.is_empty() && self.head_before != self.head_after
    }
    pub fn to_json(&self) -> Value {
        json!({
            "result_subtype": self.result_subtype,
            "num_turns": self.num_turns,
            "commit": if self.committed() { self.head_after.clone() } else { String::new() },
            "diffstat": self.diffstat,
            "children": self.children,
            "open_reviews": self.open_reviews,
            "commit_scope": self.commit_scope,
            "outside_scope_dirty": self.outside_scope_dirty,
            "commits": self.commits.iter().map(|(h, s)| json!({"hash": h, "subject": s})).collect::<Vec<_>>(),
            "outside_scope_committed": self.outside_scope_committed.iter().map(|(p, h)| json!({"path": p, "hash": h})).collect::<Vec<_>>(),
            "other_scope_commits": self.other_scope_commits.iter().map(|(h, s)| json!({"hash": h, "subject": s})).collect::<Vec<_>>(),
            "notes": self.notes.len(),
        })
    }

    /// Whether this item has git work to show: a new head this attempt, or
    /// commits of its own from any attempt.
    pub fn has_work(&self) -> bool {
        self.committed() || !self.commits.is_empty()
    }

    /// The one sentence the verifier cannot miss: the tree is shared, the
    /// diffstat is this item's own commits within its scope, what it
    /// committed outside the scope, and which in-scope commits are others'.
    /// "Uncommitted" means only paths still dirty in the checkout.
    fn scope_sentence(&self) -> String {
        let list = if self.commits.is_empty() {
            "none found (every commit since the item was received was searched for its full id, its (first-8) and its (last-12) subject suffix)".to_string()
        } else {
            self.commits.iter().map(|(h, s)| format!("{h} \"{s}\"")).collect::<Vec<_>>().join(", ")
        };
        let mut out = format!(
            "Several agents commit to this checkout concurrently. The diffstat below covers only this item's own commits, limited to its lock scope ({}); \
{} path(s) outside that scope are still dirty (uncommitted) in the checkout and are not this item's. Commits carrying this item's id, all attempts: {}.",
            if self.commit_scope.is_empty() { "unknown scope" } else { &self.commit_scope },
            self.outside_scope_dirty, list
        );
        if !self.outside_scope_committed.is_empty() {
            let v: Vec<String> = self.outside_scope_committed.iter().map(|(p, h)| format!("{p} ({h})")).collect();
            out.push_str(&format!(" Committed by this item outside its lock scope: {}.", v.join(", ")));
        }
        if !self.other_scope_commits.is_empty() {
            let v: Vec<String> = self.other_scope_commits.iter().map(|(h, s)| format!("{h} \"{s}\"")).collect();
            out.push_str(&format!(" Commits by other items in this scope (not this item's work): {}.", v.join(", ")));
        }
        out
    }
    fn describe(&self) -> String {
        let mut s = String::new();
        s.push_str(&format!("- worker session ended with result subtype '{}' after {} turn(s)\n",
            if self.result_subtype.is_empty() { "unknown" } else { &self.result_subtype }, self.num_turns));
        if self.head_after.is_empty() {
            s.push_str("- git: not a repository (no commit evidence)\n");
        } else {
            if self.committed() {
                s.push_str(&format!("- git: new head {}\n", &self.head_after[..12.min(self.head_after.len())]));
            } else {
                s.push_str("- git: NO new commit in this attempt (the tree did not change)\n");
            }
            if self.has_work() {
                s.push_str(&format!("- git: {}\n{}\n", self.scope_sentence(), indent(&self.diffstat)));
            }
        }
        s.push_str(&format!("- workitems created by this item: {}\n", self.children));
        if self.notes.is_empty() {
            s.push_str("- notes (doc rows) recorded on this item during this attempt: 0\n");
        } else {
            s.push_str(&format!("- notes (doc rows) recorded on this item during this attempt, verbatim from the item's record: {}\n", self.notes.len()));
            for n in &self.notes {
                s.push_str(&format!("{}\n", indent(&clip(n, 4000))));
            }
        }
        s.push_str(&format!("- review rows without a disposition: {}\n", self.open_reviews));
        s
    }
}

#[cfg(test)]
impl Evidence {
    pub fn describe_for_test(&self) -> String {
        self.describe()
    }
}

fn indent(s: &str) -> String {
    s.lines().map(|l| format!("    {l}")).collect::<Vec<_>>().join("\n")
}

/// The verifier's prompt: request, final message, evidence, one narrow
/// question, one json object back.
pub fn verifier_prompt(item_name: &str, request: &str, response: &str, ev: &Evidence) -> String {
    format!(
        "You are the {marker}. You judge DONE-NESS, not quality: did the worker's final message claim to \
finish EVERY obligation in the request, and does the evidence support that claim?  Persuasive summaries \
that skip an obligation, \"I'm waiting for X to finish\", \"next step is to ...\", or a plan that was written \
but whose items were never filed are all INCOMPLETE.  You may read files to check a claim, but do not modify anything.  \
Several agents share this checkout: a file outside the item's lock scope is NOT evidence about this item unless the worker's \
own message claims it or the evidence lists it as committed by this item — a path committed under this item's id is its work, \
wherever it is; never call a report dishonest over a path the evidence marks as outside the scope or as another item's commit.\n\n\
Answer with exactly one json object and nothing else:\n\
{{\"verdict\": \"complete\" | \"incomplete\" | \"unclear\", \"open\": [\"each obligation still open, one per entry\"], \"reason\": \"one or two sentences\", \
\"question\": \"ONE sentence ending in '?' naming the single fact a human must settle — empty when complete\", \
\"recommendation\": \"accept\" | \"continue\", \"why\": \"one clause: why that recommendation\"}}\n\
The question is the decision, not a summary: name the contradiction or the missing proof in plain words, answerable in one word.\n\n\
# Workitem: {item_name}\n\n## Request\n{request}\n\n## Worker's final message\n{response}\n\n## Engine evidence\n{evidence}",
        marker = VERIFIER_MARKER,
        item_name = item_name,
        request = clip(request, 40_000),
        response = clip(response, 24_000),
        evidence = ev.describe(),
    )
}

/// First json object in the text; no parseable object is `Unparsed`, a
/// verdict other than complete/incomplete is `Unclear`.  The question,
/// recommendation and why default to "" so older verifier output parses.
pub fn parse_verdict(text: &str) -> Verdict {
    let Some(start) = text.find('{') else {
        return Verdict::Unparsed { reason: format!("verifier returned no json: {}", clip(text, 300)) };
    };
    let Some(end) = text.rfind('}') else {
        return Verdict::Unparsed { reason: format!("verifier returned no json: {}", clip(text, 300)) };
    };
    if end < start {
        return Verdict::Unparsed { reason: "verifier returned malformed json".into() };
    }
    let v: Value = match serde_json::from_str(&text[start..=end]) {
        Ok(v) => v,
        Err(e) => return Verdict::Unparsed { reason: format!("verifier json did not parse: {e}") },
    };
    let field = |k: &str| v.get(k).and_then(|x| x.as_str()).unwrap_or("").trim().to_string();
    let rec = field("recommendation").to_ascii_lowercase();
    let advice = Advice {
        question: field("question"),
        recommendation: if rec == "accept" || rec == "continue" { rec } else { String::new() },
        why: field("why"),
    };
    let reason = v.get("reason").and_then(|r| r.as_str()).unwrap_or("").to_string();
    let open: Vec<String> = v
        .get("open")
        .and_then(|o| o.as_array())
        .map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).filter(|s| !s.trim().is_empty()).collect())
        .unwrap_or_default();
    match v.get("verdict").and_then(|x| x.as_str()).unwrap_or("").trim().to_ascii_lowercase().as_str() {
        "complete" => Verdict::Complete,
        "incomplete" => Verdict::Incomplete { open, reason, advice },
        other => Verdict::Unclear {
            reason: if other.is_empty() { "verifier gave no verdict".into() } else { format!("verdict '{other}': {reason}") },
            advice,
        },
    }
}

/// The blockers `item` still waits on (decided 2026-09-10, from pdy-dev item
/// cb9c52c30bd6: four attempts and $59 re-measuring a block the record already
/// declared).  Every blocker is judged with the item's own dependency rule —
/// deep unless `blockedby_shallow` — so a blocker that closed complete while
/// something it created is still open counts as waiting.  At dispatch every
/// blocker was satisfied (the item could not have started otherwise), so an
/// open one at close was declared DURING the run: the agent linked the item
/// it cannot finish without.  Unknown ids count as satisfied, like the gate.
pub fn waiting_on(item: &iter_core::WorkItem, items: &[iter_core::WorkItem]) -> Vec<String> {
    use iter_core::{DepStatus, children_index, dependency_status};
    let by_id: std::collections::HashMap<String, &iter_core::WorkItem> = items.iter().map(|i| (i.id.clone(), i)).collect();
    let kids = children_index(items);
    item.blockedby
        .iter()
        .filter(|b| {
            let mut probe = item.clone();
            probe.blockedby = vec![(*b).clone()];
            dependency_status(&probe, &by_id, &kids) != DepStatus::Satisfied
        })
        .cloned()
        .collect()
}

/// The "verify" row when the gate holds an item BEHIND its declared
/// blockers instead of bouncing it: verdict "waiting", the open list the
/// verifier saw, the blocker ids, and no bounce counted.
pub fn waiting_row(bounces: u32, source: &str, open: &[String], reason: &str, waiting_on: &[String], ev: &Evidence) -> Value {
    let mut row = verify_row(bounces, source, "waiting", open, reason, ev);
    row["waiting_on"] = json!(waiting_on);
    row
}

/// The "verify" detail row body written on every bounce.
pub fn verify_row(bounce: u32, source: &str, verdict: &str, open: &[String], reason: &str, ev: &Evidence) -> Value {
    json!({
        "bounce": bounce,
        "source": source,
        "verdict": verdict,
        "open": open,
        "reason": reason,
        "evidence": ev.to_json(),
        "ts": iter_core::now_utc(),
    })
}

/// The question a deterministic hold asks (R2): the check that failed,
/// turned into the decision the human is making.
pub fn deterministic_advice(open: &[String]) -> Advice {
    let q = match open {
        [one] if one.contains("no new git commit") => "Is this item done although no git commit was produced?".to_string(),
        [one] if one.contains("--fixed` claim") => "Is this item done although its last `iter runtests --fixed` claim was false?".to_string(),
        [one] if one.contains("review row(s)") => format!("Is this item done although {}?", one.replace(" recorded without a disposition", " have no disposition")),
        [one] if one.contains("no workitems were created") => "Is this item done although it created no workitems?".to_string(),
        [one] if one.contains("session ended with") => "Is this item done although its agent session was cut off?".to_string(),
        _ => format!("Is this item done although {} of the close gate's checks failed?", open.len()),
    };
    Advice { question: q, recommendation: "continue".into(), why: "an engine check failed; the listed obligation is still open".into() }
}

/// The advice when the verifier's output did not parse twice (R4): accept
/// on the worker's evidence when the session succeeded and git has its work.
pub fn unparsed_advice(ev: &Evidence) -> Advice {
    let ok = ev.result_subtype == "success" && ev.has_work();
    Advice {
        question: "The verifier could not judge this item (its output did not parse twice). Accept on the worker's evidence?".into(),
        recommendation: if ok { "accept" } else { "continue" }.into(),
        why: if ok { "the session succeeded and this item has commits".into() } else { "the worker's evidence is incomplete (no successful session or no commit)".into() },
    }
}

/// The question widget written when the gate hands the item to a human
/// (close-gate question shape, 2026-09-12).  Title = the question, never the
/// item's name; summary = the recommendation and why, then the bounce count
/// and the item; detail = question, reason, open list and evidence in full,
/// then the report clipped to 1,500 characters; the radio defaults to the
/// recommendation.
#[allow(clippy::too_many_arguments)]
pub fn question_widget(item_name: &str, bounces: u32, max_bounces: u32, reason: &str, open: &[String], last_response: &str, advice: &Advice, ev: &Evidence) -> Value {
    let mut advice = advice.clone();
    if advice.question.trim().is_empty() {
        advice.question = format!("Is this item done? The close gate could not decide: {}", reason.trim_end_matches(['.', ' ']));
        if !advice.question.ends_with('?') {
            advice.question.push('?');
        }
    }
    let rec = if advice.recommendation == "accept" { "accept" } else { "continue" };
    let why = if advice.why.is_empty() { "the verifier gave no reason".to_string() } else { advice.why.clone() };
    let open_text = if open.is_empty() {
        "none — the verifier's doubt is the one above".to_string()
    } else {
        open.iter().map(|o| format!("- {o}")).collect::<Vec<_>>().join("\n")
    };
    let evidence = format!(
        "commit: {}\ncommits carrying this item's id: {}\nworkitems created by this item: {}\ndiffstat (this item's commits, lock scope):\n{}",
        if ev.committed() { ev.head_after.as_str() } else { "none this attempt" },
        if ev.commits.is_empty() { "none found".to_string() } else { ev.commits.iter().map(|(h, s)| format!("{h} {s}")).collect::<Vec<_>>().join("; ") },
        ev.children,
        if ev.diffstat.is_empty() { "    (none)".to_string() } else { indent(&ev.diffstat) },
    );
    json!({
        "gate": GATE_WIDGET_KIND,
        "title": clip(&advice.question, 200),
        "summary": format!("Recommendation: {rec} — {why} · bounce {bounces} of {} · item: {}", max_bounces.max(1), clip(item_name, 90)),
        "detail": format!(
            "Question: {}\n\nWhy the gate could not decide: {reason}\n\nOpen obligations:\n{open_text}\n\nEvidence the gate saw:\n{evidence}\n\n\
Last response (first 1,500 characters; the full text is the last response row):\n{}",
            advice.question, clip(last_response, 1_500)
        ),
        "fields": [
            {"key": "action", "label": "What next?", "type": "radio",
             "options": [
                {"value": "continue", "desc": "requeue; the agent gets your guidance below plus the open list"},
                {"value": "accept", "desc": "close as complete without running an agent"}
             ],
             "value": rec},
            {"key": "guidance", "label": "Guidance for the agent", "type": "text", "value": ""}
        ]
    })
}
fn is_gate_widget(d: &Value) -> bool {
    d.get("key").and_then(|k| k.as_str()) == Some("question")
        && d.get("value").and_then(|v| v.get("gate")).and_then(|g| g.as_str()) == Some(GATE_WIDGET_KIND)
}

fn order_of(d: &Value) -> i64 {
    d.get("order").and_then(|o| o.as_i64()).unwrap_or(0)
}

fn field_value<'a>(widget: &'a Value, key: &str) -> Option<&'a Value> {
    widget
        .get("fields")
        .and_then(|f| f.as_array())
        .and_then(|fs| fs.iter().find(|f| f.get("key").and_then(|k| k.as_str()) == Some(key)))
        .and_then(|f| f.get("value"))
}

/// The latest gate widget, if it is newer than every "response" row (a
/// stale answer must not steer a later requeue).
fn latest_live_gate_widget(details: &[Value]) -> Option<&Value> {
    let last_response = details
        .iter()
        .filter(|d| d.get("key").and_then(|k| k.as_str()) == Some("response"))
        .map(order_of)
        .max()
        .unwrap_or(-1);
    details.iter().filter(|d| is_gate_widget(d)).max_by_key(|d| order_of(d)).filter(|d| order_of(d) > last_response)
}

/// A human answered the gate widget with "accept": close without running.
pub fn accepted_by_human(details: &[Value]) -> bool {
    latest_live_gate_widget(details)
        .and_then(|d| field_value(&d["value"], "action"))
        .and_then(|v| v.as_str())
        == Some("accept")
}

/// Human guidance from a live gate widget answered "continue".
fn human_guidance(details: &[Value]) -> Option<String> {
    let w = latest_live_gate_widget(details)?;
    let action = field_value(&w["value"], "action").and_then(|v| v.as_str()).unwrap_or("");
    if action != "continue" {
        return None;
    }
    let g = field_value(&w["value"], "guidance").and_then(|v| v.as_str()).unwrap_or("").trim().to_string();
    Some(g)
}

/// The latest "claim" row whose claim is "fixed" (`iter runtests --fixed`):
/// {group, claim, upheld, outcome, counts, ts}.
pub fn last_fixed_claim(details: &[Value]) -> Option<&Value> {
    details
        .iter()
        .filter(|d| d.get("key").and_then(|k| k.as_str()) == Some("claim"))
        .filter(|d| d.get("value").and_then(|v| v.get("claim")).and_then(|c| c.as_str()) == Some("fixed"))
        .max_by_key(|d| d.get("order").and_then(|o| o.as_i64()).unwrap_or(0))
        .and_then(|d| d.get("value"))
}

/// "review" rows (valuetype json) lacking a non-empty "disposition".
pub fn open_reviews(details: &[Value]) -> usize {
    details
        .iter()
        .filter(|d| d.get("key").and_then(|k| k.as_str()) == Some("review"))
        .filter(|d| d.get("valuetype").and_then(|v| v.as_str()) == Some("json"))
        .filter(|d| {
            d.get("value")
                .and_then(|v| v.get("disposition"))
                .and_then(|x| x.as_str())
                .map(|s| s.trim().is_empty())
                .unwrap_or(true)
        })
        .count()
}

/// The section appended to a re-run's prompt after a bounce (or after a
/// human answered "continue"): verdict, open list, guidance, last message —
/// plus, whenever this item has already created workitems (any earlier
/// attempt: a bounce, a failed run, a requeue), the list of them with one
/// instruction: do not file these again (2026-09-07: attempt 3 of a pdy-dev
/// item re-filed the defect attempt 2 had filed, and two agents ran against
/// the same file).  Empty when there is nothing to carry forward.
pub fn feedback_section(details: &[Value], created: &[Value]) -> String {
    let mut s = feedback_from_gate(details);
    if !created.is_empty() {
        if !s.is_empty() {
            s.push('\n');
        }
        s.push_str("# Work items this item has already created\n\
An earlier attempt of this workitem filed the items below (their `createdby` is this item's id). \
Do NOT file these again; reference them by id, and file only what is still missing.\n");
        for c in created {
            let id = c.get("id").and_then(|i| i.as_str()).unwrap_or("?");
            let state = c.get("state").and_then(|x| x.as_str()).unwrap_or("?");
            let name = c.get("name").and_then(|x| x.as_str()).unwrap_or("");
            let agent = c.get("agent").and_then(|x| x.as_str()).unwrap_or("");
            s.push_str(&format!("- {id} [{state}] ({agent}) {name}\n"));
        }
    }
    s
}

fn feedback_from_gate(details: &[Value]) -> String {
    let last_verify = details
        .iter()
        .filter(|d| d.get("key").and_then(|k| k.as_str()) == Some("verify"))
        .max_by_key(|d| order_of(d));
    let last_response = details
        .iter()
        .filter(|d| d.get("key").and_then(|k| k.as_str()) == Some("response"))
        .max_by_key(|d| order_of(d))
        .and_then(|d| d.get("value").and_then(|v| v.as_str()))
        .unwrap_or("");
    let guidance = human_guidance(details);
    if last_verify.is_none() && guidance.is_none() {
        return String::new();
    }
    let mut s = String::from("# Close-gate feedback from the previous attempt\n\
Your previous run of this workitem ended WITHOUT completing the request, so the engine did not close it. \
Continue from where it left off; do not start over and do not repeat finished work.\n");
    if let Some(v) = last_verify.map(|d| &d["value"]) {
        let bounce = v.get("bounce").and_then(|b| b.as_u64()).unwrap_or(0);
        let reason = v.get("reason").and_then(|r| r.as_str()).unwrap_or("");
        s.push_str(&format!("\nBounce {bounce}: {reason}\n"));
        if let Some(open) = v.get("open").and_then(|o| o.as_array()) {
            if !open.is_empty() {
                s.push_str("Open obligations:\n");
                for o in open {
                    if let Some(t) = o.as_str() {
                        s.push_str(&format!("- {t}\n"));
                    }
                }
            }
        }
    }
    if let Some(g) = guidance {
        if !g.is_empty() {
            s.push_str(&format!("\nHuman guidance:\n{g}\n"));
        }
    }
    if !last_response.trim().is_empty() {
        s.push_str(&format!("\nYour previous final message:\n{}\n", clip(last_response, 8_000)));
    }
    s
}

pub fn clip(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let head: String = s.chars().take(max).collect();
    format!("{head}\n...[truncated]")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Only commits whose subject ends in this item's id are its own; a
    /// sibling's sweep in the same range is not attributed to it.
    #[test]
    fn commits_with_id_keeps_only_this_items_commits() {
        let id = "aca51627-1111-4222-8333-5c22ab7f0d11";
        let rec = |h: &str, s: &str, b: &str| format!("{h}\x1f{s}\x1f{b}\x1e\n");
        let log = [
            rec("abc1234", "iter: bundle loader (aca51627)", ""),
            rec("def5678", "iter: fixture cell (a180b52e)", ""),
            rec("0000000", "agent commit with (aca51627) in the middle", ""),
            rec("1111111", "ledger row", &format!("for {id}\n")),         // full id in the body only
            rec("2222222", "techreq R219 (5c22ab7f0d11)", ""),             // the webui's last-12 form
        ].concat();
        let got: Vec<String> = commits_with_id(&log, id).into_iter().map(|(h, _)| h).collect();
        assert_eq!(got, vec!["abc1234", "1111111", "2222222"]);
        assert!(commits_with_id(&log, "ffffffff-0000-0000-0000-000000000000").is_empty());
    }

    /// F7: the verifier is told the commits are from every attempt, and never
    /// a bare "none" while the item has commits; with no new head this
    /// attempt, earlier attempts' commits still count as work.
    #[test]
    fn evidence_never_says_none_while_commits_exist() {
        let ev = Evidence { head_before: "h".repeat(12), head_after: "h".repeat(12),
            commits: vec![("abc1234".into(), "ledger row".into())], commit_scope: "lock scope (1 paths)".into(), ..Default::default() };
        let d = ev.describe();
        assert!(d.contains("NO new commit in this attempt") && d.contains("all attempts: abc1234 \"ledger row\""), "{d}");
        assert!(!d.contains(": none"), "{d}");
        assert!(ev.has_work() && !ev.committed());
    }

    /// The evidence text tells the verifier the tree is shared, the scope the
    /// diffstat was limited to, the leftover count, and this item's commits.
    #[test]
    fn evidence_describes_the_shared_checkout_and_scope() {
        let ev = Evidence {
            result_subtype: "success".into(), num_turns: 3,
            head_before: "1111111111111".into(), head_after: "2222222222222".into(),
            diffstat: " a/x.rs | 1 +".into(), children: 0, open_reviews: 0,
            commit_scope: "lock scope (1 paths)".into(), outside_scope_dirty: 2,
            commits: vec![("abc1234".into(), "iter: thing (deadbeef)".into())],
            outside_scope_committed: vec![("LEDGER.md".into(), "abc1234".into())],
            other_scope_commits: vec![("ba2d6a2".into(), "iter: sibling (99998888)".into())],
            notes: vec![],
        };
        let d = ev.describe();
        assert!(d.contains("limited to its lock scope (lock scope (1 paths))"), "{d}");
        assert!(d.contains("2 path(s) outside that scope are still dirty (uncommitted) in the checkout and are not this item's"), "{d}");
        assert!(d.contains("Committed by this item outside its lock scope: LEDGER.md (abc1234)"), "{d}");
        assert!(d.contains("Commits by other items in this scope (not this item's work): ba2d6a2"), "{d}");
        assert!(d.contains("abc1234 \"iter: thing (deadbeef)\""), "{d}");
        let j = ev.to_json();
        assert_eq!(j["commit_scope"], "lock scope (1 paths)");
        assert_eq!(j["outside_scope_dirty"], 2);
        assert_eq!(j["commits"][0]["hash"], "abc1234");
        assert!(verifier_prompt("n", "r", "m", &ev).contains("outside the item's lock scope is NOT evidence"));
    }

    fn wi(id: &str, state: &str, createdby: &str, blockedby: &[&str]) -> iter_core::WorkItem {
        iter_core::WorkItem {
            id: id.into(), state: state.into(), createdby: createdby.into(),
            blockedby: blockedby.iter().map(|b| b.to_string()).collect(),
            ..Default::default()
        }
    }

    /// A declared block holds the item behind its OPEN blockers: an open
    /// blocker, a complete blocker with an open child (deep), a failed one;
    /// never a complete lineage, an unknown id, or (shallow) a blocker's child.
    #[test]
    fn waiting_on_lists_only_unsatisfied_blockers() {
        let items = vec![
            wi("open1", "queued", "", &[]),
            wi("done", "complete", "", &[]),
            wi("done_with_kid", "complete", "", &[]),
            wi("kid", "in-progress", "done_with_kid", &[]),
            wi("failed1", "failed", "", &[]),
        ];
        let me = wi("me", "in-progress", "", &["open1", "done", "done_with_kid", "failed1", "ghost"]);
        assert_eq!(waiting_on(&me, &items), vec!["open1", "done_with_kid", "failed1"]);
        let none = wi("me", "in-progress", "", &["done"]);
        assert!(waiting_on(&none, &items).is_empty());
        assert!(waiting_on(&wi("me", "in-progress", "", &[]), &items).is_empty(), "no blockers = nothing to wait on");
        let mut shallow = wi("me", "in-progress", "", &["done_with_kid"]);
        shallow.blockedby_shallow = true;
        assert!(waiting_on(&shallow, &items).is_empty(), "shallow ignores the blocker's open child");
        let row = waiting_row(2, "verifier", &["TC-ON-14".into()], "not all five tests", &["open1".into()], &Evidence::default());
        assert_eq!(row["verdict"], "waiting");
        assert_eq!(row["bounce"], 2);
        assert_eq!(row["waiting_on"], json!(["open1"]));
    }

    #[test]
    fn verdict_parses_wrapped_json_and_defaults_unclear() {
        let v = parse_verdict("Sure. {\"verdict\":\"incomplete\",\"open\":[\"file the ten items\"],\"reason\":\"waiting on review\"}");
        assert_eq!(v, Verdict::Incomplete { open: vec!["file the ten items".into()], reason: "waiting on review".into(), advice: Advice::default() });
        assert_eq!(parse_verdict("{\"verdict\":\"COMPLETE\"}"), Verdict::Complete);
        assert!(matches!(parse_verdict("no json here"), Verdict::Unparsed { .. }));
        assert!(matches!(parse_verdict("{\"verdict\":\"maybe\"}"), Verdict::Unclear { .. }));
        assert!(matches!(parse_verdict("{not json}"), Verdict::Unparsed { .. }));
    }

    fn row(order: i64, key: &str, value: Value) -> Value {
        json!({"id": "x", "order": order, "key": key, "valuetype": "json", "value": value})
    }

    #[test]
    fn gate_widget_accept_is_honored_only_while_live() {
        let mut w = question_widget("n", 2, 1, "why", &["a".into()], "last", &Advice::default(), &Evidence::default());
        w["fields"][0]["value"] = json!("accept");
        assert!(iter_core::widget::validate(&w).is_empty(), "gate widget must validate");
        let details = vec![row(0, "request", json!("r")), row(1, "response", json!("r1")), row(2, "question", w.clone())];
        assert!(accepted_by_human(&details));
        // a newer response row means the item ran again since: the accept is stale
        let mut stale = details.clone();
        stale.push(row(3, "response", json!("r2")));
        assert!(!accepted_by_human(&stale));
        // continue + guidance shows up in the feedback section
        w["fields"][0]["value"] = json!("continue");
        w["fields"][1]["value"] = json!("look in docs/");
        let details = vec![row(0, "request", json!("r")), row(1, "response", json!("r1")), row(2, "question", w)];
        let fb = feedback_section(&details, &[]);
        assert!(fb.contains("look in docs/") && fb.contains("r1"));
        assert!(feedback_section(&[row(0, "request", json!("r"))], &[]).is_empty());
    }

    /// Defect 1 (pdy-dev 2026-09-07): the count is always taken, so an item
    /// with one child never tells the verifier "created by this item: 0";
    /// defect 2: the re-run prompt lists what the item already filed, even
    /// when there was no bounce (a failed attempt may have filed items too).
    #[test]
    fn children_are_counted_and_listed_for_the_retry() {
        let items = vec![
            json!({"id": "child-1", "createdby": "me", "state": "queued", "agent": "code", "name": "secrets-store disk guard"}),
            json!({"id": "other", "createdby": "someone", "state": "queued", "agent": "code", "name": "x"}),
        ];
        let kids = children_of(&items, "me");
        assert_eq!(kids.len(), 1);
        let ev = Evidence { children: kids.len(), ..Default::default() };
        let text = ev.describe();
        assert!(text.contains("workitems created by this item: 1"), "{text}");
        assert!(!text.contains("created by this item: 0"));
        let fb = feedback_section(&[row(0, "request", json!("r"))], &kids);
        assert!(fb.contains("Work items this item has already created"));
        assert!(fb.contains("child-1 [queued] (code) secrets-store disk guard"));
        assert!(fb.contains("Do NOT file these again"));
        assert!(!fb.contains("Close-gate feedback"), "no bounce happened, so no gate feedback header");
        // with a bounce, both sections appear in order
        let details = vec![row(0, "request", json!("r")), row(1, "response", json!("r1")),
            row(2, "verify", json!({"bounce": 1, "reason": "waiting on review", "open": ["file the items"]}))];
        let fb = feedback_section(&details, &kids);
        assert!(fb.find("Close-gate feedback").unwrap() < fb.find("already created").unwrap());
    }

    #[test]
    fn open_reviews_counts_missing_disposition() {
        let details = vec![
            row(1, "review", json!({"text": "x"})),
            row(2, "review", json!({"text": "y", "disposition": "revised"})),
            row(3, "review", json!({"text": "z", "disposition": ""})),
        ];
        assert_eq!(open_reviews(&details), 2);
    }

    // ---------- close-gate question shape (plans/close_gate_question_shape.bugfix.md) ----------

    fn advice(q: &str, rec: &str) -> Advice {
        Advice { question: q.into(), recommendation: rec.into(), why: "the one child was auto-filed by the roll script".into() }
    }

    #[test]
    fn parse_verdict_reads_question_and_recommendation() {
        let v = parse_verdict(r#"{"verdict":"unclear","open":[],"reason":"1 item created vs none filed","question":"Was the one child filed by the roll script?","recommendation":"ACCEPT","why":"both statements hold"}"#);
        assert_eq!(v, Verdict::Unclear { reason: "verdict 'unclear': 1 item created vs none filed".into(),
            advice: Advice { question: "Was the one child filed by the roll script?".into(), recommendation: "accept".into(), why: "both statements hold".into() } });
        let old = parse_verdict(r#"{"verdict":"incomplete","open":["x"],"reason":"r"}"#);
        assert_eq!(old, Verdict::Incomplete { open: vec!["x".into()], reason: "r".into(), advice: Advice::default() });
    }

    #[test]
    fn widget_title_is_the_question_not_the_item_name() {
        let name = "roll the bundle server on corridor-dev1 ".repeat(5);
        let w = question_widget(&name, 2, 1, "why", &[], "report", &advice("Was the one child filed by the roll script?", "continue"), &Evidence::default());
        assert_eq!(w["title"], "Was the one child filed by the roll script?");
        assert!(!w["title"].as_str().unwrap().contains("roll the bundle"));
        let summary = w["summary"].as_str().unwrap();
        assert!(summary.starts_with("Recommendation: continue — the one child was auto-filed"), "{summary}");
        assert!(summary.contains("bounce 2 of 1 · item: roll the bundle"), "{summary}");
        assert!(iter_core::widget::validate(&w).is_empty());
    }

    #[test]
    fn widget_never_clips_question_reason_or_open() {
        let reason = format!("{}THE-DOUBT", "fine. ".repeat(330));
        let open: Vec<String> = (1..=12).map(|n| format!("obligation {n}")).collect();
        let w = question_widget("n", 1, 1, &reason, &open, &"R".repeat(9_000), &advice("Is it done?", "continue"), &Evidence::default());
        let d = w["detail"].as_str().unwrap();
        let head = &d[..d.find("Last response").unwrap()];
        assert!(head.contains(&reason) && head.contains("THE-DOUBT"));
        for o in &open {
            assert!(head.contains(o.as_str()), "{o}");
        }
        assert!(!head.contains("...[truncated]"));
        assert!(d.contains("first 1,500 characters") && d.ends_with("...[truncated]"));
    }

    #[test]
    fn radio_default_is_the_recommendation() {
        let w = question_widget("n", 1, 1, "r", &[], "x", &advice("Done?", "accept"), &Evidence::default());
        assert_eq!(w["fields"][0]["value"], "accept");
        let w = question_widget("n", 1, 1, "r", &[], "x", &Advice::default(), &Evidence::default());
        assert_eq!(w["fields"][0]["value"], "continue", "no recommendation = continue");
        assert!(w["title"].as_str().unwrap().ends_with('?'), "a question even with no advice");
    }

    #[test]
    fn deterministic_hold_makes_a_question() {
        let a = deterministic_advice(&["no new git commit was produced (closegate.requires_commit)".into()]);
        assert_eq!(a.question, "Is this item done although no git commit was produced?");
        assert_eq!(a.recommendation, "continue");
        let a = deterministic_advice(&["x".into(), "y".into()]);
        assert!(a.question.ends_with('?') && a.question.contains("2 of the close gate's checks"));
        // R4: two unparsable verifier answers -> accept only on a successful, committed run
        let ev = Evidence { result_subtype: "success".into(), head_before: "a".into(), head_after: "b".into(), ..Default::default() };
        assert_eq!(unparsed_advice(&ev).recommendation, "accept");
        assert_eq!(unparsed_advice(&Evidence::default()).recommendation, "continue");
        assert!(unparsed_advice(&ev).question.starts_with("The verifier could not judge this item"));
    }
}
