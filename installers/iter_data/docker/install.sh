#!/usr/bin/env bash
# Install iter_data as one Docker container (ArangoDB CE 3.12 + iter_data):
# API + webui on :8400, the Arango web console on 127.0.0.1:8630. With --arango-url
# the container runs iter_data only, against an ArangoDB you already have.
#
#   ./install.sh [install] [options]  build the image here and (re)start the container
#   ./install.sh status [--name NAME] the container, health, the TLS certificate fingerprint;
#                                     removes the first-boot admin password once it is changed
#   curl -fsSL https://raw.githubusercontent.com/Stephen-Hilton/iter/main/installers/iter_data/docker/install.sh | bash -s -- --version vYYYY.MM.DD
#
# Options:
#   --version TAG        release to download the binary (and, when piped, these files) from [latest]
#   --binary PATH        use this iter_data linux binary instead of downloading one
#   --env-file PATH      take ITER_ADMIN_PASSWORD, ITER_JWT_SECRET and the ARANGO_* settings below from it
#   --port N             host port for the API + webui [8400; ITER_PORT]
#   --arango-port N      host port for the Arango console on 127.0.0.1, 0 = not published [8630]
#   --data-dir DIR       host folder for everything that persists [~/.iter/iter_data]
#   --name NAME          container, compose project and image name [iter5]
#   --model-dir DIR      an all-MiniLM-L6-v2 folder to use instead of downloading it
#   --memory SIZE        container memory cap, e.g. 6g [6g]; Arango is told 1G less
#   --tls-cert PATH      serve this certificate (PEM, e.g. a real one for your domain) instead of
#                        the self-signed one; needs --tls-key. Copied into the data folder.
#   --tls-key PATH       the private key (PEM) for --tls-cert
#   --tls-name NAME      an extra DNS name or IP on the self-signed certificate (repeatable),
#                        e.g. the name engines and browsers use; only read when it is first made
#   --tls off|auto       off = plain HTTP (only behind a TLS proxy on this host); auto = the
#                        self-signed certificate again (drops an earlier --tls-cert or --tls off)
#   --arango-url URL     use an existing ArangoDB at URL (https://host:8529) instead of the one
#                        in the container: no arangod runs and no console port is published.
#                        Also read from ARANGO_URL (environment or --env-file). `local` = back to
#                        the ArangoDB in the container [local]
#   --arango-fingerprint FP   pin that ArangoDB's certificate (sha256:<64 hex>; comma-separated
#                        for a rotation): the way to trust a self-signed certificate
#   --arango-ca-cert PATH     trust this CA (PEM) for its certificate instead; copied into the
#                        data folder. With neither, only the public CAs are trusted
#   --arango-db NAME     iter's database [iter_db; an existing install keeps its own]
#   --arango-user NAME   the Arango user iter_data signs in as, rw on that database only [iter_user]
#
# ArangoDB passwords come from the environment or --env-file: ARANGO_PASSWORD is the
# user's own (made once when not given; never root's), ARANGO_ROOT_PASSWORD lets
# iter_data create that user and database on first start (the container's own ArangoDB
# always has one; an external one needs it only when the user does not exist yet).
# Plain http:// to an external ArangoDB is refused unless ARANGO_ALLOW_PLAIN_HTTP=1
# (a private network only).
#   --dry-run            print every action, change nothing
#   -h, --help           this text
#
# Secrets live in ~/.iter/<name>.docker.env (mode 600); a fresh install without
# --env-file generates them and prints the admin password once. Keep the JWT
# secret: every engine and user token is signed with it. Rerun to upgrade.
#
# HTTPS: iter_data makes a self-signed certificate on first start (tls.crt beside
# its secrets in <data-dir>/iter, kept across reinstalls) and the script prints its
# fingerprint. Give that fingerprint to each engine installer (--fingerprint) or
# compare it with `iter_engine key status`. Browsers warn about a self-signed
# certificate; use --tls-cert/--tls-key or a TLS proxy for one they trust. The TLS
# choices are remembered in the secrets file, so a plain rerun keeps them.
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
CMD=install
FILES=(Dockerfile compose.yml compose.console.yml entrypoint.sh)
MODEL_SHA="53aa51172d142c89d9012cce15ae4d6cc0ca6895895114379cacb4fab128d9db"
TLS_CERT_SRC="" TLS_KEY_SRC="" TLS_MODE="" TLS_NAMES=()
A_URL_OPT="" A_FP_OPT="" A_CA_SRC="" A_DB_OPT="" A_USER_OPT=""

