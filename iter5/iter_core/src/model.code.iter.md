---
id: b2ed3495-2da0-4670-a8ad-992269123cf8
name: "Work-item model"
desc: "Defines the records every part exchanges — work items and their states, projects, engines, agents, accounts, locks — and decides from them which item may start (dependencies), which runs first (priority bands) and which account to use."
creator: "iter migrate5"
teststate: inherit
level: component
owner: bespoke
children:
  codedirs:  ["{topdir}/iter_core/src/lib.rs"]
  codenodes: []
  tests:     []
  reqs:      []
timestamps: {create: "2026-10-02 23:05:35Z", last_modified: "2026-10-02 23:15:00Z", last_tested: ""}
---

# Long Description

## Summary

The shared definition of a job, a project and a worker, and the basic rules about what can start when.

The work-item model is the core vocabulary of iter: what a work item, project, engine, agent and account look like, and the basic rules about them.

How it works: `iter_core/src/lib.rs` defines the records as Rust structs that serialize to the JSON bodies stored on the server — `WorkItem` (name, agent, state, priority, lock folders, dependencies, tags, lease, timestamps, and in iter5 an optional `node`: the project-graph node the item works on, which the engine's context loader starts from), `WorkItemDetail` (the history rows under an item), `Project`, `Engine`, `Account`, `AgentDef`, `LockRow`, `VersionRow`. Fields a reader does not know round-trip untouched. `STATES` lists the eight states (queued, in-progress, question, parked, paused, failed, complete, scheduled) — the same eight are the settings graph's `workitem_type` nodes. `TABLES` lists the storage tables, including iter5's settings-graph tables (`account`, `provider`, `workitem_type`, `sys_edge`). `Project.engines` and `Project.accounts` are deprecated in iter5: they still parse so the one-time migration can turn them into `serves` / `bills` edges, but who runs and pays for a project now lives in the settings graph (Settings-graph types). Priority runs 0–99, lower sooner, in bands (`PRIO_BAND_DO_NOW` 0–9, use cases 10–39, human requests 40–49, maintenance 50–99); `pick_unused_priority` gives a new root item the lowest free number in its band. `dependency_status` decides whether an item may start: a blocker must be complete, and by default so must the open follow-ups a complete blocker created; it answers `Satisfied`, `Waiting`, `Failed`, `Cycle` or `SiblingFirst`, with `goes_first` breaking mutual waits the same way on every engine. `paths_overlap` decides when two lock folders collide. `pick_account` chooses a Claude account by order and usage thresholds, never one whose token is missing. `close_gate_for` and `lockshape_for` read an agent's close-gate and lock-shape settings.

What goes in and out: iter_data uses the records to parse and validate writes and runs `dependency_status`, `paths_overlap` and `claim_tags` inside server-side get_next (`iter_data/src/next.rs`, `POST /api/projects/{p}/next`), so the server picks, claims and locks in one call. iter_engine uses them to cache the queue, decide whether to ask for work (`pick_account` over the accounts its assignments list) and for session chaining and the close gate. It calls nothing.

Why it matters: every other part reads and writes these shapes; a change here is a change to the API.

Example: item B is blocked by A; A is complete but created follow-up C, which is still queued. `dependency_status(B)` returns `Waiting(C, Some(A))`, shown as "blocked by: waiting on … (follow-up of blocker …)".
