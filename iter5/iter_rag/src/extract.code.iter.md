---
id: 3de51873-30c8-4b01-a285-cd5093ca5e38
name: "Document text extraction"
desc: "Turns an uploaded file into plain text by its extension — a PDF's text layer (flagging scans for OCR), a Word document's paragraphs with headings kept as markdown, a PowerPoint deck as one chapter per slide with speaker notes, an HTML page without scripts, or any UTF-8 text as-is — so every document GraphRAG indexes starts as the same kind of text."
creator: "agent.code"
teststate: inherit
level: component
owner: bespoke
children:
  codedirs:  ["{topdir}/iter_rag/src/extract.rs"]
  codenodes: []
  tests:     []
  reqs:      []
timestamps: {create: "2026-10-02 23:15:00Z", last_modified: "2026-10-02 23:15:00Z", last_tested: ""}
---

# Document text extraction (`iter_rag::extract`)

## Summary

Reads a dropped file and hands back its words, with the chapter headings kept.

## How it works

`extract(filename, bytes)` picks a reader by extension (`ext_of`) and returns `Extracted` (text, `needs_ocr`, page count):

- **pdf**: the text layer via pdf-extract, guarded against parser panics. Fewer than `OCR_CHARS_PER_PAGE` (40) non-blank characters per page means a scan: `needs_ocr` is set and the engine has a model read the pages.
- **docx**: `word/document.xml` paragraphs (`docx_xml_to_text`), `Heading1…` / `Title` styles written as markdown `#` lines, text in drawings kept (`drawing_paragraphs`).
- **pptx**: one `# Slide N: <title>` chapter per slide, text boxes in order, speaker notes under `Notes:`.
- **html / htm**: tags, scripts and styles dropped, h1–h3 kept as `#` lines (`html`).
- anything else that is valid UTF-8 (md, txt, csv, json, yaml, code): as-is.

`tidy` normalises whitespace and blank lines for all of them.

## What goes in and out

iter_data's GraphRAG upload route (`iter_data/src/rag/mod.rs`) uses `ext_of` to refuse unsupported types before storing an upload; the engine's GraphRAG worker (`iter_engine/src/rag.rs`) does the extraction itself, in bulk, before chunking and embedding.

## Example

A 40-slide deck becomes 40 `# Slide N:` chapters with each slide's notes beneath, ready for chunking.
