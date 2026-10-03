---
id: 80c381b8-2cee-4655-8b2a-6092363f42a5
name: "Test sweep"
desc: "Runs the project's test nodes off the project graph — choosing which ones by evaluating teststate along every chain from the project node and each use case or actor — posts each node's aggregated standard result to iter_data, and turns what it finds into work: one deduplicated `code` fix item per red test node, one `test` item per leaf code node with no runnable tests, coverage top-ups for test nodes short of their targets and `ingest` items for node text that breaks the text rules; it is `iter sweep` and the engine-owned Test sweep schedule."
creator: "agent.code"
teststate: inherit
level: component
owner: bespoke
children:
  codedirs:  ["{topdir}/iter_engine/src/sweep.rs"]
  codenodes: []
  tests:     []
  reqs:      []
timestamps: {create: "2026-10-02 23:10:52Z", last_modified: "2026-10-02 23:10:52Z", last_tested: ""}
---

# Test sweep

## Summary

Runs every test the project graph says should run, records the results, and files work for anything failing, untested or poorly described.

## How it works

`iter_engine/src/sweep.rs: sweep_verb` is `iter sweep`. It reads `GET /api/projects/{p}/graph`
(`fetch_graph`) for nodes and edges.

- **Which tests run** (`eligible`, `chain_states`, `step`): chains start at the project node and at every
  use case / actor and run down `codenodes` and `uses` edges; a `block` stops a chain, `omit` / `include`
  switch it. A test node runs when its own teststate is not `omit` / `block` and at least one chain
  reaching an owner (via a `tests` edge) includes it.
- **Running**: each test node's scripts run through iter_local's test runner; the aggregated standard
  result is posted to `POST …/graph/nodes/{id}/testresult`. A failed node files one `code` fix item locked
  to its owner's codedirs (`file_fix_item`), keyed by `check:` + `container:` tags so a repeat is
  recorded on the open item instead of filed twice. "Could not run" is recorded but files nothing.
- **Filing** (each capped per sweep): `untested_sweep` — leaf code nodes some chain includes with no
  linked test node that has a script on disk get a `test` item; `coverage_sweep` — test nodes short of
  their `coverage` targets (normal / longtail / failure counts) get a top-up item per owner;
  `text_sweep` — code nodes whose desc or body break iter_local's node-text rules get an `ingest` item.
- `install_schedule` (`--install-schedule --every 4h`) turns the project's Test sweep schedule on. The
  Engine tick loop creates that schedule paused for every project (`ensure_test_sweep`) and runs its
  clones outside the agent cap and holds, with no end-of-run commit.

## What goes in and out

In: the project graph, test scripts on disk. Out: test results and work items posted to iter_data;
exit code 0 all green, 1 something failed or could not run, 2 could not sweep.

## Why it matters

Red tests become work without anyone filing them, untested parts are noticed, and the split between
"failed" and "could not run" keeps infrastructure problems from being filed as code defects.

## Example

`iter sweep --dry-run` lists the test nodes it would run and the items it would file; without
`--dry-run`, a red `iter_local unit tests` node records its result and files one fix item on
`iter_local/`.
