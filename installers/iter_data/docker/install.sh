#!/usr/bin/env bash
# Install iter_data as one Docker container (ArangoDB CE 3.12 + iter_data):
# API + webui on :8400, the Arango web console on 127.0.0.1:8630. With --arango-url
# the container runs iter_data only, against an ArangoDB you already have.
#
#   ./install.sh [install] [options]  build the image and (re)start the container
#   ./install.sh status [--name NAME] the container, health, the TLS certificate fingerprint;
#                                     removes the first-boot admin password once it is changed
#   curl -fsSL https://raw.githubusercontent.com/Stephen-Hilton/iter/main/installers/iter_data/docker/install.sh | bash -s -- --version vYYYY.MM.DD
#
# Options:
#   --iter-dir DIR       the folder that holds everything iter installs [/opt/iter, under
#                        Docker Desktop ~/iter; $ITER_DIR;
#                        later runs: where it is installed]. Forgiving: /my/dir/iter/,
#                        /my/dir/iter and /my/dir all mean /my/dir/iter. iter_data's files go
#                        in <iter dir>/iter_data (also spelled --iter_dir)
#   --version TAG        release to download the binary (and, when piped, these files) from [latest]
#   --binary PATH        use this iter_data linux binary instead of downloading one
#   --env-file PATH      take ITER_ADMIN_PASSWORD, ITER_JWT_SECRET and the ARANGO_* settings below from it
#   --port N             host port for the API + webui [8400; ITER_PORT]
#   --arango-port N      host port for the Arango console on 127.0.0.1, 0 = not published [8630]
#   --data-dir DIR       an older install's data folder, to move into the iter dir once
#                        [the container's mount, else ~/.iter/iter_data]
#   --adopt-volumes      the container NAME keeps its database in docker volumes (an older
#                        hand-made setup): copy them into the iter dir (the volumes are kept)
#   --name NAME          container, compose project and image name [iter5]; another name
#                        keeps its files in <iter dir>/iter_data_<name>
#   --model-dir DIR      an all-MiniLM-L6-v2 folder to use instead of downloading it
#   --memory SIZE        container memory cap, e.g. 6g [6g]; Arango is told 1G less
#   --tls-cert PATH      serve this certificate (PEM, e.g. a real one for your domain) instead of
#                        the self-signed one; needs --tls-key. Copied into config/.
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
#   --arango-ca-cert PATH     trust this CA (PEM) for its certificate instead; copied into
#                        config/. With neither, only the public CAs are trusted
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
# Everything is in <iter dir>/iter_data (owner-only), mounted into the container at
# /var/lib/iter_data: docker/ (the build folder: Dockerfile, compose files, the
# binary, the model), config/ (data.env = the secrets, mode 600; arango-ca.crt;
# custom-tls.crt/.key), state/ (iter_data's own files: iter_data.secret,
# engine_session.secret, tls.crt/.key), arango/data (the database), arango/apps,
# backups/. An install from before 2026-10-18 (~/.iter/iter_data,
# ~/.iter/<name>.docker.env, ~/.iter/<name>-docker) is moved in once, the
# container stopped meanwhile. A fresh install without --env-file generates the
# secrets and prints the admin password once. Keep the JWT secret: every engine
# and user token is signed with it. Rerun to upgrade.
# Docker Desktop (macOS, Windows, Linux): the iter dir must be a folder shared
# with its VM (Settings -> Resources -> File sharing; the home folder is, /opt is
# not), so the default there is ~/iter.
#
# HTTPS: iter_data makes a self-signed certificate on first start (tls.crt beside
# its secrets in state/, kept across reinstalls) and the script prints its
# fingerprint. Give that fingerprint to each engine installer (--fingerprint) or
# compare it with `iter_engine key status`. Browsers warn about a self-signed
# certificate; use --tls-cert/--tls-key or a TLS proxy for one they trust. The TLS
# choices are remembered in the secrets file, so a plain rerun keeps them.
set -euo pipefail

