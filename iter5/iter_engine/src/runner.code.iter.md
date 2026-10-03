---
id: 0deb9eae-65e3-4123-a520-d46f24a3ef1b
name: "Work runner"
desc: "Runs one claimed work item to its single recorded outcome: enforced git prework, then the agent's turns through the provider dispatch (prework, mainwork, agent memory, postwork, self-check in one session, with the `iter` shim and an MCP config pointing at iter_data) or the item's shell command, enforced git postwork, a commit scoped to the item's lockdirs, the close gate, the close (journaled when iter_data is unreachable) and the release of every lock; then it may chain queued neighbours into the same session."
creator: "iter migrate5"
teststate: inherit
level: component
owner: bespoke
children:
  codedirs:  ["{topdir}/iter_engine/src/work.rs"]
  codenodes: []
  tests:     []
  reqs:      []
timestamps: {create: "2026-10-02 23:05:35Z", last_modified: "2026-10-02 23:15:33Z", last_tested: ""}
---

# Work runner

## Summary

Takes one job from start to finish: has the agent (or a shell command) do it, saves only that job's files, checks the result and records exactly one outcome.

## How it works

`iter_engine/src/work.rs: execute` runs on the thread the Engine tick loop started. For each item
(`execute_one`):

1. A close-gate widget already answered "accept" closes the item with no run (`accepted_by_human`).
2. `run_all` runs the enforced git prework, then either `run_exec` (an `exec` / shell item, `ITER_SHELL=1`)
   or `run_claude`: the prompt from the Agent prompt builder, each turn sent through
   `provider::call` → `dispatch_agent` with the account's token (`resolve_account_token`; a named account
   with no token fails the run, never another account's token), the agent env (`ITER_PROJECT`,
   `ITER_WORKID`, `ITER_NODE`, `ITER_NODEFILE`, `ITER_PROVIDER`, …), the `.iter/bin/iter` shim
   (`write_iter_shim`) and a private MCP config (`mcp_config`: the `iter` HTTP MCP server at
   `<data_url>/mcp` with the engine token and `X-Iter-Project` / `X-Iter-Workid`). Timeouts and stop
   requests kill the whole process group (`wait_with_stop`).
3. An agent that moved the item to question / parked mid-run keeps that state (`close_keep_state`).
4. `close` commits only the lock scope plus the project's `commit_extra_paths` (`commit_scoped`,
   leftover count logged, never listed), gathers evidence (`git_run_evidence`), runs the close gate
   (`run_gate`, `verify_with_retry`), writes spend and the outcome, and releases every lock
   (`release_all`). A close iter_data cannot take is retried, then journaled to disk
   (`journal_close`) and replayed by the next tick (`replay_pending_closes`).
5. Session chaining: up to `session_chain_max` queued neighbours with the same lockdirs and use case are
   claimed with a versioned PUT (`claim_chain_candidate`) and run in the same provider session.

`repair_ghost` fixes in-progress records this engine no longer runs; `run_critic` serves `iter critreview`.

## What goes in and out

In: a claimed item, its detail rows, agent records, the account. Out: provider sessions or shell
processes, git commits, detail / spend / close writes to iter_data, lock releases.

## Why it matters

Each item ends in exactly one recorded outcome and its locks are always released, even on crash, revoke
or outage; and the commit never sweeps another agent's unfinished files into this item's history.

## Example

A `code` item locked to `api/` finishes; the runner commits only `api/` (one stray file elsewhere is
counted as leftover), the verifier answers complete, the item closes and its lock rows are gone.
