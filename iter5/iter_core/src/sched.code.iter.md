---
id: 1f6f07cc-b5e3-4642-bf6f-7d538b33f673
name: "Schedules"
desc: "Decides when a scheduled template is due — every N minutes, daily or weekly at a local time, or when the last run went stale — and produces the queued copy that actually runs, skipping missed times instead of catching up."
creator: "iter migrate5"
teststate: inherit
level: component
owner: bespoke
children:
  codedirs:  ["{topdir}/iter_core/src/sched.rs"]
  codenodes: []
  tests:     []
  reqs:      []
timestamps: {create: "2026-10-02 23:05:35Z", last_modified: "2026-10-02 23:15:00Z", last_tested: ""}
---

# Long Description

## Summary

The rules for jobs that repeat on a timetable.

Schedules turn a template into regular runs. A schedule is an ordinary work item left in state `scheduled`, which the dispatcher never runs; when it is due, the engine queues a copy.

How it works: the template's `sched` field (`iter_core/src/sched.rs: Sched`) has a `kind` — `every` (every N minutes after the last firing), `stale` (N minutes after the newest of last completion, last firing or creation), `daily` or `weekly` (at "HH:MM", on a given weekday for weekly, in an optional IANA time zone) — and `last_fired`, which survives restarts. `due` answers yes or no for a moment in time. For daily and weekly it finds the latest occurrence and fires only if that occurrence is less than 150 seconds old (`OCCURRENCE_WINDOW_SEC`): a time missed while the engine was down is skipped, never backfilled. `clone_from` makes the run: same work, a new id assigned by the server, state `queued`, no schedule, `source_schedule` pointing back at the template, `createdby` "scheduler", attempt 0. `is_open_state` lists the clone states that keep the template from firing again, so one schedule never has two runs open.

What goes in and out: iter_engine's tick calls `due` and `clone_from` (`iter_engine/src/engine.rs: fire_schedules`) and posts the clone. iter_data refuses to create a scheduled item from the engine role, so schedules come only from people. The engine-owned test sweep (one paused, undeletable template per project; `iter sweep --install-schedule` turns it on) and the cluster-restart window are schedules. In the settings graph every project gets an `allows` edge to the `scheduled` work-item state that holds its schedule policy (seeded by iter_data).

Why it matters: without the open-run guard a slow job would pile up copies; without skip-don't-backfill, an engine back from a weekend would fire every missed run at once.

Example: a weekly template "mon 06:00 America/Los_Angeles" fires at 06:00:40 Monday; an engine that was down until 07:00 fires nothing and waits for next Monday.