REPO="Stephen-Hilton/iter"
# shellcheck disable=SC2015  # piped from curl there is no script dir: fall back to the cwd
HERE="$(cd "$(dirname "${BASH_SOURCE[0]:-$0}")" 2>/dev/null && pwd || pwd)"
IHOME="$HOME/.iter"          # where an install from before 2026-10-18 kept its files
ITER_DIR_OPT="${ITER_DIR:-}"
VERSION="${ITER_VERSION:-}"
BINARY=""
ENV_SRC=""
PORT="${ITER_PORT:-8400}"
ARANGO_PORT="${ARANGO_HOST_PORT:-8630}"
OLD_DATA=""
NAME="iter5"
MODEL_SRC=""
MEM="${ITER_MEM_LIMIT:-6g}"
DRY=0
CMD=install
FILES=(Dockerfile compose.yml compose.console.yml entrypoint.sh)
MODEL_SHA="53aa51172d142c89d9012cce15ae4d6cc0ca6895895114379cacb4fab128d9db"
TLS_CERT_SRC="" TLS_KEY_SRC="" TLS_MODE="" TLS_NAMES=()
A_URL_OPT="" A_FP_OPT="" A_CA_SRC="" A_DB_OPT="" A_USER_OPT=""
ADOPT=0

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
    --iter-dir|--iter_dir) ITER_DIR_OPT="$2"; shift 2 ;;
    --data-dir) OLD_DATA="$2"; shift 2 ;;
    --adopt-volumes) ADOPT=1; shift ;;
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

case "$NAME" in ""|*[!A-Za-z0-9_.-]*) die "--name $NAME: letters, digits, - _ . only" ;; esac

# --iter-dir: the folder that holds everything, always ending in /iter
# (/my/dir/iter/, /my/dir/iter and /my/dir all give /my/dir/iter)
iter_dir_of() {
  local d="$1"
  # shellcheck disable=SC2088  # a literal ~ typed in quotes
  case "$d" in "~") d="$HOME" ;; "~/"*) d="$HOME/${d#\~/}" ;; esac
  case "$d" in /*) ;; *) d="$PWD/$d" ;; esac
  while [ "${#d}" -gt 1 ] && [ "${d%/}" != "$d" ]; do d="${d%/}"; done
  [ "${d##*/}" = iter ] || d="${d%/}/iter"
  printf '%s' "$d"
}
PROG=iter_data; [ "$NAME" = iter5 ] || PROG="iter_data_$NAME"
TLS_CUSTOM=""
# set_paths: everything below <iter dir>/<PROG>
set_paths() {
  D="$ITER_DIR/$PROG"
  BUILD="$D/docker"
  CONF_DIR="$D/config"
  STATE="$D/state"
  ENV_PATH="$CONF_DIR/data.env"
  CTLS_HOST_CRT="$CONF_DIR/custom-tls.crt"
  CTLS_HOST_KEY="$CONF_DIR/custom-tls.key"
  OLD_KEEP="$D/old-dot-iter"
}

# the certificate's fingerprint: the line iter_data logs at every start, else openssl
# on the file (readable on Docker Desktop; on a Linux host the folder is root's)
fingerprint() {
  local fp crt="$STATE/tls.crt"
  fp="$(docker logs "$NAME" 2>&1 | sed -n 's/.*TLS certificate fingerprint.*\(sha256:[0-9a-f]\{64\}\).*/\1/p' | tail -1)"
  [ -z "$TLS_CUSTOM" ] || crt="$CTLS_HOST_CRT"
  if [ -z "$fp" ] && [ -r "$crt" ] && command -v openssl >/dev/null; then
    fp="$(openssl x509 -in "$crt" -noout -fingerprint -sha256 2>/dev/null | sed 's/.*=//; s/://g' | tr 'A-F' 'a-f')"
    [ -z "$fp" ] || fp="sha256:$fp"
  fi
  printf '%s' "$fp"
}

