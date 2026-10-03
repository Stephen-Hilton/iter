//! The `mock` provider (iter5 spec §5): deterministic, no model.  It drives
//! every automated test and e2e run.
//!
//! On a **work** turn the prompt is scanned for directive lines (the work
//! item's request carries them), one per line, leading whitespace ignored,
//! run top to bottom:
//!
//! ```text
//! mock: write <relpath> <<<single-line content>>>   write the file (relative to cwd; parents created; content + "\n")
//! mock: append <relpath> <<<text>>>                 append text + "\n" (file created when missing)
//! mock: run <shell>                                 bash -c in cwd with the agent env; non-zero exit = fail
//! mock: say <text>                                  the response text (the last `say` wins)
//! mock: fail <msg>                                  stop here: subtype error_during_execution, text <msg>
//! mock: ask <question>                              `$ITER_BIN ask --question <question>` (the iter shim)
//! mock: sleep <ms>                                  sleep, honouring stop requests and the timeout
//! ```
//! Unknown `mock:` lines are ignored (`mock: gate incomplete` is read by the
//! verifier only).  Default response `mock: done`.
//!
//! Other calls answer the way the engine parses them: the close-gate
//! **verifier** says `VERDICT: complete` (plus the verdict JSON) unless the
//! prompt holds a `mock: gate incomplete` line; the dedup **judge** answers
//! `{"candidates":[]}`; the GraphRAG **summary** agent gets one summary per
//! `<passage id=…>` / `<document id=…>`; everything else `mock: done`.
//! Directives are never run outside a work turn.
//!
//! Cost 0; tokens = prompt length / 4 in, text length / 4 out; session id
//! `mock-<uuid>` (a resumed session keeps its id).  Usage: `get_usage_mock`
//! reads `ITER_MOCK_USAGE_<ACCOUNT>` then `ITER_MOCK_USAGE` =
//! `<five_hour_pct>,<seven_day_pct>`, else 0,0.

use super::{AgentContext, DispatchOut, DispatchSettings, Role};
use crate::usage::Usage;
use serde_json::json;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

pub const DEFAULT_TEXT: &str = "mock: done";
// Failure texts say "mock run failed …", never "mock: …": a retry's prompt
// quotes the previous response, and a `mock:` line there would be parsed as
// a directive and run again (e2e 2026-10-02).
pub const GATE_INCOMPLETE: &str = "mock: gate incomplete";

/// One parsed directive.
#[derive(Debug, Clone, PartialEq)]
pub enum Directive {
    Write(String, String),
    Append(String, String),
    Run(String),
    Say(String),
    Fail(String),
    Ask(String),
    Sleep(u64),
}

/// `<relpath> <<<content>>>` -> (relpath, content); a missing `>>>` takes the rest of the line.
fn path_and_content(rest: &str) -> Option<(String, String)> {
    let (path, tail) = rest.split_once("<<<")?;
    let content = tail.strip_suffix(">>>").unwrap_or(tail);
    let path = path.trim();
    (!path.is_empty()).then(|| (path.to_string(), content.to_string()))
}

/// Every directive line in `prompt`, in order.
pub fn parse_directives(prompt: &str) -> Vec<Directive> {
    let mut out = Vec::new();
    for line in prompt.lines() {
        let Some(rest) = line.trim_start().strip_prefix("mock:") else { continue };
        let rest = rest.trim_start();
        let (verb, arg) = rest.split_once(char::is_whitespace).unwrap_or((rest, ""));
        let arg = arg.trim_end_matches(['\r']);
        let d = match verb {
            "write" => path_and_content(arg).map(|(p, c)| Directive::Write(p, c)),
            "append" => path_and_content(arg).map(|(p, c)| Directive::Append(p, c)),
            "run" if !arg.trim().is_empty() => Some(Directive::Run(arg.trim().to_string())),
            "say" => Some(Directive::Say(arg.trim().to_string())),
            "fail" => Some(Directive::Fail(arg.trim().to_string())),
            "ask" if !arg.trim().is_empty() => Some(Directive::Ask(arg.trim().to_string())),
            "sleep" => arg.trim().parse::<u64>().ok().map(Directive::Sleep),
            _ => None,
        };
        out.extend(d);
    }
    out
}

fn resolve(cwd: &Path, rel: &str) -> PathBuf {
    let p = Path::new(rel);
    if p.is_absolute() { p.to_path_buf() } else { cwd.join(p) }
}

fn env_of<'a>(ctx: &'a AgentContext, key: &str) -> Option<&'a str> {
    ctx.env.iter().rev().find(|(k, _)| k == key).map(|(_, v)| v.as_str())
}

