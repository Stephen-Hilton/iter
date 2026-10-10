---
id: b1389c4c-e62e-4ea8-9931-86eaebaf35da
name: "Server-side get_next"
desc: "Picks and claims the next work item for an engine that has room to start one, at POST /api/projects/{p}/next: only queued, ready items whose dependencies are satisfied and whose agent is enabled on the project, test-sweep runs first, then run-now, then priority and age; it skips items whose folders are locked, reserves paths for the best blocked item, claims the winner with one versioned write and takes its locks under the same lease, rolling the claim back if a lock is refused."
creator: "agent.code"
teststate: inherit
level: component
owner: bespoke
children:
  codedirs:  ["{topdir}/iter_data/src/next.rs"]
  codenodes: []
  tests:     []
  reqs:      []
timestamps: {create: "2026-10-02 23:10:16Z", last_modified: "2026-10-02 23:10:16Z", last_tested: ""}
---

# Long Description

## Summary

Hands each engine the most urgent piece of work it may start, and makes sure no two engines get the same one.

Server-side get_next (`iter_data/src/next.rs: get_next`) is the iter4 engine's dispatch pick moved onto the server, so one engine serving several projects asks the server for one project at a time and the choice is made where every engine's view agrees.

How it works: the request (`NextReq`: `engine`, `lease_ttl_sec`, optional `agents_allowed`) is refused unless the engine serves the project. The selection takes only `queued` items that do not need approval, are past `retry_after`, are not held by the cluster-restart tag while the cluster is unavailable, are not waiting for their stage-two dedup triage (`needs_triage`) and have `dependency_status == Satisfied`; the agent must be enabled on the project (an active `runs` edge) and in `agents_allowed` (shell items are exempt). Order: test-sweep runs, then `run_now` items, then (priority, receive time). An item whose lockdirs overlap a live lock is skipped; the first such item reserves its paths (`reserve` rows, 600 s) so lower-priority overlapping work is not admitted. The winner is claimed with one versioned write (in-progress, engine, attempt+1, new lease, start time, claim tags) and a lock row per lockdir is taken under that lease; if a lock is refused, the claim is rolled back and the next candidate tried. The reply is `{item, reason}` with reason `none-queued`, `blocked`, `locked`, `project-stopped` or `not-served`.

What goes in and out: iter_engine's agent service calls it after its own checks (max agents, budget, account); it reads edges from the Settings graph and work items and locks through the Storage interface, using iter_core's dependency and lock rules.

Why it matters: without it two engines could start the same item or overlapping work in the same folders.

Example: two engines ask at once; one versioned write wins, the other engine gets the next candidate or `{item: null, reason: "locked"}`.