# iter_data writes state/admin_password_changed once the first admin has chosen
# their own password; then the first-boot ITER_ADMIN_PASSWORD leaves the env file
admin_changed() {
  [ -f "$STATE/admin_password_changed" ] \
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
  st="$(docker inspect -f '{{.State.Status}}{{if .State.Health}} ({{.State.Health.Status}}){{end}} since {{.State.StartedAt}}' "$NAME" 2>/dev/null)" || st=missing
  printf '%-12s %s\n' container "$NAME: $st"
  port="$(docker port "$NAME" 8400/tcp 2>/dev/null | head -1 | sed 's/.*://' || true)"
  [ "$(envval ITER_TLS "$ENV_PATH")" != off ] || scheme=http
  TLS_CUSTOM="$(envval ITER_TLS_CERT "$ENV_PATH")"
  if [ -n "$port" ] && h="$(curl -fsSk "$scheme://127.0.0.1:$port/health" 2>/dev/null)"; then printf '%-12s %s\n' health "$scheme://127.0.0.1:$port/ $h"
  else printf '%-12s %s\n' health "no answer${port:+ on $scheme://127.0.0.1:$port}"; fi
  if [ "$scheme" = http ]; then printf '%-12s %s\n' tls "off (plain HTTP, behind a TLS proxy)"
  else fp="$(fingerprint)"; printf '%-12s %s\n' tls "$([ -n "$TLS_CUSTOM" ] && echo custom || echo auto), certificate fingerprint ${fp:-unknown}"; fi
  printf '%-12s %s\n' files "$D   secrets: $ENV_PATH"
  [ -z "$OLD_DATA$OLD_ENV" ] || [ -f "$ENV_PATH" ] || printf '%-12s %s\n' older "${OLD_DATA:-$OLD_ENV}: an install from before 2026-10-18; \`install.sh\` moves it into $D"
  admin_cleanup
}

