---
id: cc928643-4155-436f-a0c3-0699967faaed
name: "Project graph store"
desc: "Stores each project's graph as one node per *.iter.md node file — the parsed, conformed file plus its sync state (node_version, file_version, file_hash, file_state synced / pending_write / pending_delete / designed) — and keeps the edges fully derived from the nodes' children and connects lists, rewriting only the edges that changed in the same transaction. It gives the graph routes, file sync and test results one in-memory working copy per project with create, edit, edge, delete and move operations, and seeds a new project's project node and default requirements."
creator: "agent.code"
teststate: inherit
level: component
owner: bespoke
children:
  codedirs:  ["{topdir}/iter_data/src/nodes.rs"]
  codenodes: []
  tests:     []
  reqs:      []
timestamps: {create: "2026-10-02 23:10:16Z", last_modified: "2026-10-02 23:10:16Z", last_tested: ""}
---

# Long Description

## Summary

Keeps the project's map as nodes that mirror its node files one for one.

The project graph store (`iter_data/src/nodes.rs`) is iter5's "nodes = files" model on the server. Every `*.iter.md` node file of a project is one document of the ArangoDB collection `node` (key `node_key(project, id)`); the graph never holds anything a file could not say.

How it works: `StoredNode` holds the parsed file (`iter_core::nodefile::NodeDoc`) plus `node_version` (+1 on every server-side change), `file_version` (the version last written to or read from the file), `file_hash`, `file_state`, `deleted`, `test` and `updated_by`. Every write goes through `canonical`, the same render/conform the engine uses, so the file the engine later writes is byte-for-byte what conform would produce. Work on one project is serialised in-process (`lock`): `Graph::load` reads all its nodes, operations change them in memory — `insert_new`, `commit_edit`, `add_edge` / `remove_edge` (made on the node that owns the children entry; a glob that owns an edge is refused), `delete_node` (also removes it from every parent), `move_node` (rewrites references) — and `save` recomputes the whole project's edges with `nodefile::edges_of` against the project's node paths and writes the changed nodes and only the added or removed `link` edges in one stream transaction. `edit_state` makes an edit `pending_write`, or `designed` when no engine serves the project yet (`is_served`). `ensure_project_node` seeds a designed project's project node plus one philosophy, bizreq and techreq; `purge_project` removes a deleted project's nodes, links, `node_conflict` and `test_log` rows.

What goes in and out: the Project graph API, Node file sync and build, and Test results and logs all work through `Graph`; it talks to ArangoDB directly (`arango.rs`) and reads `serves` edges from the Settings graph.

Why it matters: it is what makes the graph and the repo the same thing.

Example: adding a `reqs` edge from a component to a techreq rewrites the component's `children.reqs`, bumps its `node_version`, marks it `pending_write`, and adds exactly one `link` row.
