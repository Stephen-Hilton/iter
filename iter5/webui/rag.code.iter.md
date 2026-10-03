---
id: b5a46151-b194-446e-bca8-37e304c1e871
name: "GraphRAG tab"
desc: "Shows the selected project's GraphRAG index — uploads with drag and drop, the uploaded and node-file document lists with summary progress, a plain-language search that returns full chunks with their graph neighbours, the docs-directory setting and the button that creates the scheduled change sweep — so people manage and query the project's knowledge from the browser."
creator: "iter migrate5"
teststate: inherit
level: component
owner: bespoke
children:
  codedirs:  ["{topdir}/webui/rag.js", "{topdir}/webui/rag.css"]
  codenodes: []
  tests:     []
  reqs:      []
timestamps: {create: "2026-10-02 23:05:35Z", last_modified: "2026-10-02 23:15:00Z", last_tested: ""}
---

# GraphRAG tab

## Summary

The screen for adding documents and searching everything the project knows in plain words.

## How it works (`webui/rag.js`, `webui/rag.css`)

Mounted by `index.html` as `IterRag.mount`; `show` / `hide` start and stop its polling. It follows the project picker like the other tabs.

The status strip reads `GET …/rag`: how many documents are uploaded and how many are node files, how many chunks are summarised (and failed), how many Summary agent jobs wait, chunks embedded by a different model than the one answering searches, and whether a live engine serves the project. Summaries need a running engine with an account; without one, documents are searchable by raw text only.

Uploads read each dropped file (pdf — scans are OCR'd —, docx, pptx, html, md, txt and other text) as base64 and send it to `POST …/rag/docs`; an engine extracts, chunks and embeds it within seconds, the Summary agent adds summaries, and the engine commits the original into the docs directory (default `{topdir}/docs/`, editable here). The document list polls every five seconds while the tab is open; selecting a document shows its summary, chapter summaries and every chunk with its own summary, plus Re-summarise and Remove.

Search sends the question with optional filters (uploaded or node files, node type, which vector to score by) to `POST …/rag/search` and shows each hit's full chunk, its summary, chapter and document summaries, and the node's neighbours in the project graph; a neighbour chip opens the Project graph at that node.

## Example

A developer types "who can approve a work item?" and gets the auth node's chunk on signed approvals, with chips for the nodes linked to it.