# ---- where it goes: --iter-dir, else $ITER_DIR, else where the container's
# files already are (its /var/lib/iter_data mount), else the default below
CUR_D="" OLD_ENV="" OLD_BUILD="" VOLS=""
# the default iter dir: ~/iter under Docker Desktop (macOS, Windows, Linux), whose
# containers see only the folders shared with its VM (the home folder is, /opt
# is not); /opt/iter with Docker Engine on Linux, which mounts any host folder
default_iter_dir() {
  case "$(docker info --format '{{.OperatingSystem}}' 2>/dev/null || true)" in
    *"Docker Desktop"*) printf '%s' "$HOME/iter" ;;
    *) printf '%s' /opt/iter ;;
  esac
}
# the docker volume mounted at a path in the container NAME
vol_at() { docker inspect -f "{{range .Mounts}}{{if and (eq .Type \"volume\") (eq .Destination \"$1\")}}{{.Name}}{{end}}{{end}}" "$NAME" 2>/dev/null || true; }
discover() {
  local src
  # a hand-made setup with its database in named volumes (not this script's)
  VOL_DB="$(vol_at /var/lib/arangodb3)" VOL_APPS="$(vol_at /var/lib/arangodb3-apps)" VOL_ITER="$(vol_at /var/lib/iter)"
  VOLS="$VOL_DB$VOL_ITER"
  src="$(docker inspect -f '{{range .Mounts}}{{if eq .Destination "/var/lib/iter_data"}}{{.Source}}{{end}}{{end}}' "$NAME" 2>/dev/null || true)"
  if [ -n "$src" ] && [ -d "$src" ]; then
    # this layout has config/ or state/; the older one has iter/ and arango/ at the top
    if [ -d "$src/config" ] || [ -d "$src/state" ]; then CUR_D="$src"; else [ -n "$OLD_DATA" ] || OLD_DATA="$src"; fi
  fi
  if [ -n "$ITER_DIR_OPT" ]; then ITER_DIR="$(iter_dir_of "$ITER_DIR_OPT")"
  elif [ -n "$CUR_D" ]; then ITER_DIR="$(dirname "$CUR_D")"
  else ITER_DIR="$(default_iter_dir)"; fi
  set_paths
  # an install from before 2026-10-18: ~/.iter (or ~/.iter5, before that)
  local h
  for h in "$IHOME" "$HOME/.iter5"; do
    [ -n "$OLD_DATA" ] || [ ! -d "$h/iter_data" ] || OLD_DATA="$h/iter_data"
    [ -n "$OLD_ENV" ] || [ ! -f "$h/$NAME.docker.env" ] || OLD_ENV="$h/$NAME.docker.env"
    [ -n "$OLD_BUILD" ] || [ ! -d "$h/$NAME-docker" ] || OLD_BUILD="$h/$NAME-docker"
  done
  if [ -n "$OLD_DATA" ]; then
    OLD_DATA="${OLD_DATA%/}"
    case "$OLD_DATA" in "$D"|"$D"/*) OLD_DATA="" ;; esac
    [ -z "$OLD_DATA" ] || [ -d "$OLD_DATA" ] || die "--data-dir $OLD_DATA does not exist"
  fi
  [ -z "$CUR_D" ] || [ "$CUR_D" != "$D" ] || CUR_D=""
}

# ---- 1. docker
command -v docker >/dev/null || die "docker is not installed (https://docs.docker.com/engine/install/)"
docker info >/dev/null 2>&1 || die "docker is installed but the daemon does not answer (start Docker, or add yourself to the docker group)"
docker compose version >/dev/null 2>&1 || die "the docker compose plugin is missing (docker-compose-plugin)"
discover
if [ "$CMD" = status ]; then do_status; exit 0; fi
DARCH="$(docker info --format '{{.Architecture}}')"
case "$DARCH" in x86_64|amd64) ARCH=x86_64 ;; aarch64|arm64) ARCH=aarch64 ;; *) die "unsupported docker architecture $DARCH" ;; esac
ASSET="iter_data-linux-$ARCH"
say "docker $(docker version --format '{{.Server.Version}}') on $ARCH"

# ---- 2. the iter dir: <iter dir> (root's when it needs sudo), <iter dir>/iter_data
# (yours, owner-only: secrets live in it; the container, as root, writes state/,
# arango/ and backups/ into it)
# mv as you, or with sudo where the container (root) owns the source (a Linux host)
mvp() { if [ "$DRY" = 1 ]; then echo "  + mv $1 $2"; elif [ -O "$1" ]; then mv "$1" "$2"; else sudo mv "$1" "$2"; fi; }
# the nearest folder that exists: can this user make DIR below it?
can_make() { local d="$1"; while [ ! -d "$d" ]; do d="$(dirname "$d")"; done; [ -w "$d" ]; }
make_root() {
  [ ! -d "$ITER_DIR" ] || return 0
  if can_make "$ITER_DIR"; then run mkdir -p "$ITER_DIR"
  else run sudo install -d -m 755 "$ITER_DIR"; fi
}
make_dirs() {
  if [ ! -d "$D" ]; then
    if can_make "$D"; then run mkdir -p "$D"
    else run sudo install -d -m 700 -o "$(id -u)" -g "$(id -g)" "$D"; fi
  fi
  [ "$DRY" = 1 ] || [ -w "$D" ] || die "$D is not writable by $(id -un): chown it to yourself, or pick another --iter-dir"
  run chmod 700 "$D"
  run mkdir -p "$BUILD" "$CONF_DIR" "$D/arango" "$D/backups"
  run chmod 700 "$CONF_DIR"
}
# Docker Desktop shares only some host folders with its VM (macOS: /Users,
# /Volumes, /private, /tmp, /var/folders): a mount of anything else fails
check_share() {
  if [ "$DRY" = 1 ]; then echo "  + check that docker can mount $D (docker run --rm -v $D:/x debian:bookworm-slim true)"; return; fi
  docker run --rm -v "$D:/x" debian:bookworm-slim true >/dev/null 2>&1 && return 0
  docker run --rm -v "$D:/x" debian:bookworm-slim true 2>&1 | tail -3 >&2 || true
  die "docker cannot mount $D. Docker Desktop: add $ITER_DIR in Settings -> Resources -> File sharing (and Apply), or rerun with --iter-dir under your home (e.g. --iter-dir ~/iter)"
}
# the container stopped (not removed: compose recreates it) before its files move
stop_container() {
  [ "$(docker inspect -f '{{.State.Running}}' "$NAME" 2>/dev/null)" = true ] || return 0
  say "stopping container $NAME while its files move"
  run docker stop "$NAME"
}
# an install at another iter dir: its whole folder moves to this one
relocate() {
  [ -n "$CUR_D" ] || return 0
  [ ! -e "$D" ] || [ -z "$(ls -A "$D" 2>/dev/null)" ] || die "$NAME's files are in $CUR_D, and $D is not empty: move one of them yourself, or rerun with --iter-dir $(dirname "$CUR_D")"
  say "moving $CUR_D -> $D (the container's files, from its old iter dir)"
  stop_container
  if [ -d "$D" ]; then run rmdir "$D"; fi
  if [ -w "$ITER_DIR" ] && [ -O "$CUR_D" ]; then run mv "$CUR_D" "$D"; else run sudo mv "$CUR_D" "$D"; fi
}
# the container NAME kept its database in docker volumes: copied (never moved) into
# the iter dir with --adopt-volumes, else stop: a fresh database would start beside it
adopt_volumes() {
  [ -n "$VOLS" ] || return 0
  if [ "$ADOPT" = 0 ]; then
    die "container $NAME keeps its database in docker volumes (${VOL_DB:-none} for /var/lib/arangodb3, ${VOL_ITER:-none} for /var/lib/iter), not in a host folder: this script would start an empty database beside it. Rerun with --adopt-volumes (and --env-file with its ARANGO_ROOT_PASSWORD) to copy them into $D (the volumes stay as they are), or give another --name"
  fi
  [ ! -e "$D/arango/data/SERVER" ] || [ "$DRY" = 1 ] || die "$D/arango/data holds a database already: not copying volume $VOL_DB over it"
  say "copying docker volumes into $D (the volumes are kept)"
  stop_container
  local v to
  for v in "$VOL_DB:arango/data" "$VOL_APPS:arango/apps" "$VOL_ITER:state"; do
    to="${v#*:}"; v="${v%%:*}"; [ -n "$v" ] || continue
    run mkdir -p "$D/$to"
    run docker run --rm -v "$v:/from:ro" -v "$D/$to:/to" debian:bookworm-slim cp -a /from/. /to/
  done
}
# an install from before 2026-10-18: ~/.iter/iter_data (arango/, arango-apps/,
# iter/, custom-tls/, arango-ca/), ~/.iter/<name>.docker.env, ~/.iter/<name>-docker
migrate() {
  [ -n "$OLD_DATA$OLD_ENV$OLD_BUILD" ] || return 0
  if [ -f "$ENV_PATH" ]; then
    say "an older install's files are still in ${OLD_DATA:-$OLD_ENV} (not moved: $D already has its own); move or delete them yourself"
    return 0
  fi
  say "moving the install from before 2026-10-18 into $D"
  stop_container
  local f
  if [ -n "$OLD_DATA" ]; then
    [ ! -e "$D/arango/data" ] || [ "$DRY" = 1 ] || die "$D/arango/data exists already: not moving $OLD_DATA/arango over it"
    [ ! -e "$OLD_DATA/arango" ] || mvp "$OLD_DATA/arango" "$D/arango/data"
    [ ! -e "$OLD_DATA/arango-apps" ] || mvp "$OLD_DATA/arango-apps" "$D/arango/apps"
    [ ! -e "$OLD_DATA/iter" ] || mvp "$OLD_DATA/iter" "$STATE"
    [ ! -f "$OLD_DATA/custom-tls/tls.crt" ] || mvp "$OLD_DATA/custom-tls/tls.crt" "$CTLS_HOST_CRT"
    [ ! -f "$OLD_DATA/custom-tls/tls.key" ] || mvp "$OLD_DATA/custom-tls/tls.key" "$CTLS_HOST_KEY"
    [ ! -f "$OLD_DATA/arango-ca/ca.pem" ] || mvp "$OLD_DATA/arango-ca/ca.pem" "$CONF_DIR/arango-ca.crt"
    for f in custom-tls arango-ca; do [ ! -d "$OLD_DATA/$f" ] || run rmdir "$OLD_DATA/$f" 2>/dev/null || true; done
    # anything else in it (backups, your own files) is kept, in old-dot-iter/iter_data/
    for f in "$OLD_DATA"/* "$OLD_DATA"/.[!.]*; do
      [ -e "$f" ] || continue
      case "${f##*/}" in arango|arango-apps|iter|custom-tls|arango-ca) continue ;; esac
      run mkdir -p "$OLD_KEEP/iter_data"
      mvp "$f" "$OLD_KEEP/iter_data/"
    done
    run rmdir "$OLD_DATA" || say "left $OLD_DATA (not empty)"
  fi
  if [ -n "$OLD_ENV" ]; then
    # the paths inside the container moved with the files
    if [ "$DRY" = 1 ]; then echo "  + $OLD_ENV -> $ENV_PATH (container paths of custom-tls/ and arango-ca/ rewritten)"
    else
      ( umask 077; sed -e 's#/var/lib/iter_data/custom-tls/tls\.crt#/var/lib/iter_data/config/custom-tls.crt#' \
          -e 's#/var/lib/iter_data/custom-tls/tls\.key#/var/lib/iter_data/config/custom-tls.key#' \
          -e 's#/var/lib/iter_data/arango-ca/ca\.pem#/var/lib/iter_data/config/arango-ca.crt#' "$OLD_ENV" > "$ENV_PATH" )
      mkdir -p "$OLD_KEEP"; mv -f "$OLD_ENV" "$OLD_KEEP/"
    fi
  fi
  # the old build folder is made again in docker/; kept aside, not deleted
  [ -z "$OLD_BUILD" ] || { run mkdir -p "$OLD_KEEP"; mvp "$OLD_BUILD" "$OLD_KEEP/"; }
  say "kept aside, not deleted: $OLD_KEEP (the old secrets file and build folder; delete it once all is well)"
  say "moved: database $D/arango/data, iter_data's files $D/state, secrets $ENV_PATH"
}
say "iter dir: $ITER_DIR   (iter_data's files: $D)"
make_root
relocate
make_dirs
check_share
adopt_volumes
migrate

