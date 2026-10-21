---
id: c1657c89-16f9-4452-86cf-64fa586f3329
name: "GraphRAG node-file coverage"
desc: "Checks that every node file of a project's map is searchable in GraphRAG: the number of distinct *.iter.md paths in the map (after the project's node_types setting) equals the number of node documents in state summarizing, rollup or ready, and names each map path that has none. Needs a live server, so the test sweep leaves it out; run after an iter rag sync (MCP/GraphRAG hardening MG-7)."
creator: "agent.code"
teststate: omit
children:
  codedirs:  []
  codenodes: []
  tests:     ["{thisfiledir}/rag_node_count.sh"]
  reqs:      []
timestamps: {create: "2026-10-21 20:00:00Z", last_modified: "2026-10-21 20:00:00Z", last_tested: ""}
---

# GraphRAG node-file coverage

`rag_node_count.sh <project>` reads the project's map (`GET /api/projects/{p}/graph`) and GraphRAG settings, and counts M, the distinct `*.iter.md` paths of the map's nodes after `node_types` (empty = all), the same selection `iter rag sync` makes. It counts N, the map paths whose node document (`GET …/rag/docs?kind=node`) is summarizing, rollup or ready, which means searchable. It prints `ITER_RESULT project=… M=… N=… repo_iter_md=…` and a `missing:` line per path without one, then the standard result JSON (spec §3.5). The result has one `normal` row per map path. It is green when N == M.

The map, not the repo, is the reference: `git ls-files '*.iter.md'` (printed for context) also counts agentmem and interface files, which are not map nodes.

## Needs

A running iter_data (`ITER_DATA_URL`, default `https://127.0.0.1:8400`), a token in `ITER_ENGINE_TOKEN` or `ITER_TOKEN`, the project as the first argument (or `ITER_PROJECT`). Use `ITER_CURL_OPTS` for TLS, e.g. `--cacert <iter dir>/iter_data/state/tls-ca.crt`. Without a server or token it exits 2: could not run.

`teststate: omit` keeps it out of the test sweep; run it by hand after a rollout's `iter rag sync`.
