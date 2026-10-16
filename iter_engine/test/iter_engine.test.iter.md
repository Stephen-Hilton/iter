---
id: 70687d51-78b8-4393-bc0b-10a980c796c4
name: "iter_engine unit tests"
desc: "Runs cargo test -p iter_engine — the engine's file sync (filescan, conform, sync replies, pending writes), assignments, provider dispatch with the deterministic mock provider, the close gate, dedup triage, session chaining, prompts, the iter CLI verbs and the checkout helpers in `local/` (git-ignore-aware file walk, iter validate, the deterministic test runner) — and reports one standard result line counted per test. Needs only cargo; no data server."
creator: "iter migrate5"
teststate: inherit
children:
  codedirs:  []
  codenodes: []
  tests:     ["{thisfiledir}/cargo_test.sh"]
  reqs:      []
timestamps: {create: "2026-10-02 23:05:35Z", last_modified: "2026-10-02 23:15:00Z", last_tested: ""}
---

# iter_engine — unit tests

One script, `cargo_test.sh`, runs the crate's whole `cargo test` through `tools/cargo-test-crate.sh iter_engine` and prints the standard result JSON (spec §3.5) as its last line: every test in the `normal` bucket, one `details` row per test. Exit 0 green, 1 red, 2 could not run.

## What it covers

The engine side of iter5 without a live server: file sync against fake replies (`filesync_tests.rs`), the assignments → per-project accounts view, provider dispatch through the `mock` provider (directives, usage from `ITER_MOCK_USAGE`), the close gate's verdict parsing and question widgets, dedup triage, schedules firing, prompt and context assembly, and the `iter` CLI verbs.

The checkout helpers in `src/local/` (the former iter_local crate) are covered here too: the checkout walk (git-ignore aware, skips `.git`, `target`, `node_modules`), `iter validate` (conform-check, duplicate ids) and the test runner (`runtests.rs`: a test node's `children.tests` scripts run under bash in the node's directory with `ITER_TEST_NODE_ID` / `ITER_TEST_NODE_NAME` / `ITER_TEST_OUT` set, one wall-clock budget per node, the last-line JSON or legacy `ITER_RESULT` parsed and aggregated).

The full engine ⇄ server loop (two projects, file edits both ways, build, accounts) is covered by the end-to-end test node (`e2e/e2e.test.iter.md`), not here.
