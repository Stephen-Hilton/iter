#!/usr/bin/env bash
# Run one iter4 crate's unit tests and report in the testgroup contract:
# exit 0 green / 1 red / 2 could not run, last line `ITER_RESULT pass= fail= total=`.
# iter_data's tests run on the dev ArangoDB at 127.0.0.1:8529 (./deploy.sh
# local starts it); without it they cannot run (exit 2, not red).
set -u
crate="${1:?crate name}"
root="$(cd "$(dirname "$0")/.." && pwd)"
cargo="${CARGO:-$HOME/.cargo/bin/cargo}"
[ -x "$cargo" ] || { echo "no cargo at $cargo"; echo "ITER_RESULT pass=0 fail=0 total=0"; exit 2; }
if [ "$crate" = iter_data ]; then
  if ! curl -sf -o /dev/null -u "root:${ARANGO_ROOT_PASSWORD:-iter4dev}" http://127.0.0.1:8529/_api/version; then
    echo "no ArangoDB on 127.0.0.1:8529 — iter_data's tests need it (./deploy.sh local)"
    echo "ITER_RESULT pass=0 fail=0 total=0"; exit 2
  fi
  export ITER4_TEST_ARANGO_URL=http://127.0.0.1:8529 ITER4_TEST_ARANGO_PASSWORD="${ARANGO_ROOT_PASSWORD:-iter4dev}"
fi
out="$(cd "$root" && "$cargo" test -p "$crate" 2>&1)"
code=$?
echo "$out" | grep -E '^test |test result|panicked|error(\[|:)' | tail -200
pass=$(echo "$out" | grep -oE 'test result: [a-zA-Z]+\. [0-9]+ passed' | grep -oE '[0-9]+ passed' | awk '{s+=$1} END {print s+0}')
fail=$(echo "$out" | grep -oE '[0-9]+ failed' | awk '{s+=$1} END {print s+0}')
total=$((pass + fail))
echo "ITER_RESULT pass=$pass fail=$fail total=$total"
if [ "$code" -eq 0 ]; then exit 0; fi
if [ "$total" -gt 0 ] && [ "$fail" -gt 0 ]; then exit 1; fi
exit 2   # did not compile / could not run: an error, not a red test
