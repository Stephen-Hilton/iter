---
id: 564ef9f8-59c1-417e-aaf7-b031ab7ede7a
name: "iter_engine — the machine's engine"
desc: "Runs once per machine and serves every project the settings graph assigns to it: asks iter_data for its assignments, heartbeats, syncs each checkout's node files both ways, builds designed projects, claims work through the server's get_next, dispatches each item to a model provider (claude or mock) or a shell, checks the result through the close gate, commits only the item's scope and reports usage; the same binary is the `iter` command line agents and people use."
creator: "iter migrate5"
teststate: inherit
level: container
owner: bespoke
children:
  codedirs:  ["{thisfiledir}/"]
  codenodes: ["{topdir}/iter_engine/src/loop.code.iter.md", "{topdir}/iter_engine/src/assign.code.iter.md", "{topdir}/iter_engine/src/runner.code.iter.md", "{topdir}/iter_engine/src/provider/provider.code.iter.md", "{topdir}/iter_engine/src/accounts.code.iter.md", "{topdir}/iter_engine/src/prompt.code.iter.md", "{topdir}/iter_engine/src/gate.code.iter.md", "{topdir}/iter_engine/src/dedup.code.iter.md", "{topdir}/iter_engine/src/mapsync.code.iter.md", "{topdir}/iter_engine/src/datasync.code.iter.md", "{topdir}/iter_engine/src/rag.code.iter.md", "{topdir}/iter_engine/src/sweep.code.iter.md", "{topdir}/iter_engine/src/cli.code.iter.md", "{topdir}/iter_engine/src/client.code.iter.md"]
  tests:     ["{thisfiledir}/test/*.test.iter.md"]
  reqs:      ["{topdir}/docs/iter5_spec.md"]
timestamps: {create: "2026-10-02 23:05:35Z", last_modified: "2026-10-02 23:15:33Z", last_tested: ""}
---

# iter_engine — the machine's engine

## Summary

The worker program: one per machine, it keeps every assigned project's files in step with the project graph and has AI agents (or shell commands) do the queued work.

## What it does

iter_engine is the only part of iter that touches a repository. It is started as
`iter_engine --data-url <url> --env-file <path> [--name <engine name>]` (`src/main.rs`); the name
defaults to the short hostname. The env file holds `ITER_ENGINE_TOKEN` (its iter_data credential) and
the LLM account tokens; nothing else is read from disk. Which projects it serves, where each checkout
lives and which accounts each may bill come from `GET /api/engines/{name}/assignments`, derived on the
server from the settings graph (`serves`, `bills`, `holds`, `of` edges).

## How it works

Every tick (`src/engine.rs: tick`, about every five seconds) it:

1. re-reads its engine record and assignments, renews running items' leases and reloads the env file;
2. picks an account per project from real usage (5h / 7d windows, the `bills` edge's switch / stop %)
   and sends the heartbeat (state, account, usage, running counts);
3. for each served project, from the heartbeat reply: runs a **build** when `build_waiting` names it
   (designer push: `git init`, write every pending file, one commit, `build/done`), applies GraphRAG
   document rows (`datasync_waiting`), and runs one **file-sync round** — filescan → conform → `POST
   files/sync`, then the pending graph edits (`files/pending` → write → commit → `files/ack`);
4. per running project: refreshes project / agent / work-item caches when their change counters move,
   makes sure the paused Test sweep schedule exists, replays unsaved closes, repairs ghost runs, starts
   ELI5 explains, fires due schedules, triages new items for duplicates, and — after the engine-side
   checks (cap, budget, account, holds) — asks `POST /api/projects/{p}/next` for a claimed, locked item;
5. runs each claimed item on its own thread (`src/work.rs`): agent turns through `provider::dispatch_agent`
   (claude, or the deterministic mock), or the item's shell command; then the close gate, the scoped
   commit, the spend row, the close and the lock release; it may chain queued neighbours into the same
   session.

Started as `iter_engine cli …` (installed into each checkout as `.iter/bin/iter`), it is the command line:
`iter add`, `ask`, `block`, `wait`, `doc`, `runtests`, `validate`, `sync`, `sweep`, `rag sync`, `init`,
`migrate5` and more.

## What goes in and out

In: assignments, work items, agent records and pending node writes from iter_data over HTTP JSON;
node files and code from the checkouts. Out: heartbeats, file-sync batches, test results, spend and
usage, closes and acks to iter_data; git commits and pushes; `claude` (or mock) agent processes with an
MCP config pointing back at iter_data. File-side work (walking, test scripts, validate, migrate5) is
done through iter_local and `iter_core::nodefile`.

## Why it matters

Without an engine, items stay queued, graph edits never reach the repository and file edits never reach
the graph. Running one engine per machine lets one process share accounts and caps across projects.

## Example

A person edits a node's desc in the Project graph. The next heartbeat reply lists the project under
`files_waiting`; the engine fetches the pending write, rewrites the file, commits
`iter: graph edit — …`, pushes and acks, and the node shows as synced.

Built and tested as the cargo package `iter_engine` (`cargo test -p iter_engine` from `iter5/`).
