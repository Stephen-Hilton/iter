---
id: c0f3cd16-cbb8-448d-9e35-dc58a31ed0a7
name: "iter_data unit tests"
desc: "Runs cargo test -p iter_data against the dev ArangoDB on 127.0.0.1:8529 (throwaway iter5_test_* databases) — project graph node CRUD and edges, file sync with conflicts, pending writes and acks, designer build, settings graph CRUD and placeholders, server-side get_next ordering, locks and authz, engine-token authz, test results and logs, MCP tools — and reports one standard result line counted per test; without ArangoDB it reports could-not-run (exit 2)."
creator: "iter migrate5"
teststate: inherit
children:
  codedirs:  []
  codenodes: []
  tests:     ["{thisfiledir}/cargo_test.sh"]
  reqs:      []
timestamps: {create: "2026-10-02 23:05:35Z", last_modified: "2026-10-02 23:15:00Z", last_tested: ""}
---

# iter_data — unit tests

One script, `cargo_test.sh`, runs the crate's whole `cargo test` through `tools/cargo-test-crate.sh iter_data` and prints the standard result JSON (spec §3.5) as its last line: every test in the `normal` bucket, one `details` row per test. Exit 0 green, 1 red, 2 could not run.

## Needs

The dev ArangoDB on `127.0.0.1:8529` (`./deploy.sh local` starts it); the tests create and drop `iter5_test_*` databases. `ITER5_TEST_ARANGO_URL` / `ITER5_TEST_ARANGO_PASSWORD` override the address and password (default `iter4dev`). Without a reachable ArangoDB the script exits 2 — could not run, never red.

## What it covers

Project graph store and API (`graph_tests.rs`): node create / patch / delete with designer path planning, edges add / remove / move, `files/sync` upserts and conflicts, `files/pending` + `files/ack`, build and `build/done`, test results on test nodes and the test log. Settings graph (`settings/tests.rs`): CRUD, edge-type inference, placeholders, copy, the one-time iter4 migration, assignments, `POST …/next` ordering / holds / locks / rollback / refusals, project-route and MCP authz by edges, engine heartbeats. API rules (`api.rs`): dependency cycles refused, dedup stage 1 and `duplicate_of`, lock leases and reservations, closed items, the test-sweep schedule. MCP tool schemas (`mcp.rs`), GraphRAG search (`rag/`), and the storage contract on ArangoDB (`contract_tests.rs`).

GraphRAG search quality is a separate test node (`rag_search_quality.test.iter.md`) because it needs a running server.
