---
id: 987f0173-29a1-4380-8d4b-418327b4ff94
name: "iter5 end-to-end"
desc: "Runs e2e.sh — iter_data on a throwaway ArangoDB database plus one engine serving two projects on the mock provider: work items round-trip in both, file edits reach the graph and graph edits reach files and commits, nodes created and deleted both ways, sync conflicts, designer build, test results, account switch/stop via bills edges, deactivated serves edges, MCP tools and iter migrate5 — and reports its tally as one standard result line. Slow (minutes) and needs ArangoDB on :8529, so the test sweep leaves it out."
creator: "agent.code"
teststate: omit
children:
  codedirs:  []
  codenodes: []
  tests:     ["{thisfiledir}/tests/e2e_suite.sh"]
  reqs:      []
timestamps: {create: "2026-10-02 23:15:00Z", last_modified: "2026-10-02 23:15:00Z", last_tested: ""}
---

# iter5 end-to-end

`tests/e2e_suite.sh` runs `e2e.sh` (spec §11) and turns its tally into the standard result JSON (spec §3.5) on the last line: `normal` = every PASS / FAIL check, `details` = one row per section (its pass / fail counts) plus one row per failed check. Exit 0 green, 1 red, 2 could not run.

## What e2e.sh does

Builds the binaries, starts iter_data on a random port against `iter5_e2e_<random>` on the dev ArangoDB and one engine (`E2E`) serving two projects copied from `e2e/sample5`, with every agent turn on the deterministic `mock` provider. Sections cover: work-item round trips in each project; a hand edit to a node file reaching the graph within seconds and a graph edit reaching the file and a commit; nodes created in the graph becoming files; deletes both ways; a sync conflict (newer `last_modified` wins); a designer project built into a new repository with its plan item; a test script's standard JSON landing on its test node and in the test log; account switch / stop via `bills` edges; a deactivated `serves` edge stopping a project; MCP tool calls; `iter migrate5` on `e2e/iter4_fixture`. The database is always dropped.

## Needs

cargo, jq, curl and the dev ArangoDB on `127.0.0.1:8529` (`./deploy.sh local`; `ITER5_TEST_ARANGO_URL` / `ITER5_TEST_ARANGO_PASSWORD` override). Missing any of them → exit 2. `E2E_ARGS="--live"` adds the opt-in live Claude smoke test.

`teststate: omit` keeps it out of the test sweep (it takes minutes and starts its own server); run it with `iter runtests --node "iter5 end-to-end"`.
