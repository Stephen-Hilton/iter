---
id: 1d9415a2-ba23-4872-aede-dcf62e2b259d
name: "GraphRAG search quality"
desc: "Asks every question in rag_eval.json against a running iter_data's GraphRAG search for the iter5 project in hybrid, vector and keyword modes and checks that hybrid search finds one of the expected node files in the top 5 for at least 80% of them; one standard result line, one detail row per question. Needs a live server with the project's node files indexed, so the test sweep leaves it out."
creator: "agent.code"
teststate: omit
children:
  codedirs:  []
  codenodes: []
  tests:     ["{thisfiledir}/rag_eval.sh"]
  reqs:      []
timestamps: {create: "2026-10-02 23:15:00Z", last_modified: "2026-10-02 23:15:00Z", last_tested: ""}
---

# GraphRAG search quality

`rag_eval.sh` sends each question in `rag_eval.json` to `POST /api/projects/{p}/rag/search` (the project's own documents only, no built-in guide) in the `hybrid`, `vector` and `keyword` modes, prints each question's rank of the first expected node file, hit@k and MRR per mode, and ends with the standard result JSON (spec §3.5): `normal` counts hybrid hits, one `details` row per question with its hybrid rank. Green when the hybrid hit rate reaches `min_hit_rate` (0.8).

## Needs

A running iter_data (`ITER_DATA_URL`, default `http://127.0.0.1:8400`), a token in `ITER_ENGINE_TOKEN` or `ITER_TOKEN`, and the project (`ITER_PROJECT`, else `rag_eval.json`'s `iter5`) with its node files indexed (`iter rag sync`). Without a server or token it exits 2 — could not run.

`teststate: omit` keeps it out of the test sweep; run it by hand with `iter runtests --node "GraphRAG search quality"`.

## Keeping it honest

Each question names the node files that answer it. When node files are renamed or moved, update the `expect` paths — a stale path makes a question miss even when search is right.
