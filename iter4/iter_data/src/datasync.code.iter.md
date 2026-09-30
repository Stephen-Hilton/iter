---
id: cf0ab2a4-b264-4123-8920-f0f44dedefea
name: "Graph edit inbox"
description: "Holds every graph edit the web page makes as a row waiting to sync, tells each engine on its heartbeat how many are waiting, and lets exactly one engine claim and finish each, so edits land in the repository without queueing behind agent work."
simple_description: "Keeps the changes made on the map until an engine writes them into the project's files."
level: component
owner: bespoke
teststate: inherit
children:
  codedirs:   ["{topdir}/iter_data/src/datasync.rs"]
  codenodes:  []
  inputs:     []
  outputs:    ["{topdir}/interfaces/datasync-claim/datasync-claim.interface.iter.md"]
  bizreqs:    []
  techreqs:   []
  tests:      []
---

# Long Description

The graph edit inbox is where a change made on the Project graph waits for an engine. When the web page sends an edit (`POST /api/projects/{p}/graph/edits`, handled by the Architecture map store in `iter_data/src/graph.rs: graph_edit`), the inbox stores it with `iter_data/src/datasync.rs: create` as a row in the `datasync` table: the operation, the folders it will write (`lock_scope`), a one-line summary, and the state `pending`.

Engines learn about waiting edits without polling for them: the heartbeat reply (`iter_data/src/api.rs: engine_heartbeat`) carries `datasync_waiting`, the count per project the engine serves, computed by `datasync::waiting`. An engine then lists the claimable rows (`GET …/datasync?state=claimable`, oldest first), claims one (`POST …/datasync/{id}/claim`) and reports back (`POST …/datasync/{id}/done` with `applied`, `failed` or `retry`, the commit and the files).

A claim is a versioned write, so when two engines race only one wins; a claim lapses after ten minutes (`CLAIM_SEC`), so an engine that dies mid-edit does not strand it. The web page reads the same rows for its "⟳ n waiting to sync" indicator.

Without the inbox a graph edit would have to be an ordinary work item and wait behind every running agent; with it, the fastest engine applies it within one heartbeat. Example: a developer adds "Ledger API" under Data; the row is pending for a few seconds, the StephenMBP engine claims it, writes and commits the files, and the row becomes `applied` with the commit id.
