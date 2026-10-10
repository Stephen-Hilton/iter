---
id: 81199cc2-ed0e-4830-834a-1544d0691829
name: "iter_core — shared rules and node-file library"
desc: "The Rust library the data server and the engine both compile in: the work-item record and its rules (states, priorities, dependencies, locks, schedules, repeats, cluster-restart holds, question forms), the v5 node-file library (parse, conform, render, edges, path planning), the settings-graph types and the standard test result — pure functions, no I/O, so both sides decide every question the same way."
creator: "iter migrate5"
teststate: inherit
level: container
owner: bespoke
children:
  codedirs:  ["{thisfiledir}/"]
  codenodes: ["{topdir}/iter_core/src/model.code.iter.md", "{topdir}/iter_core/src/nodefile/nodefile.code.iter.md", "{topdir}/iter_core/src/settings.code.iter.md", "{topdir}/iter_core/src/testresult.code.iter.md", "{topdir}/iter_core/src/waits.code.iter.md", "{topdir}/iter_core/src/sched.code.iter.md", "{topdir}/iter_core/src/dedupkeys.code.iter.md", "{topdir}/iter_core/src/cluster.code.iter.md", "{topdir}/iter_core/src/widget.code.iter.md"]
  tests:     ["{thisfiledir}/test/*.test.iter.md"]
  reqs:      []
timestamps: {create: "2026-10-02 23:05:35Z", last_modified: "2026-10-02 23:15:00Z", last_tested: ""}
---

# iter_core — shared rules and node-file library

## Summary

The shared rulebook and file format both the server and the engines are built with.

iter_core is a Rust library that defines iter5's records and decides questions about them. It performs no input or output: it never opens a file, a socket or a database. Given records, text and a time, it returns an answer.

## What is in it

- **Work-item model** (`src/lib.rs`): the records that cross the API — `WorkItem` (now with an optional `node`, the project-graph node the item works on), `WorkItemDetail`, `Project`, `Engine`, `Account`, `AgentDef`, `LockRow` — the storage table list (`TABLES`, including the settings-graph tables `account`, `provider`, `workitem_type`, `sys_edge`), the eight states, the priority bands, the dependency gate (`dependency_status`) and the account ladder (`pick_account`).
- **Node-file library** (`src/nodefile/`): format v5 of the `*.iter.md` files — `parse`, `conform`, `render`, `edges_of`, `resolve`, `plan_path`, `add_child` / `remove_child`. The server's node side and the engine's file side use it so they agree byte for byte.
- **Settings-graph types** (`src/settings.rs`): node and edge types of the settings graph, placeholders, endpoint/settings validation and the engine `Assignments` shape.
- **Test result** (`src/testresult.rs`): the standard result JSON a test script prints as its last line, parsing (with the legacy `ITER_RESULT` line) and the pass / fail / could-not-run verdict.
- **Locks and waits** (`src/waitgraph.rs`), **Schedules** (`src/sched.rs`), **Repeat detection keys** (`src/dedup.rs`), **Cluster-restart hold** (`src/cluster.rs`), **Question forms** (`src/widget.rs`).

Unknown fields on a record round-trip untouched, because storage keeps each record as a JSON body.

## What goes in and out

iter_data compiles it in to validate writes, run server-side get_next (`next.rs`: dependency gate, live locks, claim tags), store project-graph nodes (`nodes.rs`, `filesync.rs` through `nodefile`), keep the settings graph (`settings.rs`) and accept test results (`testlogs.rs`). iter_engine compiles it in for file sync (`filesync.rs`: conform before sync), schedules, account choice, the close gate and dedup triage. iter_local uses `nodefile` and `testresult` for validate, the test runner and `migrate5`. It calls nothing.

## Why it matters

The server and the engines run on different machines and are deployed separately. Keeping each rule — and the node-file format — in one place means both sides can only disagree if they run different versions, never because two copies drifted.

## Example

The engine conforms an edited `lib.code.iter.md` and syncs it; the server parses the same text with the same `nodefile::parse`, derives the same edges with `edges_of`, and when a person later edits the node in the graph, `render` produces exactly the text `conform` would — so the file the engine writes back never ping-pongs.

Built and tested as the cargo package `iter_core` in the iter5 workspace (`cargo test -p iter_core` from the repo root; test node `test/iter_core.test.iter.md`).
