---
id: 557274dd-7d77-4fe9-a98f-90b942fb7f20
name: "Engine scheduler loop"
description: "Wakes every few seconds to report the engine's health, keep running items' locks alive, fire due schedules, pick the next queued items that are free to run and start them, so that work flows from the queue to agents without a person pressing start."
simple_description: "The engine's heartbeat: every few seconds it checks the queue and starts the next jobs that are allowed to run."
level: component
owner: bespoke
teststate: inherit
children:
  codedirs:   ["{topdir}/iter_engine/src/engine.rs", "{topdir}/iter_engine/src/main.rs"]
  codenodes:  []
  inputs:     ["{topdir}/interfaces/versions-poll/versions-poll.interface.iter.md", "{topdir}/interfaces/lock-acquire/lock-acquire.interface.iter.md"]
  outputs:    []
  bizreqs:    []
  techreqs:   []
  tests:      []
---

# Long Description

The Engine scheduler loop is the engine's main process. It decides, every tick (default 5 seconds), what may run now and starts it.

How it works: `iter_engine/src/main.rs: main` reads `.iter/config.json`, loads tokens, resolves the data server address and calls `EngineRuntime::run` in `iter_engine/src/engine.rs`. `run` loads the engine's record (registering it on first start) and calls `tick`. Each tick: `prune_running` drops finished threads; `renew_leases` keeps each running item's lock rows alive (a lock row marks a folder as being edited by one item; a lease is the run's claim on it); `reload_env` picks up new tokens; `sync_maps` re-pushes the project map when files changed. It then chooses an account from real usage, sends the heartbeat, applies waiting graph edits, handles stop requests, and for each project it serves: replays unsaved closes, repairs ghost runs (`repair_ghosts`), starts explain requests, fires due schedules (`fire_schedules`) and calls `dispatch`.

`dispatch` sorts queued items and checks each one: dependencies closed complete, no lock held on its folders by another running item, approval, retry backoff, usage cap and daily cost. It writes the reason on every item that must wait (`reconcile_waits`, "blocked by: …"), breaks or reports deadlocks (`detect_deadlocks`), and claims the rest with a versioned write (`start_item`) before handing each to the Work runner on its own thread.

It uses the Data server client throughout, the Account usage tracker for the account choice, the Duplicate work judge before a first dispatch, the Map uploader for `sync_maps`, and the Work runner to execute items.

Why it matters: without it nothing runs. Its checks keep two agents out of the same folder and keep spend under the project's limits.

Example: a new item is queued for folder `api/`. Next tick the loop sees another running item holding `api/`, tags the new one "blocked by: lock …"; when that run closes, the next tick claims it and starts it.
