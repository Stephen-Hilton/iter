---
id: 36abe968-9dd6-431e-83d2-4477fb7c130f
name: "Account usage tracker"
desc: "Measures how much of each account's 5-hour and 7-day allowance is used — from the provider's own report after every dispatch, or an idle 1-token probe — keeps a snapshot per account, ranks accounts for the switch / stop ladder and reports every account's windows and reset times in the heartbeat; and hot-reloads account tokens from the engine's env file each tick, so that the engine picks an account with room left and a new or rotated token takes effect without a restart."
creator: "iter migrate5"
teststate: inherit
level: component
owner: bespoke
children:
  codedirs:  ["{topdir}/iter_engine/src/usage.rs", "{topdir}/iter_engine/src/envstore.rs"]
  codenodes: []
  tests:     []
  reqs:      []
timestamps: {create: "2026-10-02 23:05:35Z", last_modified: "2026-10-02 23:15:33Z", last_tested: ""}
---

# Account usage tracker

## Summary

Keeps track of how much of each AI account's allowance is used and where its key lives, so work goes to an account that can still take it.

## How it works

**Usage** (`iter_engine/src/usage.rs`): each account has a snapshot file
(`$ITER_USAGE_DIR/iter3-usage-<account>.json`, default dir `~/.claude`) with the 5h and 7d used
percentages and reset times. After a dispatch the provider's `get_usage` supplies the numbers (claude:
the stream's `rate_limit_event`; mock: `ITER_MOCK_USAGE`); with nothing running, stale accounts are
probed with a 1-output-token request whose rate-limit headers carry the same numbers
(`ITER_USAGE_PROBE_URL` overrides the endpoint). `usage_map` and `effective_pct_for` feed `iter_core::pick_account`, which
applies each project's `bills`-edge order (P0 first; among accounts of one priority the one whose 7-day window resets soonest, `resets7d_map`) and switch / stop percentages; `available_at` says when an
account comes back; `accounts_json` and `next_json` build the heartbeat's per-account windows and the
next account to free up.

**Env store** (`iter_engine/src/envstore.rs`): one process-wide map of the env file. `init` seeds it,
`get` serves token reads (never `std::env`, which is unsafe to mutate under running threads), and
`reload_if_changed` re-reads the file when it moved, refreshing only `*_TOKEN` keys and the served
projects' `token_envar`s. Keys the process environment supplied win; `ITER_ENGINE_TOKEN` is pinned;
values are never logged.

## What goes in and out

In: provider usage reports, probe responses, the env file. Out: snapshot files, the account choice for
the tick loop and Work runner, heartbeat `accounts` / `next` / `usage` fields.

## Why it matters

Without it an engine would keep starting sessions on an exhausted account, or bill an account with no
token on this machine. A token added to the env file starts working on the next tick.

## Example

Account `main` reaches 82% of its 5h window with switch 80: the next dispatch picks `backup`. When both
pass their stop percentage, the heartbeat goes up with `usage: null` and hold "all accounts at stop%".
