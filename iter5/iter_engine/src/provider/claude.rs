//! The `claude` provider: one headless Claude Code process per turn
//! (`claude -p … --output-format stream-json --verbose`), billed to the
//! account's token (`CLAUDE_CODE_OAUTH_TOKEN`).  Usage comes from the
//! stream's `rate_limit_event` line, else (idle, no dispatch) the direct
//! 1-token probe in crate::usage.

use super::{AgentContext, DispatchOut, DispatchSettings};
use crate::usage::Usage;
use serde_json::Value;
use std::process::{Command, Stdio};

/// The CLI to run: `$ITER_CLAUDE_BIN` (tests, odd installs), else `claude` on PATH.
pub fn claude_bin() -> String {
    std::env::var("ITER_CLAUDE_BIN").ok().filter(|b| !b.trim().is_empty()).unwrap_or_else(|| "claude".into())
}

/// The argv after the binary, in a fixed order (tests read it).
pub fn claude_args(model: &str, ctx: &AgentContext, s: &DispatchSettings) -> Vec<String> {
    let mut a: Vec<String> = vec!["-p".into(), ctx.prompt.clone(), "--output-format".into(), "stream-json".into(), "--verbose".into()];
    if let Some(sid) = ctx.resume.as_ref().filter(|x| !x.trim().is_empty()) {
        a.push("--resume".into());
        a.push(sid.clone());
    }
    if !model.trim().is_empty() {
        a.push("--model".into());
        a.push(model.trim().into());
    }
    if let Some(t) = &ctx.allowed_tools {
        a.push("--allowedTools".into());
        a.push(t.clone());
    }
    if let Some(n) = s.max_turns {
        a.push("--max-turns".into());
        a.push(n.to_string());
    }
    if let Some(m) = &ctx.mcp_config {
        a.push("--mcp-config".into());
        a.push(m.to_string_lossy().into_owned());
    }
    a.extend(ctx.extra_args.iter().cloned());
    a
}

/// Windows caps a whole command line at 32,767 UTF-16 units; a prompt longer
/// than this goes to `claude -p` on stdin instead of argv there.
const WINDOWS_ARGV_PROMPT_MAX: usize = 16_000;

/// Split the prompt off argv when it would not fit: (args, stdin text).
pub fn args_and_stdin(model: &str, ctx: &AgentContext, s: &DispatchSettings, argv_prompt_max: Option<usize>) -> (Vec<String>, Option<String>) {
    let mut args = claude_args(model, ctx, s);
    match argv_prompt_max {
        Some(max) if ctx.prompt.len() > max => {
            args.remove(1); // the prompt, right after "-p"
            (args, Some(ctx.prompt.clone()))
        }
        _ => (args, None),
    }
}

pub fn dispatch_agent_claude(model: &str, ctx: &AgentContext, s: &DispatchSettings) -> Result<DispatchOut, String> {
    let mut cmd = Command::new(claude_bin());
    let (args, input) = args_and_stdin(model, ctx, s, cfg!(windows).then_some(WINDOWS_ARGV_PROMPT_MAX));
    cmd.args(args);
    // route billing to the account's token; a named account without one never
    // reaches here (provider::call / work::resolve_account_token refuse it)
    if let Some(tok) = &s.token {
        cmd.env("CLAUDE_CODE_OAUTH_TOKEN", tok);
    }
    cmd.env_remove("ANTHROPIC_API_KEY").env_remove("ANTHROPIC_AUTH_TOKEN");
    for (k, v) in &ctx.env {
        cmd.env(k, v);
    }
    let cwd = if ctx.cwd.as_os_str().is_empty() { std::env::temp_dir() } else { ctx.cwd.clone() };
    cmd.current_dir(cwd).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    let raw = crate::work::wait_with_stop_input(cmd, input, s.timeout.as_secs(), &s.stop)?;
    let (sid, mut out) = parse_stream(&raw);
    if out.session_id.is_empty() {
        out.session_id = sid;
    }
    out.raw = raw;
    Ok(out)
}

/// Parse `--output-format stream-json` output: one JSON object per line; the
/// `result` line becomes the DispatchOut (+ session id for `--resume`).  A
/// lone result object (older CLIs, test doubles) or plain text still parse.
/// Usage is not touched here (see `get_usage_claude`).
pub fn parse_stream(raw: &str) -> (String, DispatchOut) {
    let mut sid = String::new();
    let mut result: Option<DispatchOut> = None;
    for line in raw.lines() {
        let line = line.trim();
        if !line.starts_with('{') {
            continue;
        }
        let Ok(v) = serde_json::from_str::<Value>(line) else { continue };
        if v.get("type").and_then(|t| t.as_str()) == Some("result") {
            if let Some(s) = v.get("session_id").and_then(|s| s.as_str()) {
                sid = s.to_string();
            }
            result = Some(parse_result_json(line));
        }
    }
    if let Some(out) = result {
        return (sid, out);
    }
    let trimmed = raw.trim();
    let candidate = trimmed.find('{').map(|i| &trimmed[i..]).unwrap_or("");
    if let Ok(v) = serde_json::from_str::<Value>(candidate) {
        sid = v.get("session_id").and_then(|s| s.as_str()).unwrap_or("").to_string();
    }
    (sid, parse_result_json(raw))
}