fn shell(ctx: &AgentContext, cwd: &Path, script: &str, s: &DispatchSettings, deadline: Instant) -> Result<String, String> {
    let mut cmd = Command::new("bash");
    cmd.arg("-c").arg(script).current_dir(cwd).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    for (k, v) in &ctx.env {
        cmd.env(k, v);
    }
    let left = deadline.saturating_duration_since(Instant::now()).as_secs().max(1);
    crate::work::wait_with_stop(cmd, left, &s.stop)
}

fn session_id(ctx: &AgentContext) -> String {
    ctx.resume.clone().filter(|s| !s.trim().is_empty()).unwrap_or_else(|| format!("mock-{}", uuid::Uuid::new_v4()))
}

fn finish(ctx: &AgentContext, s: &DispatchSettings, text: String, subtype: &str) -> DispatchOut {
    let raw = json!({"type": "result", "subtype": subtype, "result": text, "provider": "mock", "account": s.account}).to_string();
    DispatchOut {
        input_tokens: (ctx.prompt.len() / 4) as u64,
        output_tokens: (text.len() / 4) as u64,
        text,
        subtype: subtype.into(),
        num_turns: 1,
        cost_usd: 0.0,
        session_id: session_id(ctx),
        raw,
        ..Default::default()
    }
}

/// The verifier's answer: complete, unless the item asks otherwise.
pub fn verifier_answer(prompt: &str) -> String {
    let incomplete = prompt.lines().any(|l| l.trim() == GATE_INCOMPLETE);
    if incomplete {
        format!(
            "VERDICT: incomplete\n{}",
            json!({"verdict": "incomplete", "open": ["the mock item asked for an incomplete verdict (mock: gate incomplete)"],
                   "reason": "mock verifier: the item carries `mock: gate incomplete`.",
                   "question": "Should the mock item be accepted as done?", "recommendation": "continue", "why": "mock directive"})
        )
    } else {
        format!(
            "VERDICT: complete\n{}",
            json!({"verdict": "complete", "open": [], "reason": "mock verifier: complete.", "question": "", "recommendation": "accept", "why": "mock"})
        )
    }
}

/// `id="…"` values of every `<tag id="…"` opening in `prompt`, with the
/// chapter idx values that follow each document.
fn tag_ids(prompt: &str, tag: &str) -> Vec<String> {
    let open = format!("<{tag} id=\"");
    prompt
        .match_indices(&open)
        .filter_map(|(i, _)| {
            let rest = &prompt[i + open.len()..];
            rest.find('"').map(|e| rest[..e].to_string())
        })
        .collect()
}

/// The Summary agent's answer: chunk summaries for passages, else doc +
/// chapter summaries for documents.
pub fn summary_answer(prompt: &str) -> String {
    let passages = tag_ids(prompt, "passage");
    if !passages.is_empty() {
        let sums: Vec<_> = passages.iter().map(|id| json!({"id": id, "summary": format!("mock summary of {id}")})).collect();
        return json!({"summaries": sums}).to_string();
    }
    let mut docs = Vec::new();
    for (i, _) in prompt.match_indices("<document id=\"") {
        let rest = &prompt[i + "<document id=\"".len()..];
        let Some(e) = rest.find('"') else { continue };
        let id = &rest[..e];
        let body = rest.find("</document>").map(|end| &rest[..end]).unwrap_or(rest);
        let chapters: Vec<_> = body
            .match_indices("<chapter idx=\"")
            .filter_map(|(j, _)| {
                let r = &body[j + "<chapter idx=\"".len()..];
                r.find('"').and_then(|k| r[..k].parse::<i64>().ok())
            })
            .map(|idx| json!({"idx": idx, "summary": format!("mock summary of chapter {idx}")}))
            .collect();
        docs.push(json!({"doc": id, "doc_summary": format!("mock summary of {id}"), "chapters": chapters}));
    }
    json!({"docs": docs}).to_string()
}

