---
id: 90efb652-ebd6-461b-98da-1da3827d66a5
name: "Project graph viewer"
description: "Draws a project's architecture map (contexts holding containers holding components, their interface connections and each use case's numbered steps) with several layouts, search and a detail pane, so that a reader can see how the parts fit and follow a user journey through them."
simple_description: "An interactive diagram of the whole project: its parts, how they connect, and the path each user journey takes."
level: component
owner: bespoke
teststate: inherit
children:
  codedirs:   ["{topdir}/webui/graph.js", "{topdir}/webui/graph.css", "{topdir}/webui/vendor/"]
  codenodes:  []
  inputs:     ["{topdir}/interfaces/graph-view/graph-view.interface.iter.md"]
  outputs:    []
  bizreqs:    []
  techreqs:   []
  tests:      []
---

# Long Description

The Project graph viewer draws the map of one project in the Project graph tab.

How it works: `index.html: loadGraph` fetches `GET /api/projects/{p}/graph/view` (the map in drawing shape, prepared by the data server) and calls `IterGraph.draw(el, G, ctx)` in `webui/graph.js`. The viewer, ported from pdy-dev's usecase_map app and restyled dark by `webui/graph.css`, uses Cytoscape and its layout plug-ins, stored offline in `webui/vendor/`. `start` builds the drawing; `render` redraws it whenever a control changes. Five layouts are offered (`runLayout`): Cluster (boxes inside boxes: each context holds its containers, each container its components), Top-down tiered, Inside-out rings, Sequence (one row per use-case step) and Flow (left to right). Every layout is a starting point and nodes stay draggable. Parallel interface edges between two parts are merged into one (`mergeEdges`), while use-case steps each get their own labelled curve (`spreadParallel`). Picking a use case (`setUc`, `onUcChange`) shows its numbered process and data steps. Search (`applySearch`) finds nodes and edges; clicking one fills the detail pane (`nodeDetail`, `edgeDetail`) with descriptions, interfaces and use-case steps; `renderLegend` explains the colours. Hidden nodes and view settings are kept in the URL hash and browser storage.

It reads the map produced by the Map snapshot builder and stored by the data server, and hands each drawing to the Project graph editor, which adds its controls. The URL hash (`#tab=graph&p=<project>`) is shared with the rest of the page.

Why it matters: it is how a person who has never seen the code learns what the system is made of and where a change would land.

Example: a reader picks the use case "Map a repository" and chooses Sequence. The viewer shows one row per step, from the developer typing `iter sync` down to the page drawing the map; clicking a step shows its plain-language text.
