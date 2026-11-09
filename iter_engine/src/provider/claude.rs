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

/// The argv after the binary, in a fixed order (tests read it).  It never
/// carries the prompt: `-p` with no prompt argument reads it from stdin
/// (`args_and_stdin`).  Linux refuses any single argument over 131,071 bytes
/// (E2BIG, "Argument list too long") and Windows a command line over 32,767
/// characters; prompts pass both (docs/bugfixes/2026-11-08_agent_prompt_via_stdin.md).
pub fn claude_args(model: &str, ctx: &AgentContext, s: &DispatchSettings) -> Vec<String> {
    let mut a: Vec<String> = vec!["-p".into(), "--output-format".into(), "stream-json".into(), "--verbose".into()];
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

/// (args, stdin text): the prompt always goes to `claude -p` on stdin, on
/// every operating system, written from memory through a pipe
/// (crate::work::wait_scoped); argv never carries it.
pub fn args_and_stdin(model: &str, ctx: &AgentContext, s: &DispatchSettings) -> (Vec<String>, String) {
    (claude_args(model, ctx, s), ctx.prompt.clone())
}

pub fn dispatch_agent_claude(model: &str, ctx: &AgentContext, s: &DispatchSettings) -> Result<DispatchOut, String> {
    let mut cmd = Command::new(claude_bin());
    let (args, input) = args_and_stdin(model, ctx, s);
    cmd.args(args);
    // no account token but this session's own: the process may hold others
    crate::envstore::scrub(&mut cmd);
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
    // stdin is a pipe the prompt is written into (wait_scoped, given `input`)
    cmd.current_dir(cwd).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
    // its own cgroup with the session memory limit (crate::memlimit), named
    // after the item it works on
    let label = session_label(ctx);
    let raw = crate::work::wait_in_session(cmd, Some(input), s.timeout.as_secs(), &s.stop, "agent", &label)?;
    let (sid, mut out) = parse_stream(&raw);
    if out.session_id.is_empty() {
        out.session_id = sid;
    }
    out.raw = raw;
    Ok(out)
}

/// The name a session's cgroup and log lines carry: its item (`ITER_WORKID`,
/// short), else what the call is for (`verifier`, `summary`, …).
pub fn session_label(ctx: &AgentContext) -> String {
    match ctx.env.iter().rev().find(|(k, _)| k == "ITER_WORKID").map(|(_, v)| v.trim()).filter(|v| !v.is_empty()) {
        Some(w) => { let c: Vec<char> = w.chars().collect(); c[c.len().saturating_sub(12)..].iter().collect() }
        None => format!("{:?}", ctx.role).to_ascii_lowercase(),
    }
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
            ["-p", "--output-format", "stream-json", "--verbose", "--resume", "sid", "--model", "sonnet",
             "--allowedTools", "Read,Glob", "--max-turns", "12", "--mcp-config", "/tmp/m.json", "--disallowedTools", "Bash"]
                .map(String::from)
                .to_vec()
        );
    }

    /// T2 (docs/bugfixes/2026-11-08_agent_prompt_via_stdin.md §5): argv
    /// carries no prompt on any operating system, whatever its size; the
    /// prompt is always the stdin text.  No `cfg` gate.
    /// Breaks again if: a size cap or the `cfg!(windows)` condition is reinstated.
    #[test]
    fn t2_argv_carries_no_prompt_on_any_os() {
        let s = DispatchSettings::default();
        for p in ["P".to_string(), "x".repeat(16_000), "x".repeat(16_001)] {
            let ctx = AgentContext { prompt: p.clone(), ..Default::default() };
            let (args, input) = args_and_stdin("haiku", &ctx, &s);
            assert_eq!(args, ["-p", "--output-format", "stream-json", "--verbose", "--model", "haiku"].map(String::from).to_vec(), "prompt of {} bytes", p.len());
            assert_eq!(input, p, "prompt of {} bytes", p.len());
        }
    }

    /// ITER_CLAUDE_BIN is process-wide: the dispatch tests take turns.
    static CLAUDE_BIN_ENV: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// The stand-in `claude` (examples/fake_claude.rs): `cargo test` builds it
    /// next to the test binary (target/<profile>/examples); else build it.
    fn fake_claude() -> std::path::PathBuf {
        static BIN: std::sync::OnceLock<std::path::PathBuf> = std::sync::OnceLock::new();
        BIN.get_or_init(|| {
            let exe = std::env::current_exe().unwrap();
            let profile_dir = exe.parent().and_then(|d| d.parent()).unwrap().to_path_buf();
            let bin = profile_dir.join("examples").join(format!("fake_claude{}", std::env::consts::EXE_SUFFIX));
            if !bin.is_file() {
                let mut c = std::process::Command::new(env!("CARGO"));
                c.args(["build", "--example", "fake_claude", "--manifest-path", concat!(env!("CARGO_MANIFEST_DIR"), "/Cargo.toml")]);
                if !cfg!(debug_assertions) {
                    c.arg("--release");
                }
                let st = c.status().expect("run cargo build --example fake_claude");
                assert!(st.success() && bin.is_file(), "could not build the stand-in claude at {}", bin.display());
            }
            bin
        })
        .clone()
    }

    /// What one stand-in dispatch gave: its result, its working folder
    /// (stdin.bin, argv.bin), the engine log lines it wrote, how long it took.
    struct FakeRun {
        result: Result<DispatchOut, String>,
        dir: std::path::PathBuf,
        log: Vec<String>,
        took: std::time::Duration,
    }

    /// Dispatch `prompt` through `dispatch_agent_claude` (the engine's own
    /// in-memory path) to the stand-in in `mode`, under a 30 s watchdog.
    fn dispatch_fake(tag: &str, prompt: String, mode: &str, timeout_secs: u64) -> FakeRun {
        let bin = fake_claude();
        let dir = std::env::temp_dir().join(format!("iter-fake-claude-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let ctx = AgentContext {
            prompt,
            cwd: dir.clone(),
            env: vec![("ITER_WORKID".into(), format!("fake-{tag}")), ("FAKE_CLAUDE_MODE".into(), mode.into())],
            ..Default::default()
        };
        let s = DispatchSettings { timeout: std::time::Duration::from_secs(timeout_secs), ..Default::default() };
        let _env = CLAUDE_BIN_ENV.lock().unwrap_or_else(|p| p.into_inner());
        // SAFETY: only these tests set ITER_CLAUDE_BIN, one at a time (the lock)
        unsafe { std::env::set_var("ITER_CLAUDE_BIN", &bin) };
        let (tx, rx) = std::sync::mpsc::channel();
        let t0 = std::time::Instant::now();
        std::thread::spawn(move || {
            let result = dispatch_agent_claude("haiku", &ctx, &s);
            let _ = tx.send((result, crate::work::take_test_log()));
        });
        let got = rx.recv_timeout(std::time::Duration::from_secs(30));
        let took = t0.elapsed();
        unsafe { std::env::remove_var("ITER_CLAUDE_BIN") };
        let (result, log) = got.expect("watchdog: the dispatch did not return within 30 s");
        FakeRun { result, dir, log, took }
    }

    /// `n` bytes exactly: a start marker, lines with multi-byte UTF-8 and
    /// `\r\n` ends, ASCII padding, an end marker.
    fn big_prompt(n: usize, start: &str, end: &str) -> String {
        let line = "a requirement line — with café, é and a CRLF end\r\n";
        let mut p = format!("{start}\r\n");
        while p.len() + line.len() + end.len() + 2 <= n {
            p.push_str(line);
        }
        while p.len() + end.len() + 2 < n {
            p.push('x');
        }
        p.push_str("\r\n");
        p.push_str(end);
        assert_eq!(p.len(), n);
        p
    }

    fn contains(hay: &[u8], needle: &str) -> bool {
        hay.windows(needle.len()).any(|w| w == needle.as_bytes())
    }

    /// T1: a prompt over 200 KB reaches `claude` byte for byte, through the
    /// engine's in-memory path (AgentContext.prompt; nothing read from a file).
    /// Breaks again if: the prompt is put back in argv, stdin is set back to
    /// `Stdio::null()`, or the pipe is not closed after writing (the stand-in
    /// never sees end of input and the watchdog fails the test).
    #[test]
    fn t1_a_250kb_prompt_reaches_claude_byte_for_byte_on_stdin() {
        let (start, end) = ("ITER-T1-START-MARK-5b1e", "ITER-T1-END-MARK-9c42");
        let prompt = big_prompt(250_000, start, end);
        let run = dispatch_fake("t1", prompt.clone(), "echo", 60);
        assert!(run.result.is_ok(), "dispatch failed: {:?}", run.result.as_ref().err());
        let stdin = std::fs::read(run.dir.join("stdin.bin")).unwrap_or_default();
        assert!(stdin == prompt.as_bytes(), "stdin.bin holds {} bytes, not the prompt's {}", stdin.len(), prompt.len());
        let argv = std::fs::read(run.dir.join("argv.bin")).unwrap();
        assert!(!contains(&argv, start) && !contains(&argv, end), "argv carries the prompt");
        assert!(argv.len() < 4096, "argv is {} bytes", argv.len());
        assert!(run.log.iter().any(|l| l.contains("wrote 250000 of 250000 bytes")), "log: {:?}", run.log);
        let _ = std::fs::remove_dir_all(&run.dir);
    }

    /// T4's extra case: a 20,000-character prompt (just past the old Windows
    /// cap) arrives byte for byte.  Runs on every operating system.
    #[test]
    fn t4_a_20000_character_prompt_arrives_byte_for_byte() {
        let prompt = big_prompt(20_000, "ITER-T4-START", "ITER-T4-END");
        let run = dispatch_fake("t4", prompt.clone(), "echo", 60);
        assert!(run.result.is_ok(), "dispatch failed: {:?}", run.result.as_ref().err());
        let stdin = std::fs::read(run.dir.join("stdin.bin")).unwrap_or_default();
        assert!(stdin == prompt.as_bytes(), "stdin.bin holds {} bytes, not the prompt's {}", stdin.len(), prompt.len());
        let _ = std::fs::remove_dir_all(&run.dir);
    }

    /// The broken-pipe error the OS gives a writer whose reader is gone.
    fn is_broken_pipe(text: &str) -> bool {
        if cfg!(windows) { text.contains("os error 232") || text.contains("os error 109") } else { text.contains("os error 32") }
    }

    /// `n` in "stdin: wrote <n> of <total> bytes".
    fn bytes_written(text: &str, total: usize) -> Option<usize> {
        let tail = format!(" of {total} bytes");
        let at = text.find(&tail)?;
        text[..at].rsplit("wrote ").next()?.trim().parse().ok()
    }

    /// T3: a child that exits (status 3) without reading its stdin gives a
    /// logged write error, not a hang.
    /// Breaks again if: `let _ = stdin.write_all(...)` is restored (no stdin
    /// text in the error).
    #[test]
    fn t3_a_child_that_exits_unread_gives_a_logged_write_error() {
        let run = dispatch_fake("t3a", "y".repeat(1_000_000), "exit:3", 20);
        assert!(run.took < std::time::Duration::from_secs(10), "took {:?}", run.took);
        let e = run.result.as_ref().map(|_| ()).unwrap_err();
        assert!(e.contains("exit Some(3)"), "{e}");
        assert!(e.contains("stdin: wrote ") && e.contains(" of 1000000 bytes, then"), "{e}");
        assert!(bytes_written(e, 1_000_000).is_some_and(|n| n < 1_000_000), "{e}");
        assert!(is_broken_pipe(e), "{e}");
        assert!(run.log.iter().any(|l| l.contains(" of 1000000 bytes, then") && is_broken_pipe(l)), "log: {:?}", run.log);
        let _ = std::fs::remove_dir_all(&run.dir);
    }

    /// T3, second case: exit status 0 without reading is a failed run (the
    /// agent would have worked from a cut prompt).
    #[test]
    fn t3_a_child_that_exits_0_unread_is_a_failed_run() {
        let run = dispatch_fake("t3b", "y".repeat(1_000_000), "exit:0", 20);
        let e = match &run.result {
            Ok(o) => panic!("a short write passed as success: {:?}", o.text),
            Err(e) => e.clone(),
        };
        assert!(e.contains("stdin: wrote ") && bytes_written(&e, 1_000_000).is_some_and(|n| n < 1_000_000), "{e}");
        let _ = std::fs::remove_dir_all(&run.dir);
    }

    /// T3, third case: a child that neither reads nor exits is timed out, with
    /// the stdin result attached, and the dispatch does not hang.
    /// Breaks again if: the write is moved onto the dispatching thread before
    /// the wait loop (the watchdog then fails the test).
    #[test]
    fn t3_a_child_that_never_reads_times_out_with_the_stdin_result() {
        let run = dispatch_fake("t3c", "y".repeat(1_000_000), "sleep", 20);
        let e = run.result.as_ref().map(|_| ()).unwrap_err();
        assert!(e.starts_with("timed out after 20s"), "{e}");
        assert!(e.contains("stdin") && e.contains(" of 1000000 bytes"), "{e}");
        let _ = std::fs::remove_dir_all(&run.dir);
    }

    /// A stand-in `claude` binary: the dispatch runs it with the token in
    /// CLAUDE_CODE_OAUTH_TOKEN, the agent env and the cwd, and parses its stream.
    #[cfg(unix)]
    #[test]
    fn dispatch_runs_the_cli_with_token_env_and_cwd() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("iter-fake-claude-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let bin = dir.join("claude");
        std::fs::write(&bin, "#!/bin/sh\ncat >/dev/null\necho \"{\\\"type\\\":\\\"result\\\",\\\"subtype\\\":\\\"success\\\",\\\"session_id\\\":\\\"S\\\",\\\"result\\\":\\\"$CLAUDE_CODE_OAUTH_TOKEN $ITER_WORKID $(basename $(pwd))\\\"}\"\n").unwrap();
        std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
        // ITER_CLAUDE_BIN is read per call; the dispatch tests take turns setting it
        let ctx = AgentContext { prompt: "x".into(), cwd: dir.clone(), env: vec![("ITER_WORKID".into(), "W1".into())], ..Default::default() };
        let s = DispatchSettings { token: Some("TK".into()), ..Default::default() };
        let _env = CLAUDE_BIN_ENV.lock().unwrap_or_else(|p| p.into_inner());
        // SAFETY: test-only variable, set by one test at a time (the lock)
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