say() { echo "[iter_data/docker] $*"; }
die() { echo "[iter_data/docker] ERROR: $*" >&2; exit 1; }
usage() { sed -n '2,/^set -euo/p' "${BASH_SOURCE[0]:-$0}" 2>/dev/null | sed -e '$d' -e 's/^# \{0,1\}//' || echo "see installers/iter_data/docker/README.md"; }
run() { echo "  + $*"; [ "$DRY" = 1 ] || "$@"; }
envval() { [ -f "$2" ] && grep -E "^\s*(export\s+)?$1=" "$2" | tail -1 | sed -E 's/^[^=]*=//; s/^["'\'']//; s/["'\'']$//' | tr -d '\r' || true; }
sha() { if command -v sha256sum >/dev/null; then sha256sum "$1" | cut -d' ' -f1; else shasum -a 256 "$1" | cut -d' ' -f1; fi; }
gen() { LC_ALL=C tr -dc 'A-Za-z0-9' </dev/urandom | head -c "$1" || true; }

while [ $# -gt 0 ]; do
  case "$1" in
    install|status) CMD="$1"; shift ;;
    --version) VERSION="$2"; shift 2 ;;
    --binary) BINARY="$2"; shift 2 ;;
    --env-file) ENV_SRC="$2"; shift 2 ;;
    --port) PORT="$2"; shift 2 ;;
    --arango-port) ARANGO_PORT="$2"; shift 2 ;;
    --data-dir) DATA_DIR="$2"; shift 2 ;;
    --name) NAME="$2"; shift 2 ;;
    --model-dir) MODEL_SRC="$2"; shift 2 ;;
    --memory) MEM="$2"; shift 2 ;;
    --tls-cert) TLS_CERT_SRC="$2"; shift 2 ;;
    --tls-key) TLS_KEY_SRC="$2"; shift 2 ;;
    --tls-name) TLS_NAMES+=("$2"); shift 2 ;;
    --tls) TLS_MODE="$2"; shift 2 ;;
    --arango-url) A_URL_OPT="$2"; shift 2 ;;
    --arango-fingerprint) A_FP_OPT="$2"; shift 2 ;;
    --arango-ca-cert) A_CA_SRC="$2"; shift 2 ;;
    --arango-db) A_DB_OPT="$2"; shift 2 ;;
    --arango-user) A_USER_OPT="$2"; shift 2 ;;
    --dry-run) DRY=1; shift ;;
    -h|--help) usage; exit 0 ;;
    *) usage; die "unknown option $1" ;;
  esac
done
[ "$DRY" = 1 ] && say "dry run: nothing is changed"
case "$TLS_MODE" in ""|auto|off) ;; *) die "--tls is auto or off (a certificate of your own: --tls-cert/--tls-key)" ;; esac
{ [ -n "$TLS_CERT_SRC" ] && [ -n "$TLS_KEY_SRC" ]; } || { [ -z "$TLS_CERT_SRC" ] && [ -z "$TLS_KEY_SRC" ]; } \
  || die "--tls-cert and --tls-key go together"
[ -z "$TLS_CERT_SRC" ] || [ "$TLS_MODE" != off ] || die "--tls off and --tls-cert contradict each other"
[ -z "$TLS_CERT_SRC" ] || [ -f "$TLS_CERT_SRC" ] || die "--tls-cert $TLS_CERT_SRC does not exist"
[ -z "$TLS_KEY_SRC" ] || [ -f "$TLS_KEY_SRC" ] || die "--tls-key $TLS_KEY_SRC does not exist"
[ -z "$A_CA_SRC" ] || [ -f "$A_CA_SRC" ] || die "--arango-ca-cert $A_CA_SRC does not exist"

