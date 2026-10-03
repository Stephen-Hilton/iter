#!/usr/bin/env bash
# Download the GraphRAG embedding model (sentence-transformers/all-MiniLM-L6-v2,
# Apache-2.0) into iter4/models/all-MiniLM-L6-v2/ — iter_data loads it from
# there (or from --embed-model / $ITER_EMBED_MODEL). ~91 MB; kept out of git.
# Idempotent: files already present with the expected checksum are kept.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
DIR="$ROOT/models/all-MiniLM-L6-v2"
BASE="https://huggingface.co/sentence-transformers/all-MiniLM-L6-v2/resolve/main"
# the weights this build was verified with (2026-09-29)
SHA_MODEL="53aa51172d142c89d9012cce15ae4d6cc0ca6895895114379cacb4fab128d9db"
mkdir -p "$DIR/1_Pooling"
sum() { shasum -a 256 "$1" 2>/dev/null | cut -d' ' -f1 || sha256sum "$1" | cut -d' ' -f1; }
if [ -f "$DIR/model.safetensors" ] && [ "$(sum "$DIR/model.safetensors")" = "$SHA_MODEL" ] \
   && [ -f "$DIR/tokenizer.json" ] && [ -f "$DIR/config.json" ]; then
  echo "[fetch_model] all-MiniLM-L6-v2 already present in $DIR"; exit 0
fi
for f in config.json tokenizer.json model.safetensors special_tokens_map.json tokenizer_config.json \
         modules.json sentence_bert_config.json 1_Pooling/config.json README.md; do
  echo "[fetch_model] $f"
  curl -sfL --retry 3 -o "$DIR/$f.part" "$BASE/$f"
  mv "$DIR/$f.part" "$DIR/$f"
done
got="$(sum "$DIR/model.safetensors")"
if [ "$got" != "$SHA_MODEL" ]; then
  echo "[fetch_model] WARNING: model.safetensors sha256 $got differs from the verified $SHA_MODEL (upstream changed?)" >&2
fi
echo "[fetch_model] done: $DIR"
