---
id: cf15b5de-50e7-47e6-8512-3e26b311e5f5
name: "Edit a node in the graph and see it in the file"
desc: "A developer (or an agent through MCP) adds, edits, links, moves or deletes a node in the Project graph; the server applies the change at once, renders the node through iter_core::nodefile and marks it pending_write; the engine serving the project learns files_waiting from its heartbeat, pulls files/pending, writes the files outside any live lock, commits only those files and acks, and the graph's sync badge returns to zero."
creator: "stephen"
teststate: inherit
actors: ["{topdir}/global/usecases/developer.actor.iter.md", "{topdir}/global/usecases/claude_agent.actor.iter.md"]
flowmap:
  summary: "The graph editor sends one edit (create, patch, delete, move, edge add/remove/move). iter_data loads the project graph, changes the owning node, re-renders and conforms it, bumps node_version, sets file_state pending_write and recomputes edges, so every viewer sees the edit immediately. The next heartbeat reply names the project in files_waiting; the engine fetches files/pending, writes or removes each file (refusing paths outside the topdir, waiting on paths inside a live lock), commits only those files as \"iter: graph edit — …\", pushes if there is a remote, and acks, which marks the nodes synced."
  sequence: ["{topdir}/global/usecases/developer.actor.iter.md", "{topdir}/webui/grapheditor.code.iter.md", "{topdir}/iter_data/src/graph.code.iter.md", "{topdir}/iter_data/src/nodes.code.iter.md", "{topdir}/iter_core/src/nodefile/nodefile.code.iter.md", "{topdir}/iter_data/src/api.code.iter.md", "{topdir}/iter_engine/src/loop.code.iter.md", "{topdir}/iter_engine/src/mapsync.code.iter.md", "{topdir}/iter_data/src/filesync.code.iter.md"]
  process_flow:
    - step: 1
      from: "{topdir}/global/usecases/developer.actor.iter.md"
      to: "{topdir}/webui/grapheditor.code.iter.md"
      what: "In the Project graph the developer creates a node under a parent, edits one in the configure lightbox, drags an edge endpoint, pastes a copied edge, or deletes a node (a reason is required)."
      plain: "The developer changes the drawing."
      evidence: "webui/graphedit.js: POST graph/nodes, PATCH graph/nodes/{id}, POST graph/edges/move, DELETE graph/nodes/{id}"
    - step: 2
      from: "{topdir}/webui/grapheditor.code.iter.md"
      to: "{topdir}/iter_data/src/graph.code.iter.md"
      what: "The editor sends the edit to the project-graph routes; an agent can send the same edit through the MCP tools graph_node_create / graph_node_update / graph_edge_add / graph_edge_remove."
      plain: "The edit reaches the server."
      evidence: "iter_data/src/graph.rs routes; iter_data/src/mcp.rs"
    - step: 3
      from: "{topdir}/iter_data/src/graph.code.iter.md"
      to: "{topdir}/iter_data/src/nodes.code.iter.md"
      what: "The server loads the project graph, changes the node that owns the edit (for supplies/connects edges the connection node; for a new node the path comes from the designer folder rules), and saves it with node_version + 1 and file_state pending_write; edges are recomputed."
      plain: "The node changes and waits to be written."
      evidence: "iter_data/src/nodes.rs: Graph::load, Graph::save"
    - step: 4
      from: "{topdir}/iter_data/src/nodes.code.iter.md"
      to: "{topdir}/iter_core/src/nodefile/nodefile.code.iter.md"
      what: "Every stored node is conformed and rendered by the shared node-file library, so the text the engine will write is exactly what conform would produce from the file."
      plain: "The node is turned into its final file text."
      evidence: "iter_data/src/nodes.rs: canonical → nodefile::render"
    - step: 5
      from: "{topdir}/iter_engine/src/loop.code.iter.md"
      to: "{topdir}/iter_data/src/api.code.iter.md"
      what: "On its next tick the engine heartbeats; the reply's files_waiting lists the projects with pending_write or pending_delete nodes."
      plain: "The engine learns there is something to write."
      evidence: "iter_data/src/api.rs heartbeat → sync_hooks::files_waiting"
    - step: 6
      from: "{topdir}/iter_engine/src/mapsync.code.iter.md"
      to: "{topdir}/iter_data/src/filesync.code.iter.md"
      what: "The engine fetches files/pending, writes, deletes or moves each file (paths leaving the topdir are refused; a path inside a live work-item lock waits for a later tick), commits only the touched files as \"iter: graph edit — …\", pushes if a remote exists, and acks each file with its node_version, hash and commit."
      plain: "The engine writes and commits the files."
      evidence: "iter_engine/src/filesync.rs: apply_pending, commit_paths; POST …/files/ack"
  data_flow:
    - step: 1
      from: "{topdir}/webui/grapheditor.code.iter.md"
      to: "{topdir}/iter_data/src/nodes.code.iter.md"
      data: "The edit (node fields or edge from/kind/to); it rests as the updated node with file_state pending_write."
      stored: true
      plain: "The change rests in the graph at once."
    - step: 2
      from: "{topdir}/iter_data/src/filesync.code.iter.md"
      to: "{topdir}/iter_engine/src/mapsync.code.iter.md"
      data: "Pending ops [{id, op: write|delete|move, path, old_path?, text, node_version}]."
      plain: "The final file texts travel to the engine."
    - step: 3
      from: "{topdir}/iter_engine/src/mapsync.code.iter.md"
      to: "{topdir}/iter_data/src/filesync.code.iter.md"
      data: "Acks [{id, node_version, path, hash, commit}]; the node becomes synced with file_version = node_version."
      stored: true
      plain: "The server records that the file matches."
children:
  codedirs:  []
  codenodes: ["{topdir}/webui/grapheditor.code.iter.md", "{topdir}/iter_data/src/graph.code.iter.md", "{topdir}/iter_data/src/nodes.code.iter.md", "{topdir}/iter_data/src/mcp.code.iter.md", "{topdir}/iter_core/src/nodefile/nodefile.code.iter.md", "{topdir}/iter_data/src/filesync.code.iter.md", "{topdir}/iter_engine/src/loop.code.iter.md", "{topdir}/iter_engine/src/mapsync.code.iter.md"]
  tests:     []
  reqs:      []
timestamps: {create: "2026-10-02 23:12:30Z", last_modified: "2026-10-02 23:12:30Z", last_tested: ""}
---
# Edit a node in the graph and see it in the file

The graph and the files are one thing in two places. A developer changes the
graph — a new component under a container, a new description, an edge
dragged from one supplier to another, a deleted node — and the change shows
in every open Project graph immediately, with a sync badge counting the
edits still waiting to be written.

The server cannot write the repo; the engine serving the project does. Its
next heartbeat says `files_waiting`, it pulls the pending list, writes each
file, commits only those files and acknowledges them. The badge returns to
zero. An agent can make the same edits through the MCP graph tools, and they
travel the same way.

If someone edited the same file by hand in the meantime, the newer
`timestamps.last_modified` wins and the loser is kept as a conflict
(`GET …/graph/conflicts`).
