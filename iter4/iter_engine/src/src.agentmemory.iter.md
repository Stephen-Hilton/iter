# Agent memory: iter4/iter_engine/src
The iter4 engine: the loop that claims work items, runs headless Claude agents, keeps locks and heartbeats, and runs the GraphRAG workers (document search: ingest, OCR, summaries).

## Where things live
- `engine.rs` / `work.rs`: the tick loop, and `spawn_claude` + `parse_claude_stream` (every Claude session goes through these).
- `rag.rs`: GraphRAG node sync and workers; `ingest()` and `ocr_pdf()` handle uploaded files.
- `gate.rs`: the close gate that checks the final message against the request. `dedup.rs`: duplicate-item judge.
- `prompt.rs`: builds agent prompts. `client.rs`: HTTP to iter_data. `envstore.rs` / `usage.rs`: accounts and usage%.
- `*.code.iter.md` beside each file: the node docs, so read them first.
- Text extraction for uploads lives outside this codepath, in `../../iter_rag/src/extract.rs`.

## Build and test
- From `iter4/`: `cargo build --release -p iter_engine` (workspace: iter_core, iter_data, iter_engine, iter_local, iter_rag).
- Tests: `"$ITER_BIN" runtests --project "$ITER_PROJECT" --group "iter_engine-unit"` (runs `cargo test -p iter_engine`; 71/71 passing on 2026-09-29). Registry and script are in `../test/`, outside this write fence, so read them but do not edit them.

## Gotchas
- Scanned PDF: a PDF with fewer than 40 text characters per page (`OCR_CHARS_PER_PAGE`) is sent to OCR. Pages are rendered with poppler `pdftoppm` (must be installed on the engine host), 6 pages per Claude session, and only the first 120 pages are read (`OCR_MAX_PAGES`).
- A holding engine (no account under its stop%) runs no GraphRAG work.
- iter4 is ArangoDB-only. Never add SQLite back.

## Recent changes
- 2026-09-29 fa41ef — answer-only item: documented the scanned-PDF OCR path in a work-item note; no code changed.
