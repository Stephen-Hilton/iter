#!/usr/bin/env bash
# GraphRAG search quality (iter5 test contract, docs/iter5_spec.md §3.5:
# exit 0 green / 1 red / 2 could not run; the LAST stdout line is the standard
# result JSON). Asks every question in rag_eval.json against a running
# iter_data, in all three search modes, and reports each question's rank of
# the first expected node file, hit@k and MRR (mean reciprocal rank). The
# `normal` bucket counts hybrid-mode hits; green when the hybrid hit rate
# reaches min_hit_rate. Needs ITER_DATA_URL (default http://127.0.0.1:8400)
# and a token in ITER_ENGINE_TOKEN or ITER_TOKEN; the project (ITER_PROJECT,
# else rag_eval.json's "project") must have its node files indexed.
set -u
here="$(cd "$(dirname "$0")" && pwd)"
url="${ITER_DATA_URL:-http://127.0.0.1:8400}"
name="${ITER_TEST_NODE_NAME:-GraphRAG search quality}"; id="${ITER_TEST_NODE_ID:-}"
not_run() { printf '{"name":"%s","id":"%s","overall_success":false,"normal":{"total":0,"pass":0,"err":0},"longtail":{"total":0,"pass":0,"err":0},"failure":{"total":0,"pass":0,"err":0}}\n' "$name" "$id"; exit 2; }
tok="${ITER_ENGINE_TOKEN:-${ITER_TOKEN:-}}"
if [ -z "$tok" ] || ! curl -sf -o /dev/null "$url/health"; then
  echo "no iter_data at $url or no token (ITER_ENGINE_TOKEN / ITER_TOKEN)"
  not_run
fi
URL="$url" TOK="$tok" NAME="$name" ID="$id" python3 - "$here/rag_eval.json" <<'PY'
import json, os, sys, urllib.request
cfg = json.load(open(sys.argv[1]))
url, tok, k = os.environ["URL"], os.environ["TOK"], cfg.get("k", 5)
project = os.environ.get("ITER_PROJECT") or cfg["project"]
def result(ok, passed, n, details):
    print(json.dumps({"name": os.environ["NAME"], "id": os.environ["ID"], "overall_success": ok,
                      "normal": {"total": n, "pass": passed, "err": n - passed},
                      "longtail": {"total": 0, "pass": 0, "err": 0}, "failure": {"total": 0, "pass": 0, "err": 0},
                      "details": details}))
def search(q, mode):
    # the project's own documents only: the built-in user guide answers "how
    # does iter work" questions too, and in iter5 (whose graph documents iter
    # itself) it competes with the node files this set expects
    body = json.dumps({"query": q, "k": k, "mode": mode, "graph": False, "include_guide": False}).encode()
    r = urllib.request.Request(f"{url}/api/projects/{project}/rag/search", data=body, method="POST",
                               headers={"content-type": "application/json", "authorization": "Bearer " + tok})
    with urllib.request.urlopen(r, timeout=60) as x:
        return [h["document"]["path"] for h in json.loads(x.read())["results"]]
def rank(paths, expect):
    want = {"{topdir}/" + e for e in expect}
    return next((i + 1 for i, p in enumerate(paths) if p in want), None)
try:
    rows = []
    for item in cfg["questions"]:
        rows.append((item["q"], {m: rank(search(item["q"], m), item["expect"]) for m in ("hybrid", "vector", "keyword")}))
except Exception as e:
    print(f"search failed: {e}"); result(False, 0, 0, []); sys.exit(2)
n = len(rows)
for q, r in rows:
    print(" ".join(f"{m[:3]}={r[m] or '-':>2}" for m in ("hybrid", "vector", "keyword")), "|", q)
for m in ("hybrid", "vector", "keyword"):
    hits = sum(1 for _, r in rows if r[m]); mrr = sum(1 / r[m] for _, r in rows if r[m]) / n
    print(f"{m:8} hit@{k} {hits}/{n} = {hits / n:.2f}   MRR {mrr:.3f}")
passed = sum(1 for _, r in rows if r["hybrid"])
ok = passed / n >= cfg.get("min_hit_rate", 0.8)
result(ok, passed, n, [{"name": q, "bucket": "normal", "pass": bool(r["hybrid"]), "msg": f"hybrid rank {r['hybrid'] or '-'}"} for q, r in rows])
sys.exit(0 if ok else 1)
PY
