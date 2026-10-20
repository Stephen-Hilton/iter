#!/usr/bin/env bash
# Deploy iter locally.
#
#   ./deploy.sh docker [installer options]
#                        the all-in-one container (ArangoDB CE + iter_data), https://127.0.0.1:8400:
#                        builds iter_data-linux-<arch> with installers/build.sh (a bullseye
#                        container), then runs installers/iter_data/docker/install.sh (extra
#                        options go to it, e.g. --adopt-volumes for a container on docker volumes)
#   ./deploy.sh local    native iter_data (release build) against a dev Arango container on :8529
#
# The container keeps everything that persists (database, Arango apps,
# iter_data's files) in one host folder, <iter dir>/iter_data, with the iter dir
# ITER_DIR [~/iter: Docker Desktop shares the home folder, not /opt]. An older
# data folder (ITER_DATA_DIR, or ~/.iter/iter_data) is moved in.
# Secrets (ITER_ADMIN_PASSWORD, ITER_JWT_SECRET) come from ITER_ENV_FILE,
# default the repo root .env.
# Release binaries land in bin/<os>-<arch>/ (rm+cp, never cp-over: macOS kills
# a binary copied over a live one). On Windows use deploy.ps1 instead.
set -euo pipefail

MODE="${1:-docker}"
[ $# = 0 ] || shift
ROOT="$(cd "$(dirname "$0")" && pwd)"
case "$(uname -s)" in
  Linux*) OS=linux ;; Darwin*) OS=macos ;; MINGW*|MSYS*|CYGWIN*) OS=windows ;; *) OS="$(uname -s | tr A-Z a-z)" ;;
esac
ARCH="$(uname -m)"; [ "$ARCH" = arm64 ] && ARCH=aarch64
EXE=""; [ "$OS" = windows ] && EXE=".exe"
BIN="$ROOT/bin/$OS-$ARCH"
RUN="$ROOT/run"
PORT="${ITER_PORT:-8400}"
ENV_FILE="${ITER_ENV_FILE:-$ROOT/.env}"
ARANGO_PW="${ARANGO_ROOT_PASSWORD:-iterdev}"
ITER_DIR="${ITER_DIR:-$HOME/iter}"
# an older data folder to move into the iter dir, when there is one
OLD_DATA="${ITER_DATA_DIR:-}"
[ -n "$OLD_DATA" ] || [ ! -d "$HOME/.iter/iter_data" ] || OLD_DATA="$HOME/.iter/iter_data"
CARGO="${CARGO:-$HOME/.cargo/bin/cargo}"
mkdir -p "$BIN" "$RUN"

# pull single keys out of the .env without sourcing it (it has lines a shell chokes on)
envval() { [ -f "$ENV_FILE" ] && grep -E "^$1=" "$ENV_FILE" | tail -1 | cut -d= -f2- | sed -e 's/^["'\'']//' -e 's/["'\'']$//' || true; }

build_native() {
  echo "[deploy] building release binaries"
  (cd "$ROOT" && "$CARGO" build --release -p iter_data -p iter_engine)
  for b in iter_data iter_engine; do
    rm -f "$BIN/$b$EXE"
    cp "$ROOT/target/release/$b$EXE" "$BIN/$b$EXE"
  done
  echo "[deploy] binaries -> $BIN"
}

stop_native() {
  if [ -f "$RUN/iter_data.pid" ] && kill -0 "$(cat "$RUN/iter_data.pid")" 2>/dev/null; then
    echo "[deploy] stopping native iter_data ($(cat "$RUN/iter_data.pid"))"
    kill "$(cat "$RUN/iter_data.pid")" || true
    sleep 1
  fi
}

# SCHEME: local mode is plain http on loopback (--tls off); docker serves its
# self-signed certificate (installers/iter_data/docker)
SCHEME=http
wait_health() {
  for i in $(seq 1 120); do
    if curl -sfk "$SCHEME://127.0.0.1:$PORT/health" 2>/dev/null | grep -q '"ok":true'; then
      echo "[deploy] iter_data up: $SCHEME://127.0.0.1:$PORT  $(curl -sk "$SCHEME://127.0.0.1:$PORT/health")"
      return 0
    fi
    sleep 1
  done
  echo "[deploy] iter_data did not come up"; return 1
}