# the build folder: this folder's files (a checkout or a downloaded copy), else
# (piped from curl) fetched from GitHub at --version
if [ -f "$HERE/Dockerfile" ] && [ -f "$HERE/compose.yml" ]; then
  say "build folder $BUILD (files from $HERE)"
  if [ "$HERE" != "$BUILD" ]; then for f in "${FILES[@]}"; do run cp "$HERE/$f" "$BUILD/$f"; done; fi
else
  ref="${VERSION:-main}"
  say "no Dockerfile beside the script: fetching this folder ($ref) into $BUILD"
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
elif [ -f "$HERE/$ASSET" ] && [ -z "$VERSION" ]; then
  say "binary: $HERE/$ASSET (beside the script) -> $BUILD/$ASSET"
  [ "$HERE" = "$BUILD" ] || { run cp "$HERE/$ASSET" "$BUILD/.$ASSET.tmp"; run mv -f "$BUILD/.$ASSET.tmp" "$BUILD/$ASSET"; }
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
# (in the build folder: the image carries it at /opt/iter/models)
MDIR="$BUILD/models/all-MiniLM-L6-v2"
[ -n "$MODEL_SRC" ] || [ ! -f "$HERE/models/all-MiniLM-L6-v2/model.safetensors" ] || [ "$HERE" = "$BUILD" ] || MODEL_SRC="$HERE/models/all-MiniLM-L6-v2"
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

