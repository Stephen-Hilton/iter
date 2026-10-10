---
id: b16b4b8d-680b-4e2a-a775-b0747a27cb08
name: "Chapters and chunks"
desc: "Splits extracted text into chapters (the sections under the top heading level, or heading-like lines in a PDF) and packs whole paragraphs into chunks that never cross a chapter and always fit the embedding model's 256-token window, measured in the model's own word-pieces, each carrying its heading path."
creator: "agent.code"
teststate: inherit
level: component
owner: bespoke
children:
  codedirs:  ["{topdir}/iter_rag/src/chunk.rs"]
  codenodes: []
  tests:     []
  reqs:      []
timestamps: {create: "2026-10-02 23:15:00Z", last_modified: "2026-10-02 23:15:00Z", last_tested: ""}
---

# Chapters and chunks (`iter_rag::chunk`)

## Summary

Cuts a document into passages small enough to search and big enough to make sense.

## How it works

`chunk_with(text, title, sizer)` returns `(Vec<Chapter>, Vec<Chunk>)`:

- **Chapters** are the sections under the document's top heading level (markdown `#` lines, which docx / html extraction also produce; a lone title drops a level). Text without markdown headings uses heading-like lines ("Chapter 3 …", "2.1 Scope") when there are at least two. Text before the first heading is its own chapter.
- **Chunks** are runs of whole paragraphs (a fenced code block counts as one) up to the size budget; a longer paragraph is split at sentence, then line, then word boundaries. A chunk never spans two chapters and carries its heading path ("Design > Storage").
- A **`Sizer`** measures: `Sizer::tokens` counts the embedding model's word-pieces so the text the model sees — `embed_input`: "<title> — <heading>" plus the chunk — fits its 256-token window whole; `Sizer::chars` (`TARGET` 1000, `MAX` 1500 characters) is the fallback. `split_frontmatter` lets the engine index a node file's body without its YAML frontmatter.

## What goes in and out

`iter_rag::prepare` (lib.rs) runs it with the token sizer before embedding; the engine's GraphRAG worker and iter_data's built-in user guide index use `prepare`.

## Example

A design document with "# Storage" and "# Sync" sections becomes two chapters; a long code block under "Sync" becomes its own chunk with the heading path "Design > Sync".