if [ -z "$DATA_DIR" ]; then
  DATA_DIR="$IHOME/iter_data"
  # a machine set up before ~/.iter5 became ~/.iter keeps its data where it is
  [ -e "$DATA_DIR" ] || [ ! -d "$HOME/.iter5/iter_data" ] || DATA_DIR="$HOME/.iter5/iter_data"
fi
ENV_PATH="$IHOME/$NAME.docker.env"
CTLS_DIR="$DATA_DIR/custom-tls"
TLS_CUSTOM=""

# the certificate's fingerprint: the line iter_data logs at every start, else openssl
# on the file (readable on Docker Desktop; on a Linux host the folder is root's)
fingerprint() {
  local fp crt="$DATA_DIR/iter/tls.crt"
  fp="$(docker logs "$NAME" 2>&1 | sed -n 's/.*TLS certificate fingerprint.*\(sha256:[0-9a-f]\{64\}\).*/\1/p' | tail -1)"
  [ -z "$TLS_CUSTOM" ] || crt="$CTLS_DIR/tls.crt"
  if [ -z "$fp" ] && [ -r "$crt" ] && command -v openssl >/dev/null; then
    fp="$(openssl x509 -in "$crt" -noout -fingerprint -sha256 2>/dev/null | sed 's/.*=//; s/://g' | tr 'A-F' 'a-f')"
    [ -z "$fp" ] || fp="sha256:$fp"
  fi
  printf '%s' "$fp"
}

# iter_data writes <data-dir>/iter/admin_password_changed once the first admin has
# chosen their own password; then the first-boot ITER_ADMIN_PASSWORD leaves the env file
admin_changed() {
  [ -f "$DATA_DIR/iter/admin_password_changed" ] \
    || docker exec "$NAME" test -f /var/lib/iter/admin_password_changed >/dev/null 2>&1
}
admin_cleanup() {
  [ -f "$ENV_PATH" ] && grep -q '^ITER_ADMIN_PASSWORD=' "$ENV_PATH" || return 0
  admin_changed || return 0
  if [ "$DRY" = 1 ]; then echo "  + remove ITER_ADMIN_PASSWORD from $ENV_PATH (the admin has chosen their own password)"; return 0; fi
  ( umask 077; { grep -v '^ITER_ADMIN_PASSWORD=' "$ENV_PATH" || true; } > "$ENV_PATH.tmp" && mv -f "$ENV_PATH.tmp" "$ENV_PATH" )
  say "removed ITER_ADMIN_PASSWORD from $ENV_PATH: the admin has chosen their own password"
}

do_status() {
  local st port scheme=https h fp
  st="$(docker inspect -f '{{.State.Status}}{{if .State.Health}} ({{.State.Health.Status}}){{end}} since {{.State.StartedAt}}' "$NAME" 2>/dev/null || echo "missing")"
  printf '%-12s %s\n' container "$NAME: $st"
  port="$(docker port "$NAME" 8400/tcp 2>/dev/null | head -1 | sed 's/.*://')"
  [ "$(envval ITER_TLS "$ENV_PATH")" != off ] || scheme=http
  TLS_CUSTOM="$(envval ITER_TLS_CERT "$ENV_PATH")"
  if [ -n "$port" ] && h="$(curl -fsSk "$scheme://127.0.0.1:$port/health" 2>/dev/null)"; then printf '%-12s %s\n' health "$scheme://127.0.0.1:$port/ $h"
  else printf '%-12s %s\n' health "no answer${port:+ on $scheme://127.0.0.1:$port}"; fi
  if [ "$scheme" = http ]; then printf '%-12s %s\n' tls "off (plain HTTP, behind a TLS proxy)"
  else fp="$(fingerprint)"; printf '%-12s %s\n' tls "$([ -n "$TLS_CUSTOM" ] && echo custom || echo auto), certificate fingerprint ${fp:-unknown}"; fi
  printf '%-12s %s\n' data "$DATA_DIR   secrets: $ENV_PATH"
  admin_cleanup
}

