---
id: 01e11745-c2f8-403a-a620-7753bdf0d2ea
name: "iter command line"
desc: "Reads the command a person or agent typed (`iter add`, `ask`, `reject`, `block`, `wait`, `doc`, `critreview`, `capability`, `status` for the queue; `runtests`, `validate`, `sync`, `sweep`, `rag sync`, `init`, `migrate5`, `markers`, `teststate`, `usecase` for the checkout), checks its arguments and hands the work to the part that does it, so that every change to the queue or the checkout goes through one checked front door and agents never write to the database directly."
creator: "iter migrate5"
teststate: inherit
level: component
owner: bespoke
children:
  codedirs:  ["{topdir}/iter_engine/src/cli.rs", "{topdir}/iter_engine/src/init.rs"]
  codenodes: []
  tests:     []
  reqs:      []
timestamps: {create: "2026-10-02 23:05:35Z", last_modified: "2026-10-02 23:15:33Z", last_tested: ""}
---

# iter command line

## Summary

The typed commands people and AI agents use to add work, ask a human a question, run tests, check node files and sync a checkout.

## How it works

`iter_engine/src/main.rs` sees the `cli` subcommand and calls `cli::run` (`iter_engine/src/cli.rs`);
clap parses the words into the `Verb` enum. Inside an engine run, agents reach it through the
`.iter/bin/iter` shim the Work runner writes, with `ITER_DATA_URL`, `ITER_ENGINE_TOKEN`,
`ITER_PROJECT`, `ITER_WORKID`, `ITER_AGENT`, `ITER_TOPDIR`, `ITER_NODE`, `ITER_NODEFILE`,
`ITER_PROVIDER` and `ITER_ACCOUNT` set (`env`).

- **Queue verbs** (`Add`, `Ask`, `Reject`, `Block`, `Wait`, `Doc`, `Critreview`, `Capability`, `Status`)
  talk to iter_data. `iter add` refuses a bad request naming the offending key (`check_add_file`,
  `check_add_request`); `iter wait --on` records blockers the Close gate waits behind; `iter block
  --cluster-restart` parks an item until the cluster is back.
- **Checkout verbs** (`mod local`): `runtests` finds a test node (id, unique tail, name, stem or path, or
  the calling item's `$ITER_NODEFILE`), runs it through iter_local's test runner, posts the standard
  result to the test node and, with `--broken` / `--fixed`, records a claim the Close gate checks;
  `validate [--fix]` is iter_local's validate; `markers` prints every node file parsed as JSON;
  `teststate` sets or lists teststate on node files; `usecase` edits a use case's `children.codenodes`.
- **Connected checkout verbs** (via `sync::conn`): `sync` (one Node-file sync round), `sweep` (Test
  sweep), `rag sync` (GraphRAG change sweep).
- `init` (`iter_engine/src/init.rs: init_project`) scaffolds a v5 project with `iter_core::nodefile`:
  `global/<slug>.project.iter.md` and `global/requirements/{philosophy,bizreq,techreq}` files, already
  conformed, never overwriting without `--force`; no engine config is written.
- `migrate5 --from --to [--dry-run] [--json]` runs iter_local's converter into a copy.
- Anything else gets a plain message (`retired`): `ids` and `graph-apply` are gone in iter5.

## What goes in and out

In: command words, the `ITER_*` environment, the checkout. Out: iter_data API calls through the Data
server client, node-file writes, test runs, exit codes and one-line reports.

## Why it matters

Agents ask for the logical thing and this code does the deterministic part, so a half-formed request is
refused instead of stored, and claims such as "I fixed it" are recorded where the gate can check them.

## Example

Inside a run an agent types `iter runtests --fixed`; the CLI runs the item's test node, posts the result,
and records the claim. One red script means the item cannot close complete.
