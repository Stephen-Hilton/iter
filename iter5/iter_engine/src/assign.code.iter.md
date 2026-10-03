---
id: 2563b645-4a91-4101-ad17-30a4c9b3ccfa
name: "Engine assignments"
desc: "Reads `GET /api/engines/{name}/assignments` into the list of projects this engine serves — each with its checkout topdir, read-only flag, run state, edge tag and the billing accounts the engine also holds (provider, token envar, order, switch / stop percent, model) — and expands `~` in topdirs, so that the engine learns everything about its projects and accounts from the settings graph and reads nothing about them from disk."
creator: "agent.code"
teststate: inherit
level: component
owner: bespoke
children:
  codedirs:  ["{topdir}/iter_engine/src/assign.rs"]
  codenodes: []
  tests:     []
  reqs:      []
timestamps: {create: "2026-10-02 23:10:52Z", last_modified: "2026-10-02 23:10:52Z", last_tested: ""}
---

# Engine assignments

## Summary

Tells the engine which projects it works on, where their files are and which AI accounts it may use for each.

## How it works

`iter_engine/src/assign.rs` holds the serde shapes of the assignments reply (iter5 spec §4.2):
`Assignments { engine, projects: [Assignment], accounts: [AssignedAccount] }`. An `Assignment` is one
active `serves` edge (engine → project) with its `topdir`, `read_only`, project `state` and `edge_tag`,
plus the accounts that bill the project (`bills` edge: `order`, `switch`, `stop`, `model`) and that this
engine holds (`holds` edge: `token_envar` override); an account that bills the project but is not held is
left out by the server. An account's `provider` comes from its `of` edge and defaults to `claude`.

- `Assignments::parse` reads the reply; `get(project)` finds one assignment; `accounts_of` merges the
  per-project account entries with the engine-wide account list.
- `core_account` / `core_accounts` turn them into `iter_core::Account` for the usage ladder.
- `expand_topdir` expands a leading `~` so a topdir set in the settings graph works on any machine.
- `reply_names(reply, key, project)` reads heartbeat-reply lists such as `files_waiting` and
  `build_waiting`.

## What goes in and out

In: the assignments JSON from iter_data. Out: typed assignments for the Engine tick loop, account lists
for the usage ladder and the provider registry.

## Why it matters

A project with no active `serves` edge to this engine is never run, and an account is never billed from a
machine that does not hold its token. Moving an edge onto the `_deactivated` placeholder in the settings
graph stops the work on the next tick.

## Example

The reply lists project `pdy` with topdir `~/dev/pdy` and accounts `main` (order 1, switch 80, stop 95).
`expand_topdir` gives `/Users/me/dev/pdy`, and `accounts_of` hands the ladder `main` with provider
`claude` and envar `CLAUDE_TOKEN_MAIN`.
