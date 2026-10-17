#!/usr/bin/env bash
# Install iter_data as one Docker container (ArangoDB CE 3.12 + iter_data):
# API + webui on :8400, the Arango web console on 127.0.0.1:8630.
#
#   ./install.sh [options]          build the image here and (re)start the container
#   curl -fsSL https://raw.githubusercontent.com/Stephen-Hilton/iter/main/installers/iter_data/docker/install.sh | bash -s -- --version vYYYY.MM.DD
#
# Options:
#   --version TAG        release to download the binary (and, when piped, these files) from [latest]
#   --binary PATH        use this iter_data linux binary instead of downloading one
#   --env-file PATH      take ITER_ADMIN_PASSWORD, ITER_JWT_SECRET (and ARANGO_ROOT_PASSWORD) from it
#   --port N             host port for the API + webui [8400; ITER_PORT]
#   --arango-port N      host port for the Arango console on 127.0.0.1, 0 = not published [8630]
#   --data-dir DIR       host folder for everything that persists [~/.iter/iter_data]
#   --name NAME          container, compose project and image name [iter5]
#   --model-dir DIR      an all-MiniLM-L6-v2 folder to use instead of downloading it
#   --memory SIZE        container memory cap, e.g. 6g [6g]; Arango is told 1G less
#   --dry-run            print every action, change nothing
#   -h, --help           this text
#
# Secrets live in ~/.iter/<name>.docker.env (mode 600); a fresh install without
# --env-file generates them and prints the admin password once. Keep the JWT
# secret: every engine and user token is signed with it. Rerun to upgrade.
set -euo pipefail

REPO="Stephen-Hilton/iter"
# shellcheck disable=SC2015  # piped from curl there is no script dir: fall back to the cwd
HERE="$(cd "$(dirname "${BASH_SOURCE[0]:-$0}")" 2>/dev/null && pwd || pwd)"
IHOME="$HOME/.iter"
VERSION="${ITER_VERSION:-}"
BINARY=""
ENV_SRC=""
PORT="${ITER_PORT:-8400}"
ARANGO_PORT="${ARANGO_HOST_PORT:-8630}"
DATA_DIR="${ITER_DATA_DIR:-}"
NAME="iter5"
MODEL_SRC=""
MEM="${ITER_MEM_LIMIT:-6g}"
DRY=0
FILES=(Dockerfile compose.yml compose.console.yml entrypoint.sh)
MODEL_SHA="53aa51172d142c89d9012cce15ae4d6cc0ca6895895114379cacb4fab128d9db"

say() { echo "[iter_data/docker] $*"; }
die() { echo "[iter_data/docker] ERROR: $*" >&2; exit 1; }
usage() { sed -n '2,24p' "${BASH_SOURCE[0]:-$0}" 2>/dev/null | sed 's/^# \{0,1\}//' || echo "see installers/iter_data/docker/README.md"; }
run() { echo "  + $*"; [ "$DRY" = 1 ] || "$@"; }
envval() { [ -f "$2" ] && grep -E "^\s*(export\s+)?$1=" "$2" | tail -1 | sed -E 's/^[^=]*=//; s/^["'\'']//; s/["'\'']$//' | tr -d '\r' || true; }
sha() { if command -v sha256sum >/dev/null; then sha256sum "$1" | cut -d' ' -f1; else shasum -a 256 "$1" | cut -d' ' -f1; fi; }
gen() { LC_ALL=C tr -dc 'A-Za-z0-9' </dev/urandom | head -c "$1" || true; }

while [ $# -gt 0 ]; do
  case "$1" in
    --version) VERSION="$2"; shift 2 ;;
    --binary) BINARY="$2"; shift 2 ;;
    --env-file) ENV_SRC="$2"; shift 2 ;;
    --port) PORT="$2"; shift 2 ;;
    --arango-port) ARANGO_PORT="$2"; shift 2 ;;
    --data-dir) DATA_DIR="$2"; shift 2 ;;
    --name) NAME="$2"; shift 2 ;;
    --model-dir) MODEL_SRC="$2"; shift 2 ;;
    --memory) MEM="$2"; shift 2 ;;
    --dry-run) DRY=1; shift ;;
    -h|--help) usage; exit 0 ;;
    *) usage; die "unknown option $1" ;;
  esac
done
[ "$DRY" = 1 ] && say "dry run: nothing is changed"

# ---- 1. docker
command -v docker >/dev/null || die "docker is not installed (https://docs.docker.com/engine/install/)"
docker info >/dev/null 2>&1 || die "docker is installed but the daemon does not answer (start Docker, or add yourself to the docker group)"
docker compose version >/dev/null 2>&1 || die "the docker compose plugin is missing (docker-compose-plugin)"
DARCH="$(docker info --format '{{.Architecture}}')"
case "$DARCH" in x86_64|amd64) ARCH=x86_64 ;; aarch64|arm64) ARCH=aarch64 ;; *) die "unsupported docker architecture $DARCH" ;; esac
ASSET="iter_data-linux-$ARCH"
say "docker $(docker version --format '{{.Server.Version}}') on $ARCH"

