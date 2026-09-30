---
id: b5a46151-b194-446e-bca8-37e304c1e871
name: "GraphRAG tab"
description: "Shows the selected project's GraphRAG index — uploads with drag and drop, the uploaded and node-file document lists with summary progress, a plain-language search that returns full chunks with their map neighbours, the docs-directory setting and the button that creates the scheduled change sweep — so people manage and query the project's knowledge from the browser."
simple_description: "The screen for adding documents and searching everything the project knows in plain words."
level: component
owner: bespoke
teststate: inherit
children:
  codedirs:   ["{topdir}/webui/rag.js", "{topdir}/webui/rag.css"]
  codenodes:  []
  inputs:     ["{topdir}/interfaces/rag-search/rag-search.interface.iter.md"]
  outputs:    ["{topdir}/interfaces/rag-doc-upload/rag-doc-upload.interface.iter.md"]
  bizreqs:    []
  techreqs:   []
  tests:      []
---

# Long Description

The GraphRAG tab is the fourth tab of the web page (`webui/rag.js`, `webui/rag.css`, mounted by `index.html: showRag` as `IterRag.mount`). It follows the project picker like the other tabs.

The status strip at the top reads `GET …/rag`: how many documents are uploaded and how many are node files, how many chunks are summarised, how many Summary agent jobs wait, and whether a live engine serves the project. GraphRAG summaries need a running engine with an account; without one the strip says documents are searchable by raw text only.

Uploads (`upload`) read each dropped file as base64 and send it to `POST …/rag/docs`. The reply names the chunk and chapter counts and where the engine will commit the original (the docs directory, default `{topdir}/docs/`, editable under Settings). The document list polls every five seconds while the tab is open: summary progress, rollup, ready, and whether the original is in the repository yet. Selecting a document shows its summary, its chapter summaries and every chunk with its own summary, plus Re-summarise and Remove.

Search (`search`) sends the question with optional filters (uploaded or node files, node type, which vector to score by) to `POST …/rag/search`. It shows each hit's full chunk, its summary, chapter and document summaries, and the node's neighbours in the map; a neighbour chip opens the Project graph at that node.

Example: a developer types "who can approve a work item?" and gets the Auth node's chunk on signed approvals, with chips for the HTTP API and the command line.
