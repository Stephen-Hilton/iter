---
id: b24af7a3-5786-4a11-9e66-b251120b8e22
name: "iter_rag — document text and embeddings"
description: "Turns files into text (pdf, docx, pptx, html, markdown, plain text; flags scanned PDFs for OCR), splits text into chapters and chunks sized in the embedding model's own tokens, and runs the all-MiniLM-L6-v2 embedding model, so the engine and the data server index and search documents with exactly the same code and model."
simple_description: "The shared toolkit that reads documents, cuts them into passages and turns passages into searchable fingerprints."
level: container
owner: bespoke
teststate: inherit
children:
  codedirs:   ["{thisfiledir}/"]
  codenodes:  []
  inputs:     []
  outputs:    []
  bizreqs:    []
  techreqs:   []
  tests:      []
---

# Long Description

iter_rag is the library both halves of GraphRAG are built with. It does three things and holds no state.

Extraction (`iter_rag/src/extract.rs: extract`) reads an uploaded file by its extension: a PDF's text layer (pdf-extract, guarded against parser panics), a Word document's paragraphs with its headings kept as markdown `#` lines, a PowerPoint deck as one `# Slide N: <title>` chapter per slide with its speaker notes, an HTML page without its scripts, or any UTF-8 text. A PDF with fewer than 40 characters per page is a scan: `needs_ocr` is set and the engine has Claude read its pages.

Chunking (`chunk.rs: chunk_with`) makes chapters from the sections under the top heading level (a lone title drops a level; heading-like lines stand in for PDFs) and packs whole paragraphs into chunks. A `Sizer` measures them: the engine counts the model's own word-pieces (`Sizer::tokens`), so a chunk plus its "title — heading" prefix (`embed_input`) always fits the model's 256-token window and nothing is cut off.

Embedding (`embed.rs: Embedder`) runs sentence-transformers all-MiniLM-L6-v2 in-process with candle on the CPU: 384 numbers per text, mean-pooled and normalised. Every vector carries the model's stamp (name plus the weights' checksum). `ensure_model` downloads the weights into `~/.cache/iter/models` when a machine has none; `prepare` does chunk plus embed in one call.

The engine calls all of it for uploads and node files; the data server calls only the embedder, for search questions and the built-in user guide. Example: a 40-slide deck becomes 40 chapters and about 60 chunks, each with a vector, in a few seconds on a laptop.