/// The result object — the last line of stream-json, or all of
/// `--output-format json`.  Anything else is plain text output.
pub fn parse_result_json(raw: &str) -> DispatchOut {
    let trimmed = raw.trim();
    let candidate = trimmed.find('{').map(|i| &trimmed[i..]).unwrap_or("");
    if let Ok(v) = serde_json::from_str::<Value>(candidate) {
        if v.get("type").and_then(|t| t.as_str()) == Some("result") || v.get("result").is_some() {
            let text = match v.get("result") {
                Some(Value::String(s)) => s.clone(),
                Some(other) if !other.is_null() => other.to_string(),
                _ => String::new(),
            };
            let u = |k: &str| v.get("usage").and_then(|u| u.get(k)).and_then(|n| n.as_u64()).unwrap_or(0);
            return DispatchOut {
                text,
                subtype: v.get("subtype").and_then(|s| s.as_str()).unwrap_or("success").to_string(),
                num_turns: v.get("num_turns").and_then(|n| n.as_u64()).unwrap_or(0),
                cost_usd: v.get("total_cost_usd").and_then(|c| c.as_f64()).unwrap_or(0.0),
                input_tokens: u("input_tokens"),
                output_tokens: u("output_tokens"),
                cache_read_tokens: u("cache_read_input_tokens"),
                cache_create_tokens: u("cache_creation_input_tokens"),
                session_id: v.get("session_id").and_then(|s| s.as_str()).unwrap_or("").to_string(),
                raw: String::new(),
            };
        }
    }
    DispatchOut::plain(raw.to_string())
}

