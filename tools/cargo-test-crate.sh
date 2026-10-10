#!/usr/bin/env bash
# Run one iter5 crate's `cargo test` and report in the iter5 test contract
# (docs/iter5_spec.md §3.5): the LAST stdout line is the standard result JSON
#   {"name","id","overall_success","normal":{total,pass,err},"longtail":{…},"failure":{…},"details":[…]}
# exit 0 green / 1 red / 2 could not run (no cargo, no ArangoDB, did not compile).
#
# Usage: tools/cargo-test-crate.sh <crate>
# The test runner (`iter runtests`) sets ITER_TEST_NODE_NAME / ITER_TEST_NODE_ID;
# run by hand they default to "<crate> unit tests" / "".
# Buckets: every cargo test is `normal` (passed → pass, failed → err; ignored
# tests are not counted). details = one row per test (capped at 500).
# iter_data's tests run on the dev ArangoDB at 127.0.0.1:8529 (databases
# iter5_test_*; ITER5_TEST_ARANGO_URL / ITER5_TEST_ARANGO_PASSWORD override);
# without it they cannot run (exit 2, not red).
set -u
crate="${1:?crate name}"
root="$(cd "$(dirname "$0")/.." && pwd)"
cargo="${CARGO:-$HOME/.cargo/bin/cargo}"
name="${ITER_TEST_NODE_NAME:-$crate unit tests}"
id="${ITER_TEST_NODE_ID:-}"

jstr() { printf '%s' "$1" | sed -e 's/\\/\\\\/g' -e 's/"/\\"/g' | tr -d '\n\r\t'; }

# result <pass> <fail> <overall true|false> [details-json-array]
result() {
  local pass="$1" fail="$2" ok="$3" details="${4:-}"
  local total=$((pass + fail))
  printf '{"name":"%s","id":"%s","overall_success":%s,"normal":{"total":%d,"pass":%d,"err":%d},"longtail":{"total":0,"pass":0,"err":0},"failure":{"total":0,"pass":0,"err":0}' \
    "$(jstr "$name")" "$(jstr "$id")" "$ok" "$total" "$pass" "$fail"
  [ -n "$details" ] && printf ',"details":%s' "$details"
  printf '}\n'
}

if [ ! -x "$cargo" ]; then
  echo "no cargo at $cargo"; result 0 0 false; exit 2
fi
if [ "$crate" = iter_data ]; then
  aurl="${ITER5_TEST_ARANGO_URL:-${ITER4_TEST_ARANGO_URL:-http://127.0.0.1:8529}}"
  apw="${ITER5_TEST_ARANGO_PASSWORD:-${ITER4_TEST_ARANGO_PASSWORD:-${ARANGO_ROOT_PASSWORD:-iter4dev}}}"
  if ! curl -sf -o /dev/null -u "root:$apw" "$aurl/_api/version"; then
    echo "no ArangoDB at $aurl — iter_data's tests need it (./deploy.sh local starts the dev container)"
    result 0 0 false; exit 2
  fi
  export ITER5_TEST_ARANGO_URL="$aurl" ITER5_TEST_ARANGO_PASSWORD="$apw"
fi

out="$(cd "$root" && "$cargo" test -p "$crate" 2>&1)"
code=$?
# the human-readable part: test lines, summaries, panics and compile errors
echo "$out" | grep -E '^test |test result|panicked|error(\[|:)' | tail -200

pass=$(echo "$out" | grep -oE 'test result: [a-zA-Z]+\. [0-9]+ passed' | grep -oE '[0-9]+ passed' | awk '{s+=$1} END {print s+0}')
fail=$(echo "$out" | grep -E 'test result:' | grep -oE '[0-9]+ failed' | awk '{s+=$1} END {print s+0}')
details=$(echo "$out" | awk '
  /^test .* \.\.\. (ok|FAILED)$/ {
    if (n >= 500) next
    line = $0; sub(/^test /, "", line)
    st = line; sub(/.* \.\.\. /, "", st)
    nm = line; sub(/ \.\.\. (ok|FAILED)$/, "", nm)
    gsub(/\\/, "\\\\", nm); gsub(/"/, "\\\"", nm)
    printf "%s{\"name\":\"%s\",\"bucket\":\"normal\",\"pass\":%s,\"msg\":\"%s\"}", (n ? "," : ""), nm, (st == "ok" ? "true" : "false"), (st == "ok" ? "" : "failed")
    n++
  }
  END { }' )
[ -n "$details" ] && details="[$details]"

if [ "$code" -eq 0 ]; then
  result "$pass" "$fail" true "$details"; exit 0
fi
if [ "$((pass + fail))" -gt 0 ] && [ "$fail" -gt 0 ]; then
  result "$pass" "$fail" false "$details"; exit 1
fi
# did not compile / could not run: an error, not a red test
result "$pass" "$fail" false "$details"; exit 2
