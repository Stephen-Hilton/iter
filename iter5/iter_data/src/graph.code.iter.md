---
id: 721bbfd0-b65d-44a9-b34f-daeb3f047340
name: "Project graph API"
desc: "Serves the project graph routes under /api/projects/{p}/graph: reads (whole graph, stats, the view the Project graph tab draws, lookup by path or name, owner of a path, sync conflicts, a use case's parts, one node, neighbours) and direct edits (create a node with its file path planned by the folder rules and attached to a parent, patch, delete with a reason, move, add / remove / move an edge). Every edit is conformed, visible at once, and becomes a pending file write for an engine."
creator: "iter migrate5"
teststate: inherit
level: component
owner: bespoke
children:
  codedirs:  ["{topdir}/iter_data/src/graph.rs", "{topdir}/iter_data/src/graph_view.rs", "{topdir}/iter_data/src/graph_tests.rs"]
  codenodes: []
  tests:     []
  reqs:      []
timestamps: {create: "2026-10-02 23:05:35Z", last_modified: "2026-10-02 23:10:16Z", last_tested: ""}
---

# Long Description

## Summary

Lets people and agents look at the project's design and change it directly in the graph.

The project graph API (`iter_data/src/graph.rs`) is the HTTP face of the Project graph store: the webui's Project graph tab, the graph editor and the MCP `graph_*` tools all go through it.

How it works: `routes` serves the reads — `GET graph` (live nodes, derived edges, stats), `graph/stats`, `graph/view`, `graph/lookup?path=|name=&nodetype=`, `graph/owner?path=` (the code nodes whose codedirs contain a path, deepest first), `graph/conflicts`, `graph/usecases/{id}`, `graph/nodes/{id}` and `graph/nodes/{id}/neighbors?depth&direction` — and the edits: `POST graph/nodes` (`node_create`: checks the level, plans the file path with the designer folder rules, attaches it to `attach_to` with the inferred or given edge kind), `PATCH graph/nodes/{id}` (name, desc, body, teststate, level, front, children, with optional `expect_version`), `DELETE graph/nodes/{id}` (needs a reason), `POST graph/nodes/{id}/move`, `POST`/`DELETE graph/edges` and `POST graph/edges/move` (drag an endpoint). Each edit loads the project's `Graph`, changes it, saves it, and returns the node plus every node it touched. `graph_view.rs: build` reshapes the graph for the viewer: each node with its `level`, `parent` and owner chain, `file_state`, test result and use cases; every derived edge by kind plus `flow` edges from use-case flowmaps; and per use case its parts, actors and steps. The project record's `graph.hide` still hides folders from the picture. `graph_tests.rs` drives these routes end to end against a throwaway ArangoDB database.

What goes in and out: callers are the webui and the MCP gateway; it works through the Project graph store and merges the Test results and Node file sync route groups into its router.

Why it matters: without it there is no Project graph and no designer.

Example: `POST …/graph/nodes {nodetype: code, level: container, name: API, attach_to: <context id>}` creates `{topdir}/src/<context>/api/api.code.iter.md` in the graph at once; the next engine tick writes and commits the file.