case "$MODE" in
  docker)
    stop_native
    # without it iter_data mints a new secret into the data folder and every
    # existing token (engines, users) stops verifying
    [ -n "$(envval ITER_JWT_SECRET)" ] || { echo "[deploy] ITER_JWT_SECRET is not set in $ENV_FILE (set ITER_ENV_FILE)" >&2; exit 1; }
    # the image takes a prebuilt glibc-2.31 linux binary for the docker host's CPU
    DARCH="$(docker info --format '{{.Architecture}}')"; [ "$DARCH" = arm64 ] && DARCH=aarch64; [ "$DARCH" = amd64 ] && DARCH=x86_64
    echo "[deploy] building iter_data-linux-$DARCH (installers/build.sh, placed in installers/iter_data/docker/)"
    "$ROOT/installers/build.sh" --only "iter_data-linux-$DARCH"
    "$ROOT/installers/fetch_model.sh"
    echo "[deploy] building + starting the iter container (data: $ITER_DIR/iter_data)"
    # secrets (ITER_ADMIN_PASSWORD, ITER_JWT_SECRET) from $ENV_FILE -> <iter dir>/iter_data/config/data.env;
    # the Arango root password from ARANGO_ROOT_PASSWORD [iterdev] unless that file or $ENV_FILE has one
    ARANGO_ROOT_PASSWORD="$ARANGO_PW" "$ROOT/installers/iter_data/docker/install.sh" \
      --env-file "$ENV_FILE" --port "$PORT" --iter-dir "$ITER_DIR" ${OLD_DATA:+--data-dir "$OLD_DATA"} --name iter \
      --arango-port "${ARANGO_HOST_PORT:-8630}" --model-dir "$ROOT/models/all-MiniLM-L6-v2" ${@+"$@"}
    SCHEME=https
    ;;
  local)
    build_native
    "$ROOT/installers/fetch_model.sh"
    stop_native
    if ! docker ps --format '{{.Names}}' | grep -qx iter-arango-dev; then
      docker start iter-arango-dev 2>/dev/null || docker run -d --name iter-arango-dev -p 127.0.0.1:8529:8529 \
        -e ARANGO_ROOT_PASSWORD="$ARANGO_PW" -v iter-arango-dev:/var/lib/arangodb3 arangodb:3.12 arangod --vector-index >/dev/null
    fi
    # iter_data signs in as iter_user (rw on iter_db only); root sets that up on
    # the first start. The user's password is made once and kept in run/.
    [ -s "$RUN/arango_user.pw" ] || { LC_ALL=C tr -dc 'A-Za-z0-9' </dev/urandom | head -c 24 > "$RUN/arango_user.pw"; chmod 600 "$RUN/arango_user.pw"; }
    ARANGO_URL=http://127.0.0.1:8529 ARANGO_ROOT_PASSWORD="$ARANGO_PW" ARANGO_USER=iter_user ARANGO_DB=iter_db \
      ARANGO_PASSWORD="$(cat "$RUN/arango_user.pw")" nohup "$BIN/iter_data" --backend arango \
      --listen "127.0.0.1:$PORT" --secret-file "$RUN/iter_data.secret" --env-file "$ENV_FILE" --tls off \
      --webui-dir "$ROOT/iter_data/webui" --embed-model "$ROOT/models/all-MiniLM-L6-v2" > "$RUN/iter_data.log" 2>&1 &
    echo $! > "$RUN/iter_data.pid"
    wait_health || { tail -20 "$RUN/iter_data.log"; exit 1; }
    ;;
  *)
    echo "usage: $0 docker|local"; exit 2 ;;
esac
echo "[deploy] webui: $SCHEME://127.0.0.1:$PORT/  (login: admin / ITER_ADMIN_PASSWORD)"
