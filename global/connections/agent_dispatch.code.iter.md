---
id: 5f31ed6e-34c6-4a3f-a37e-5e04d0a6b44d
name: "Agent dispatch (provider CLI)"
desc: "How the engine runs a model: provider::dispatch_agent(provider, model, context, settings) picks the account's provider (its of edge; default claude). claude starts one headless Claude Code process per call — claude -p <prompt> --output-format stream-json --verbose [--resume] [--model] [--allowedTools] [--max-turns] [--mcp-config] — in the checkout, billed via CLAUDE_CODE_OAUTH_TOKEN, and reads the result and rate_limit_event usage from stdout. mock runs no model: it executes mock: directives in-process for deterministic tests."
creator: "stephen"
teststate: inherit
connects:
  from: ["{topdir}/iter_engine/src/provider/provider.code.iter.md"]
  to: ["{topdir}/iter_engine/src/runner.code.iter.md", "{topdir}/iter_engine/src/gate.code.iter.md", "{topdir}/iter_engine/src/dedup.code.iter.md", "{topdir}/iter_engine/src/rag.code.iter.md", "{topdir}/iter_engine/src/loop.code.iter.md", "{topdir}/iter_engine/src/cli.code.iter.md", "{topdir}/map/external/claude_code/claude_code.code.iter.md"]
level: connection
children:
  codedirs:  []
  codenodes: []
  tests:     []
  reqs:      []
timestamps: {create: "2026-10-02 23:12:30Z", last_modified: "2026-10-02 23:12:30Z", last_tested: ""}
---
# Agent dispatch (provider CLI)

Every model call the engine makes — work turns, the close-gate verifier,
ELI5 explain, the dedup judge, GraphRAG summaries and OCR, the critic and the
nudge — goes through `iter_engine/src/provider/mod.rs: dispatch_agent`. Usage
is read separately with `get_usage`, so a provider that reports usage another
way can plug in.

## Protocol (provider `claude`)

- **Start**: a child process `claude` (`$ITER_CLAUDE_BIN` overrides) with
  `-p <prompt> --output-format stream-json --verbose`, then optionally
  `--resume <session id>`, `--model <m>`, `--allowedTools <list>`,
  `--max-turns <n>`, `--mcp-config <file>` and the agent's own flags;
  `cwd` = the project's checkout; stdin closed.
- **Environment**: `CLAUDE_CODE_OAUTH_TOKEN` = the account's token (read from
  the engine's env file under the account's `token_envar`);
  `ANTHROPIC_API_KEY` / `ANTHROPIC_AUTH_TOKEN` removed; `ITER_*` addressing
  and a `PATH` with the `iter` shim.
- **Output**: stream-json lines on stdout; the `result` line gives text,
  subtype, turns, cost, tokens and session id; the `rate_limit_event` line
  gives the account's 5-hour / 7-day usage. A timeout or a stop request kills
  the process group.
- **Idle usage probe**: with no dispatch to read, `get_usage` makes one
  1-token `POST https://api.anthropic.com/v1/messages` with the token as
  Bearer and reads the rate-limit headers (`iter_engine/src/usage.rs`).

## Provider `mock`

No model and no network: the work item's `mock:` directives (`write`,
`append`, `run`, `say`, `fail`, `ask`, `sleep`) run in order in the checkout;
the verifier answers complete unless `mock: gate incomplete`; usage comes from
`ITER_MOCK_USAGE`. It drives `cargo test`, `e2e.sh` and Playwright.

## Who calls it

The work runner (work turns, session chaining), the close gate (verifier),
the dedup judge, GraphRAG summaries and OCR, the tick loop (explain, nudge)
and `iter critreview` (critic). The provider's far end is the Claude Code CLI.

## Auth

The account → provider choice is the settings graph's `of` edge; the
credential never leaves the engine's machine (the `holds` edge only says the
engine has it).