/// The last `rate_limit_event` in a dispatch's raw stream; with no dispatch
/// output (the idle probe) and a token, the direct 1-token probe.  A
/// dispatch whose stream carried no event reports nothing rather than
/// spending a probe after every turn.
pub fn get_usage_claude(token: Option<&str>, last: Option<&DispatchOut>) -> Option<Usage> {
    match last {
        Some(out) => out
            .raw
            .lines()
            .map(str::trim)
            .filter(|l| l.starts_with('{'))
            .filter_map(|l| serde_json::from_str::<Value>(l).ok())
            .filter_map(|v| crate::usage::usage_from_stream_event(&v))
            .last(),
        None => {
            let tok = token.filter(|t| !t.trim().is_empty())?;
            match crate::usage::probe(tok) {
                Ok(u) => Some(u),
                Err(e) => {
                    eprintln!("[engine] usage probe failed: {e}");
                    None
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn result_json_is_parsed_and_text_falls_back() {
        let out = parse_result_json(r#"{"type":"result","subtype":"error_max_turns","num_turns":40,"result":"partial"}"#);
        assert_eq!((out.text.as_str(), out.subtype.as_str(), out.num_turns), ("partial", "error_max_turns", 40));
        let out = parse_result_json("plain words from an older cli");
        assert_eq!((out.subtype.as_str(), out.text.as_str()), ("success", "plain words from an older cli"));
        let out = parse_result_json("warn: x\n{\"type\":\"result\",\"subtype\":\"success\",\"result\":\"ok\"}");
        assert_eq!(out.text, "ok");
    }

    /// The stream yields the result and session id; usage is read from the
    /// raw stream by get_usage (not by the parser).
    #[test]
    fn stream_yields_result_and_get_usage_reads_the_rate_limit_event() {
        let raw = concat!(
            "{\"type\":\"system\",\"subtype\":\"init\",\"session_id\":\"sid-1\"}\n",
            "{\"type\":\"rate_limit_event\",\"rate_limit_info\":{\"status\":\"allowed\",\"isUsingOverage\":false,",
            "\"unifiedWindows\":{\"five_hour\":{\"utilization\":0.25,\"resetsAt\":99999999999},",
            "\"seven_day\":{\"utilization\":0.5,\"resetsAt\":99999999999}}}}\n",
            "{\"type\":\"result\",\"subtype\":\"success\",\"session_id\":\"sid-1\",\"num_turns\":2,",
            "\"total_cost_usd\":0.01,\"usage\":{\"input_tokens\":10,\"output_tokens\":3,",
            "\"cache_read_input_tokens\":5000,\"cache_creation_input_tokens\":700},\"result\":\"done\"}\n"
        );
        let (sid, mut out) = parse_stream(raw);
        assert_eq!(sid, "sid-1");
        assert_eq!((out.text.as_str(), out.subtype.as_str(), out.num_turns, out.input_tokens), ("done", "success", 2, 10));
        assert_eq!((out.cache_read_tokens, out.cache_create_tokens), (5000, 700));
        out.raw = raw.to_string();
        let u = get_usage_claude(Some("tok"), Some(&out)).expect("usage from the stream");
        assert!((u.five_hour_pct - 25.0).abs() < 1e-9 && (u.seven_day_pct - 50.0).abs() < 1e-9);
        assert_eq!(u.source, "stream");
        // a stream without the event: nothing (no probe after a dispatch)
        let quiet = DispatchOut { raw: "{\"type\":\"result\",\"result\":\"x\"}".into(), ..Default::default() };
        assert!(get_usage_claude(Some("tok"), Some(&quiet)).is_none());
        // idle without a token: nothing to probe with
        assert!(get_usage_claude(None, None).is_none());
        let (sid, out) = parse_stream("{\"type\":\"result\",\"subtype\":\"success\",\"session_id\":\"s2\",\"result\":\"x\"}");
        assert_eq!((sid.as_str(), out.text.as_str()), ("s2", "x"));
    }

    #[test]
    fn args_carry_resume_model_tools_turns_and_mcp_in_order() {
        let ctx = AgentContext {
            prompt: "P".into(),
            resume: Some("sid".into()),
            allowed_tools: Some("Read,Glob".into()),
            mcp_config: Some("/tmp/m.json".into()),
            extra_args: vec!["--disallowedTools".into(), "Bash".into()],
            ..Default::default()
        };
        let s = DispatchSettings { max_turns: Some(12), ..Default::default() };
        assert_eq!(
            claude_args("sonnet", &ctx, &s),
            ["-p", "P", "--output-format", "stream-json", "--verbose", "--resume", "sid", "--model", "sonnet",
             "--allowedTools", "Read,Glob", "--max-turns", "12", "--mcp-config", "/tmp/m.json", "--disallowedTools", "Bash"]
                .map(String::from)
                .to_vec()
        );
    }

    #[test]
    fn a_long_prompt_moves_to_stdin_only_past_the_cap() {
        let s = DispatchSettings::default();
        let short = AgentContext { prompt: "P".into(), ..Default::default() };
        assert_eq!(args_and_stdin("", &short, &s, Some(10)), (claude_args("", &short, &s), None));
        let long = AgentContext { prompt: "x".repeat(11), ..Default::default() };
        let (args, input) = args_and_stdin("haiku", &long, &s, Some(10));
        assert_eq!(args, ["-p", "--output-format", "stream-json", "--verbose", "--model", "haiku"].map(String::from).to_vec());
        assert_eq!(input.as_deref(), Some("xxxxxxxxxxx"));
        assert_eq!(args_and_stdin("", &long, &s, None).1, None);
    }

    /// A stand-in `claude` binary: the dispatch runs it with the token in
    /// CLAUDE_CODE_OAUTH_TOKEN, the agent env and the cwd, and parses its stream.
    #[cfg(unix)]
    #[test]
    fn dispatch_runs_the_cli_with_token_env_and_cwd() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("iter5-fake-claude-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let bin = dir.join("claude");
        std::fs::write(&bin, "#!/bin/sh\necho \"{\\\"type\\\":\\\"result\\\",\\\"subtype\\\":\\\"success\\\",\\\"session_id\\\":\\\"S\\\",\\\"result\\\":\\\"$CLAUDE_CODE_OAUTH_TOKEN $ITER_WORKID $(basename $(pwd))\\\"}\"\n").unwrap();
        std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
        // ITER_CLAUDE_BIN is read per call; only this test sets it
        let ctx = AgentContext { prompt: "x".into(), cwd: dir.clone(), env: vec![("ITER_WORKID".into(), "W1".into())], ..Default::default() };
        let s = DispatchSettings { token: Some("TK".into()), ..Default::default() };
        // SAFETY: test-only variable no other test reads
        unsafe { std::env::set_var("ITER_CLAUDE_BIN", &bin) };
        let out = dispatch_agent_claude("", &ctx, &s).unwrap();
        unsafe { std::env::remove_var("ITER_CLAUDE_BIN") };
        let base = dir.file_name().unwrap().to_string_lossy().into_owned();
        assert_eq!(out.text, format!("TK W1 {base}"));
        assert_eq!(out.session_id, "S");
        assert!(out.raw.contains("\"result\""));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