# ---- 1. docker
command -v docker >/dev/null || die "docker is not installed (https://docs.docker.com/engine/install/)"
docker info >/dev/null 2>&1 || die "docker is installed but the daemon does not answer (start Docker, or add yourself to the docker group)"
docker compose version >/dev/null 2>&1 || die "the docker compose plugin is missing (docker-compose-plugin)"
if [ "$CMD" = status ]; then do_status; exit 0; fi
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
say "data: $DATA_DIR   secrets: $ENV_PATH"
run mkdir -p "$DATA_DIR" "$IHOME"
# owner-only: iter_data's secrets, engine_session.secret and tls.key live in it
run chmod 700 "$DATA_DIR"
# true when PATH exists, or might: on a Linux host the container makes iter/ owner-only
# as root, so this user cannot look inside it
present() { [ -e "$1" ] || { [ -d "$(dirname "$1")" ] && ! [ -x "$(dirname "$1")" ]; }; }

admin_cleanup
pick() {  # pick KEY -> --env-file value, else the existing env file's, else empty
  local v=""
  [ -n "$ENV_SRC" ] && v="$(envval "$1" "$ENV_SRC")"
  [ -n "$v" ] || v="$(envval "$1" "$ENV_PATH")"
  printf '%s' "$v"
}
[ -z "$ENV_SRC" ] || [ -f "$ENV_SRC" ] || die "--env-file $ENV_SRC does not exist"
ADMIN_PW="$(pick ITER_ADMIN_PASSWORD)"
JWT="$(pick ITER_JWT_SECRET)"

# ---- ArangoDB: the container's own (local), or an existing one at --arango-url
# the host of a URL, without brackets or port; local = this container's own ArangoDB
url_host() { local h="${1#*://}"; h="${h%%/*}"; case "$h" in \[*) h="${h%%]*}"; h="${h#[}" ;; *) h="${h%%:*}" ;; esac; printf '%s' "$h"; }
is_local() { [ -z "$1" ] || [ "$1" = local ] && return 0; case "$(url_host "$1")" in 127.*|localhost|::1) return 0 ;; esac; return 1; }
OLD_URL="$(envval ARANGO_URL "$ENV_PATH")"
if [ -n "$A_URL_OPT" ]; then A_URL="$A_URL_OPT"
elif [ -n "${ARANGO_URL:-}" ]; then A_URL="$ARANGO_URL"
elif [ -n "$ENV_SRC" ] && [ -n "$(envval ARANGO_URL "$ENV_SRC")" ]; then A_URL="$(envval ARANGO_URL "$ENV_SRC")"
else A_URL="$OLD_URL"; fi
is_local "$A_URL" && A_URL=""
# the existing env file's ArangoDB settings count only for the same ArangoDB
SAME=0
if { [ -z "$A_URL" ] && is_local "$OLD_URL"; } || [ "$A_URL" = "$OLD_URL" ]; then SAME=1; fi
apick() {  # apick KEY -> --env-file, else the environment, else the env file (same ArangoDB only)
  local v=""
  [ -n "$ENV_SRC" ] && v="$(envval "$1" "$ENV_SRC")"
  [ -n "$v" ] || v="${!1:-}"
  [ -n "$v" ] || [ "$SAME" = 0 ] || v="$(envval "$1" "$ENV_PATH")"
  printf '%s' "$v"
}
A_USER="$A_USER_OPT"; [ -n "$A_USER" ] || A_USER="$(apick ARANGO_USER)"; [ -n "$A_USER" ] || A_USER=iter_user
[ "$A_USER" != root ] || die "ARANGO_USER root: iter_data signs in as a user of its own (rw on its database only); pick another name"
# empty = iter_data's default iter_db, or (container ArangoDB) the database iter5 an older install made
A_DB="$A_DB_OPT"; [ -n "$A_DB" ] || A_DB="$(apick ARANGO_DB)"
A_ROOT="$(apick ARANGO_ROOT_PASSWORD)"
A_PW="$(apick ARANGO_PASSWORD)"
# the container's ArangoDB secrets, kept aside while an external one is in use
STASH_ROOT="$(envval ITER_LOCAL_ARANGO_ROOT_PASSWORD "$ENV_PATH")"
STASH_PW="$(envval ITER_LOCAL_ARANGO_PASSWORD "$ENV_PATH")"
if is_local "$OLD_URL" && [ -f "$ENV_PATH" ]; then
  STASH_ROOT="$(envval ARANGO_ROOT_PASSWORD "$ENV_PATH")"; STASH_PW="$(envval ARANGO_PASSWORD "$ENV_PATH")"
