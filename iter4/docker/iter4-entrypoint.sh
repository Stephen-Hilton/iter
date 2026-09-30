#!/bin/sh
# Run arangod (through the image's own entrypoint, which initialises the root
# password on first start) and iter_data side by side; if either exits, stop
# the container so Docker's restart policy brings both back together.
set -u

if [ -z "${ARANGO_ROOT_PASSWORD:-}" ] && [ -z "${ARANGO_NO_AUTH:-}" ] && [ -z "${ARANGO_RANDOM_ROOT_PASSWORD:-}" ]; then
  echo "[iter4] set ARANGO_ROOT_PASSWORD (or ARANGO_NO_AUTH=1) — refusing to start an unauthenticated guess" >&2
  exit 2
fi
export ARANGO_PASSWORD="${ARANGO_PASSWORD:-${ARANGO_ROOT_PASSWORD:-}}"

# no endpoint argument: the image config already listens on 0.0.0.0:8529, and
# the image entrypoint passes our arguments to its first-run init server too —
# an endpoint here made that throwaway server answer on 8529 (iter_data then
# created its database on it). --vector-index is harmless on that server.
# --vector-index: GraphRAG's vector indexes (ArangoDB 3.12.4+); without it
# iter_data falls back to exact cosine search
/entrypoint.sh arangod --vector-index &
ARANGO_PID=$!

# iter_data waits for arangod itself (up to 60s); first-run init can take longer
for i in $(seq 1 120); do
  wget -qO- --header "Authorization: Basic $(printf 'root:%s' "$ARANGO_PASSWORD" | base64)" \
    http://127.0.0.1:8529/_api/version >/dev/null 2>&1 && break
  kill -0 "$ARANGO_PID" 2>/dev/null || { echo "[iter4] arangod exited during startup" >&2; exit 1; }
  sleep 1
done

iter_data --backend arango \
  --listen "${ITER_LISTEN:-0.0.0.0:8300}" \
  --secret-file /var/lib/iter/iter_data.secret \
  --env-file /var/lib/iter/.env \
  "$@" &
ITER_PID=$!

trap 'kill "$ITER_PID" "$ARANGO_PID" 2>/dev/null; wait; exit 0' TERM INT
while kill -0 "$ARANGO_PID" 2>/dev/null && kill -0 "$ITER_PID" 2>/dev/null; do
  sleep 2
done
echo "[iter4] a process exited (arangod alive: $(kill -0 $ARANGO_PID 2>/dev/null && echo yes || echo no), iter_data alive: $(kill -0 $ITER_PID 2>/dev/null && echo yes || echo no)); stopping" >&2
kill "$ITER_PID" "$ARANGO_PID" 2>/dev/null
wait
exit 1
