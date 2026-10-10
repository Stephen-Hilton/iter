---
id: 30daf0e6-6384-4e47-9009-8a5a0805e8f5
name: "Edit a node file and see it in the graph"
desc: "A developer or an agent edits, adds or deletes a *.iter.md node file in a checkout; within one engine tick the engine's file scan sees the changed mtime/size, conform repairs and canonicalises the file (writing it back outside live locks), and POST files/sync hands the text to iter_data, which parses it, upserts the node by id, resolves conflicts (newer last_modified wins), recomputes edges, and the Project graph shows the change."
creator: "stephen"
teststate: inherit
actors: ["{topdir}/global/usecases/developer.actor.iter.md", "{topdir}/global/usecases/claude_agent.actor.iter.md"]
flowmap:
  summary: "Every tick, for every served project, the engine stats the known node files and folders under scandirs (git-ignore aware; a full walk every few minutes), re-reads only what moved, conforms each changed file with iter_core::nodefile (stamping a hand edit with the file's mtime) and writes back what conform changed, then posts the changed texts and deleted paths to files/sync with the node_version it last acked. iter_data parses each file, upserts the node by id, settles conflicts and id collisions, recomputes edges and replies with applied versions and any rewrites; the engine commits conform write-backs and rewrites together."
  sequence: ["{topdir}/global/usecases/developer.actor.iter.md", "{topdir}/iter_engine/src/loop.code.iter.md", "{topdir}/iter_engine/src/mapsync.code.iter.md", "{topdir}/iter_local/src/walk.code.iter.md", "{topdir}/iter_core/src/nodefile/nodefile.code.iter.md", "{topdir}/iter_data/src/filesync.code.iter.md", "{topdir}/iter_data/src/nodes.code.iter.md", "{topdir}/webui/projectgraph.code.iter.md"]
  process_flow:
    - step: 1
      from: "{topdir}/global/usecases/developer.actor.iter.md"
      to: "{topdir}/iter_engine/src/mapsync.code.iter.md"
      what: "The developer (or an agent inside a work item) saves a change to a node file, adds a new *.<type>.iter.md file or deletes one."
      plain: "Someone edits a file."
      evidence: "any editor; agents edit inside their lock scope"
    - step: 2
      from: "{topdir}/iter_engine/src/loop.code.iter.md"
      to: "{topdir}/iter_engine/src/mapsync.code.iter.md"
      what: "On its next tick the engine runs the file-sync service for each served project."
      plain: "The engine checks the files every few seconds."
      evidence: "iter_engine/src/engine.rs: run_filesync → filesync::tick"
    - step: 3
      from: "{topdir}/iter_engine/src/mapsync.code.iter.md"
      to: "{topdir}/iter_local/src/walk.code.iter.md"
      what: "The filescan stats known files and folders; files whose (mtime, size) moved are re-read and folders whose mtime moved are re-listed, skipping git-ignored paths, .git, target, node_modules, .iter and .claude."
      plain: "Only the changed files are read."
      evidence: "iter_engine/src/filesync.rs: scan; iter_local/src/walk.rs"
    - step: 4
      from: "{topdir}/iter_engine/src/mapsync.code.iter.md"
      to: "{topdir}/iter_core/src/nodefile/nodefile.code.iter.md"
      what: "Each changed file is conformed (missing keys added, legacy keys renamed, a missing id assigned, key order canonical, last_modified set from the file's mtime when the content changed); a changed text is written back unless the file is inside a live lock or the checkout is read-only."
      plain: "The file is tidied into the standard shape."
      evidence: "iter_engine/src/filesync.rs: tick → nodefile::conform_against"
    - step: 5
      from: "{topdir}/iter_engine/src/mapsync.code.iter.md"
      to: "{topdir}/iter_data/src/filesync.code.iter.md"
      what: "The engine posts files/sync {engine, full, files: [{path, text, hash, base_version}], deleted: [path]}."
      plain: "The changes go to the server."
      evidence: "POST /api/projects/{p}/files/sync"
    - step: 6
      from: "{topdir}/iter_data/src/filesync.code.iter.md"
      to: "{topdir}/iter_data/src/nodes.code.iter.md"
      what: "The server parses each file and upserts the node by id. If the node also changed in the graph since base_version, the newer last_modified wins (tie: server) and the loser is kept as a node_conflict row; a second file with an existing id gets a new id via a rewrite. Edges are recomputed. The reply lists applied versions, rewrites and removed ids; the engine writes the rewrites and commits them with the conform write-backs (\"iter: conform <n> node files\")."
      plain: "The graph takes the file's version."
      evidence: "iter_data/src/filesync.rs: apply_sync, files_sync"
    - step: 7
      from: "{topdir}/webui/projectgraph.code.iter.md"
      to: "{topdir}/iter_data/src/filesync.code.iter.md"
      what: "When the Project graph next loads graph/view it draws the updated node and edges; conflicts are listed from graph/conflicts."
      plain: "The drawing shows the edit."
      evidence: "GET /api/projects/{p}/graph/view, GET …/graph/conflicts"
  data_flow:
    - step: 1
      from: "{topdir}/iter_engine/src/mapsync.code.iter.md"
      to: "{topdir}/iter_data/src/filesync.code.iter.md"
      data: "Changed files as {path, text, hash, base_version} plus deleted paths."
      plain: "The changed file texts travel to the server."
    - step: 2
      from: "{topdir}/iter_data/src/filesync.code.iter.md"
      to: "{topdir}/iter_data/src/nodes.code.iter.md"
      data: "The parsed node (id, nodetype, level, name, desc, children, front, body, path) with file_hash and file_version; derived edges in link."
      stored: true
      plain: "The node and its links rest in the graph."
children:
  codedirs:  []
  codenodes: ["{topdir}/iter_engine/src/loop.code.iter.md", "{topdir}/iter_engine/src/mapsync.code.iter.md", "{topdir}/iter_local/src/walk.code.iter.md", "{topdir}/iter_core/src/nodefile/nodefile.code.iter.md", "{topdir}/iter_data/src/filesync.code.iter.md", "{topdir}/iter_data/src/nodes.code.iter.md", "{topdir}/webui/projectgraph.code.iter.md"]
  tests:     []
  reqs:      []
timestamps: {create: "2026-10-02 23:12:30Z", last_modified: "2026-10-02 23:12:30Z", last_tested: ""}
---
# Edit a node file and see it in the graph

Node files can be changed outside iter — by a person in an editor, by `git
pull`, or by an agent during a work item. Within a few seconds the project
graph says the same thing as the file.

The engine does the watching: it stats only, reads only what changed, and
first repairs the file with the same conform rules the server uses, so a
half-written frontmatter becomes a well-formed node (and the repaired text is
committed). The server takes the file's version unless the node was edited in
the graph more recently, in which case the newer edit wins and the other is
kept as a conflict for a person to review.

`iter sync` runs the same round once from a shell; a first sync of a checkout
with no saved state sends every file (`full`), and nodes whose files are gone
are marked deleted.
