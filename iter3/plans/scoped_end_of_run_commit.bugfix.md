# Bugfix spec: the engine's end-of-run commit sweeps other agents' files, and the close gate bounces finished work because of it

Written 2026-09-12 by Stephen's session in pdy-dev, from two measured incidents. Send as-is to
the iter agent. Every file:line below was read from `~/dev/iter/iter3` on 2026-09-12; verify each
before relying on it.

Terms, defined once: the **engine** is the `iter_engine` binary that runs one `claude` session
per work item. A **checkout** is the git working tree the engine runs those sessions in; on the
Mac engine "StephenMBP" four or five sessions share one checkout at a time. An item's **lock
scope** is its `lockdirs` list (directories or single files, written with a `{topdir}` prefix)
that no other running item may touch. The **close gate** is the check run after a session that
decides whether the item is done; its LLM half, the **verifier**, is shown an **evidence block**
built by the engine.

## 1. What happened

**Item ending `aca5162724a8`** (teach the shared rules library to load a multi-file bundle) did
its work on attempt 2 and committed it cleanly with a pathspec at 19:14 Pacific on 2026-09-11.
Attempts 3, 4 and 5 changed nothing and were bounced by the verifier every time. Each bounce
cited "the commit this run made" as touching a 1,125-line test file and a reference server in
two other repos' test directories, outside the item's lock scope, and called the worker's
report dishonest. Those files belonged to two sibling items running at the same time. The
worker explained this on attempt 5, naming the sibling ids; the verifier rejected the
explanation because the evidence block only shows the sweep. Three wasted attempts cost about
$16 and ended in a close-gate question to a human.