fi
A_FP="" A_CA="" A_PLAIN=""
CA_DIR="$DATA_DIR/arango-ca"
A_CA_IN=/var/lib/iter_data/arango-ca/ca.pem
if [ -z "$A_URL" ]; then
  [ -z "$A_FP_OPT$A_CA_SRC" ] || die "--arango-fingerprint / --arango-ca-cert are for an external ArangoDB (--arango-url)"
  [ -n "$A_ROOT" ] || A_ROOT="$STASH_ROOT"
  if [ -z "$A_ROOT" ]; then
    if present "$DATA_DIR/arango/SERVER"; then
      A_ROOT=iter4dev; say "existing database, no ARANGO_ROOT_PASSWORD given: assuming the old default"
    else
      A_ROOT="$(gen 24)"; say "generated an Arango root password (kept in $ENV_PATH)"
    fi
  fi
  [ -n "$A_PW" ] || A_PW="$STASH_PW"
  if [ -z "$A_PW" ] || [ "$A_PW" = "$A_ROOT" ]; then A_PW="$(gen 24)"; say "generated a password for Arango user $A_USER (kept in $ENV_PATH)"; fi
  say "ArangoDB: the container's own; iter_data signs in as $A_USER (rw on ${A_DB:-iter_db} only)"
  STASH_ROOT="" STASH_PW=""
