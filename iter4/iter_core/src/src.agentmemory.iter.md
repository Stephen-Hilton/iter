# Agent memory: iter4/iter_core/src — the shared Rust library (work item model, schedules, dedup keys, wait graph, cluster, widgets) used by iter_data, iter_engine and iter_local.

## Where things live
- lib.rs — the work item model and shared types (node file: model.code.iter.md).
- sched.rs — decides when a scheduled template is due (every/stale/daily/weekly); skips missed times instead of catching up (sched.code.iter.md).
- dedup.rs — dedup tag keys (dedupkeys.code.iter.md).
- waitgraph.rs — blocker/wait graph and cycle checks (waits.code.iter.md).
- cluster.rs, widget.rs — cluster state and webui widgets (cluster/widget .code.iter.md).
- Each .rs has a sibling *.code.iter.md node file; it answers most "what does this do" questions, so read it before the code.
- ../test/ — iter_core.tests.iter.md + cargo_test.sh (tests belong to the testwriter; do not edit).

## Build and test
- Build: from iter4/, `cargo build --release -p iter_core` (a workspace with iter_data, iter_engine, iter_local, iter_rag).
- Test: `"$ITER_BIN" runtests --project "$ITER_PROJECT" --group "iter_core-unit"` (cargo test -p iter_core; 53/53 green on 2026-09-29; under a minute when cached).

## Gotchas
- Every change here recompiles every crate that depends on it; check the callers (iter_engine fire_schedules uses sched due/clone_from).
- GraphRAG summary work is not here: see iter_engine/src/rag.code.iter.md and interfaces/rag-work-claim (a claim lapses after 10 min).
- Question-only items: answer with a workitem_doc note; change no files.

## Recent changes
- 2026-09-29 a3935ad7 — answered the operator's schedule and GraphRAG-claim questions in a note; no code changed.
