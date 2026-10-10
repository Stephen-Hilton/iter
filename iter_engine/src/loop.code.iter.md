---
id: 557274dd-7d77-4fe9-a98f-90b942fb7f20
name: "Engine tick loop"
desc: "Starts the machine's one engine from `--data-url`, `--env-file` and `--name`, then wakes every few seconds to reload its assignments, renew running leases, pick an account per project, heartbeat, run each served project's build / document / file-sync hooks, refresh cached project data, fire schedules, triage duplicates and — once the engine-side cap, budget and account checks pass — ask iter_data's `next` for a claimed item and start it on its own thread, so that every assigned project's queue flows to agents without a person pressing start."
creator: "iter migrate5"
teststate: inherit
level: component
owner: bespoke
children:
  codedirs:  ["{topdir}/iter_engine/src/engine.rs", "{topdir}/iter_engine/src/main.rs"]
  codenodes: []
  tests:     []
  reqs:      []
timestamps: {create: "2026-10-02 23:05:35Z", last_modified: "2026-10-02 23:15:33Z", last_tested: ""}
---

# Engine tick loop

## Summary

The engine's heartbeat: every few seconds it checks each assigned project and starts the next jobs that are allowed to run.

## How it works

`iter_engine/src/main.rs: main` parses `--data-url` (else `$ITER_DATA_URL`), `--env-file` (default `.env`)
and `--name` (default the short hostname), seeds the env store, builds an `EngineRuntime` and runs it. The
same `main` also routes `iter_engine cli …` to the command line and serves the one-shot helper flags
(`--adduser`, `--approve`, `--accounts` / `--probe`, `--question-widget`, `--doc`).

`EngineRuntime::run` (`iter_engine/src/engine.rs`) registers the engine on first start and calls `tick`
until stopped (`--ticks N` for tests, then `drain`). Each `tick`:

1. `prune_running` drops finished threads; `renew_leases` keeps each running item's lock rows alive
   (`retake_locks` re-takes lapsed rows or requeues the run); `reload_env` picks up new tokens.
2. `load_assignments` (see Engine assignments) registers each account's provider and model with the
   provider module; the account to show is chosen with `iter_core::pick_account` against other live engines.
3. The heartbeat posts state, account, usage (null while holding: "all accounts at stop%" or "no account
   token"), running counts and every account's windows; its reply drives `run_filesync` per served
   project (`build_waiting` → `filesync::build`, `datasync_waiting` → `datasync::apply_waiting`,
   `files_waiting` → `filesync::tick`) and `start_summaries` for GraphRAG.
4. Per served project: caches reload only when `/versions` counters moved (or the periodic full
   refresh); the assignment's accounts and run state overlay the project record; `ensure_test_sweep`
   creates the paused Test sweep schedule; `replay_pending_closes`, `repair_ghosts`, `start_explains`;
   Draining projects are settled; read-only checkouts and missing checkouts (a designed project before
   its build) start nothing; otherwise `fire_schedules` and `dispatch`.
5. `dispatch` checks the cluster-restart tag, the account ladder and usage-based `maxagents` gates,
   daily cost, retry backoff and approvals; starts test-sweep runs and human-accepted closes outside the
   cap (`claim_direct`); runs stage-2 dedup triage on new items; writes wait reasons
   (`reconcile_waits`, "blocked by: …") and breaks or reports deadlocks (`detect_deadlocks`); then calls
   `get_next` (`POST /api/projects/{p}/next`, with the `run_now` hint when the cap is full) and hands each
   claimed item to `start_claimed`, which spawns the Work runner.

## What goes in and out

In: engine record, assignments, `/versions`, project / agent / work-item lists, heartbeat reply. Out:
heartbeat, next / claim calls, schedule clones, wait-reason writes, threads running items.

## Why it matters

Without it nothing runs. Its checks keep spend under each project's limits, never bill an account
without a token, and keep one engine fair across every project it serves.

## Example

An engine serves projects `pdy` and `iter5`. In one tick it syncs both checkouts, finds `pdy` Stopped
(starts nothing there), and for `iter5` asks `next`, gets an item claimed and locked on the server, and
starts it billed to the account whose usage is below its switch percentage.
