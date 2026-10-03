#!/usr/bin/env bash
# The iter5 webui Playwright suite (e2e/playwright/run.sh: builds iter_data,
# starts it on a random port against a throwaway ArangoDB database, seeds it,
# runs every spec) as a test-node script (docs/iter5_spec.md §3.5): streams
# the list reporter's output, then prints the standard result JSON as the LAST
# line — `normal` = specs passed / failed (flaky counts as passed), details =
# one row per spec. Exit 0 green / 1 red / 2 could not run (no ArangoDB on
# :8529, no node / cargo, or the run ended without a summary).
# Env: ITER_TEST_NODE_NAME / ITER_TEST_NODE_ID (set by `iter runtests`),
# ITER5_TEST_ARANGO_URL / ITER5_TEST_ARANGO_PASSWORD, PW_ARGS (extra playwright args).
set -u
here="$(cd "$(dirname "$0")/.." && pwd)"   # iter5/e2e
name="${ITER_TEST_NODE_NAME:-webui Playwright suite}"; id="${ITER_TEST_NODE_ID:-}"
export PATH="$HOME/.cargo/bin:/opt/homebrew/bin:$PATH"
result() {  # result <ok> <pass> <fail> [details-json]
  local d="${4:-}"
  printf '{"name":"%s","id":"%s","overall_success":%s,"normal":{"total":%d,"pass":%d,"err":%d},"longtail":{"total":0,"pass":0,"err":0},"failure":{"total":0,"pass":0,"err":0}%s}\n' \
    "$name" "$id" "$1" "$(( $2 + $3 ))" "$2" "$3" "${d:+,\"details\":$d}"
}
aurl="${ITER5_TEST_ARANGO_URL:-http://127.0.0.1:8529}"
apw="${ITER5_TEST_ARANGO_PASSWORD:-${ARANGO_ROOT_PASSWORD:-iter4dev}}"
for t in node cargo curl; do command -v "$t" >/dev/null || { echo "$t is required"; result false 0 0; exit 2; }; done
if ! curl -sf -o /dev/null -u "root:$apw" "$aurl/_api/version"; then
  echo "no ArangoDB at $aurl — the Playwright suite needs the dev container (./deploy.sh local)"; result false 0 0; exit 2
fi
log="${ITER_TEST_OUT:-$(mktemp -d)}/playwright.log"
# shellcheck disable=SC2086
FORCE_COLOR=0 bash "$here/playwright/run.sh" --reporter=list ${PW_ARGS:-} 2>&1 | tee "$log"
code=${PIPESTATUS[0]}
plain="$(sed -e 's/\x1b\[[0-9;]*m//g' "$log")"
num() { echo "$plain" | grep -E "^ +[0-9]+ $1( |\$)" | tail -1 | awk '{print $1}'; }
passed=$(num passed); failed=$(num failed); flaky=$(num flaky)
passed=$(( ${passed:-0} + ${flaky:-0} )); failed=${failed:-0}
if [ $((passed + failed)) -eq 0 ]; then
  echo "playwright (exit $code) ended without a summary"; result false 0 0; exit 2
fi
details="$(echo "$plain" | awk '
  function js(s) { gsub(/\\/, "\\\\", s); gsub(/"/, "\\\"", s); gsub(/\t/, " ", s); return s }
  /^ +(✓|ok) +[0-9]+ / || /^ +(✘|x) +[0-9]+ / {
    ok = ($0 ~ /^ +(✓|ok) /); s = $0; sub(/^ +[^ ]+ +[0-9]+ +/, "", s); sub(/ \([0-9.]+m?s\)$/, "", s)
    if (n < 500) out = out (n ? "," : "") sprintf("{\"name\":\"%s\",\"bucket\":\"normal\",\"pass\":%s,\"msg\":\"%s\"}", js(s), (ok ? "true" : "false"), (ok ? "" : "failed"))
    n++
  }
  END { if (out) print "[" out "]" }')"
if [ "$code" -eq 0 ] && [ "$failed" -eq 0 ]; then result true "$passed" 0 "$details"; exit 0; fi
result false "$passed" "$failed" "$details"
[ "$failed" -gt 0 ] && exit 1
exit 2
