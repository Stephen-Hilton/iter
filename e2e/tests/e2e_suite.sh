#!/usr/bin/env bash
# The iter5 end-to-end suite (e2e.sh: iter_data + one engine serving two
# projects, mock provider) as a test-node script (docs/iter5_spec.md §3.5):
# streams e2e.sh's output, then prints the standard result JSON as the LAST
# line — `normal` = e2e.sh's tally (every PASS / FAIL check), details = one row
# per section plus one per failed check. Exit 0 green / 1 red / 2 could not run
# (no ArangoDB on :8529, no jq / cargo, or e2e.sh ended without its tally).
# Env: ITER_TEST_NODE_NAME / ITER_TEST_NODE_ID (set by `iter runtests`),
# ITER5_TEST_ARANGO_URL / ITER5_TEST_ARANGO_PASSWORD, E2E_ARGS (e.g. "--live").
set -u
root="$(cd "$(dirname "$0")/../.." && pwd)"   # the iter5 directory
name="${ITER_TEST_NODE_NAME:-iter5 end-to-end}"; id="${ITER_TEST_NODE_ID:-}"
export PATH="$HOME/.cargo/bin:/opt/homebrew/bin:$PATH"
result() {  # result <ok> <pass> <fail> [details-json]
  local d="${4:-}"
  printf '{"name":"%s","id":"%s","overall_success":%s,"normal":{"total":%d,"pass":%d,"err":%d},"longtail":{"total":0,"pass":0,"err":0},"failure":{"total":0,"pass":0,"err":0}%s}\n' \
    "$name" "$id" "$1" "$(( $2 + $3 ))" "$2" "$3" "${d:+,\"details\":$d}"
}
aurl="${ITER5_TEST_ARANGO_URL:-http://127.0.0.1:8529}"
apw="${ITER5_TEST_ARANGO_PASSWORD:-${ARANGO_ROOT_PASSWORD:-iter4dev}}"
for t in jq cargo curl; do command -v "$t" >/dev/null || { echo "$t is required"; result false 0 0; exit 2; }; done
if ! curl -sf -o /dev/null -u "root:$apw" "$aurl/_api/version"; then
  echo "no ArangoDB at $aurl — e2e.sh needs the dev container (./deploy.sh local)"; result false 0 0; exit 2
fi
log="${ITER_TEST_OUT:-$(mktemp -d)}/e2e.log"
# shellcheck disable=SC2086
bash "$root/e2e.sh" ${E2E_ARGS:-} 2>&1 | tee "$log"
code=${PIPESTATUS[0]}
plain="$(sed -e 's/\x1b\[[0-9;]*m//g' "$log")"
total_line="$(echo "$plain" | grep -E '^  TOTAL +pass +[0-9]+ +fail +[0-9]+' | tail -1)"
if [ -z "$total_line" ]; then
  echo "e2e.sh (exit $code) ended without its tally"; result false 0 0; exit 2
fi
pass=$(echo "$total_line" | awk '{print $3}'); fail=$(echo "$total_line" | awk '{print $5}')
details="$(echo "$plain" | awk '
  function js(s) { gsub(/\\/, "\\\\", s); gsub(/"/, "\\\"", s); gsub(/\t/, " ", s); return s }
  /^== tally/ { t = 1; next }
  t && /^  TOTAL / { t = 0; next }
  t && /^  .* pass +[0-9]+ +fail +[0-9]+$/ {
    n = split($0, a, " "); f = a[n]; p = a[n-2]; sec = $0; sub(/ +pass +[0-9]+ +fail +[0-9]+$/, "", sec); sub(/^ +/, "", sec)
    out = out (out ? "," : "") sprintf("{\"name\":\"%s\",\"bucket\":\"normal\",\"pass\":%s,\"msg\":\"pass %d fail %d\"}", js("section: " sec), (f == 0 ? "true" : "false"), p, f)
    next
  }
  /^FAIL / { m = substr($0, 6); out = out (out ? "," : "") sprintf("{\"name\":\"%s\",\"bucket\":\"normal\",\"pass\":false,\"msg\":\"failed\"}", js(substr(m, 1, 200))) }
  END { if (out) print "[" out "]" }')"
if [ "$code" -eq 0 ] && [ "$fail" -eq 0 ]; then result true "$pass" "$fail" "$details"; exit 0; fi
result false "$pass" "$fail" "$details"
[ "$fail" -gt 0 ] && exit 1
exit 2
