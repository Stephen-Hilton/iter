---
id: db5b28c3-28ba-4495-8795-b8b32b6f1b22
name: "GraphRAG engine worker"
description: "Does GraphRAG's heavy work on the engine's own hardware — extracting uploaded files (OCR of scanned PDFs through Claude), chunking and embedding them and every changed node file, and running the Summary agent, a short tool-less Haiku session per batch whose summaries it embeds — whenever the heartbeat says work is waiting or the map changes, so every document is indexed and summarised without a work item or a queue."
simple_description: "Keeps the searchable copy of the project's files up to date and writes short summaries of every passage."
level: component
owner: bespoke
teststate: inherit
children:
  codedirs:   ["{topdir}/iter_engine/src/rag.rs"]
  codenodes:  []
  inputs:     ["{topdir}/interfaces/rag-work-claim/rag-work-claim.interface.iter.md"]
  outputs:    ["{topdir}/interfaces/rag-nodes-sync/rag-nodes-sync.interface.iter.md"]
  bizreqs:    []
  techreqs:   []
  tests:      []
---

# Long Description

The GraphRAG engine worker is the engine half of GraphRAG (`iter_engine/src/rag.rs`). It owns the work that needs a checkout, a Claude account or real CPU, using the shared iter_rag library.

The change sweep, `iter rag sync` (`sync`), reads the stored map's vertices and each node file from the checkout, and for files whose hash (text plus `INDEX_VERSION`) differs from the index, builds the node's text (`node_text`), chunks and embeds it (`prepare`), and sends chunks with vectors (`PUT …/rag/nodes`, 25 files per call). The last call lists every current path so deleted files drop out. The engine loop runs it after every map push that changed a file (`Engine::sync_maps`); the scheduled "GraphRAG change sweep" item and a person can run it too.

The worker: when the heartbeat reply's `rag_waiting` names a served project, `Engine::start_summaries` starts up to the `summary` agent record's `max` workers (default 2), outside the agent cap. Each claims jobs (`summarize_waiting`): **ingest** (`ingest`: decode, `extract`, OCR a scan with `ocr_pdf` — `pdftoppm` pages read by a Claude session limited to the Read tool — then `prepare` and PUT), **chunks** and **rollup** (one `claude -p` session with no tools, Haiku unless the agent record says otherwise; `parse_json_answer` salvages every valid summary from a broken answer; each summary is embedded here before it is reported). A holding engine does none of it.

Example: a scanned one-page runbook is uploaded; within ten seconds the worker has OCR'd it, chunked it into three chunks, embedded them, and the page is searchable by its exact code word.
