//! A stand-in `claude` CLI for iter_engine's dispatch tests (provider/claude.rs,
//! `ITER_CLAUDE_BIN`).  std only, so it builds and runs on Linux, macOS and
//! Windows alike (`cargo test` builds examples; the tests find it beside the
//! test binary, or build it with `cargo build --example fake_claude`).
//!
//! `FAKE_CLAUDE_MODE` (from the agent env) picks what it does:
//! - unset / `echo`: copy stdin to `stdin.bin` in the cwd, write every
//!   argument to `argv.bin` (each followed by a zero byte), print a stream-json
//!   result line whose text is the number of stdin bytes read;
//! - `exit:<code>`: exit at once with `<code>`, never reading stdin;
//! - `sleep`: never read stdin, never exit (the dispatch timeout ends it).

use std::io::{Read, Write};

fn main() {
    let mode = std::env::var("FAKE_CLAUDE_MODE").unwrap_or_default();
    if let Some(code) = mode.strip_prefix("exit:") {
        std::process::exit(code.trim().parse().unwrap_or(1));
    }
    if mode == "sleep" {
        loop {
            std::thread::sleep(std::time::Duration::from_secs(3600));
        }
    }
    let mut argv = Vec::new();
    for a in std::env::args_os().skip(1) {
        argv.extend_from_slice(a.to_string_lossy().as_bytes());
        argv.push(0);
    }
    std::fs::write("argv.bin", &argv).expect("write argv.bin");
    let mut input = Vec::new();
    std::io::stdin().read_to_end(&mut input).expect("read stdin");
    std::fs::write("stdin.bin", &input).expect("write stdin.bin");
    let mut out = std::io::stdout();
    writeln!(out, "{{\"type\":\"result\",\"subtype\":\"success\",\"session_id\":\"FAKE\",\"result\":\"read {} bytes\"}}", input.len()).unwrap();
}
