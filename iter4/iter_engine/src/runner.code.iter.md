---
id: 0deb9eae-65e3-4123-a520-d46f24a3ef1b
name: "Work runner"
description: "Runs one claimed work item from start to finish (pulls the repo, runs the agent session or shell command, commits only the item's own files, passes the close gate and writes the result), so that each item ends in exactly one recorded outcome and its locks are always released."
simple_description: "Carries out one task from start to finish and records how it went."
level: component
owner: bespoke
teststate: inherit
children:
  codedirs:   ["{topdir}/iter_engine/src/work.rs"]
  codenodes:  []
  inputs:     ["{topdir}/interfaces/lock-acquire/lock-acquire.interface.iter.md"]
  outputs:    []
  bizreqs:    []
  techreqs:   []
  tests:      []
---

# Long Description

The Work runner does one work item. The Engine scheduler loop claims the item and hands it over on a worker thread; the runner does everything from there to the recorded result.

How it works (`iter_engine/src/work.rs`): `execute` runs `execute_one` and, when the session finished cleanly, may continue the same Claude session with a queued neighbour on the same folders (`claim_chain_candidate`, up to the project's chain limit). `execute_one` fetches the item's detail rows and closes at once if a human already accepted it. `run_all` then runs git prework (`git pull --no-rebase` when there is a remote), any shell pre-steps, and the main work: `run_exec` for a shell item, or `run_claude` for an agent item, which asks the Agent prompt builder for the turns, starts `claude` with the chosen account's token (`spawn_claude`, `resolve_account_token`) and parses its stream (`parse_claude_stream`), which also feeds the Account usage tracker. After the work, `commit_scoped` commits only paths inside the item's lock folders plus the project's `commit_extra_paths`, so a sibling agent's unfinished files are never committed under this item's name; then it pushes. If the agent moved its own item to `question` or `parked`, `close_keep_state` keeps that.

`close` runs the Close gate verifier's checks (`run_gate`), appends the response and spend rows, writes the new state with a versioned write, retrying through outages (`with_retry`) and saving the close to disk to replay later if the server stays away (`journal_close`, `replay_pending_closes`), and finally releases every lock (`release_all`). A run whose locks were taken away is stopped (`revoke`) and requeued or left alone.

It is called by the Engine scheduler loop and calls the Data server client, the Agent prompt builder, the Close gate verifier and the Account usage tracker.

Why it matters: without it a finished run could lose its result, leave locks held or commit someone else's files.

Example: a `code` item on `webui/` runs, commits two files in `webui/`, and the verifier finds it complete; the item closes complete and its lock on `webui/` is released.
