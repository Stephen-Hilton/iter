---
id: 7e7cc111-f267-4cab-ab84-1edab3381f89
name: "Locks and waits"
desc: "Decides when a lock on a folder counts (only while its holder's run is live), builds the graph of which open item waits on which, and finds loops in it, so a stuck queue is refused at write time or named instead of stalling silently."
creator: "iter migrate5"
teststate: inherit
level: component
owner: bespoke
children:
  codedirs:  ["{topdir}/iter_core/src/waitgraph.rs"]
  codenodes: []
  tests:     []
  reqs:      []
timestamps: {create: "2026-10-02 23:05:35Z", last_modified: "2026-10-02 23:15:00Z", last_tested: ""}
---

# Long Description

## Summary

The rules that stop two jobs editing the same files and spot jobs stuck waiting on each other.

Locks and waits keeps two runs out of the same folders and makes sure the queue can never deadlock silently.

How it works: a lock belongs to a run, not to an item. When the engine claims an item it writes a random `lease` on it; every exit from the run clears it. `iter_core/src/waitgraph.rs: lock_grant` gives a lock only to an item that holds a live lease (and a reservation only to a queued item), and `lock_row_is_live` counts a lock row only while it is unexpired, `kind` lock, and carries its holder's current lease — a queued holder never counts. Rows last `LOCK_LEASE_TTL_SEC` (600 s) and the engine renews them every 60 s. `lock_sk` stores a reservation under `reserve:<path>`, so it never blocks another item's lock on the same path. `sweep_reason` says why a stale row may be deleted. On top of that, `wait_edges` builds a wait-for graph over every open item — dependency edges, implied follow-up edges, and lock edges to live holders — and `find_wait_cycles` finds loops in it. `dependency_cycle_on_write` checks a proposed item before it is stored. `describe_cycle` writes the loop in words for a person.

What goes in and out: iter_data calls `lock_grant`, `live_lock_rows` and the cycle check (in `lock_acquire`, `refuse_dependency_cycle` and `GET …/deadlocks`), and in iter5 its server-side get_next (`iter_data/src/next.rs`) uses `live_lock_rows`, `lock_sk` and `sweep_reason` to drop stale rows and take an item's locks under the same lease as its claim, rolling the claim back when a lock cannot be had; iter_engine uses `live_lock_rows`, `lock_holders` and `find_wait_cycles` when dispatching and in `detect_deadlocks`. Folder overlap itself comes from the Work-item model (`paths_overlap`).

Why it matters: on 2026-09-25 a queued item still held 42 lock rows, and four priority-0 items stalled for almost six hours with nothing saying why. Tying locks to live runs makes a loop through a lock impossible; the cycle finder names any dependency loop.

Example: item A declares it is blocked by B while B is blocked by A; the create is refused with the loop's path named.
