---
id: db5b28c3-28ba-4495-8795-b8b32b6f1b22
name: "GraphRAG engine worker"
desc: "Does GraphRAG's heavy work on the engine's own hardware — extracting uploaded files (OCR of scanned PDFs through a model session), chunking and embedding them and every changed node file, and running the Summary agent, a short tool-less Haiku session per batch whose summaries it embeds — whenever the heartbeat says work is waiting or a file-sync round changed node files, so every document is indexed and summarised without a work item or a queue."
creator: "iter migrate5"
teststate: inherit
level: component
owner: bespoke
children:
  codedirs:  ["{topdir}/iter_engine/src/rag.rs"]
  codenodes: []
  tests:     []
  reqs:      []
timestamps: {create: "2026-10-02 23:05:35Z", last_modified: "2026-10-02 23:15:33Z", last_tested: ""}
---

# Long Description

## Summary

Keeps the searchable copy of the project's files up to date and writes short summaries of every passage.

The GraphRAG engine worker is the engine half of GraphRAG (`iter_engine/src/rag.rs`). It owns the work that needs a checkout, a model account or real CPU, using the shared iter_rag library.

The change sweep, `iter rag sync` (`sync`), reads the project graph's nodes (`GET /api/projects/{p}/graph`, filtered by the project's `rag/settings` node types) and each node file from the checkout, and for files whose hash (text plus `INDEX_VERSION`) differs from the index, builds the node's text (`node_text`), chunks and embeds it (`prepare`), and sends chunks with vectors (`PUT …/rag/nodes`, 25 files per call). The last call lists every current path so deleted files drop out. Node-file sync runs it on its own thread after a sync round that changed something (`filesync::maybe_rag`, at most once a minute); the scheduled "GraphRAG change sweep" item and a person can run it too.

The worker: when the heartbeat reply's `rag_waiting` names a served project, `Engine::start_summaries` starts up to the `summary` agent record's `max` workers (default 2), outside the agent cap. Each claims jobs (`summarize_waiting`): **ingest** (`ingest`: decode, `extract`, OCR a scan with `ocr_pdf` — `pdftoppm` pages read by a model session limited to the Read tool — then `prepare` and PUT), **chunks** and **rollup** (one tool-less session through the provider dispatch, Haiku unless the agent record says otherwise; `parse_json_answer` salvages every valid summary from a broken answer; each summary is embedded here before it is reported). A holding engine does none of it.

Example: a scanned one-page runbook is uploaded; within ten seconds the worker has OCR'd it, chunked it into three chunks, embedded them, and the page is searchable by its exact code word.
