---
id: b18d7a67-a2e4-4955-ab10-19abb2b90e77
name: "HTTP API"
description: "Answers every request to the data server — login, users, agents, projects, engines and heartbeats, work items and their history rows, locks, schedules, spend — checking the caller's role and the queue's rules before each read or write."
simple_description: "Where every request from the workers and the web page arrives and gets its answer."
level: component
owner: bespoke
teststate: inherit
children:
  codedirs:   ["{topdir}/iter_data/src/api.rs"]
  codenodes:  []
  inputs:     []
  outputs:    ["{topdir}/interfaces/workitem-create/workitem-create.interface.iter.md", "{topdir}/interfaces/lock-acquire/lock-acquire.interface.iter.md", "{topdir}/interfaces/versions-poll/versions-poll.interface.iter.md"]
  bizreqs:    []
  techreqs:   []
  tests:      []
---

# Long Description

The HTTP API is the set of routes that make up iter_data's public face. Every request from an engine, an agent's `iter` command or the web page lands here.

How it works: `iter_data/src/api.rs: router` lists the routes and merges in the map routes (`graph.rs`) and datasync routes (`datasync.rs`). Each handler first extracts `AuthUser`: it verifies the bearer token and compares its `tokenver` with the user's row, so bumping that number revokes every outstanding token. Roles decide what is allowed (`require_admin`, `require_writer`; a viewer can only read). Then the handler applies the queue's rules. `workitem_create` refuses placeholder requests and schedules created by the engine role, assigns a priority in the right band (`place_new_item`), returns an already-open twin instead of filing a repeat (stage-one dedup), and refuses a dependency loop (`refuse_dependency_cycle`). `workitem_put` needs `expect_version` and refuses edits to closed items except tag changes. `lock_acquire` grants a lock only to a running item with a live lease (`iter_core::waitgraph::lock_grant`). `engine_heartbeat` stores the engine's status and replies with how many graph edits wait for it. Every write bumps the change counter for its table.

What goes in and out: callers are iter_engine — local engine, the `iter` command line and webui — web page. It calls the Storage interface for every read and write, and iter_core for the rules. It provides the interfaces workitem-create, lock-acquire and versions-poll.

Why it matters: this is where the rules are enforced. Without it every client would need to be trusted to follow them.

Example: an agent runs `iter add` for a follow-up; `POST /api/projects/iter4/workitems` stores it with the parent's priority and use-case tag, writes the request as detail row 0, and returns the new record, with a warning if the requested priority was overridden.
