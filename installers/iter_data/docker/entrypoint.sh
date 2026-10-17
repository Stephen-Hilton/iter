#!/bin/bash
# Run arangod (through the ArangoDB image's own entrypoint, which initialises the
# root password on first start) and iter_data side by side; if either exits, stop
# the container so Docker's restart policy brings both back together.
#
# ARANGO_URL on another machine (not 127.0.0.1 / localhost / ::1): an existing
# ArangoDB elsewhere. Then arangod is not started at all and only iter_data runs;
# iter_data reads ARANGO_USER, ARANGO_PASSWORD, ARANGO_DB, ARANGO_ROOT_PASSWORD,
# ARANGO_CERT_FINGERPRINT, ARANGO_CA_CERT and ARANGO_ALLOW_PLAIN_HTTP itself.
set -u

ARANGO_URL="${ARANGO_URL:-http://127.0.0.1:8529}"
export ARANGO_URL
export ARANGO_USER="${ARANGO_USER:-iter_user}"
# the one data mount (a host folder in compose.yml); /var/lib/arangodb3,
# /var/lib/arangodb3-apps and /var/lib/iter are links into it
mkdir -p /var/lib/iter_data/iter
# iter_data's own files (iter_data.secret, engine_session.secret, tls.key,
# arango_user.pw) are owner-only, and so is the folder that holds them
chmod 700 /var/lib/iter_data/iter

host="${ARANGO_URL#*://}"; host="${host%%/*}"
case "$host" in \[*) host="${host%%]*}"; host="${host#[}" ;; *) host="${host%%:*}" ;; esac
case "$host" in 127.*|localhost|::1) LOCAL=1 ;; *) LOCAL=0 ;; esac

ARANGO_PID=""
if [ "$LOCAL" = 1 ]; then
  if [ -z "${ARANGO_ROOT_PASSWORD:-}" ] && [ -z "${ARANGO_NO_AUTH:-}" ] && [ -z "${ARANGO_RANDOM_ROOT_PASSWORD:-}" ]; then
    echo "[iter_data] set ARANGO_ROOT_PASSWORD (or ARANGO_NO_AUTH=1): refusing to start an unauthenticated guess" >&2
    exit 2
  fi
  mkdir -p /var/lib/iter_data/arango /var/lib/iter_data/arango-apps
  # iter_data signs in as its own user (rw on its database only); root creates it
  # on the first start. Its password is never root's: without one of its own, one
  # is made once and kept beside iter_data's other secrets.
  if [ -z "${ARANGO_PASSWORD:-}" ] || [ "${ARANGO_PASSWORD:-}" = "${ARANGO_ROOT_PASSWORD:-}" ]; then
    pwf=/var/lib/iter_data/iter/arango_user.pw
    if [ ! -s "$pwf" ]; then
      (umask 077; LC_ALL=C tr -dc 'A-Za-z0-9' </dev/urandom | head -c 24 > "$pwf")
    fi
    chmod 600 "$pwf"
    ARANGO_PASSWORD="$(cat "$pwf")"
  fi
  export ARANGO_PASSWORD

  # No endpoint argument: the image config already listens on 0.0.0.0:8529, and the
  # image entrypoint passes our arguments to its first-run init server too (an
  # endpoint here made that throwaway server answer on 8529, and iter_data then
  # created its database on it). --vector-index: GraphRAG's vector indexes
  # (ArangoDB 3.12.4+); without it iter_data falls back to exact cosine search.
  # The ArangoDB entrypoint is written for busybox sh (it uses `let`): run it with bash.
  bash /usr/local/bin/arango-entrypoint.sh arangod --vector-index &
  ARANGO_PID=$!

  # iter_data waits for arangod itself (up to 60s); first-run init can take longer
  for _ in $(seq 1 120); do
    curl -fsS -u "root:${ARANGO_ROOT_PASSWORD:-}" http://127.0.0.1:8529/_api/version >/dev/null 2>&1 && break
    kill -0 "$ARANGO_PID" 2>/dev/null || { echo "[iter_data] arangod exited during startup" >&2; exit 1; }
    sleep 1
  done

  # Images before 2026-10-17 kept iter's data in database iter5 (ARANGO_DB=iter5 in
  # the image); with no ARANGO_DB given, keep using it while iter_db does not exist.
  if [ -z "${ARANGO_DB:-}" ]; then
    dbs="$(curl -fsS -u "root:${ARANGO_ROOT_PASSWORD:-}" http://127.0.0.1:8529/_api/database 2>/dev/null || true)"
    if grep -q '"iter5"' <<<"$dbs" && ! grep -q '"iter_db"' <<<"$dbs"; then
      export ARANGO_DB=iter5
      echo "[iter_data] using the existing database iter5 (from before iter_db; set ARANGO_DB to choose)"
    fi
  fi
else
  echo "[iter_data] ArangoDB at $ARANGO_URL: not starting arangod in this container"
fi

# HTTPS (iter_data's default, --tls auto): a self-signed certificate made on first
# start beside the secret file. ITER_TLS=off (read by iter_data itself) serves plain
# HTTP; ITER_TLS_CERT + ITER_TLS_KEY serve a certificate of your own; ITER_TLS_NAMES
# (comma- or space-separated) adds names to the self-signed one.
tls=()
[ -z "${ITER_TLS_CERT:-}" ] || tls+=(--tls-cert "$ITER_TLS_CERT")
[ -z "${ITER_TLS_KEY:-}" ] || tls+=(--tls-key "$ITER_TLS_KEY")
IFS=', ' read -r -a names <<< "${ITER_TLS_NAMES:-}"
for n in "${names[@]}"; do [ -z "$n" ] || tls+=(--tls-name "$n"); done

iter_data --backend arango \
  --listen "${ITER_LISTEN:-0.0.0.0:8400}" \
  --secret-file /var/lib/iter/iter_data.secret \
  --env-file /var/lib/iter/.env \
  "${tls[@]}" "$@" &
ITER_PID=$!

pids=("$ITER_PID"); [ -z "$ARANGO_PID" ] || pids+=("$ARANGO_PID")
trap 'kill "${pids[@]}" 2>/dev/null; wait; exit 0' TERM INT
alive() { kill -0 "$1" 2>/dev/null && echo yes || echo no; }
if [ -z "$ARANGO_PID" ]; then
  wait "$ITER_PID"
  echo "[iter_data] iter_data exited; stopping" >&2
  exit 1
fi
while kill -0 "$ARANGO_PID" 2>/dev/null && kill -0 "$ITER_PID" 2>/dev/null; do
  sleep 2
done
echo "[iter_data] a process exited (arangod alive: $(alive "$ARANGO_PID"), iter_data alive: $(alive "$ITER_PID")); stopping" >&2
kill "${pids[@]}" 2>/dev/null
wait
exit 1
