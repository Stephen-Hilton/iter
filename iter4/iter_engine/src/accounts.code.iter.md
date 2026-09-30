---
id: 36abe968-9dd6-431e-83d2-4477fb7c130f
name: "Account usage tracker"
description: "Measures how much of each Claude account's 5-hour and 7-day allowance is used and re-reads account tokens from the engine's env file while it runs, so that the scheduler can pick an account with room left and a new or rotated token takes effect without a restart."
simple_description: "Keeps track of how much AI capacity each account has left, and notices new account keys without restarting."
level: component
owner: bespoke
teststate: inherit
children:
  codedirs:   ["{topdir}/iter_engine/src/usage.rs", "{topdir}/iter_engine/src/envstore.rs"]
  codenodes:  []
  inputs:     []
  outputs:    []
  bizreqs:    []
  techreqs:   []
  tests:      []
---

# Long Description

The Account usage tracker answers two questions for the engine: how much of each Claude account's allowance is used right now, and what each account's token (its access key) currently is.

How usage is measured (`iter_engine/src/usage.rs`): every agent session streams its output, and Claude Code emits a `rate_limit_event` line with the 5-hour and 7-day utilisation and reset times. The Work runner passes that line to `record_event`, which parses it (`usage_from_stream_event`) and saves a per-account snapshot file (`write_snapshot`, `iter3-usage-<account>.json` under `~/.claude` or `$ITER_USAGE_DIR`). When nothing is running, `probe` sends one tiny request straight to the API and reads the same numbers from the response headers (about 9 tokens, no agent process). `effective_pct` treats an expired window as 0 and an account on pay-as-you-go overage as 100. `usage_map` hands the scheduler every account's percentage; `available_at`, `accounts_json` and `next_json` say when a full account comes back, for the heartbeat and the webui.

How tokens are reloaded (`iter_engine/src/envstore.rs`): `init` loads the env file once, `reload_if_changed` re-reads it each tick when it changed, and `get` answers token reads from that map rather than the process environment. Only `*_TOKEN` keys and the tokens projects name are refreshed; the engine's own server token stays pinned; values are never logged.

The Engine scheduler loop uses these numbers to choose the account (the ladder of accounts and their stop percentages) and to cap how many agents run at once; the choice itself lives in `engine.rs`, not here. The Work runner uses the tokens when it starts a session.

Why it matters: without it the engine would keep sending work to an exhausted account, or keep using a revoked key until someone restarted it.

Example: account A reaches its stop percentage mid-afternoon. Its snapshot shows 5h at 96%, the scheduler switches to account B, and the heartbeat tells the webui when A's window resets.
