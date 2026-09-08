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

#[derive(Debug, Clone, PartialEq)]
pub enum Verdict {
    Complete,
    Incomplete { open: Vec<String>, reason: String },
    Unclear { reason: String },
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
        })
    }
    fn describe(&self) -> String {
        let mut s = String::new();
        s.push_str(&format!("- worker session ended with result subtype '{}' after {} turn(s)\n",
            if self.result_subtype.is_empty() { "unknown" } else { &self.result_subtype }, self.num_turns));
        if self.committed() {
            s.push_str(&format!("- git: new commit {}\n{}\n", &self.head_after[..12.min(self.head_after.len())],
                indent(&self.diffstat)));
        } else if self.head_after.is_empty() {
            s.push_str("- git: not a repository (no commit evidence)\n");
        } else {
            s.push_str("- git: NO new commit (the tree did not change)\n");
        }
        s.push_str(&format!("- workitems created by this item: {}\n", self.children));
        s.push_str(&format!("- review rows without a disposition: {}\n", self.open_reviews));
        s
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
but whose items were never filed are all INCOMPLETE.  You may read files to check a claim, but do not modify anything.\n\n\
Answer with exactly one json object and nothing else:\n\
{{\"verdict\": \"complete\" | \"incomplete\" | \"unclear\", \"open\": [\"each obligation still open, one per entry\"], \"reason\": \"one or two sentences\"}}\n\n\
# Workitem: {item_name}\n\n## Request\n{request}\n\n## Worker's final message\n{response}\n\n## Engine evidence\n{evidence}",
        marker = VERIFIER_MARKER,
        item_name = item_name,
        request = clip(request, 40_000),
        response = clip(response, 24_000),
        evidence = ev.describe(),
    )
}

/// First json object in the text; anything unparseable is `Unclear`.
pub fn parse_verdict(text: &str) -> Verdict {
    let Some(start) = text.find('{') else {
        return Verdict::Unclear { reason: format!("verifier returned no json: {}", clip(text, 300)) };
    };
    let Some(end) = text.rfind('}') else {
        return Verdict::Unclear { reason: format!("verifier returned no json: {}", clip(text, 300)) };
    };
    if end < start {
        return Verdict::Unclear { reason: "verifier returned malformed json".into() };
    }
    let v: Value = match serde_json::from_str(&text[start..=end]) {
        Ok(v) => v,
        Err(e) => return Verdict::Unclear { reason: format!("verifier json did not parse: {e}") },
    };
    let reason = v.get("reason").and_then(|r| r.as_str()).unwrap_or("").to_string();
    let open: Vec<String> = v
        .get("open")
        .and_then(|o| o.as_array())
        .map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).filter(|s| !s.trim().is_empty()).collect())
        .unwrap_or_default();
    match v.get("verdict").and_then(|x| x.as_str()).unwrap_or("").trim().to_ascii_lowercase().as_str() {
        "complete" => Verdict::Complete,
        "incomplete" => Verdict::Incomplete { open, reason },
        other => Verdict::Unclear {
            reason: if other.is_empty() { "verifier gave no verdict".into() } else { format!("verdict '{other}': {reason}") },
        },
    }
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

/// The question widget written when the gate hands the item to a human.
pub fn question_widget(item_name: &str, bounces: u32, reason: &str, open: &[String], last_response: &str) -> Value {
    let open_text = if open.is_empty() {
        "(none listed)".to_string()
    } else {
        open.iter().map(|o| format!("- {o}")).collect::<Vec<_>>().join("\n")
    };
    json!({
        "gate": GATE_WIDGET_KIND,
        "title": format!("Close gate held '{}' after {} attempt(s)", clip(item_name, 120), bounces),
        "summary": clip(reason, 400),
        "detail": format!("Open obligations:\n{}\n\nLast response:\n{}", open_text, clip(last_response, 6_000)),
        "fields": [
            {"key": "action", "label": "What next?", "type": "radio",
             "options": [
                {"value": "continue", "desc": "requeue; the agent gets your guidance below plus the open list"},
                {"value": "accept", "desc": "close as complete without running an agent"}
             ],
             "value": "continue"},
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

    #[test]
    fn verdict_parses_wrapped_json_and_defaults_unclear() {
        let v = parse_verdict("Sure. {\"verdict\":\"incomplete\",\"open\":[\"file the ten items\"],\"reason\":\"waiting on review\"}");
        assert_eq!(v, Verdict::Incomplete { open: vec!["file the ten items".into()], reason: "waiting on review".into() });
        assert_eq!(parse_verdict("{\"verdict\":\"COMPLETE\"}"), Verdict::Complete);
        assert!(matches!(parse_verdict("no json here"), Verdict::Unclear { .. }));
        assert!(matches!(parse_verdict("{\"verdict\":\"maybe\"}"), Verdict::Unclear { .. }));
        assert!(matches!(parse_verdict("{not json}"), Verdict::Unclear { .. }));
    }

    fn row(order: i64, key: &str, value: Value) -> Value {
        json!({"id": "x", "order": order, "key": key, "valuetype": "json", "value": value})
    }

    #[test]
    fn gate_widget_accept_is_honored_only_while_live() {
        let mut w = question_widget("n", 2, "why", &["a".into()], "last");
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
}
