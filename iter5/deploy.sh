#!/usr/bin/env bash
# Deploy iter5 locally.
#
#   ./deploy.sh docker   the all-in-one container (ArangoDB CE + iter_data), http://127.0.0.1:8400
#   ./deploy.sh local    native iter_data (release build) against a dev Arango container on :8529
#
# Secrets (ITER_ADMIN_PASSWORD, ITER_JWT_SECRET) come from ITER_ENV_FILE,
# default ../.env (the repo .env iter3 already uses, so tokens work on both).
# Engine binaries always land in bin/ (rm+cp, never cp-over: macOS kills a
# binary copied over a live one).
set -euo pipefail

MODE="${1:-docker}"
ROOT="$(cd "$(dirname "$0")" && pwd)"
BIN="$ROOT/bin"
RUN="$ROOT/run"
PORT="${ITER_PORT:-8400}"
ENV_FILE="${ITER_ENV_FILE:-$ROOT/../.env}"
ARANGO_PW="${ARANGO_ROOT_PASSWORD:-iter4dev}"
CARGO="${CARGO:-$HOME/.cargo/bin/cargo}"
mkdir -p "$BIN" "$RUN"

# pull single keys out of the .env without sourcing it (it has lines a shell chokes on)
envval() { [ -f "$ENV_FILE" ] && grep -E "^$1=" "$ENV_FILE" | tail -1 | cut -d= -f2- | sed -e 's/^["'\'']//' -e 's/["'\'']$//' || true; }

build_native() {
  echo "[deploy] building release binaries"
  (cd "$ROOT" && "$CARGO" build --release -p iter_data -p iter_engine)
  for b in iter_data iter_engine; do
    rm -f "$BIN/$b"
    cp "$ROOT/target/release/$b" "$BIN/$b"
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

wait_health() {
  for i in $(seq 1 120); do
    if curl -sf "http://127.0.0.1:$PORT/health" 2>/dev/null | grep -q '"ok":true'; then
      echo "[deploy] iter_data up: http://127.0.0.1:$PORT  $(curl -s "http://127.0.0.1:$PORT/health")"
      return 0
    fi
    sleep 1
  done
  echo "[deploy] iter_data did not come up"; return 1
}

case "$MODE" in
  docker)
    stop_native
    umask 077
    {
      echo "ITER_ADMIN_PASSWORD=$(envval ITER_ADMIN_PASSWORD)"
      echo "ITER_JWT_SECRET=$(envval ITER_JWT_SECRET)"
    } > "$RUN/docker.env"
    "$ROOT/tools/fetch_model.sh"
    echo "[deploy] building + starting the iter5 container"
    (cd "$ROOT/docker" && ARANGO_ROOT_PASSWORD="$ARANGO_PW" ITER_PORT="$PORT" docker compose up -d --build)
    wait_health || { docker logs --tail 40 iter5; exit 1; }
    echo "[deploy] Arango console: http://127.0.0.1:${ARANGO_HOST_PORT:-8630}/ (root / \$ARANGO_ROOT_PASSWORD)"
    ;;
  local)
    build_native
    stop_native
    if ! docker ps --format '{{.Names}}' | grep -qx iter4-arango-dev; then
      docker start iter4-arango-dev 2>/dev/null || docker run -d --name iter4-arango-dev -p 127.0.0.1:8529:8529 \
        -e ARANGO_ROOT_PASSWORD="$ARANGO_PW" -v iter4-arango-dev:/var/lib/arangodb3 arangodb:3.12 arangod --vector-index >/dev/null
    fi
    ARANGO_URL=http://127.0.0.1:8529 ARANGO_PASSWORD="$ARANGO_PW" nohup "$BIN/iter_data" --backend arango \
      --listen "127.0.0.1:$PORT" --secret-file "$RUN/iter_data.secret" --env-file "$ENV_FILE" \
      --webui-dir "$ROOT/webui" > "$RUN/iter_data.log" 2>&1 &
    echo $! > "$RUN/iter_data.pid"
    wait_health || { tail -20 "$RUN/iter_data.log"; exit 1; }
    ;;
  *)
    echo "usage: $0 docker|local"; exit 2 ;;
esac
echo "[deploy] webui: http://127.0.0.1:$PORT/  (login: admin / ITER_ADMIN_PASSWORD)"
