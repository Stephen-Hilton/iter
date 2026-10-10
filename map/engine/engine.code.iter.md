---
id: 759585ed-1241-4c90-b98e-4dd129a4ce56
name: "Engines and checkout tools"
desc: "The engine, one per machine, and the checkout library it uses. iter_engine starts with only a server URL and an env file, learns from the settings graph which projects (and checkouts) it serves and which accounts it may bill, and for each project every tick: syncs node files with the project graph (filescan, conform, sync, pending writes, designer builds), fires schedules, asks the server for the next item, dispatches it to a provider, runs the close gate and commits. iter_local walks, validates, tests and migrates checkouts."
creator: "iter migrate5"
teststate: inherit
level: context
owner: bespoke
children:
  codedirs:  ["{topdir}/iter_engine/", "{topdir}/iter_local/"]
  codenodes: ["{topdir}/iter_engine/iter_engine.code.iter.md", "{topdir}/iter_local/iter_local.code.iter.md"]
  tests:     []
  reqs:      []
timestamps: {create: "2026-10-02 23:05:35Z", last_modified: "2026-10-02 23:12:30Z", last_tested: ""}
---
# Engines and checkout tools

## Summary

The workers that sit next to the code: the only part of iter5 that reads or
writes a repository.

It holds two containers: **iter_engine — local engine** (the long-running
process, and also the `iter` command line through `iter_engine cli`) and
**iter_local — checkout tools** (the library of things done to files on disk).

## How it works

`iter_engine --data-url <url> --env-file <path> [--name <engine>]` reads
nothing else from disk. The env file holds `ITER_ENGINE_TOKEN` and each
account's token, and is re-read while the engine runs. On each tick
(`iter_engine/src/engine.rs`) the engine heartbeats, refreshes its
**assignments** (`GET /api/engines/{name}/assignments`: the projects it serves
with their `topdir`, the accounts it holds that bill each project), and then,
for every served project:

1. **File sync** (`filesync.rs`): filescan (git-ignore aware, stat-only between
   full walks) → conform with `iter_core::nodefile` (write back what changed)
   → `POST …/files/sync`; when the heartbeat reply says `files_waiting`, pull
   `files/pending`, write, commit only those files, push, ack; when it says
   `build_waiting`, build the designed project (`git init`, `.gitignore`,
   every file, first commit, `build/done`).
2. **Schedules** fire into the queue (including the engine-owned test sweep).
3. **Dispatch**: after checking its own caps (max agents, budget, account
   switch/stop percentages from the `bills` edge), it asks `POST …/next`, then
   runs the claimed item on its own thread (`work.rs`): load the node's
   context, call `provider::dispatch_agent` (claude CLI or mock), read usage
   with `get_usage`, run the close gate (`gate.rs`), commit the item's lock
   scope, report and release.

Agents inside a run use the `iter` shim (`{topdir}/.iter/bin/iter`) and the
MCP endpoint the engine configures for the session.

## What goes in and out

Everything about work and the graph goes through iter_data's HTTP API; the
checkout is read through iter_local and written (node files, commits) by the
engine and its agents; model calls go to the provider.

## Why it matters

The server never holds a checkout, so without an engine nothing changes in a
repository and the graph never learns what the files say.
