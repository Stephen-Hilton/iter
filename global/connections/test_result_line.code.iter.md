---
id: f0ecfa98-f3cd-43f7-888b-64b9d3b53d81
name: "Test result line (script stdout)"
desc: "How a test script reports to iter5: a script listed in a test node's children.tests runs under bash in the checkout and prints the standard result JSON as its LAST stdout line ({name, id, overall_success, normal, longtail, failure, details}); exit 0 = pass, 1 = fail, anything else = could not run. The legacy last line ITER_RESULT pass= fail= total= is still accepted. Read by the test runner via iter_core::testresult, then posted to the test node."
creator: "stephen"
teststate: inherit
connects:
  from: ["{topdir}/tools/tools.code.iter.md"]
  to: ["{topdir}/iter_local/src/runner.code.iter.md", "{topdir}/iter_core/src/testresult.code.iter.md"]
level: connection
children:
  codedirs:  []
  codenodes: []
  tests:     []
  reqs:      []
timestamps: {create: "2026-10-02 23:12:30Z", last_modified: "2026-10-02 23:12:30Z", last_tested: ""}
---
# Test result line (script stdout)

## Protocol

The test runner (`iter_local/src/runtests.rs`) runs each script of a test node
with `bash`, in the checkout, under a time limit (the process group is killed
on timeout). The script's **last stdout line** is the result:

```json
{"name":"…","id":"…","overall_success":true,
 "normal":{"total":3,"pass":3,"err":0},"longtail":{"total":0,"pass":0,"err":0},
 "failure":{"total":0,"pass":0,"err":0},
 "details":[{"name":"t1","bucket":"normal","pass":true,"msg":""}]}
```

`details` is optional. Exit code 0 = pass, 1 = fail, anything else = could not
run; the exit code wins over a contradicting line. A last line
`ITER_RESULT pass=<n> fail=<n> total=<n>` is still accepted (counted as
`normal`). `iter_core/src/testresult.rs` parses and combines the line and the
exit code; the runner aggregates a node's scripts into one result.

## Where it goes next

The aggregated result is posted to `POST …/graph/nodes/{id}/testresult`
(HTTP JSON API): it lands on the test node and in the test log, and a red or
could-not-run result files one deduplicated fix item.

## Suppliers in this repository

`tools/cargo-test-crate.sh` (called by each crate's `test/cargo_test.sh`)
prints the legacy line; any project's test scripts supply the same
connection.
