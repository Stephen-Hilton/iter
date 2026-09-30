---
id: 564ef9f8-59c1-417e-aaf7-b031ab7ede7a
name: "iter_engine — local engine"
description: "Loops on a machine with a checkout, claiming queued work items, locking their folders, running a headless Claude Code session or a shell command for each, checking the result through the close gate and committing it; the same binary is the `iter` command line agents and people use."
simple_description: "The worker program that picks up jobs, has an AI agent do them, checks the result and saves it."
level: container
owner: bespoke
teststate: inherit
children:
  codedirs:   ["{thisfiledir}/"]
  codenodes:  ["{topdir}/iter_engine/src/loop.code.iter.md", "{topdir}/iter_engine/src/runner.code.iter.md", "{topdir}/iter_engine/src/gate.code.iter.md", "{topdir}/iter_engine/src/prompt.code.iter.md", "{topdir}/iter_engine/src/cli.code.iter.md", "{topdir}/iter_engine/src/mapsync.code.iter.md", "{topdir}/iter_engine/src/accounts.code.iter.md", "{topdir}/iter_engine/src/dedup.code.iter.md", "{topdir}/iter_engine/src/client.code.iter.md", "{topdir}/iter_engine/src/datasync.code.iter.md", "{topdir}/iter_engine/src/rag.code.iter.md"]
  inputs:     []
  outputs:    []
  bizreqs:    []
  techreqs:   []
  tests:      ["{thisfiledir}/test/*.tests.iter.md"]
---

# Long Description

iter_engine is the program that does the work. It runs on a laptop or server next to a git checkout of the project and talks to iter_data over HTTP. It keeps only its connection settings locally (`.iter/config.json` and a `.env` of account tokens); everything else it reads from the server.

How it works: started with no subcommand, `iter_engine/src/main.rs` builds an `EngineRuntime` and runs its tick loop (`engine.rs: run`, `tick`), about every five seconds. Each tick it re-reads only the tables whose change counters moved, sends a heartbeat, applies any waiting graph edits (`datasync.rs`), fires due schedules, and dispatches: it chooses the next queued item whose dependencies are met and whose folders no running item holds, picks an account with usage left (`usage.rs`, `envstore.rs`), claims the item, takes its locks and starts it on a thread (`work.rs: execute`). The run builds a prompt (`prompt.rs`), starts `claude -p` or the item's shell command, records cost and output, passes the **close gate** (`gate.rs`, a second model reviews the result against the request), commits only the item's lock scope, and releases the locks. Once a minute at most it pushes the program map if the files changed (`sync.rs`). Started as `iter_engine cli …` (installed as `.iter/bin/iter`), it is the command line: `iter add`, `ask`, `block`, `doc`, `ids`, `sync`, `sweep`, `init`, `validate`, `runtests` and more (`cli.rs`).

What goes in and out: it calls iter_data for all state and iter_local for everything done to files; it starts Claude Code and git as child processes.

Why it matters: it is the only part that changes code. Without it items stay queued forever.

Example: `iter sweep` runs every test group in the map, stores each verdict on its vertex, and files one fix item per red group.

Built and tested as the cargo package `iter_engine` in the iter4 workspace (`cargo test -p iter_engine` from `iter4/`).
