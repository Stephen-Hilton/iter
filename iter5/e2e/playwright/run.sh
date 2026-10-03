#!/usr/bin/env bash
# iter5 webui Playwright suite: builds iter_data (debug), starts it on a random
# port against a throwaway ArangoDB database on the dev container (:8529),
# seeds it, runs every spec, then stops the server and drops the database.
#   ./run.sh                    all specs
#   ./run.sh specs/graph.spec.js -g "drag"   any playwright test arguments
# Env: ITER5_TEST_ARANGO_URL / ITER5_TEST_ARANGO_PASSWORD (default :8529 / iter4dev),
#      PW_SKIP_BUILD=1 (reuse target/debug/iter_data), PW_KEEP_DB=1 (keep the db + log),
#      PW_CHROMIUM=<path> (browser binary).
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
cd "$here"
export PATH="$HOME/.cargo/bin:/opt/homebrew/bin:$PATH"
if [ ! -e node_modules/@playwright/test ]; then
  shared="$here/../../../e2e/node_modules"
  if [ -d "$shared/@playwright/test" ]; then ln -sfn ../../../e2e/node_modules node_modules
  else npm install --no-audit --no-fund; fi
fi
mkdir -p screenshots
exec node node_modules/@playwright/test/cli.js test "$@"
