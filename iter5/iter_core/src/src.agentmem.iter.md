# Agent memory: iter5/iter_core/src — the shared Rust library (work item model, node-file library, settings-graph types, test result, schedules, dedup keys, wait graph, cluster, question forms) used by iter_data, iter_engine and iter_local.

## Where things live
- lib.rs — the work item model and shared types (node file: model.code.iter.md).
- nodefile/ — the v5 node-file library: parse / conform / render / edges / paths (nodefile/nodefile.code.iter.md). nodefile/fixtures/ holds iter4-era test fixtures: never edit them.
- settings.rs — settings-graph node/edge types, validation, Assignments (settings.code.iter.md).
- testresult.rs — the standard test result JSON and pass/fail/could-not-run verdict (testresult.code.iter.md).
- sched.rs — decides when a scheduled template is due (every/stale/daily/weekly); skips missed times instead of catching up (sched.code.iter.md).
- dedup.rs — dedup tag keys (dedupkeys.code.iter.md).
- waitgraph.rs — locks, blocker/wait graph and cycle checks (waits.code.iter.md).
- cluster.rs, widget.rs — cluster-restart hold and question forms (cluster/widget .code.iter.md).
- Each module has a *.code.iter.md node file; it answers most "what does this do" questions, so read it before the code.
- ../test/ — iter_core.test.iter.md + cargo_test.sh (tests belong to the testwriter; do not edit).

## Build and test
- Build: from iter5/, `cargo build -p iter_core` (workspace: iter_core, iter_data, iter_engine, iter_local, iter_rag).
- Test: `iter runtests iter_core/test/iter_core.test.iter.md` (cargo test -p iter_core via tools/cargo-test-crate.sh; prints the standard result JSON).
- `nodefile::tests::conform_idempotent_over_repo_tree` conforms every *.iter.md in the checkout: a malformed node file anywhere breaks it.

## Gotchas
- Every change here recompiles every crate that depends on it; check the callers (iter_data next.rs / nodes.rs / filesync.rs, iter_engine filesync.rs / engine.rs).
- nodefile is shared byte-for-byte by server and engine: a render change rewrites node files on the next scan.
- Question-only items: answer with a workitem_doc note; change no files.
