---
id: 2bb2b822-5b1a-4d0c-8b1c-0b9248dc2238
name: "GraphRAG index"
description: "Stores a project's GraphRAG index — uploaded documents and *.iter.md node files as chunks with a raw-text and a summary vector each — hands engines the ingest and Summary agent jobs that fill it, and answers plain-language searches by fusing keyword (BM25) and meaning rankings, returning full chunks with their summaries and their place in the map, so people and agents find the right passage without reading files blind."
simple_description: "Lets anyone ask a question in plain words and get back the passages of the project's documents that answer it."
level: component
owner: bespoke
teststate: inherit
children:
  codedirs:   ["{topdir}/iter_data/src/rag/"]
  codenodes:  []
  inputs:     ["{topdir}/interfaces/rag-doc-upload/rag-doc-upload.interface.iter.md", "{topdir}/interfaces/rag-nodes-sync/rag-nodes-sync.interface.iter.md"]
  outputs:    ["{topdir}/interfaces/rag-search/rag-search.interface.iter.md", "{topdir}/interfaces/rag-work-claim/rag-work-claim.interface.iter.md"]
  bizreqs:    []
  techreqs:   []
  tests:      []
---

# Long Description

The GraphRAG index keeps a project's knowledge searchable. It holds **file** documents a person uploads (pdf, docx, pptx, html, markdown, text), **node** documents (the `*.iter.md` files of the map) and the built-in iter4 user guide (`guide.rs`, scope `_iter`, included in every project's search).

The heavy work happens on engines (decided 2026-09-29). An upload is kept as bytes (`rag_blob`) with state `queued`; an engine claims an **ingest** job (`pipeline.rs: work_claim`), extracts and chunks the text in the model's tokens, embeds each chunk, and sends it back (`doc_chunks_put`). Node files arrive the same way from `iter rag sync` (`nodes_put`). `store_prepared` keeps the summaries of chunks whose text did not change. Engines then claim **chunks** jobs for the Summary agent and **rollup** jobs for chapter and document summaries, and report each summary with its vector (`work_done`).

Search (`search.rs: run_search`) embeds the question here (the only embedding iter_data does) and ranks the chunks three ways: ArangoSearch BM25 over text, summary, heading and title (`keyword_ranking`), and cosine similarity to the raw-text and summary vectors, merged into one meaning ranking (`merge_by_best`). Reciprocal rank fusion (`fuse`) combines meaning and keyword, and at most two chunks of one document are kept. Each hit carries the full chunk, its chapter and document summaries, and its map context: a node's neighbours and linked documents (`children.documents`), or the nodes that link an uploaded document.

The HTTP API, the MCP gateway and the GraphRAG tab all call it. Example: "what is the code word for the password exercise" finds a scanned runbook's OCR'd page first, because the keyword ranking catches "code word" where meaning alone chose a login component.