# ---- 2. the build folder: this folder, or (piped from curl) ~/.iter/<name>-docker
if [ -f "$HERE/Dockerfile" ] && [ -f "$HERE/compose.yml" ]; then
  BUILD="$HERE"
  say "building from $BUILD"
else
  BUILD="$IHOME/$NAME-docker"
  ref="${VERSION:-main}"
  say "no Dockerfile beside the script: fetching this folder ($ref) into $BUILD"
  run mkdir -p "$BUILD"
  for f in "${FILES[@]}"; do
    run curl -fsSL --retry 3 -o "$BUILD/$f" "https://raw.githubusercontent.com/$REPO/$ref/installers/iter_data/docker/$f"
  done
fi

# ---- 3. the binary
if [ -n "$BINARY" ]; then
  [ -f "$BINARY" ] || die "--binary $BINARY does not exist"
  say "binary: $BINARY (--binary) -> $BUILD/$ASSET"
  if [ "$(cd "$(dirname "$BINARY")" && pwd)/$(basename "$BINARY")" != "$BUILD/$ASSET" ]; then
    run cp "$BINARY" "$BUILD/.$ASSET.tmp"
    run mv -f "$BUILD/.$ASSET.tmp" "$BUILD/$ASSET"
  fi
elif [ -f "$BUILD/$ASSET" ] && [ -z "$VERSION" ]; then
  say "binary: $BUILD/$ASSET (already in the folder)"
else
  if [ -n "$VERSION" ]; then base="https://github.com/$REPO/releases/download/$VERSION"; else base="https://github.com/$REPO/releases/latest/download"; fi
  say "binary: downloading $ASSET from $base"
  if [ "$DRY" = 1 ]; then
    echo "  + curl -fsSL $base/$ASSET and $base/SHA256SUMS; verify sha256; mv into $BUILD/$ASSET"
  else
    t="$(mktemp -d)"; trap 'rm -rf "$t"' EXIT
    curl -fsSL --retry 3 -o "$t/$ASSET" "$base/$ASSET" || die "could not download $base/$ASSET"
    curl -fsSL --retry 3 -o "$t/SHA256SUMS" "$base/SHA256SUMS" || die "could not download $base/SHA256SUMS"
    want="$(awk -v a="$ASSET" '$2==a || $2=="*"a {print $1}' "$t/SHA256SUMS")"
    [ -n "$want" ] || die "$ASSET is not listed in SHA256SUMS"
    [ "$(sha "$t/$ASSET")" = "$want" ] || die "$ASSET sha256 does not match SHA256SUMS"
    say "sha256 verified"
    mv -f "$t/$ASSET" "$BUILD/$ASSET"
  fi
fi

# ---- 4. the embedding model (all-MiniLM-L6-v2, Apache-2.0, ~91 MB)
MDIR="$BUILD/models/all-MiniLM-L6-v2"
if [ -f "$MDIR/model.safetensors" ] && [ "$(sha "$MDIR/model.safetensors")" = "$MODEL_SHA" ] && [ -f "$MDIR/tokenizer.json" ]; then
  say "model: $MDIR (present)"
elif [ -n "$MODEL_SRC" ]; then
  [ -f "$MODEL_SRC/model.safetensors" ] || die "--model-dir $MODEL_SRC has no model.safetensors"
  say "model: copying $MODEL_SRC"
  run mkdir -p "$BUILD/models"
  run rm -rf "$MDIR.tmp"
  run cp -R "$MODEL_SRC" "$MDIR.tmp"
  [ -e "$MDIR" ] && run mv "$MDIR" "$MDIR.old.$$"
  run mv "$MDIR.tmp" "$MDIR"
  [ "$DRY" = 1 ] || rm -rf "$MDIR.old.$$"
else
  say "model: downloading all-MiniLM-L6-v2 into $MDIR"
  hf="https://huggingface.co/sentence-transformers/all-MiniLM-L6-v2/resolve/main"
  run mkdir -p "$MDIR/1_Pooling"
  for f in config.json tokenizer.json model.safetensors special_tokens_map.json tokenizer_config.json \
           modules.json sentence_bert_config.json 1_Pooling/config.json README.md; do
    run curl -fsSL --retry 3 -o "$MDIR/$f.part" "$hf/$f"
    run mv "$MDIR/$f.part" "$MDIR/$f"
  done
  if [ "$DRY" = 0 ] && [ "$(sha "$MDIR/model.safetensors")" != "$MODEL_SHA" ]; then
    say "WARNING: model.safetensors sha256 differs from the verified $MODEL_SHA (upstream changed?)"
  fi
fi

# ---- 5. data folder and secrets
if [ -z "$DATA_DIR" ]; then
  DATA_DIR="$IHOME/iter_data"
  # a machine set up before ~/.iter5 became ~/.iter keeps its data where it is
  [ -e "$DATA_DIR" ] || [ ! -d "$HOME/.iter5/iter_data" ] || DATA_DIR="$HOME/.iter5/iter_data"
