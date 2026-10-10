---
id: eb9e9612-37f6-40dc-9274-91992e5a30f9
name: "Node file sync and build"
desc: "Keeps the project graph and the repo's node files in step through the engine, since iter_data never touches a repo: files/sync takes the node files an engine scanned (upsert by id, id collisions renumbered, conflicting edits settled by newer last_modified with the loser kept), files/pending hands out the writes, deletes and moves still to do, files/ack records them as synced, and build / build/done turn a designed project into a new repo. It also tells the heartbeat which projects have files or builds waiting."
creator: "agent.code"
teststate: inherit
level: component
owner: bespoke
children:
  codedirs:  ["{topdir}/iter_data/src/filesync.rs", "{topdir}/iter_data/src/sync_hooks.rs"]
  codenodes: []
  tests:     []
  reqs:      []
timestamps: {create: "2026-10-02 23:10:16Z", last_modified: "2026-10-02 23:10:16Z", last_tested: ""}
---

# Long Description

## Summary

Carries changes both ways between the map on the server and the files in the project's repository.

Node file sync and build (`iter_data/src/filesync.rs`) is the engine's side door into the Project graph store. The engine serving a project pushes its node files here and pulls the graph edits waiting to be written; designed projects are built the same way.

How it works: `POST /api/projects/{p}/files/sync` (`SyncReq {engine, full, files: [{path, text, hash, base_version}], deleted}`) runs `apply_sync`: each file is parsed and re-rendered with the shared library (a differing canonical text is answered with a rewrite); nodes are upserted by id; an id another file already holds gets a new id (rewrite); when the node holds a server edit not yet written (`node_version > base_version`) and the file changed too, the newer `timestamps.last_modified` wins, a tie goes to the server, and the loser is kept as a `node_conflict` row. A newer test result on the node (`front.last_result`, `timestamps.last_tested`: posted by the engine, never edited in a file) is carried onto the winning content and the file rewritten; when the pending edit is only test results (`change` starts "test result ") nothing is in conflict and no row is written. Deleted paths — and with `full: true` every once-written file not sent — mark nodes deleted. The reply (`SyncReply`) lists `applied`, `rewrite`, `removed` and `conflicts`. `GET files/pending` (`pending_ops`) lists `write | delete | move` ops with the text and `node_version`; `POST files/ack` marks them `synced` with `file_version = node_version`. `POST build {engine, topdir, queue_plan, plan_note}` activates the `serves` edge (`activate_serves`), sets `project.build` to requested and flips `designed` nodes to `pending_write`; `POST build/done` records the commit (and queues the plan item when asked); `GET build` reports the state. `sync_hooks.rs` exposes `files_waiting`, `build_waiting` and `project_created` to the core API.

What goes in and out: iter_engine's filescan / sync and build services call these routes; the HTTP API's heartbeat reply includes `files_waiting` and `build_waiting` from here.

Why it matters: without it a graph edit would never reach the repo, and a file edit would never reach the graph.

Example: a person renames a node in the graph; the next heartbeat lists the project in `files_waiting`, the engine fetches one `write` op, commits the file, and acks it.
