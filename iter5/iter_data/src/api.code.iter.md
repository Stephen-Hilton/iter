---
id: b18d7a67-a2e4-4955-ab10-19abb2b90e77
name: "HTTP API"
desc: "Answers the core requests to the data server — login, users and tokens, agents and tooling, projects, engines and their heartbeats, work items and their history rows, locks, spend, structure and pre/post work — checking the caller's role and the queue's rules (priority bands, dedup, dependency loops, versioned writes, closed-item immutability, lease-bound locks) before each write, and assembles the one router that merges every other route group under the project access layer."
creator: "iter migrate5"
teststate: inherit
level: component
owner: bespoke
children:
  codedirs:  ["{topdir}/iter_data/src/api.rs"]
  codenodes: []
  tests:     []
  reqs:      []
timestamps: {create: "2026-10-02 23:05:35Z", last_modified: "2026-10-02 23:10:16Z", last_tested: ""}
---

# Long Description

## Summary

Where most requests from the workers and the web page arrive, and where the queue's rules are enforced.

The HTTP API (`iter_data/src/api.rs`) holds the work queue's state machine: the webui, the engines, the `iter` command line and MCP all reach these same handlers, so the rules live in one place.

How it works: `router` lists the core routes and merges the project graph (`graph.rs`, which brings test logs and file sync), datasync, GraphRAG and settings-graph route groups, plus `POST /api/projects/{p}/next` (`next.rs`); one `authz::project_authz` layer covers them all. Each handler first extracts `AuthUser`: it verifies the bearer token (`auth.rs`) and compares the token's `tokenver` with the user's row, so bumping that number revokes every outstanding token; `require_admin` / `require_writer` gate writes (a `viewer` only reads). Then the queue's rules: `workitem_create` normalises the request, places it in its priority band (`place_new_item`), returns an already-open twin instead of filing a repeat (stage-one dedup), refuses a dependency loop (`refuse_dependency_cycle`) and writes the request as detail row 0; `workitem_put` needs `expect_version` and refuses edits to closed items except tag changes; `lock_acquire` grants a lock only to a running item with a live lease; `engine_heartbeat` stores the engine's status and replies with `datasync_waiting`, `files_waiting`, `build_waiting` and `rag_waiting` for the projects its `serves` edges name. Creating a project also creates its settings-graph edges and its project node (`sync_hooks::project_created`). Every write bumps the change counter for its table (`versions` route).

What goes in and out: callers are iter_engine (heartbeat, locks, results, explain claims), the webui and the MCP gateway; it calls the Storage interface for every read and write and iter_core for the rules.

Why it matters: without it every client would have to be trusted to follow the rules.

Example: an agent runs `iter add` for a follow-up; `POST /api/projects/iter5/workitems` stores it with the parent's priority and use-case tag and returns the new record, with a warning if the requested priority was overridden.
