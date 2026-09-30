---
id: 667257a5-2162-4213-ae35-e78284eb2d9b
name: "Map snapshot builder"
description: "Turns the scanned node files into the graph the data server stores (one vertex per file, one typed edge per link, plus descriptions, use-case flowmaps, actors and a content hash), so that the Project graph can be drawn and queried without reading the repo."
simple_description: "Packs the project's structure into a map that the central server can store and draw."
level: component
owner: bespoke
teststate: inherit
children:
  codedirs:   ["{topdir}/iter_local/src/graph.rs"]
  codenodes:  []
  inputs:     []
  outputs:    []
  bizreqs:    []
  techreqs:   []
  tests:      []
---

# Long Description

The Map snapshot builder converts the checkout's node files into a snapshot: the architecture map in the shape the data server stores.

How it works (`iter_local/src/graph.rs`): `snapshot_with` calls the Node-file scanner (`markers::scan`) and makes every node file a vertex, keyed by its frontmatter id. Each vertex carries nodetype, name, description, the plain-language summary, a trimmed `# Long Description` (`long_description`), level, owner, declared and effective test setting, `{topdir}/…` path, code folders, the whole frontmatter as JSON (`yaml_front`, which keeps nested keys such as a use case's `flowmap`) and a hash of the body; the body itself stays in git. Each `children` link becomes an edge typed by the key that declared it (`codenodes`, `inputs`, `outputs`, `tests`, …), and `main.iter.md` gets `root` edges to every root so one traversal reaches everything. Files nothing links are still vertices, flagged orphan; links that could not be resolved are listed. Actors, the people and outside organisations at the edge of the system, are read from the actors file. With `SnapOpts.derive_ids`, a file with no id gets one derived from project and path (`derived_id`), so a checkout can be mapped read-only. The hash covers vertices and edges, and `sync_body` shapes the result for `PUT /api/projects/{p}/graph`.

The Map uploader and test sweep calls it for `iter sync` and for the engine's once-a-minute check, and pushes the result only when the hash changed.

Why it matters: the Project graph, its use-case flows and the test sweep all read this snapshot; if it drops a field, the viewer and the sweep lose it too.

Example: `iter sync` on this repo produces one vertex per node file and one edge per link, including a `codenodes` edge from the webui container to the Work queue page.