# ---- 5. secrets
say "secrets: $ENV_PATH"
# true when PATH exists, or might: on a Linux host the container makes state/ owner-only
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
A_CA_IN=/var/lib/iter_data/config/arango-ca.crt
if [ -z "$A_URL" ]; then
  [ -z "$A_FP_OPT$A_CA_SRC" ] || die "--arango-fingerprint / --arango-ca-cert are for an external ArangoDB (--arango-url)"
  [ -n "$A_ROOT" ] || A_ROOT="$STASH_ROOT"
  if [ -z "$A_ROOT" ]; then
    if present "$D/arango/data/SERVER"; then
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
    say "ArangoDB CA: $A_CA_SRC (copied to $CONF_DIR/arango-ca.crt)"
    run install -m 644 "$A_CA_SRC" "$CONF_DIR/arango-ca.crt"
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
  if present "$STATE/iter_data.secret"; then
    say "no ITER_JWT_SECRET given: iter_data keeps using $STATE/iter_data.secret"
  else
    JWT="$(gen 48)"; say "generated a new ITER_JWT_SECRET"
  fi
fi
if [ -z "$ADMIN_PW" ]; then
  # iter_data only reads it when it creates admin, i.e. on an empty database
  if [ -z "$A_URL" ] && present "$D/arango/data/SERVER"; then say "existing database: admin keeps its password"
  elif [ -n "$A_URL" ] && present "$STATE/iter_data.secret"; then say "iter_data has run here before: admin keeps its password"
  else ADMIN_PW="$(gen 20)"; FRESH_ADMIN=1; fi
