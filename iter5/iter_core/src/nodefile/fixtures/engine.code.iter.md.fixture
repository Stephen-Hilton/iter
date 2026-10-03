---
id: 759585ed-1241-4c90-b98e-4dd129a4ce56
name: "Engines and checkout tools"
description: "Runs the actual work on machines that hold a copy of the project: picks queued items, locks the folders they touch, starts a headless Claude Code session or a shell command, checks and commits the result, and keeps the program map in step with the files."
simple_description: "The workers that sit next to the code, do the queued jobs and report back."
level: context
owner: bespoke
teststate: inherit
children:
  codedirs:   ["{topdir}/iter_engine/", "{topdir}/iter_local/"]
  codenodes:  ["{topdir}/iter_engine/iter_engine.code.iter.md", "{topdir}/iter_local/iter_local.code.iter.md"]
  inputs:     []
  outputs:    []
  bizreqs:    []
  techreqs:   []
  tests:      []
---

# Long Description

The Engine area is where work gets done. An engine is a long-running process on a developer laptop or a server that has a git checkout of the project. It holds two containers: **iter_engine — local engine**, the process itself plus the `iter` command line, and **iter_local — checkout tools**, the library of things done against files on disk.

How it works: every few seconds the engine runs one tick (`iter_engine/src/engine.rs: EngineRuntime::tick`). It re-reads only the tables whose change counters moved, sends a heartbeat, fires due schedules, and then dispatches: it picks the most urgent queued item whose dependencies are done and whose folders are free, takes locks on those folders from the data server, and runs the item on its own thread (`iter_engine/src/work.rs: execute`). A run is either a headless Claude Code session with a prompt built from the item and the project's rules, or a plain shell command for `exec` and `test` items. Before the item closes, the close gate (`iter_engine/src/gate.rs`) has a second model check the result against the request; then the engine commits only the files in the item's lock scope and releases its locks.

What goes in and out: it reads and writes everything through the Data and API area's HTTP API; it reads and writes the checkout through iter_local (node-file scan, stable ids, map snapshot, test runner, validate, graph edits). Agents inside a run call the `iter` command line to file follow-up items, ask questions or take extra locks.

Why it matters: this is the only part with repository access, so without it nothing changes in the code and the map never updates.

Example: a red test group produces an item "Tests non-green: iter_data-unit 41/42"; an engine claims it, locks `{topdir}/iter_data/`, starts Claude, the agent fixes the test, the gate agrees, and the engine commits and closes the item.
