---
id: c7766582-9607-4806-bbfe-37a024468c6f
name: "Map a repository"
description: "A developer types `iter sync` in a checkout and gets a stored, drawable map of the project's parts and how they link, which the Project graph tab then shows."
teststate: inherit
children:
  codenodes:  ["{topdir}/iter_engine/src/mapsync.code.iter.md", "{topdir}/iter_local/src/snapshot.code.iter.md", "{topdir}/iter_data/src/graph.code.iter.md"]
  tests:      []
flowmap:
  summary: "A developer types `iter sync` in a checkout. The map uploader first makes sure every node file has a permanent id, then the snapshot builder reads the files and turns them into a map of parts and links. The whole map goes to the data server in one request, which replaces the project's stored map in one ArangoDB transaction; the Project graph tab then draws it."
  sequence:
  - actor:developer
  - '{topdir}/iter_engine/src/cli.code.iter.md'
  - '{topdir}/iter_engine/src/mapsync.code.iter.md'
  - '{topdir}/iter_local/src/ids.code.iter.md'
  - '{topdir}/iter_local/src/scan.code.iter.md'
  - '{topdir}/iter_local/src/snapshot.code.iter.md'
  - '{topdir}/iter_data/src/graph.code.iter.md'
  - '{topdir}/iter_data/src/arango.code.iter.md'
  - '{topdir}/webui/projectgraph.code.iter.md'
  process_flow:
  - step: 1
    from: actor:developer
    to: '{topdir}/iter_engine/src/cli.code.iter.md'
    what: "The developer types `iter sync` in a terminal inside the checkout."
    plain: "The developer asks iter to map the checkout."
    evidence: "iter_engine/src/cli.rs: Verb::Sync"
  - step: 2
    from: '{topdir}/iter_engine/src/cli.code.iter.md'
    to: '{topdir}/iter_engine/src/mapsync.code.iter.md'
    what: "The command line hands the request to the map uploader, which works out which checkout to map and which data server and token to use (a flag first, then the engine's environment, then `.iter/config.json`)."
    plain: "iter finds the project and the server."
    evidence: "iter_engine/src/sync.rs: checkout_root, conn, sync_verb"
  - step: 3
    from: '{topdir}/iter_engine/src/mapsync.code.iter.md'
    to: '{topdir}/iter_local/src/ids.code.iter.md'
    what: "The map uploader asks the id stamper to give every node file without an `id:` one, reusing the id the stored map already knows for that file so its history stays attached."
    plain: "Every file gets a permanent id."
    evidence: "iter_engine/src/sync.rs: fix_ids → iter_local/src/ids.rs: fix"
  - step: 4
    from: '{topdir}/iter_engine/src/mapsync.code.iter.md'
    to: '{topdir}/iter_local/src/snapshot.code.iter.md'
    what: "The map uploader hands the checkout to the snapshot builder, which turns every node file into a vertex and every `children` link into a typed edge, and hashes the result."
    plain: "The files become a map."
    evidence: "iter_local/src/graph.rs: snapshot_with"
  - step: 5
    from: '{topdir}/iter_local/src/snapshot.code.iter.md'
    to: '{topdir}/iter_local/src/scan.code.iter.md'
    what: "The snapshot builder asks the node-file scanner to find every `*.iter.md` file, expand its path placeholders and follow its `children` links into one tree."
    plain: "The links between files are followed."
    evidence: "iter_local/src/markers.rs: scan"
  - step: 6
    from: '{topdir}/iter_engine/src/mapsync.code.iter.md'
    to: '{topdir}/iter_data/src/graph.code.iter.md'
    what: "The map uploader sends the whole snapshot to the data server in one request, asking it to replace this project's stored map."
    plain: "The map is sent to the server."
    evidence: "PUT /api/projects/{p}/graph"
  - step: 7
    from: '{topdir}/iter_data/src/graph.code.iter.md'
    to: '{topdir}/iter_data/src/arango.code.iter.md'
    what: "The data server writes the new vertices and edges and deletes the ones that disappeared, all inside one ArangoDB transaction, so no reader ever sees half a map."
    plain: "The server stores it in one step."
    evidence: "iter_data/src/arango.rs: stream transaction"
  - step: 8
    from: '{topdir}/webui/projectgraph.code.iter.md'
    to: '{topdir}/iter_data/src/graph.code.iter.md'
    what: "When someone opens the Project graph tab, the page asks the data server for the map in drawing shape, and the viewer draws it."
    plain: "The page draws the map."
    evidence: "GET /api/projects/{p}/graph/view → webui/graph.js: IterGraph.draw"
  data_flow:
  - step: 1
    from: '{topdir}/iter_engine/src/mapsync.code.iter.md'
    to: '{topdir}/iter_data/src/graph.code.iter.md'
    data: "The whole map in one request: vertices (id, nodetype, name, path, descriptions, use-case flowmaps), edges (from, kind, to), actors and a content hash."
    plain: "The whole map travels in one request."
  - step: 2
    from: '{topdir}/iter_data/src/graph.code.iter.md'
    to: '{topdir}/iter_data/src/arango.code.iter.md'
    data: "Node and link documents that make up the project's part of the `iter_map` graph; they rest in ArangoDB's `node` and `link` collections."
    stored: true
    plain: "The map rests in the database."
---

# Map a repository

A developer wants a picture of how their project is put together. They type `iter sync` in a terminal inside the checkout.

iter first finds the checkout and the data server it belongs to, then makes sure every node file (every `*.<type>.iter.md` file describing a part, interface or use case) carries a permanent id. A file that lost its id gets its old one back from the stored map, so its history stays attached. The snapshot builder then reads every node file and follows the links between them, producing one vertex per file and one typed edge per link, with a hash of the whole.

The whole map goes to the data server in one request. The server replaces the project's stored map in a single database transaction, adding new parts, updating changed ones and removing parts that disappeared. Opening the Project graph tab then draws the map. A running engine repeats this on its own, at most once a minute, whenever the files change.