fi

# TLS: the flags given now, else what the secrets file kept from an earlier run.
# A certificate of your own is copied to config/custom-tls.crt + .key (owner-only)
# and read by iter_data at /var/lib/iter_data/config/ inside the container.
CTLS_CERT=/var/lib/iter_data/config/custom-tls.crt
CTLS_KEY=/var/lib/iter_data/config/custom-tls.key
TLS_OFF="$(pick ITER_TLS)"; [ "$TLS_OFF" = off ] || TLS_OFF=""
TLS_CUSTOM="$(pick ITER_TLS_CERT)"
NAMES="$(pick ITER_TLS_NAMES)"
case "$TLS_MODE" in
  off) TLS_OFF=off; TLS_CUSTOM="" ;;
  auto) TLS_OFF=""; TLS_CUSTOM="" ;;
esac
if [ -n "$TLS_CERT_SRC" ]; then
  TLS_OFF=""; TLS_CUSTOM="$CTLS_CERT"
  say "TLS: serving your certificate $TLS_CERT_SRC (copied to $CTLS_HOST_CRT)"
  run install -m 644 "$TLS_CERT_SRC" "$CTLS_HOST_CRT"
  run install -m 600 "$TLS_KEY_SRC" "$CTLS_HOST_KEY"
fi
if [ ${#TLS_NAMES[@]} -gt 0 ]; then NAMES="$(IFS=,; echo "${TLS_NAMES[*]}")"; fi
if [ "$TLS_OFF" = off ]; then say "TLS: off (plain HTTP): only for iter_data behind a TLS proxy; engines refuse plain http across a network"
elif [ -n "$TLS_CUSTOM" ]; then [ -n "$TLS_CERT_SRC" ] || say "TLS: your certificate $CTLS_HOST_CRT (kept from the last run)"
else say "TLS: a self-signed certificate (made on first start, kept in $STATE)${NAMES:+, extra names $NAMES}"; fi
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
export ITER_NAME="$NAME" ITER_PORT="$PORT" ARANGO_HOST_PORT="$ARANGO_PORT" ITER_DATA_DIR="$D" \
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
