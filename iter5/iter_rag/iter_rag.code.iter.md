---
id: b24af7a3-5786-4a11-9e66-b251120b8e22
name: "iter_rag — document text and embeddings"
desc: "Turns files into text (pdf, docx, pptx, html, markdown, plain text; flags scanned PDFs for OCR), splits text into chapters and chunks sized in the embedding model's own tokens, and runs the all-MiniLM-L6-v2 embedding model, so the engine and the data server index and search documents with exactly the same code and model."
creator: "iter migrate5"
teststate: inherit
level: container
owner: bespoke
children:
  codedirs:  ["{thisfiledir}/"]
  codenodes: ["{topdir}/iter_rag/src/extract.code.iter.md", "{topdir}/iter_rag/src/chunk.code.iter.md", "{topdir}/iter_rag/src/embed.code.iter.md"]
  tests:     ["{thisfiledir}/test/*.test.iter.md"]
  reqs:      []
timestamps: {create: "2026-10-02 23:05:35Z", last_modified: "2026-10-02 23:15:00Z", last_tested: ""}
---

# iter_rag — document text and embeddings

## Summary

The shared toolkit that reads documents, cuts them into passages and turns passages into searchable fingerprints.

iter_rag is the library both halves of GraphRAG are built with; it is unchanged from iter4 apart from living in the iter5 workspace. It does three things and holds no state:

- **Extraction** (`src/extract.rs`, component *Document text extraction*): file → text by extension; a PDF with almost no text layer is flagged `needs_ocr` for the engine to have a model read.
- **Chunking** (`src/chunk.rs`, component *Chapters and chunks*): text → chapters and chunks of whole paragraphs, sized in the model's own word-pieces so a chunk plus its "title — heading" prefix always fits the 256-token window.
- **Embedding** (`src/embed.rs`, component *Embedding model*): all-MiniLM-L6-v2 in-process with candle on the CPU, 384 numbers per text, every vector stamped with the model name and weights checksum; the weights are found locally or downloaded into `~/.cache/iter/models`.

`prepare` (`src/lib.rs`) does chunk plus embed in one call, in the shape iter_data stores.

## What goes in and out

The engine's GraphRAG worker (`iter_engine/src/rag.rs`) uses all of it for uploads and node files (`iter rag sync`); the data server (`iter_data/src/rag/`) uses the embedder for search questions and `prepare` for its built-in user guide. It calls no service except the one-time model download.

## Example

A 40-slide deck becomes 40 chapters and about 60 chunks, each with a vector, in a few seconds on a laptop; a search question embedded by the server lands next to the right slide because both sides ran the same stamped model.

Tested by `test/iter_rag.test.iter.md` (`cargo test -p iter_rag`).
