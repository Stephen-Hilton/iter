---
id: 6a737446-4b3b-4b64-babf-ab6706dc4f0b
name: "Graph edit applier"
description: "Applies the graph edits waiting for this engine's projects: it skips any edit whose folders a running item has locked, claims the rest one at a time, writes the files, commits exactly those files, pushes, reports back and re-uploads the map, so a change made on the map reaches the repository within one heartbeat."
simple_description: "Writes the changes made on the map into the project's files and saves them."
level: component
owner: bespoke
teststate: inherit
children:
  codedirs:   ["{topdir}/iter_engine/src/datasync.rs"]
  codenodes:  []
  inputs:     ["{topdir}/interfaces/datasync-claim/datasync-claim.interface.iter.md", "{topdir}/interfaces/lock-acquire/lock-acquire.interface.iter.md"]
  outputs:    []
  bizreqs:    []
  techreqs:   []
  tests:      []
---

# Long Description

The graph edit applier is the engine half of datasync (`iter_engine/src/datasync.rs`). On every tick the engine scheduler loop sends its heartbeat; when the reply's `datasync_waiting` names a project this engine serves, the loop calls `apply_waiting` for that project's checkout.

For each claimable edit, oldest first, it reads the project's live lock rows (`GET …/locks`) and leaves the edit waiting if a running item holds a lock on any folder the edit will write, so an agent's work in progress is never disturbed. Otherwise it claims the edit (only one engine can win), runs `git pull --no-rebase` when the checkout has a remote, and hands the operation to the graph edit writer (`iter_local/src/graph_edit.rs: apply`).

It then commits exactly the files the edit wrote (`git add -- <files>` and `git commit -- <files>`, never anyone else's changes), pushes, and reports `applied` with the commit id, or `failed` or `retry` with the reason. When an edit defines tests and asks for the test agent, it also files a `test` work item that writes and runs them. Finally it uploads the map (`sync::sync_if_changed`) so every open Project graph redraws.

Without it, edits made on the map would never reach the repository. Example: "Ledger API" is added under Data; on the next tick the applier writes four files, commits them with the message "iter: graph edit — + container Ledger API", pushes, and the map shows the new part.