else
  case "$A_URL" in
    https://*) ;;
    http://*)
      case "$(apick ARANGO_ALLOW_PLAIN_HTTP)" in
        1|true|yes|TRUE|YES) A_PLAIN=1; say "WARNING: plain http to $A_URL (ARANGO_ALLOW_PLAIN_HTTP): passwords and data cross the network unencrypted" ;;
        *) die "--arango-url $A_URL: plain http:// to another machine is refused; use https:// (with --arango-fingerprint or --arango-ca-cert), or set ARANGO_ALLOW_PLAIN_HTTP=1 on a private network" ;;
      esac ;;
    *) die "--arango-url $A_URL: must start with https:// or http://" ;;
  esac
  A_FP="$A_FP_OPT"; [ -n "$A_FP" ] || A_FP="$(apick ARANGO_CERT_FINGERPRINT)"
  if [ -n "$A_FP" ]; then
    fps=""
    for f in ${A_FP//,/ }; do
      h="${f#sha256:}"; h="${h#SHA256:}"; h="$(printf '%s' "${h//:/}" | tr 'A-F' 'a-f')"
      [ ${#h} = 64 ] && [ -z "${h//[0-9a-f]/}" ] || die "--arango-fingerprint $f: not a sha256 fingerprint (sha256:<64 hex>)"
      fps="${fps:+$fps,}sha256:$h"
    done
    A_FP="$fps"
  fi
  if [ -n "$A_CA_SRC" ]; then
    A_CA="$A_CA_IN"
    say "ArangoDB CA: $A_CA_SRC (copied into $CA_DIR)"
    run mkdir -p "$CA_DIR"
    run install -m 644 "$A_CA_SRC" "$CA_DIR/ca.pem"
  else
    A_CA="$(apick ARANGO_CA_CERT)"
  fi
  if [ -z "$A_PW" ]; then
    [ -n "$A_ROOT" ] || die "external ArangoDB: give ARANGO_PASSWORD (user $A_USER's), or ARANGO_ROOT_PASSWORD so iter_data can create $A_USER (environment or --env-file)"
    A_PW="$(gen 24)"; say "generated a password for Arango user $A_USER (root creates the user on first start; kept in $ENV_PATH)"
  fi
  [ "$A_PW" != "$A_ROOT" ] || die "ARANGO_PASSWORD is root's password: give $A_USER a password of its own"
  trust="public CAs"; [ -z "$A_CA" ] || trust="CA $A_CA"; [ -z "$A_FP" ] || trust="pinned $A_FP"
  case "$A_URL" in http://*) trust="none (plain http)" ;; esac
  say "ArangoDB: $A_URL (trust: $trust), user $A_USER, database ${A_DB:-iter_db}; no arangod in the container"
  ARANGO_PORT=0
fi
FRESH_ADMIN=""
if [ -n "$ADMIN_PW" ] && admin_changed; then
  say "the admin has chosen their own password: ITER_ADMIN_PASSWORD is left out of $ENV_PATH"; ADMIN_PW=""
fi
if [ -z "$JWT" ]; then
  if present "$DATA_DIR/iter/iter_data.secret"; then
    say "no ITER_JWT_SECRET given: iter_data keeps using $DATA_DIR/iter/iter_data.secret"
  else
    JWT="$(gen 48)"; say "generated a new ITER_JWT_SECRET"
  fi
fi
if [ -z "$ADMIN_PW" ]; then
  # iter_data only reads it when it creates admin, i.e. on an empty database
  if [ -z "$A_URL" ] && present "$DATA_DIR/arango/SERVER"; then say "existing database: admin keeps its password"
  elif [ -n "$A_URL" ] && present "$DATA_DIR/iter/iter_data.secret"; then say "iter_data has run here before: admin keeps its password"
  else ADMIN_PW="$(gen 20)"; FRESH_ADMIN=1; fi
fi

# TLS: the flags given now, else what the secrets file kept from an earlier run.
# A certificate of your own is copied to <data-dir>/custom-tls/ (owner-only) and
# read by iter_data at /var/lib/iter_data/custom-tls/ inside the container.
CTLS_CERT=/var/lib/iter_data/custom-tls/tls.crt
CTLS_KEY=/var/lib/iter_data/custom-tls/tls.key
TLS_OFF="$(pick ITER_TLS)"; [ "$TLS_OFF" = off ] || TLS_OFF=""
TLS_CUSTOM="$(pick ITER_TLS_CERT)"
NAMES="$(pick ITER_TLS_NAMES)"
case "$TLS_MODE" in
  off) TLS_OFF=off; TLS_CUSTOM="" ;;
  auto) TLS_OFF=""; TLS_CUSTOM="" ;;
esac
if [ -n "$TLS_CERT_SRC" ]; then
  TLS_OFF=""; TLS_CUSTOM="$CTLS_CERT"
  say "TLS: serving your certificate $TLS_CERT_SRC (copied into $CTLS_DIR)"
  run mkdir -p "$CTLS_DIR"
  run chmod 700 "$CTLS_DIR"
  run install -m 644 "$TLS_CERT_SRC" "$CTLS_DIR/tls.crt"
  run install -m 600 "$TLS_KEY_SRC" "$CTLS_DIR/tls.key"
fi
if [ ${#TLS_NAMES[@]} -gt 0 ]; then NAMES="$(IFS=,; echo "${TLS_NAMES[*]}")"; fi
if [ "$TLS_OFF" = off ]; then say "TLS: off (plain HTTP): only for iter_data behind a TLS proxy; engines refuse plain http across a network"
elif [ -n "$TLS_CUSTOM" ]; then [ -n "$TLS_CERT_SRC" ] || say "TLS: your certificate in $CTLS_DIR (kept from the last run)"
else say "TLS: a self-signed certificate (made on first start, kept in $DATA_DIR/iter)${NAMES:+, extra names $NAMES}"; fi
if [ "$DRY" = 1 ]; then
  echo "  + write $ENV_PATH (mode 600): ITER_ADMIN_PASSWORD, ITER_JWT_SECRET, ARANGO_* settings, TLS settings"
else
  ( umask 077
    { [ -z "$ADMIN_PW" ] || echo "ITER_ADMIN_PASSWORD=$ADMIN_PW"
      [ -z "$JWT" ] || echo "ITER_JWT_SECRET=$JWT"
      [ -z "$A_URL" ] || echo "ARANGO_URL=$A_URL"
      echo "ARANGO_USER=$A_USER"
      echo "ARANGO_PASSWORD=$A_PW"
      [ -z "$A_ROOT" ] || echo "ARANGO_ROOT_PASSWORD=$A_ROOT"
      [ -z "$A_DB" ] || echo "ARANGO_DB=$A_DB"
      [ -z "$A_FP" ] || echo "ARANGO_CERT_FINGERPRINT=$A_FP"
      [ -z "$A_CA" ] || echo "ARANGO_CA_CERT=$A_CA"
      [ -z "$A_PLAIN" ] || echo "ARANGO_ALLOW_PLAIN_HTTP=1"
      [ -z "$STASH_ROOT" ] || echo "ITER_LOCAL_ARANGO_ROOT_PASSWORD=$STASH_ROOT"
      [ -z "$STASH_PW" ] || echo "ITER_LOCAL_ARANGO_PASSWORD=$STASH_PW"
      [ -z "$TLS_OFF" ] || echo "ITER_TLS=off"
      [ -z "$TLS_CUSTOM" ] || printf 'ITER_TLS_CERT=%s\nITER_TLS_KEY=%s\n' "$CTLS_CERT" "$CTLS_KEY"
      [ -z "$NAMES" ] || echo "ITER_TLS_NAMES=$NAMES"
    } > "$ENV_PATH.tmp" && mv -f "$ENV_PATH.tmp" "$ENV_PATH" )
fi
SCHEME=https; [ "$TLS_OFF" != off ] || SCHEME=http

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
  echo "  + wait for $SCHEME://127.0.0.1:$PORT/health to answer {\"ok\":true}$([ "$SCHEME" = http ] || echo ' (curl -k: the certificate is self-signed)')"
  [ "$SCHEME" = http ] || echo "  + print the TLS certificate fingerprint (from the container log)"
  say "dry run done"; exit 0
fi
for _ in $(seq 1 180); do
  if h="$(curl -fsSk "$SCHEME://127.0.0.1:$PORT/health" 2>/dev/null)" && echo "$h" | grep -q '"ok":true'; then
    say "iter_data up: $h"
    if grep -q '^ITER_ADMIN_PASSWORD=' "$ENV_PATH" 2>/dev/null; then say "webui: $SCHEME://127.0.0.1:$PORT/  (first sign-in: admin / ITER_ADMIN_PASSWORD in $ENV_PATH)"
    else say "webui: $SCHEME://127.0.0.1:$PORT/  (sign in as admin with the password you chose)"; fi
    if [ -n "$FRESH_ADMIN" ]; then
      say "admin password (shown once; also in $ENV_PATH until you change it): $ADMIN_PW"
      [ -z "$A_URL" ] || say "  (used only if database ${A_DB:-iter_db} has no admin yet)"
      say "  the webui makes you change it at the first sign-in (a new password needs 12+ characters);"
      say "  then \`$0 status\` (or the next run) removes it from $ENV_PATH"
    fi
    [ "$ARANGO_PORT" = 0 ] || say "Arango console: http://127.0.0.1:$ARANGO_PORT/ (root / ARANGO_ROOT_PASSWORD in $ENV_PATH)"
    if [ "$SCHEME" = http ]; then
      say "TLS is off: put a TLS proxy in front; engines refuse plain http to another machine"
    else
      fp="$(fingerprint)"
      say "TLS certificate fingerprint: ${fp:-(not found: docker logs $NAME | grep fingerprint)}"
      say "  give it to each engine installer (--fingerprint ${fp:-sha256:...}), or compare it with \`iter_engine key status\` on the engine"
      say "  (also in the webui's Engine keys screen; later: docker logs $NAME 2>&1 | grep fingerprint)"
      [ -n "$TLS_CUSTOM" ] || say "  browsers warn about this self-signed certificate: accept it once, or rerun with --tls-cert/--tls-key (or put a TLS proxy in front)"
    fi
    exit 0
  fi
  sleep 1
done
docker logs --tail 40 "$NAME" >&2 || true
die "iter_data did not answer on :$PORT within 3 minutes"