pub fn dispatch_agent_mock(_model: &str, ctx: &AgentContext, s: &DispatchSettings) -> Result<DispatchOut, String> {
    if let Some(why) = s.stop.check() {
        return Err(why);
    }
    match ctx.role {
        Role::Verifier => return Ok(finish(ctx, s, verifier_answer(&ctx.prompt), "success")),
        Role::Judge => return Ok(finish(ctx, s, json!({"candidates": []}).to_string(), "success")),
        Role::Summary => return Ok(finish(ctx, s, summary_answer(&ctx.prompt), "success")),
        Role::Work => {}
        _ => return Ok(finish(ctx, s, DEFAULT_TEXT.into(), "success")),
    }
    let cwd = if ctx.cwd.as_os_str().is_empty() { std::env::temp_dir() } else { ctx.cwd.clone() };
    let deadline = Instant::now() + s.timeout.max(Duration::from_millis(1));
    let mut text = DEFAULT_TEXT.to_string();
    for d in parse_directives(&ctx.prompt) {
        if let Some(why) = s.stop.check() {
            return Err(why);
        }
        if Instant::now() > deadline {
            return Err(format!("timed out after {}s", s.timeout.as_secs()));
        }
        match d {
            Directive::Write(rel, content) => {
                let p = resolve(&cwd, &rel);
                if let Some(dir) = p.parent() {
                    let _ = std::fs::create_dir_all(dir);
                }
                if let Err(e) = std::fs::write(&p, format!("{content}\n")) {
                    return Ok(finish(ctx, s, format!("mock write {rel} failed: {e}"), "error_during_execution"));
                }
            }
            Directive::Append(rel, content) => {
                use std::io::Write;
                let p = resolve(&cwd, &rel);
                if let Some(dir) = p.parent() {
                    let _ = std::fs::create_dir_all(dir);
                }
                let r = std::fs::OpenOptions::new().create(true).append(true).open(&p).and_then(|mut f| f.write_all(format!("{content}\n").as_bytes()));
                if let Err(e) = r {
                    return Ok(finish(ctx, s, format!("mock append {rel} failed: {e}"), "error_during_execution"));
                }
            }
            Directive::Run(script) => {
                if let Err(e) = shell(ctx, &cwd, &script, s, deadline) {
                    if e.starts_with(crate::work::REVOKED_PREFIX) || e == crate::work::STOPPED_BY_USER {
                        return Err(e);
                    }
                    return Ok(finish(ctx, s, format!("mock run failed: {e}"), "error_during_execution"));
                }
            }
            Directive::Say(t) => text = t,
            Directive::Fail(msg) => return Ok(finish(ctx, s, msg, "error_during_execution")),
            Directive::Ask(q) => {
                let bin = env_of(ctx, "ITER_BIN").unwrap_or("iter").to_string();
                let script = format!("{} ask --question {}", sh_quote(&bin), sh_quote(&q));
                if let Err(e) = shell(ctx, &cwd, &script, s, deadline) {
                    return Ok(finish(ctx, s, format!("mock ask failed: {e}"), "error_during_execution"));
                }
            }
            Directive::Sleep(ms) => {
                let until = Instant::now() + Duration::from_millis(ms);
                while Instant::now() < until {
                    if let Some(why) = s.stop.check() {
                        return Err(why);
                    }
                    if Instant::now() > deadline {
                        return Err(format!("timed out after {}s", s.timeout.as_secs()));
                    }
                    std::thread::sleep(Duration::from_millis(20).min(until.saturating_duration_since(Instant::now())));
                }
            }
        }
    }
    Ok(finish(ctx, s, text, "success"))
}

fn sh_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// `ITER_MOCK_USAGE_<ACCOUNT>` (account upper-cased, non-alphanumerics `_`)
/// then `ITER_MOCK_USAGE`: `<five_hour_pct>,<seven_day_pct>`; else 0,0.
pub fn mock_usage_keys(account: &str) -> Vec<String> {
    let suffix: String = account.trim().chars().map(|c| if c.is_ascii_alphanumeric() { c.to_ascii_uppercase() } else { '_' }).collect();
    let mut v = Vec::new();
    if !suffix.is_empty() {
        v.push(format!("ITER_MOCK_USAGE_{suffix}"));
    }
    v.push("ITER_MOCK_USAGE".into());
    v
}