**Item ending `a180b52e47b8`** (one fixture cell in pdyadmin's onboarding checks) changed one
file and committed it alone at 20:36 Pacific. Five minutes later the engine's own commit under
this item's title carried another agent's edit to `devops/pdyadmin/ops.py`. The verifier read
that as the worker changing a file it said it had not touched, returned "unclear", and an
unclear verdict goes straight to a human without using the bounce budget.

In both cases the work was complete and the human's only correct answer was "accept".

## 2. Why it happens

1. **The sweep.** When a run ends, `iter_engine/src/work.rs:292-300` does
   `git add -A` then `git commit -m 'iter: <name> (<id8>)'` on the whole checkout, and pushes.
   Every other running agent's uncommitted files go into a commit titled with this item's name.
2. **The evidence spans the sweep and more.** `work.rs:979-982` computes the evidence diffstat
   as `git diff --stat <head_before> <head_after>`, where the heads are read at session start
   (`work.rs:116`, `git_head` at `work.rs:884`) and end (`work.rs:979`). That range includes
   every commit anyone landed in the checkout during the run, including other items' sweeps.
3. **The verifier is told it is this item's commit.** `gate.rs:58-67` puts `commit` and
   `diffstat` in the evidence JSON, and `Evidence::describe` at `gate.rs:68-83` renders it as
   "git: new commit <hash>" followed by the diffstat, with no statement that the tree is shared.
   The verifier prompt (`gate.rs:92-107`) then asks it to judge the worker's final message
   against that evidence, so any honest "I changed nothing else" reads as a lie.

The result is a loop that cannot converge while siblings are running: every retry produces a
new sweep and a new bounce.

## 3. Goal

The engine commits, and shows the verifier, only paths inside the item's lock scope plus a
short project-level list of always-shared paths. Another agent's unfinished files are never
committed under this item's name and never appear in this item's evidence.

## 4. Design

### R1 — the end-of-run commit is limited to the item's lock scope

Replace the two commands at `work.rs:294-296` with a pathspec commit, extracted into one
testable function:

```rust
pub(crate) struct CommitOutcome { pub committed: bool, pub outside_scope_dirty: usize, pub scope_label: String }
pub(crate) fn commit_scoped(topdir: &str, item: &WorkItem, extra_paths: &[String]) -> Result<CommitOutcome, String>
```

Rules:

1. Build `scope` from `item.lockdirs` with `{topdir}` expanded to the checkout path (the same
   expansion `expand_topdir` at `engine.rs:91` performs), made relative to `topdir`, plus every
   entry of the new project setting `commit_extra_paths` (R3). A lock entry that names a single
   file, as `a180b52e47b8`'s does, is a valid git pathspec and needs no special case.
2. Run `git add -A -- <scope...>` then `git commit -m 'iter: <name> (<id8>)' -- <scope...>`
   with the same message as today. Nothing outside `scope` is staged, committed or pushed by
   this run. `git push` at `work.rs:297-299` is unchanged; it pushes only what was committed.
3. After the commit, run `git status --porcelain` and count the paths still dirty. Print one
   line, `[engine] <id8> left <n> uncommitted path(s) outside its lock scope untouched`, and
   return the count. Never list those paths; they belong to other items.
4. An item with empty `lockdirs` keeps today's whole-tree commit, and `scope_label` is
   `whole tree (item has no lockdirs)`. In every other case `scope_label` is
   `lock scope (<k> paths)`. No other case may sweep.

### R2 — the evidence is limited to the same scope and says so

1. Change the diffstat at `work.rs:981` to
   `git diff --stat <head_before> <head_after> -- <scope...>` using the same `scope` as R1
   (whole tree when `lockdirs` is empty).
2. Add to the evidence struct (`gate.rs:44-52`) and its JSON (`gate.rs:58-67`):
   `"commit_scope": <scope_label>`, `"outside_scope_dirty": <n>`, and
   `"commits": [{"hash","subject"}, ...]` — the commits between the two heads whose subject ends
   in this item's `(<id8>)`, read from `git log --format=%h%x09%s <head_before>..<head_after>`.
   The `commit` field keeps its meaning (the head after the run) so nothing that reads it today
   breaks.
3. `Evidence::describe` (`gate.rs:68-83`) renders, before the diffstat, one sentence the
   verifier cannot miss: "Several agents commit to this checkout concurrently. The diffstat
   below is limited to this item's lock scope (<scope_label>); <n> path(s) outside that scope
   were left uncommitted and are not this item's. Commits carrying this item's id: <list>."
4. The verifier prompt (`gate.rs:92-107`) gains one rule: a file outside the item's lock scope
   is not evidence about this item unless the worker's own message claims it.

### R3 — a project-level list of always-shared paths

Add `commit_extra_paths: Vec<String>` to `Project` in `iter_core/src/lib.rs` (serde default
empty), settable through the existing project PUT and shown in the webui project settings next
to the accounts list. pdy-dev will set it to the paths its agents write outside their lock scope
by design: `Agent_Recommendations.md` at the repo root and the `*.agentmemory.iter.md` markers.
A glob is allowed because git pathspecs accept them.

### R4 — nothing else changes

Agents' own mid-run commits are untouched; pdy-dev already requires those to name a pathspec.
The lock mechanism, the bounce budget and the "accept" and "continue" actions are unchanged.

## 5. Tests

Each test is shown red under its named mutation before it counts as evidence.

| # | Test (location) | Setup | Assertion | Mutation that must turn it red |
|---|---|---|---|---|
| T1 | `work.rs` `#[cfg(test)] end_of_run_commit_stays_inside_lockdirs` | temp git repo with one prior commit; item `lockdirs=["{topdir}/a"]`; dirty files `a/x.rs` and `b/y.rs` | HEAD holds `a/x.rs` only; `b/y.rs` still dirty; `outside_scope_dirty == 1`; `scope_label` starts with `lock scope` | restore `git add -A` |
| T2 | `work.rs` `a_single_file_lockdir_is_a_valid_pathspec` | `lockdirs=["{topdir}/a/only.tsv"]`; dirty `a/only.tsv` and `a/other.tsv` | HEAD holds `only.tsv` only | expand a file lock to its parent directory |
| T3 | `work.rs` `extra_paths_are_committed_with_the_scope` | `commit_extra_paths=["Agent_Recommendations.md"]`; that file and `b/y.rs` dirty | the recommendations file is in HEAD; `b/y.rs` is not | drop the extra-path argument |
| T4 | `work.rs` `no_lockdirs_sweeps_and_says_so` | item with empty `lockdirs`; dirty `a/x.rs` and `b/y.rs` | both in HEAD; `scope_label == "whole tree (item has no lockdirs)"` | omit the label |
| T5 | `work.rs` `leftover_paths_are_counted_not_listed` | as T1, capture the log line | line contains `left 1 uncommitted path(s)` and does not contain `b/y.rs` | print the paths |
| T6 | `gate.rs` `#[cfg(test)] evidence_diffstat_ignores_sibling_commits` | between `head_before` and `head_after`: one commit in `a/` with subject ending `(<this id8>)`, one commit in `b/` ending `(<other id8>)` | diffstat names `a/` files only; `commits` holds exactly the first; `describe()` contains "limited to this item's lock scope" | drop the pathspec from the diff, or the id filter from the log |
| T7 | `e2e.sh`, new block after the existing account-rotation section | two items with disjoint lockdirs run on one checkout; the fake `claude` dirties both directories in every session | each item's commit holds only its own directory; neither verify row's diffstat names the other's files; both items close with `gate_bounces == 0` | restore `git add -A` — the second item to finish bounces on the first item's files |

T7 is the guard that would have caught both incidents.

## 6. Rollout

No change to `iter_data` storage or to any persisted record shape except the additive
`commit_extra_paths` field on the project record and three additive fields inside the
`verify` row's `evidence` object, which `iter_data` stores as opaque JSON. The webui gains one
settings field. Both engines, "StephenMBP" on the Mac and the Linux engine, need the new binary
on their next restart. Until then each behaves as today.

## 7. Acceptance, by hand

With two agents running in the shared pdy-dev checkout, let one finish:

1. Its end-of-run commit lists only its lock-scope paths and the extra paths.
2. The engine log reports the other agent's dirty count, without naming files.
3. `git status` still shows the other agent's files as uncommitted.
4. The item's `verify` row, if any, carries a diffstat naming no file outside its scope, the
   `commit_scope` label, and the `commits` list.
5. The item closes without a bounce caused by another agent's work.

## 8. Out of scope

Locking the working tree itself, so that two agents cannot dirty it at once; changing how
agents make their own mid-run commits; and the token hot-reload spec in
`iter_spec_account_hot_reload.md`, which shares no code with this fix.
