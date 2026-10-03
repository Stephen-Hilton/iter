---
id: d784decd-29db-46ea-886c-da0402a9a8dd
name: "iter_local unit tests"
desc: "Runs cargo test -p iter_local — the git-ignore-aware file walk, iter validate, the deterministic test runner (scripts per test node, budgets, standard result parsing and aggregation) and the iter4 → iter5 converter iter migrate5 — and reports one standard result line counted per test. Needs only cargo."
creator: "iter migrate5"
teststate: inherit
children:
  codedirs:  []
  codenodes: []
  tests:     ["{thisfiledir}/cargo_test.sh"]
  reqs:      []
timestamps: {create: "2026-10-02 23:05:35Z", last_modified: "2026-10-02 23:15:00Z", last_tested: ""}
---

# iter_local — unit tests

One script, `cargo_test.sh`, runs the crate's whole `cargo test` through `tools/cargo-test-crate.sh iter_local` and prints the standard result JSON (spec §3.5) as its last line: every test in the `normal` bucket, one `details` row per test. Exit 0 green, 1 red, 2 could not run.

## What it covers

- the checkout walk (git-ignore aware, skips `.git`, `target`, `node_modules`),
- `iter validate` (conform-check, duplicate ids),
- the test runner (`runtests.rs`): a test node's `children.tests` scripts run under bash in the node's directory with `ITER_TEST_NODE_ID` / `ITER_TEST_NODE_NAME` / `ITER_TEST_OUT` set, one wall-clock budget per node, the last-line JSON (or legacy `ITER_RESULT`) parsed and aggregated,
- `iter migrate5` (`migrate5_tests.rs`): iter4 tree → v5 node files in a copy.
