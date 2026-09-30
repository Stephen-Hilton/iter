---
id: 1d81b937-e18d-4650-8856-233c825b7e09
name: "Shared rules and types"
description: "Defines, once, the records and the rules both the data server and the engines must agree on — what a work item is, when a lock counts, when a schedule is due, when two items are the same fault — so the two sides can never disagree."
simple_description: "The common rulebook that the server and the workers both follow."
level: context
owner: bespoke
teststate: inherit
children:
  codedirs:   ["{topdir}/iter_core/", "{topdir}/iter_rag/"]
  codenodes:  ["{topdir}/iter_core/iter_core.code.iter.md", "{topdir}/iter_rag/iter_rag.code.iter.md"]
  inputs:     []
  outputs:    []
  bizreqs:    []
  techreqs:   []
  tests:      []
---

# Long Description

The Shared model area holds the definitions every other part agrees on. It has one container, **iter_core — shared rules library**, which does no input or output of its own: no network, no files, no database. It only defines records and decides questions about them.

How it works: `iter_core/src/lib.rs` defines the records that travel over the API — `WorkItem`, `Project`, `Engine`, `AgentDef`, `LockRow`, `Account` — plus the list of work-item states (`STATES`), the priority bands, and the dependency gate (`dependency_status`). Separate modules hold the lock and wait rules (`waitgraph.rs`), schedules (`sched.rs`), repeat detection (`dedup.rs`), the cluster-restart hold (`cluster.rs`) and the question-form schema (`widget.rs`). Each rule is a pure function: give it records and a time, get an answer.

What goes in and out: the data server (`iter_data`) and the engine (`iter_engine`) both compile this library in. The server uses it to refuse bad writes — a lock for an item that is not running, a dependency loop, a schedule created by an agent. The engine uses it to decide what to run next, which account to use and when a scheduled template fires.

Why it matters: the server and the engine run in different places and are released separately. If each had its own copy of "when is a lock live", one would count a lock the other ignored, and the queue would stall with nothing on screen saying why — which is exactly the 2026-09-25 incident this library's lock rules were rewritten to prevent.

Example: `iter_core::waitgraph::lock_grant` is called by the server on every lock request; it refuses a lock for an item whose state is queued, with the message "work item … is not running".
