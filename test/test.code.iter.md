---
id: 8855b650-a6ee-43a3-bdbe-b862f6a41bdb
name: "test — every test suite outside the crates"
desc: "Holds every test that runs from outside the Rust crates: one test node per crate (test/iter_core, test/iter_data, test/iter_engine) whose cargo_test.sh runs that crate's cargo tests through cargo-test-crate.sh and reports one standard result line, the GraphRAG search-quality check (test/iter_data/rag_eval.sh), the end-to-end run (test/e2e.sh: iter_data plus one engine on the mock provider) and the Playwright web suite (test/e2e/playwright)."
creator: "stephen"
teststate: inherit
level: component
owner: bespoke
children:
  codedirs:  ["{thisfiledir}/"]
  codenodes: []
  tests:     []
  reqs:      []
timestamps: {create: "2026-10-02 23:12:30Z", last_modified: "2026-10-19 21:56:52Z", last_tested: ""}
---
# test — every test suite outside the crates

The Rust unit tests live next to the code they test (`#[cfg(test)]` modules and `*_tests.rs` files inside
`iter_core/`, `iter_data/` and `iter_engine/`). Everything that runs them, or tests the programs from
outside, lives here.

## What is here

- `cargo-test-crate.sh <crate>` runs `cargo test -p <crate>` from the repo root and prints the standard
  result JSON (spec §3.5) as its last line: exit 0 green, 1 red, 2 could not run (no cargo, no dev
  ArangoDB on :8529 for iter_data, or a build error).
- `iter_core/`, `iter_data/`, `iter_engine/` each hold one test node and its `cargo_test.sh`, which calls
  `cargo-test-crate.sh` for that crate. `iter_data/` also holds the GraphRAG search-quality check
  (`rag_eval.sh`, `rag_eval.json`).
- `e2e.sh` with `e2e/`: the end-to-end run (iter_data on a throwaway ArangoDB database plus one engine
  serving two copies of `e2e/sample5` on the mock provider), its test node, `deploy_smoke.sh` for a
  deployed server, and the Playwright web suite in `e2e/playwright/` (screenshots git-ignored).

## How to run

`iter runtests test/<crate>/<crate>.test.iter.md`, or by hand `test/cargo-test-crate.sh iter_core`,
`./test/e2e.sh [--live] [--keep]`, `test/e2e/playwright/run.sh`. iter_data's tests, the e2e run and the
Playwright suite need the dev ArangoDB on 127.0.0.1:8529 (`docker start iter-arango-dev`).
