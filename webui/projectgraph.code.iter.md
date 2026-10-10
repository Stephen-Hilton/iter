---
id: 90efb652-ebd6-461b-98da-1da3827d66a5
name: "Project graph viewer"
desc: "Draws a project's graph — one node per *.iter.md file (project, context / container / component / connection code nodes, tests, requirements, use cases, actors) and the edges derived from their frontmatter — with node-type filter chips and presets such as the Network map, five layouts, search, use-case step views, a file-sync badge on every node and a detail pane, so a reader can see how the parts fit and where a change would land."
creator: "iter migrate5"
teststate: inherit
level: component
owner: bespoke
children:
  codedirs:  ["{topdir}/webui/graph.js", "{topdir}/webui/graph.css", "{topdir}/webui/vendor/"]
  codenodes: []
  tests:     []
  reqs:      []
timestamps: {create: "2026-10-02 23:05:35Z", last_modified: "2026-10-02 23:15:00Z", last_tested: ""}
---

# Project graph viewer

## Summary

An interactive diagram of the whole project, drawn straight from its node files: its parts, how they connect, what tests and requirements hang off them, and the path each use case takes.

## How it works

`index.html` fetches `GET /api/projects/{p}/graph` (every node and every derived edge: codenodes, tests, reqs, supplies, connects, drives, touches, uses) and calls `IterGraph.draw` in `webui/graph.js`, which draws it with Cytoscape and its layout plug-ins vendored in `webui/vendor/` and styled dark by `webui/graph.css`. One table holds the visual encoding (shape and colour per node type and level, connection nodes as small diamonds with `supplies` edges in and `connects` edges out); the stylesheet, the chips and the legend all read it.

- **Filters**: a chip per node type / level (project, context, container, component, connection, test, bizreq, techreq, philosophy, usecase, actor) shows or hides it (Alt+click: only that type); presets such as **Network map** (code and connections only, left to right). The state lives in the URL hash.
- **Layouts**: Cluster (fcose over compound nodes — a code node sits inside the code node that lists it in `children.codenodes`), Top-down tiered, Inside-out rings, Sequence (one row per use-case step) and Flow (dagre, left to right). Nodes stay draggable; in Cluster a dragged node pulls its neighbours along and the graph settles around where it is dropped (`IterKit.springDrag`), and no two nodes are left overlapping (`IterKit.separate`); P pins the selected node in place (per project, layout and use case; not in Sequence). Every edge is a straight line. `update()` redraws after an edit without moving anything.
- **Use cases**: picking one shows its flowmap's numbered process and data steps.
- **Search and detail**: search finds nodes and edges; clicking one fills the detail pane with its frontmatter, body, links, a test node's last result and log, and a **file-sync badge** (synced, ⟳ pending sync, pending delete, designed).

Editing is not here: the viewer fires a `g-detail` event and exposes `IterGraph.api`, which the Project graph editor uses to add its controls.

## What goes in and out

Reads the graph the data server keeps in step with the repository's node files (engine filescan → conform → sync). Shares the URL hash with the rest of the page.

## Why it matters

It is how a person who has never seen the code learns what the system is made of, and where a change would land, without opening a file.

## Example

A reader presses **Network map**: only code and connection nodes remain, laid out left to right, so "iter_engine supplies *API call*, which connects to iter_data" reads as one arrow chain.
