---
id: 9bcedd1c-b4d0-41b9-bd7f-0c298786468e
name: "Model providers"
desc: "Routes every model call the engine makes — work turns, the close-gate verifier, ELI5 explain, the dedup judge, GraphRAG summaries and OCR, the critic and the connectivity nudge — through one `dispatch_agent(provider, model, ctx, settings)` and reads each account's usage through `get_usage`, with two providers behind it: `claude` (a headless `claude -p` stream-json process billed to the account's token) and `mock` (deterministic, driven by `mock:` directive lines, used by every automated test)."
creator: "agent.code"
teststate: inherit
level: component
owner: bespoke
children:
  codedirs:  ["{thisfiledir}/"]
  codenodes: []
  tests:     []
  reqs:      []
timestamps: {create: "2026-10-02 23:10:52Z", last_modified: "2026-10-02 23:10:52Z", last_tested: ""}
---

# Model providers

## Summary

The one doorway through which the engine talks to an AI model, with a real Claude backend and a scripted fake one for tests.

## How it works

`iter_engine/src/provider/mod.rs` (iter5 spec §5) defines `AgentContext` (prompt, cwd, env, MCP config,
resume session, allowed tools, extra args), `DispatchSettings` (account, token, timeout, max turns,
stop check), `DispatchOut` (text, subtype, turns, cost, token counts, session id, raw output) and a
`Role` (work, verifier, judge, explain, summary, ocr, critic, nudge). `dispatch_agent` sends
`"claude"` to the Claude provider, `"mock"` to the mock, and anything else to an error naming the known
providers. A registry filled each tick from the assignments maps (project, account) to its provider and
`bills`-edge model override (`register_accounts`, `provider_for`, `model_override`); `call` is the path
every engine call uses: resolve provider and token, dispatch, then record the account's usage snapshot.

- `provider/claude.rs`: builds `claude -p … --output-format stream-json --verbose` arguments
  (`claude_args`, `--resume`, `--mcp-config`, `--allowedTools`), runs it with
  `CLAUDE_CODE_OAUTH_TOKEN`, and parses the result line (`parse_result_json`); usage comes from the
  stream's `rate_limit_event`, else the idle probe in `usage.rs`.
- `provider/mock.rs`: on a work turn runs directive lines top to bottom — `mock: write`, `append`,
  `run`, `say`, `fail`, `ask` (through the `iter` shim), `sleep`; answers the verifier
  `VERDICT: complete` unless `mock: gate incomplete` is present, the judge `{"candidates":[]}`, the
  summary agent one summary per passage; cost 0, tokens = length / 4, session id `mock-<uuid>`; usage
  from `ITER_MOCK_USAGE_<ACCOUNT>` / `ITER_MOCK_USAGE`.

## What goes in and out

In: a prompt and context from the Work runner, Close gate, Duplicate judge, GraphRAG worker or CLI.
Out: a child process (`claude` or `bash` for mock directives) and a `DispatchOut`; usage snapshots.

## Why it matters

One entry point means caps, token rules and usage accounting hold for every model call, and the mock
makes the whole engine testable end to end with no model and no cost.

## Example

An e2e item's request says `mock: write notes.txt <<<hello>>>` and `mock: say wrote it`; the mock
writes the file in the checkout and answers "wrote it", and the verifier turn answers complete.
