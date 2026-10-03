---
id: 1e53b2d3-8f1f-46e5-a446-6babaa910480
name: "Embedding model"
desc: "Runs the sentence-transformers all-MiniLM-L6-v2 model in-process on the CPU with candle — 384 numbers per text, mean-pooled and normalised — finds or downloads its weights, counts its word-pieces for chunk sizing, and stamps every vector with the model name and weights checksum, so the engine's chunk vectors and the server's question vectors always come from the same model."
creator: "agent.code"
teststate: inherit
level: component
owner: bespoke
children:
  codedirs:  ["{topdir}/iter_rag/src/embed.rs"]
  codenodes: []
  tests:     []
  reqs:      []
timestamps: {create: "2026-10-02 23:15:00Z", last_modified: "2026-10-02 23:15:00Z", last_tested: ""}
---

# Embedding model (`iter_rag::embed`)

## Summary

Turns a passage or a question into a list of 384 numbers so that similar meanings land close together.

## How it works

`Embedder::load(dir)` loads the BERT weights and tokenizer (6 layers, `DIM` 384); `embed(texts)` mean-pools over the attention mask and L2-normalises, so cosine similarity (`cosine`) is a dot product; `count_tokens` counts word-pieces without truncation for chunk sizing. Input past `MAX_TOKENS` (256) is cut off by the model itself, which is why chunks are sized in tokens.

The model directory is found once, lazily: `set_model_dir` (iter_data's `--embed-model` flag) or `$ITER_EMBED_MODEL`, else `models/all-MiniLM-L6-v2` beside the working directory or binary, else `/opt/iter/models/…` (the container), else `~/.cache/iter/models/…`. `ensure_model` / `get_or_download` downloads the weights into that cache when none is found and checks their sha256 (`MODEL_SHA256`). Every vector carries the **stamp** (`stamp_of`: name plus the first 12 hex of the weights' checksum); `status` reports it.

## What goes in and out

The engine embeds in bulk (chunks and summaries) through `get_or_download`; iter_data embeds only search questions (and its built-in guide) through `get`, so search never waits on an engine.

## Why it matters

A question embedded by different weights than the chunks would score garbage; the stamp makes such a mismatch visible instead of silently returning bad hits.
