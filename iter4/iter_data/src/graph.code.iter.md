---
id: 721bbfd0-b65d-44a9-b34f-daeb3f047340
name: "Architecture map store"
description: "Stores each project's program map — one vertex per *.iter.md file, one edge per children link — replaces it when an engine pushes a new snapshot, and answers questions about it: neighbours, owners of a path, lookups, stats, test results, the viewer's drawing and graph edits."
simple_description: "Keeps the map of how the program's parts fit together and answers questions about it."
level: component
owner: bespoke
teststate: inherit
children:
  codedirs:   ["{topdir}/iter_data/src/graph.rs", "{topdir}/iter_data/src/graph_view.rs"]
  codenodes:  []
  inputs:     []
  outputs:    ["{topdir}/interfaces/graph-sync/graph-sync.interface.iter.md", "{topdir}/interfaces/graph-neighbors/graph-neighbors.interface.iter.md", "{topdir}/interfaces/graph-testresult/graph-testresult.interface.iter.md", "{topdir}/interfaces/graph-view/graph-view.interface.iter.md", "{topdir}/interfaces/graph-edits/graph-edits.interface.iter.md"]
  bizreqs:    []
  techreqs:   []
  tests:      []
---

# Long Description

The Architecture map store keeps the picture of the program that engines build from the `*.iter.md` files, and answers questions the folder tree alone cannot.

How it works: `iter_data/src/graph.rs: routes` adds the `/graph` routes. `graph_put` receives an engine's snapshot (vertices, edges, a content hash); if the hash matches the stored one it answers `unchanged`, otherwise `replace` computes added, changed and removed counts, drops edges whose ends are unknown, and rewrites the project's vertices and edges — on ArangoDB inside one stream transaction, elsewhere as rows in `graph_node` and `graph_link`. A vertex's stored test result survives the rewrite. `node_neighbors` walks N links out, in or both ways (an AQL traversal on ArangoDB, a breadth-first walk in Rust elsewhere). `graph_owner` returns the code nodes whose folders contain a path, deepest first. `graph_lookup` finds a vertex by path or by type and name, which `iter ids --fix` uses to reuse an id. `node_testresult` records green, red, error or skipped on a test-group vertex. `graph_view` calls `iter_data/src/graph_view.rs: build`, which reshapes the map for the web viewer: contexts, containers and components with their parents, ownership edges, interface connections (a node that inputs interface I is linked to the node that outputs I), actors and use-case steps, honouring the project's `graph.hide` and `graph.parents` settings. `graph_edit` accepts a web-page edit as a pending datasync row, or queues a `test` work item for `run_tests`.

What goes in and out: iter_engine pushes snapshots and test results; the webui reads the view and posts edits; the `iter` command line uses lookup. It provides graph-sync, graph-neighbors, graph-testresult, graph-view and graph-edits.

Why it matters: without it there is no Project graph and no test sweep.

Example: `GET …/graph/owner?path={topdir}/iter_data/src/api.rs` returns the HTTP API component, then iter_data, then the Data context.
