---
id: 1d81b937-e18d-4650-8856-233c825b7e09
name: "Shared rules and types"
desc: "The libraries both sides compile in so they can never disagree: iter_core (work items, projects, engines, accounts, the settings-graph types, the queue rules — dependencies, locks, waits, schedules, dedup keys, cluster holds — the standard test result, and nodefile, which parses, conforms, renders, names and links v5 node files byte-for-byte the same on the server and in the engine) and iter_rag (GraphRAG text extraction, chunking and embedding)."
creator: "iter migrate5"
teststate: inherit
level: context
owner: bespoke
children:
  codedirs:  ["{topdir}/iter_core/", "{topdir}/iter_rag/"]
  codenodes: ["{topdir}/iter_core/iter_core.code.iter.md", "{topdir}/iter_rag/iter_rag.code.iter.md"]
  tests:     []
  reqs:      []
timestamps: {create: "2026-10-02 23:05:35Z", last_modified: "2026-10-02 23:12:30Z", last_tested: ""}
---
# Shared rules and types

## Summary

The common rulebook the server and the engines both follow. Nothing here does
network or database I/O; each rule is a function from records to an answer.

It holds two containers: **iter_core — shared rules library** and
**iter_rag — document text and embeddings**.

## How it works

- `iter_core/src/lib.rs` defines the records that travel over the API
  (`WorkItem`, `Project`, `Engine`, `AgentDef`, `LockRow`, `Account`), the
  work-item states, priority bands and the dependency gate; `waitgraph.rs`,
  `sched.rs`, `dedup.rs`, `cluster.rs` and `widget.rs` hold the lock and wait
  rules, schedules, repeat detection, cluster-restart holds and the question
  form. `settings.rs` holds the settings-graph node and edge types and the
  engine assignments shape; `testresult.rs` the standard test result JSON and
  how a script's last stdout line and exit code combine into pass / fail /
  could-not-run.
- `iter_core/src/nodefile/` is the node-file library of format v5: `type_of`,
  `parse`, `render`, `conform` (idempotent repair of missing and legacy keys),
  `slug` and `plan_path` (the designer folder rules), `resolve` (paths and
  globs against the project's node paths) and `edges_of` (codenodes, tests,
  reqs, supplies, connects, drives, touches, uses). iter_data conforms and
  renders every stored node with it; the engine conforms every file with it,
  so a node and its file are the same text.
- `iter_rag` extracts text from documents (pdf, docx, pptx, html, md),
  chunks it and embeds it for GraphRAG.

## What goes in and out

iter_data and iter_engine compile these crates in (iter_local uses iter_core
too). No I/O of their own.

## Why it matters

The server and the engines run in different places and are released
separately. If each had its own copy of "when is a lock live" or "what edges
does this file declare", the queue or the graph would drift with nothing on
screen saying why.
