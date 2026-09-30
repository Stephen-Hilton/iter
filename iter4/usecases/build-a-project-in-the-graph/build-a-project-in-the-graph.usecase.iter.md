---
id: 1636948d-0dc2-4397-bb9d-f6861fa947a0
name: "Build a project from the graph"
description: "A developer adds a new part to the project from the Project graph and connects it; an engine that serves the project writes the files into the repo, commits them, and the map redraws with the new part."
teststate: inherit
children:
  codenodes:  ["{topdir}/webui/grapheditor.code.iter.md", "{topdir}/iter_data/src/graph.code.iter.md", "{topdir}/iter_local/src/edits.code.iter.md", "{topdir}/iter_data/src/datasync.code.iter.md", "{topdir}/iter_engine/src/datasync.code.iter.md"]
  tests:      []
flowmap:
  summary: In the Project graph a developer adds a container under a context. The page sends the change to the data server, which accepts it at once as an edit waiting to sync. On its next heartbeat an engine that serves the project claims the edit, writes the node files and the parent link into its checkout, commits exactly those files and pushes, then uploads the new map, and the page redraws with the new part.
  sequence:
  - actor:developer
  - '{topdir}/webui/grapheditor.code.iter.md'
  - '{topdir}/iter_data/src/graph.code.iter.md'
  - '{topdir}/iter_data/src/datasync.code.iter.md'
  - '{topdir}/iter_engine/src/loop.code.iter.md'
  - '{topdir}/iter_engine/src/datasync.code.iter.md'
  - '{topdir}/iter_local/src/edits.code.iter.md'
  - '{topdir}/iter_engine/src/mapsync.code.iter.md'
  process_flow:
  - step: 1
    from: actor:developer
    to: '{topdir}/webui/grapheditor.code.iter.md'
    what: The developer selects a context on the map, clicks `+ Container`, and types the new part's name and description.
    plain: The developer asks for a new part.
    evidence: 'webui/graphedit.js: open(''child'', node)'
  - step: 2
    from: '{topdir}/webui/grapheditor.code.iter.md'
    to: '{topdir}/iter_data/src/graph.code.iter.md'
    what: 'The graph editor sends the change to the data server as one small operation (`new_node`: kind, name, description, parent folder) and shows “Accepted — waiting to sync to an engine”.'
    plain: The page sends the change to the server.
    evidence: POST /api/projects/{p}/graph/edits
  - step: 3
    from: '{topdir}/iter_data/src/graph.code.iter.md'
    to: '{topdir}/iter_data/src/datasync.code.iter.md'
    what: The data server stores the change as a waiting-to-sync row (only a Run tests request becomes a work item), and the next heartbeat reply to each engine serving the project says an edit is waiting.
    plain: The change waits for an engine to pick it up.
    evidence: 'iter_data/src/datasync.rs: create; heartbeat reply datasync_waiting'
  - step: 4
    from: '{topdir}/iter_engine/src/loop.code.iter.md'
    to: '{topdir}/iter_data/src/datasync.code.iter.md'
    what: On its next tick the engine scheduler loop sends its heartbeat; the reply says an edit is waiting, so it hands the project to the graph edit applier, which checks no running item holds a lock on the edit's folders and claims the edit so only one engine applies it.
    plain: An engine takes the change.
    evidence: 'iter_engine/src/engine.rs: tick → datasync::apply_waiting; POST …/datasync/{id}/claim'
  - step: 5
    from: '{topdir}/iter_engine/src/datasync.code.iter.md'
    to: '{topdir}/iter_local/src/edits.code.iter.md'
    what: The graph edit applier hands the operation to the graph edit writer, which writes the code, bizreq, techreq and tests files and adds the new part to its parent's children list; the applier commits exactly those files, pushes, and reports the edit applied with its commit id.
    plain: The files are written and committed.
    evidence: 'iter_engine/src/datasync.rs: apply_waiting → iter_local/src/graph_edit.rs: apply'
  - step: 6
    from: '{topdir}/iter_engine/src/datasync.code.iter.md'
    to: '{topdir}/iter_data/src/graph.code.iter.md'
    what: The graph edit applier uploads the checkout's new map snapshot straight away, so every open Project graph redraws with the new part.
    plain: The map is updated and the page redraws.
    evidence: iter_engine/src/datasync.rs → sync::sync_if_changed → PUT /api/projects/{p}/graph
  data_flow:
  - step: 1
    from: '{topdir}/webui/grapheditor.code.iter.md'
    to: '{topdir}/iter_data/src/datasync.code.iter.md'
    data: The edit operation as one small JSON (op, kind, name, description, parent); it rests in the data server as a waiting-to-sync row until an engine claims and applies it.
    plain: The change travels as one small message.
  - step: 2
    from: '{topdir}/iter_engine/src/datasync.code.iter.md'
    to: '{topdir}/iter_data/src/graph.code.iter.md'
    data: The new map snapshot (every vertex and edge, now including the new part and its link to the parent); it rests in the data server's stored map.
    stored: true
    plain: The new part rests in the map.
---

# Build a project from the graph

A developer grows the project from its map instead of from the repo. In the Project graph they add a container under a context. The page sends the change to the data server, which accepts it at once as an edit waiting to sync; nothing in the repo has changed yet, because the data server has no access to it.

On its next heartbeat an engine that serves the project learns an edit is waiting. It checks that no running work item holds a lock on the folders the edit will write, claims the edit so no other engine applies it too, pulls the repo, and has the graph edit writer create the new node files and add the link from the parent. It commits exactly those files, pushes, and reports the commit back.

The engine then uploads the new map snapshot, and every open Project graph redraws with the new part in place. The toolbar's "waiting to sync" count drops back to zero. If the edit fails, the toolbar shows it as failed with the reason.
