---
id: a6678803-fc20-412b-81f9-af351be46305
name: "Test results and logs"
desc: "Records each test run an engine reports for a test node — the standard result JSON, or the legacy ITER_RESULT line — on the node itself (test, front.last_result, timestamps.last_tested, so the file is rewritten like any edit) and as a row of the test log, files exactly one work item when a test goes red or cannot run, lists the log for the webui and MCP, and queues a 'Run tests' work item when someone asks from the Project graph."
creator: "agent.code"
teststate: inherit
level: component
owner: bespoke
children:
  codedirs:  ["{topdir}/iter_data/src/testlogs.rs"]
  codenodes: []
  tests:     []
  reqs:      []
timestamps: {create: "2026-10-02 23:10:16Z", last_modified: "2026-10-02 23:10:16Z", last_tested: ""}
---

# Long Description

## Summary

Remembers every test result and turns a failing test into work.

Test results and logs (`iter_data/src/testlogs.rs`) is where test outcomes land. Test scripts print the standard result JSON (`iter_core::testresult::TestResult`: overall success plus normal / longtail / failure buckets and optional details) as their last line of output, and the engine posts it here.

How it works: `POST /api/projects/{p}/graph/nodes/{id}/testresult` (`testresult`) accepts the result bare, wrapped with an exit code, or as stdout to read the last line from (`read_result`; a legacy `ITER_RESULT pass= fail= total=` line maps to the normal bucket, and a non-zero exit overrides success). The outcome (`evaluate`, `combine`) is written onto the test node — `test`, `front.last_result`, `timestamps.last_tested` — through the Project graph store, which makes the node a pending write, and appended to the `test_log` collection with engine, work item and counts. A failed or could-not-run result files a work item deduped on `check:tests-failing` + `container:<node>`, so a red test becomes one item, not one per run. `GET /api/projects/{p}/testlogs?node=&limit=` lists the log, newest first. `POST /api/projects/{p}/graph/run_tests {node}` queues a `test` shell work item that runs `iter runtests --node <id>` on an engine.

What goes in and out: iter_engine's test service and the test sweep post results; the webui and the MCP `testlogs` tool read the log; it files work items through the HTTP API's `workitem_create`.

Why it matters: without it test history would be lost and red tests would go unnoticed.

Example: `iter_data unit tests` reports `overall_success: false`; the test node shows red in the graph, a `test_log` row is added, and one "tests failing" work item is filed for that node.
