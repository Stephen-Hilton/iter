---
id: 2bb2b822-5b1a-4d0c-8b1c-0b9248dc2238
name: "GraphRAG index"
desc: "Stores a project's GraphRAG index — uploaded documents, the project's *.iter.md node files and the built-in user guide, as chunks with a raw-text and a summary vector each — hands engines the ingest, summary and rollup jobs that fill it, and answers plain-language searches by fusing keyword (BM25) and meaning rankings, returning full chunks with their summaries and their place in the project graph, so people and agents find the right passage without reading files blind."
creator: "iter migrate5"
teststate: inherit
level: component
owner: bespoke
children:
  codedirs:  ["{topdir}/iter_data/src/rag/"]
  codenodes: []
  tests:     []
  reqs:      []
timestamps: {create: "2026-10-02 23:05:35Z", last_modified: "2026-10-02 23:10:16Z", last_tested: ""}
---

# Long Description

## Summary

Lets anyone ask a question in plain words and get back the passages of the project's documents that answer it.

The GraphRAG index (`iter_data/src/rag/`) keeps a project's knowledge searchable. It holds **file** documents a person uploads (pdf, docx, pptx, html, markdown, text and more, up to 25 MB), **node** documents (the project's `*.iter.md` files) and the built-in user guide (`guide.rs`: `docs/iter4_guide.md` compiled in, scope `_iter`, included in every project's search unless `include_guide: false`). Collections: `rag_doc`, `rag_chunk`, `rag_blob`, `rag_setting`, ArangoSearch view `rag_view`.

How it works: the heavy work happens on engines. An upload is kept as bytes (`rag_blob`) with state `queued`, and a `store_doc` op is handed to the GraphRAG file inbox so an engine also commits the original under the docs directory (setting `docs_dir`, default `{topdir}/docs/`). When the heartbeat's `rag_waiting` says so, an engine claims jobs (`pipeline.rs: work_claim`): **ingest** (extract, chunk in the model's tokens, embed, `PUT …/rag/docs/{id}/chunks`), **chunks** (the Summary agent summarises a batch, each summary embedded as the chunk's second vector) and **rollup** (chapter and document summaries), reporting with `work_done`. Node files arrive pre-chunked through `PUT …/rag/nodes` after the engine compares `…/rag/nodes/hashes`; `store_prepared` keeps the summaries of chunks whose text did not change. Search (`search.rs: search`) embeds the question here with the shared `iter_rag` model (all-MiniLM-L6-v2, the only embedding iter_data does) and ranks chunks by ArangoSearch BM25 over text, summary, heading and title, and by cosine similarity to both vectors (merged into one meaning ranking); reciprocal rank fusion combines them, keeping at most two chunks per document. Each hit carries the full chunk, its chapter and document summaries, and its graph context: a node's neighbours and linked documents (`children.documents`), or the nodes that link an uploaded document. `mod.rs` also serves status, settings, document list / get / delete / resummarize / link and the change-sweep schedule.

What goes in and out: the webui's GraphRAG tab, the MCP gateway's `rag_*` tools and iter_engine's GraphRAG worker call it; it reads the Project graph store for context.

Example: "what is the code word for the password exercise" finds a scanned runbook's OCR'd page first, because the keyword ranking catches "code word" where meaning alone chose a login component.