fi
ENV_PATH="$IHOME/$NAME.docker.env"
say "data: $DATA_DIR   secrets: $ENV_PATH"
run mkdir -p "$DATA_DIR" "$IHOME"

pick() {  # pick KEY -> --env-file value, else the existing env file's, else empty
  local v=""
  [ -n "$ENV_SRC" ] && v="$(envval "$1" "$ENV_SRC")"
  [ -n "$v" ] || v="$(envval "$1" "$ENV_PATH")"
  printf '%s' "$v"
}
[ -z "$ENV_SRC" ] || [ -f "$ENV_SRC" ] || die "--env-file $ENV_SRC does not exist"
ADMIN_PW="$(pick ITER_ADMIN_PASSWORD)"
JWT="$(pick ITER_JWT_SECRET)"
ARANGO_PW="$(pick ARANGO_ROOT_PASSWORD)"
[ -n "$ARANGO_PW" ] || ARANGO_PW="${ARANGO_ROOT_PASSWORD:-}"
FRESH_ADMIN=""
if [ -z "$JWT" ]; then
  if [ -f "$DATA_DIR/iter/iter_data.secret" ]; then
    say "no ITER_JWT_SECRET given: iter_data keeps using $DATA_DIR/iter/iter_data.secret"
  else
    JWT="$(gen 48)"; say "generated a new ITER_JWT_SECRET"
  fi
fi
if [ -z "$ADMIN_PW" ]; then
  # iter_data only reads it when it creates admin, i.e. on an empty database
  if [ -f "$DATA_DIR/arango/SERVER" ]; then say "existing database: admin keeps its password"
  else ADMIN_PW="$(gen 20)"; FRESH_ADMIN=1; fi
fi
if [ -z "$ARANGO_PW" ]; then
  if [ -f "$DATA_DIR/arango/SERVER" ]; then
    ARANGO_PW=iter4dev; say "existing database, no ARANGO_ROOT_PASSWORD given: assuming the old default"
  else
    ARANGO_PW="$(gen 24)"; say "generated an Arango root password (kept in $ENV_PATH)"
  fi
fi
if [ "$DRY" = 1 ]; then
  echo "  + write $ENV_PATH (mode 600): ITER_ADMIN_PASSWORD, ITER_JWT_SECRET, ARANGO_ROOT_PASSWORD"
else
  ( umask 077
    { [ -z "$ADMIN_PW" ] || echo "ITER_ADMIN_PASSWORD=$ADMIN_PW"
      [ -z "$JWT" ] || echo "ITER_JWT_SECRET=$JWT"
      echo "ARANGO_ROOT_PASSWORD=$ARANGO_PW"
    } > "$ENV_PATH.tmp" && mv -f "$ENV_PATH.tmp" "$ENV_PATH" )
fi

# ---- 6. build + start
case "$MEM" in
  *[gG]) n="${MEM%[gG]}"; AMEM="$(( n > 2 ? n - 1 : 1 ))G" ;;
  *) AMEM="${ITER_ARANGO_MEMORY:-5G}" ;;
esac
compose=(docker compose -p "$NAME" -f "$BUILD/compose.yml")
[ "$ARANGO_PORT" = 0 ] || compose+=(-f "$BUILD/compose.console.yml")
say "building image $NAME:latest and starting container $NAME (:$PORT$([ "$ARANGO_PORT" = 0 ] || echo ", console 127.0.0.1:$ARANGO_PORT"))"
export ITER_NAME="$NAME" ITER_PORT="$PORT" ARANGO_HOST_PORT="$ARANGO_PORT" ITER_DATA_DIR="$DATA_DIR" \
  ITER_ENV_PATH="$ENV_PATH" ITER_MEM_LIMIT="$MEM" ITER_ARANGO_MEMORY="$AMEM"
run "${compose[@]}" up -d --build

# ---- 7. health
if [ "$DRY" = 1 ]; then
  echo "  + wait for http://127.0.0.1:$PORT/health to answer {\"ok\":true}"
  say "dry run done"; exit 0
fi
for _ in $(seq 1 180); do
  if h="$(curl -fsS "http://127.0.0.1:$PORT/health" 2>/dev/null)" && echo "$h" | grep -q '"ok":true'; then
    say "iter_data up: $h"
    say "webui: http://127.0.0.1:$PORT/  (login: admin / ITER_ADMIN_PASSWORD in $ENV_PATH)"
    [ -z "$FRESH_ADMIN" ] || say "admin password (shown once; also in $ENV_PATH): $ADMIN_PW"
    [ "$ARANGO_PORT" = 0 ] || say "Arango console: http://127.0.0.1:$ARANGO_PORT/ (root / ARANGO_ROOT_PASSWORD in $ENV_PATH)"
    exit 0
  fi
  sleep 1
done
docker logs --tail 40 "$NAME" >&2 || true
die "iter_data did not answer on :$PORT within 3 minutes"
