---
id: 81199cc2-ed0e-4830-834a-1544d0691829
name: "iter_core — shared rules library"
description: "Defines the work-item record and the rules for states, priorities, dependencies, locks, schedules, repeats, cluster-restart holds and question forms as pure functions, so the data server and the engines make every decision the same way."
simple_description: "The shared rulebook both the server and the workers are built with."
level: container
owner: bespoke
teststate: inherit
children:
  codedirs:   ["{thisfiledir}/"]
  codenodes:  ["{topdir}/iter_core/src/model.code.iter.md", "{topdir}/iter_core/src/waits.code.iter.md", "{topdir}/iter_core/src/sched.code.iter.md", "{topdir}/iter_core/src/dedupkeys.code.iter.md", "{topdir}/iter_core/src/cluster.code.iter.md", "{topdir}/iter_core/src/widget.code.iter.md"]
  inputs:     []
  outputs:    []
  bizreqs:    []
  techreqs:   []
  tests:      ["{thisfiledir}/test/*.tests.iter.md"]
---

# Long Description

iter_core is a Rust library that defines iter's records and decides questions about them. It performs no input or output: it never opens a file, a socket or a database. Given records and a time, it returns an answer.

How it works: `iter_core/src/lib.rs` holds the records that cross the API — `WorkItem`, `WorkItemDetail`, `Project`, `Engine`, `Account`, `AgentDef`, `LockRow`, `VersionRow` — plus the list of tables (`TABLES`), the work-item states (`STATES`), the priority bands (0–9 do now, 10–39 use cases, 40–49 human, 50–99 maintenance), the dependency gate (`dependency_status`, `DepStatus`) and the account ladder (`pick_account`). Five modules sit beside it: **Locks and waits** (`waitgraph.rs`), **Schedules** (`sched.rs`), **Repeat detection keys** (`dedup.rs`), **Cluster-restart hold** (`cluster.rs`) and **Question forms** (`widget.rs`). Unknown fields on a record round-trip untouched, because storage keeps each record as a JSON body.

What goes in and out: iter_data — data server compiles it in to validate writes (lock grants, dependency cycles, lock shape, question forms); iter_engine — local engine compiles it in to choose what to run, which account to use, when a schedule fires and whether an item is held for a cluster restart. It calls nothing.

Why it matters: the server and engines run on different machines and are deployed separately. Keeping each rule in one place means both sides can only disagree if they run different versions, never because two copies drifted.

Example: `iter_core::dependency_status` tells the engine that item B, blocked by A, must still wait because A is complete but one of A's follow-up items is still open; the engine shows that as the tag "blocked by: waiting on …".

Built and tested as the cargo package `iter_core` in the iter4 workspace (`cargo test -p iter_core` from `iter4/`).
