#!/usr/bin/env bash
# GraphRAG node-file coverage (MCP/GraphRAG hardening MG-7; iter test contract,
# docs/iter_spec.md §3.5: exit 0 green / 1 red / 2 could not run; the LAST
# stdout line is the standard result JSON).
#
#   M = distinct *.iter.md paths in the project's map (GET …/graph), after the
#       project's GraphRAG `node_types` setting (empty = all) — the same
#       selection `iter rag sync` makes (iter_engine/src/rag.rs);
#   N = node documents (GET …/rag/docs?kind=node) in state summarizing, rollup
#       or ready (searchable).
# Green when N == M; red names every map path with no searchable document.
# The repo's `git ls-files '*.iter.md'` count is printed for context only: it
# includes agentmem and interface files, which are not map nodes.
#
# Usage: test/iter_data/rag_node_count.sh <project>
# Env: ITER_DATA_URL (default https://127.0.0.1:8400), a token in
# ITER_ENGINE_TOKEN or ITER_TOKEN, ITER_CURL_OPTS (e.g. "--cacert <iter dir>/iter_data/state/tls-ca.crt").
set -u
project="${1:-${ITER_PROJECT:-}}"
url="${ITER_DATA_URL:-https://127.0.0.1:8400}"
tok="${ITER_ENGINE_TOKEN:-${ITER_TOKEN:-}}"
name="${ITER_TEST_NODE_NAME:-GraphRAG node-file coverage}"; id="${ITER_TEST_NODE_ID:-}"
root="$(cd "$(dirname "$0")/../.." && pwd)"
# the answers go through files: a big project's map is too long for the environment
tmp="$(mktemp -d)"; trap 'rm -rf "$tmp"' EXIT
not_run() {
  echo "$1"
  printf '{"name":"%s","id":"%s","overall_success":false,"normal":{"total":0,"pass":0,"err":0},"longtail":{"total":0,"pass":0,"err":0},"failure":{"total":0,"pass":0,"err":0}}\n' "$name" "$id"
  exit 2
}
[ -n "$project" ] || not_run "usage: $0 <project>"
[ -n "$tok" ] || not_run "no token: set ITER_ENGINE_TOKEN or ITER_TOKEN"
# shellcheck disable=SC2086
get() { curl -sf ${ITER_CURL_OPTS:-} -H "authorization: Bearer $tok" "$url$1"; }
enc="$(python3 -c 'import sys,urllib.parse;print(urllib.parse.quote(sys.argv[1],safe=""))' "$project")"
get "/api/projects/$enc/graph" > "$tmp/graph" || not_run "could not read the map of '$project' at $url"
get "/api/projects/$enc/rag/settings" > "$tmp/settings" || echo '{}' > "$tmp/settings"
get "/api/projects/$enc/rag/docs?kind=node" > "$tmp/docs" || not_run "could not read the GraphRAG documents of '$project'"
repo="$(cd "$root" && git ls-files '*.iter.md' 2>/dev/null | wc -l | tr -d ' ')"
DIR="$tmp" REPO="$repo" NAME="$name" ID="$id" PROJECT="$project" python3 - <<'PY'
import json, os, sys
graph, settings, docs = (json.load(open(os.path.join(os.environ["DIR"], k))) for k in ("graph", "settings", "docs"))
types = settings.get("node_types") or []
want = sorted({n["path"] for n in graph.get("nodes", [])
               if str(n.get("path", "")).endswith(".iter.md") and (not types or n.get("nodetype") in types)})
ready = {d["path"] for d in docs if d.get("state") in ("summarizing", "rollup", "ready")}
missing = [p for p in want if p not in ready]
m, n = len(want), len([p for p in want if p in ready])
print(f"ITER_RESULT project={os.environ['PROJECT']} M={m} N={n} repo_iter_md={os.environ['REPO']} node_types={','.join(types) or 'all'}")
for p in missing:
    print(f"missing: {p}")
ok = m > 0 and not missing
print(json.dumps({"name": os.environ["NAME"], "id": os.environ["ID"], "overall_success": ok,
                  "normal": {"total": m, "pass": n, "err": m - n},
                  "longtail": {"total": 0, "pass": 0, "err": 0}, "failure": {"total": 0, "pass": 0, "err": 0},
                  "details": [{"name": p, "bucket": "normal", "pass": False, "msg": "no searchable node document"} for p in missing[:500]]}))
sys.exit(0 if ok else 1)
PY
