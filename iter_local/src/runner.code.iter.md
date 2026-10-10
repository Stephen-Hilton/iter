---
id: f85558b1-6a4b-4575-b032-50c4db370c9c
name: "Test runner"
desc: "Runs a test node's scripts (its `children.tests` globs) under `bash` in the node's folder, each in its own process group inside one shared time budget, reads each script's standard result JSON from its last stdout line (legacy `ITER_RESULT pass= fail= total=` accepted) and its exit code (0 pass, 1 fail, anything else could not run), and aggregates them into one standard `TestResult` for the node, so that test verdicts come from the scripts alone and a broken script is never mistaken for a failing test."
creator: "iter migrate5"
teststate: inherit
level: component
owner: bespoke
children:
  codedirs:  ["{topdir}/iter_local/src/runtests.rs"]
  codenodes: []
  tests:     []
  reqs:      []
timestamps: {create: "2026-10-02 23:05:35Z", last_modified: "2026-10-02 23:15:33Z", last_tested: ""}
---

# Test runner

## Summary

Runs a set of automated tests the same way every time and reports, in one standard shape, whether they passed, failed or could not run.

## How it works

A test node (`*.test.iter.md`) is metadata; its `children.tests` entries are globs or paths of scripts
(iter5 spec §3.5). `iter_local/src/runtests.rs`:

- `test_node_files`, `load_test_node` and `find_test_node` locate a test node by id, id tail, name,
  file stem or path; `scripts_of` resolves its scripts on disk.
- `run_node` runs every script (or one, with a filter) through `run_script`: `bash` in the test node's
  folder, its own process group so a timeout kills everything it started, inside one wall-clock budget
  per node (`DEFAULT_TIMEOUT_MIN`, 20 minutes). Each script gets `ITER_TEST_OUT` (an emptied scratch
  folder outside the checkout, `test_out_dir`), `ITER_TESTS_SHARED` (`{topdir}/.iter/tests`),
  `ITER_TEST_NODE_ID`, `ITER_TEST_NODE_NAME` and `ITER_TOPDIR`.
- The last stdout line is parsed as the standard result JSON (`{name, id, overall_success, normal,
  longtail, failure, details}`, the `iter_core::testresult` type), or a legacy `ITER_RESULT` line mapped
  to `normal`. The exit code decides the `Outcome`.
- `aggregate` folds the scripts into one `NodeRun` / `TestResult` for the node; `log_header`,
  `log_detail` and `tail_bytes` format the run for a work item's history (8 KB per script, 64 KB total).

## What goes in and out

In: a topdir and a test node. Out: a `NodeRun` with per-script outcomes and the aggregated standard
result, which `iter runtests` and the Test sweep post to `POST …/graph/nodes/{id}/testresult`.

## Why it matters

The pass / fail / could-not-run split keeps an infrastructure problem from being filed as a code
defect, and the fixed contract means an agent's claim "I fixed it" is checked by running the same
scripts.

## Example

A test node lists `tests/iter_local.test*.sh`; the script runs `cargo test -p iter_local` and prints
`{"name":"iter_local unit tests",…,"overall_success":true,"normal":{"total":40,"pass":40,"err":0},…}` as
its last line with exit 0, so the node's result is pass, 40 of 40.