pub fn get_usage_mock(account: &str) -> Option<Usage> {
    let raw = mock_usage_keys(account)
        .iter()
        .find_map(|k| crate::envstore::get(k).or_else(|| std::env::var(k).ok().filter(|v| !v.trim().is_empty())))
        .unwrap_or_default();
    let mut it = raw.split(',').map(|x| x.trim().parse::<f64>().unwrap_or(0.0));
    let five = it.next().unwrap_or(0.0).clamp(0.0, 100.0);
    let seven = it.next().unwrap_or(0.0).clamp(0.0, 100.0);
    Some(Usage {
        ts: Some(chrono::Utc::now()),
        five_hour_pct: five,
        seven_day_pct: seven,
        status: if five >= 100.0 || seven >= 100.0 { "rejected".into() } else { "allowed".into() },
        source: "mock".into(),
        ..Default::default()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("iter5-mock-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }
    fn work(prompt: &str, cwd: &Path) -> AgentContext {
        AgentContext { prompt: prompt.into(), cwd: cwd.to_path_buf(), ..Default::default() }
    }

    #[test]
    fn directives_parse_in_order_and_ignore_the_rest() {
        let p = "# Work item\nTitle: mock: say not-a-directive\n  mock: write a/b.txt <<<hello world>>>\nmock: append log.txt <<<one>>>\n\
                 mock: run echo hi > ran.txt\nmock: say finished\nmock: gate incomplete\nmock: sleep 5\nmock: ask Which one?\nmock: fail nope\nmock: bogus x";
        assert_eq!(parse_directives(p), vec![
            Directive::Write("a/b.txt".into(), "hello world".into()),
            Directive::Append("log.txt".into(), "one".into()),
            Directive::Run("echo hi > ran.txt".into()),
            Directive::Say("finished".into()),
            Directive::Sleep(5),
            Directive::Ask("Which one?".into()),
            Directive::Fail("nope".into()),
        ]);
    }

    #[test]
    fn work_turn_writes_appends_runs_and_says() {
        let d = tmp("work");
        std::fs::write(d.join("log.txt"), "zero\n").unwrap();
        let ctx = AgentContext {
            env: vec![("MOCK_T_VAR".into(), "v1".into())],
            ..work("mock: write sub/x.txt <<<hello>>>\nmock: append log.txt <<<one>>>\nmock: run echo $MOCK_T_VAR > ran.txt\nmock: sleep 10\nmock: say all good", &d)
        };
        let out = dispatch_agent_mock("", &ctx, &DispatchSettings::default()).unwrap();
        assert_eq!((out.text.as_str(), out.subtype.as_str(), out.cost_usd, out.num_turns), ("all good", "success", 0.0, 1));
        assert_eq!(std::fs::read_to_string(d.join("sub/x.txt")).unwrap(), "hello\n");
        assert_eq!(std::fs::read_to_string(d.join("log.txt")).unwrap(), "zero\none\n");
        assert_eq!(std::fs::read_to_string(d.join("ran.txt")).unwrap(), "v1\n");
        assert_eq!(out.input_tokens, (ctx.prompt.len() / 4) as u64);
        assert!(out.session_id.starts_with("mock-"));
        // a resumed session keeps its id; no directives = the default text
        let again = dispatch_agent_mock("", &AgentContext { resume: Some(out.session_id.clone()), ..work("next turn", &d) }, &DispatchSettings::default()).unwrap();
        assert_eq!((again.text.as_str(), again.session_id.as_str()), (DEFAULT_TEXT, out.session_id.as_str()));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn fail_and_a_failing_run_end_the_turn_as_errors() {
        let d = tmp("fail");
        let out = dispatch_agent_mock("", &work("mock: fail broke it\nmock: write never.txt <<<x>>>", &d), &DispatchSettings::default()).unwrap();
        assert_eq!((out.text.as_str(), out.subtype.as_str()), ("broke it", "error_during_execution"));
        assert!(!d.join("never.txt").exists(), "nothing after fail runs");
        let out = dispatch_agent_mock("", &work("mock: run exit 3", &d), &DispatchSettings::default()).unwrap();
        assert_eq!(out.subtype, "error_during_execution");
        assert!(out.text.starts_with("mock run failed: exit Some(3)"), "{}", out.text);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn sleep_honours_the_timeout() {
        let d = tmp("sleep");
        let s = DispatchSettings { timeout: Duration::from_millis(100), ..Default::default() };
        let started = Instant::now();
        let e = dispatch_agent_mock("", &work("mock: sleep 5000", &d), &s).unwrap_err();
        assert!(e.starts_with("timed out"), "{e}");
        assert!(started.elapsed() < Duration::from_secs(2));
        let _ = std::fs::remove_dir_all(&d);
    }

    /// A stop request for the run ends a sleeping mock turn at once.
    #[test]
    fn sleep_is_ended_by_a_stop_request() {
        let d = tmp("stop");
        let w = "mockstop-0000-4000-8000-000000000001";
        let s = DispatchSettings { stop: super::super::StopCheck { workid: w.into(), lease: "L".into() }, ..Default::default() };
        let t = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(150));
            crate::work::STOP_REQUESTED.lock().unwrap().push(w.to_string());
        });
        let e = dispatch_agent_mock("", &work("mock: sleep 10000", &d), &s).unwrap_err();
        t.join().unwrap();
        assert_eq!(e, crate::work::STOPPED_BY_USER);
        crate::work::STOP_REQUESTED.lock().unwrap().retain(|x| x != w);
        let _ = std::fs::remove_dir_all(&d);
    }

    /// `mock: ask` calls the iter shim named by ITER_BIN with --question.
    #[cfg(unix)]
    #[test]
    fn ask_calls_the_iter_shim() {
        use std::os::unix::fs::PermissionsExt;
        let d = tmp("ask");
        let shim = d.join("iter");
        std::fs::write(&shim, "#!/bin/sh\nprintf '%s|' \"$@\" > asked.txt\n").unwrap();
        std::fs::set_permissions(&shim, std::fs::Permissions::from_mode(0o755)).unwrap();
        let ctx = AgentContext { env: vec![("ITER_BIN".into(), shim.to_string_lossy().into_owned())], ..work("mock: ask Which DB, 'pg' or sqlite?", &d) };
        let out = dispatch_agent_mock("", &ctx, &DispatchSettings::default()).unwrap();
        assert_eq!(out.subtype, "success");
        assert_eq!(std::fs::read_to_string(d.join("asked.txt")).unwrap(), "ask|--question|Which DB, 'pg' or sqlite?|");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn verifier_judge_and_summary_answers_parse_and_never_run_directives() {
        let d = tmp("roles");
        let p = "## Request\nmock: write touched.txt <<<x>>>\nmock: say hi";
        let v = dispatch_agent_mock("", &AgentContext { role: Role::Verifier, ..work(p, &d) }, &DispatchSettings::default()).unwrap();
        assert!(v.text.starts_with("VERDICT: complete"));
        assert_eq!(crate::gate::parse_verdict(&v.text), crate::gate::Verdict::Complete);
        let inc = dispatch_agent_mock("", &AgentContext { role: Role::Verifier, ..work(&format!("{p}\n{GATE_INCOMPLETE}"), &d) }, &DispatchSettings::default()).unwrap();
        assert!(matches!(crate::gate::parse_verdict(&inc.text), crate::gate::Verdict::Incomplete { .. }), "{}", inc.text);
        let j = dispatch_agent_mock("", &AgentContext { role: Role::Judge, ..work(p, &d) }, &DispatchSettings::default()).unwrap();
        assert_eq!(iter_core::dedup::parse_judgements(&j.text).unwrap().len(), 0);
        let e = dispatch_agent_mock("", &AgentContext { role: Role::Explain, ..work(p, &d) }, &DispatchSettings::default()).unwrap();
        assert_eq!(e.text, DEFAULT_TEXT);
        assert!(!d.join("touched.txt").exists(), "directives run on work turns only");
        let sp = "<passage id=\"c1\" document=\"D\">t</passage>\n<passage id=\"c2\" document=\"D\">u</passage>";
        let s = dispatch_agent_mock("", &AgentContext { role: Role::Summary, ..work(sp, &d) }, &DispatchSettings::default()).unwrap();
        let v: serde_json::Value = serde_json::from_str(&s.text).unwrap();
        assert_eq!(v["summaries"][1]["id"], "c2");
        let dp = "<document id=\"d1\" title=\"T\">\n<chapter idx=\"0\" title=\"a\">\n- x\n</chapter>\n<chapter idx=\"2\" title=\"b\">\n</chapter>\n</document>\n";
        let s = dispatch_agent_mock("", &AgentContext { role: Role::Summary, ..work(dp, &d) }, &DispatchSettings::default()).unwrap();
        let v: serde_json::Value = serde_json::from_str(&s.text).unwrap();
        assert_eq!((v["docs"][0]["doc"].as_str(), v["docs"][0]["chapters"][1]["idx"].as_i64()), (Some("d1"), Some(2)));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn usage_reads_the_account_key_then_the_global_key() {
        crate::envstore::set_for_test("ITER_MOCK_USAGE_MOCK_ACCT_1", "81.5, 12");
        let u = get_usage_mock("mock-acct.1").unwrap();
        assert_eq!((u.five_hour_pct, u.seven_day_pct, u.source.as_str()), (81.5, 12.0, "mock"));
        assert_eq!(mock_usage_keys("a b"), vec!["ITER_MOCK_USAGE_A_B".to_string(), "ITER_MOCK_USAGE".into()]);
        // unset everywhere -> 0,0 (assuming the test env has no ITER_MOCK_USAGE)
        if std::env::var("ITER_MOCK_USAGE").is_err() && crate::envstore::get("ITER_MOCK_USAGE").is_none() {
            let z = get_usage_mock("mock-acct-unset-zz").unwrap();
            assert_eq!((z.five_hour_pct, z.seven_day_pct), (0.0, 0.0));
        }
        crate::envstore::unset_for_test("ITER_MOCK_USAGE_MOCK_ACCT_1");
    }
}
