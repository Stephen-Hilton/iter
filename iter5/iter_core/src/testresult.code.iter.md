---
id: 0896dc30-b252-49a2-9226-f517b7696264
name: "Standard test result"
desc: "Defines the standard result JSON every test script prints as its last stdout line (name, id, overall_success, normal / longtail / failure buckets of total, pass and err, optional per-test details), parses it — or the legacy ITER_RESULT line — from a script's output, and combines it with the exit code into pass, fail or could-not-run, so the runner, the engine and the server judge a test run the same way."
creator: "agent.code"
teststate: inherit
level: component
owner: bespoke
children:
  codedirs:  ["{topdir}/iter_core/src/testresult.rs"]
  codenodes: []
  tests:     []
  reqs:      []
timestamps: {create: "2026-10-02 23:15:00Z", last_modified: "2026-10-02 23:15:00Z", last_tested: ""}
---

# Standard test result (`iter_core::testresult`)

## Summary

The one format a test reports in, and the rule for whether it passed.

## How it works

`TestResult` is the shape: `name` and `id` (the test node's), `overall_success`, three `Bucket`s — `normal`, `longtail`, `failure` — each `{total, pass, err}`, and optional `details` (`{name, bucket, pass, msg}` per test). A script prints it as one JSON line, last on stdout:

```json
{"name":"iter_core unit tests","id":"14c76725-…","overall_success":true,"normal":{"total":159,"pass":159,"err":0},"longtail":{"total":0,"pass":0,"err":0},"failure":{"total":0,"pass":0,"err":0}}
```

`parse_last_line` reads the last non-empty line as that JSON (missing buckets default to zero; a missing `overall_success` is derived: at least one test and every bucket green), else falls back to the last legacy `ITER_RESULT pass= fail= total=` line, mapped to `normal`. `combine` turns result plus exit code into an `Outcome`: exit 0 passes unless the line says `overall_success: false`, exit 1 fails, any other exit (or a kill) means the script could not run — never mistaken for a red test. `evaluate` does both and, when a script printed nothing, synthesises a one-test result from the exit code (`from_exit`); `with_identity` fills a missing name or id.

## What goes in and out

iter_local's runner (`iter_local/src/runtests.rs`) evaluates each script and aggregates a test node's scripts into one result; `iter runtests` and the test sweep post it to `POST /api/projects/{p}/graph/nodes/{id}/testresult`, where iter_data (`testlogs.rs`) stores it on the test node (`front.last_result`) and in the test log, and files a fix item when it is red or could not run.

## Why it matters

Test scripts are written by people and agents in any language. One small contract, parsed in one place, lets the queue tell "red" from "broken harness" and keep a comparable history per test node.

## Example

`tools/cargo-test-crate.sh iter_rag` ends with `{"name":"iter_rag unit tests",…,"normal":{"total":11,"pass":11,"err":0},…}` and exits 0 → pass; the same script with no ArangoDB for iter_data exits 2 → could not run: the test log records it as `error`, not `red`, so a broken harness is never read as broken code.
