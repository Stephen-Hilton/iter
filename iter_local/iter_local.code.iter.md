---
id: d189ff3e-7930-4fff-9092-ec1a0a80a006
name: "iter_local — checkout tools"
desc: "Works on a project checkout with no server involved: walks the tree for node files (git-ignore aware, incremental stat-based pieces for the engine's filescan), runs a test node's scripts and aggregates them into the standard result JSON, conform-checks every node file (`iter validate`) and converts an iter4 checkout into iter5 node files in a copy (`iter migrate5`)."
creator: "iter migrate5"
teststate: inherit
level: container
owner: bespoke
children:
  codedirs:  ["{thisfiledir}/"]
  codenodes: ["{topdir}/iter_local/src/walk.code.iter.md", "{topdir}/iter_local/src/runner.code.iter.md", "{topdir}/iter_local/src/validate.code.iter.md", "{topdir}/iter_local/src/migrate5.code.iter.md"]
  tests:     ["{thisfiledir}/test/*.test.iter.md"]
  reqs:      ["{topdir}/docs/iter5_spec.md"]
timestamps: {create: "2026-10-02 23:05:35Z", last_modified: "2026-10-02 23:15:33Z", last_tested: ""}
---

# iter_local — checkout tools

## Summary

The toolkit the engine and the `iter` command use on the files of a checkout: find node files, run tests, check files, convert old checkouts.

## What it does

iter_local is a Rust library with no network and no queue: give it a project folder and it answers from
the files. Parsing, rendering and conforming a node file is not done here — `iter_core::nodefile` is the
one node-file library, shared byte-for-byte with iter_data. iter4's marker scan, graph snapshot, id
stamper and graph-edit writer were removed in iter5.

## How it works

- **Walker** (`src/walk.rs`, `src/lib.rs`): finds every `*.iter.md` whose filename names a node type,
  under the project's `scandirs`, skipping `.git`, `target`, `node_modules`, `.iter`, `.claude` and
  whatever git ignores; exposes both a full walk (`node_files`) and the incremental pieces (`walk`,
  `list_dir`, `stat`) the engine's filescan is built from.
- **Test runner** (`src/runtests.rs`): runs a test node's `children.tests` scripts under `bash`, each in
  its own process group inside one time budget, reads the standard result JSON from each script's last
  stdout line (legacy `ITER_RESULT` accepted) and the exit code (0 pass, 1 fail, other = could not run),
  and aggregates one `TestResult` for the node.
- **Validate** (`src/validate.rs`): conforms every node file in memory, reports files whose text would
  change, conform's findings and duplicate ids; `--fix` rewrites them. Also holds the node-text rules
  the test sweep files `ingest` items from.
- **migrate5** (`src/migrate5.rs`, reading iter4 registries via `src/testgroups.rs`): copies an iter4
  checkout and converts it in the copy.

## What goes in and out

Callers are iter_engine only: the engine's file sync and test sweep, and the `iter runtests`,
`validate`, `sweep` and `migrate5` verbs. It reads and writes files and spawns `bash` and `git`
(`git check-ignore` / `ls-files` for ignore rules); it never calls iter_data.

## Why it matters

The graph, the test results and the conform rewrites all start from what these functions read on disk.
If the walker missed a file, its node would never sync; if the runner confused a broken script with a
failing test, infrastructure problems would be filed as code defects.

## Example

`iter validate` in this repository conforms every node file in memory and exits 0 when none would change
and no id is duplicated.

Built and tested as the cargo package `iter_local` (`cargo test -p iter_local` from the repo root).
