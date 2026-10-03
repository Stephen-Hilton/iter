---
id: aa5a25e1-6fbd-4d35-9d7c-ad76672f861d
name: "Claude Code CLI (agent runtime)"
desc: "Anthropic's claude command-line program, run headless by the engine's claude provider for every model call (claude -p <prompt> --output-format stream-json --verbose, billed to the account token in CLAUDE_CODE_OAUTH_TOKEN). Inside a session the agent reads and edits the checkout, runs the iter shim ({topdir}/.iter/bin/iter) and calls iter_data's MCP tools. Not iter5 code; the mock provider stands in for it in tests."
creator: "stephen"
teststate: inherit
level: container
owner: "3rdparty"
children:
  codedirs:  []
  codenodes: []
  tests:     []
  reqs:      []
timestamps: {create: "2026-10-02 23:12:30Z", last_modified: "2026-10-02 23:12:30Z", last_tested: ""}
---
# Claude Code CLI (agent runtime)

The program behind the `claude` provider (`iter_engine/src/provider/claude.rs`).
`$ITER_CLAUDE_BIN` overrides which binary runs (tests use a stand-in);
otherwise `claude` on `PATH`. `tools/iter_engine_setup.sh` checks it is
installed.

One process per turn, in the checkout (`cwd` = the project's topdir), with:
the prompt, `--resume <session>` for chained sessions, `--model`,
`--allowedTools`, `--max-turns`, `--mcp-config <file>` pointing at iter_data's
`/mcp` with this engine's token and the calling item, plus the agent's own
flags. Environment: `CLAUDE_CODE_OAUTH_TOKEN` (the account's token; the
ambient `ANTHROPIC_API_KEY` / `ANTHROPIC_AUTH_TOKEN` are removed), the
`ITER_*` addressing variables and a `PATH` that finds the `iter` shim.
Its stdout is stream-json: the `result` line becomes the dispatch output and
the `rate_limit_event` line the account's usage.

Inside the session the agent is the "Agent session" actor: it edits files in
the checkout, runs `iter <verb>` and calls the iter MCP tools.
