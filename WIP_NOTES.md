# WIP branch `wip-2026-11-18`: Stephen's stashed work, restored

This branch is `fe6bf9e` with `stash@{0}` ("stephen-wip-before-engine-git-hardening-2026-11-18", ref
`2eaa0e2618187635c10e5d77abf680fe10f42f76`) applied by `git stash apply`. The stash itself is still in
`~/dev/iter` (`stash@{0}`), untouched. It is not merged into `main`, and it does not build as it stands
(see "Still unresolved").

`main` moved on after `fe6bf9e`: `bd6e5a5` and `6218f1b` added `iter_engine/src/gitsafe.rs`,
`gitrepair.rs` and `gitwrap.rs` and removed `work::git_lock`. That overlaps `githeal.rs` on this branch,
so this branch will not merge into `main` without the work described below.

## Conflict markers the stash carried (from the 2026-10-26 `stash pop`)

The stash held four files with conflict markers as text. "Updated upstream" was `main` at the time and
"Stashed changes" was the 2026-10-26 stash (`stash@{1}`, based on `b455836`). What that stash changed
was read from `git diff stash@{1}^1 stash@{1}`.

- `iter_data/src/main.rs`: resolved. Upstream added `mod mcp_docs;` and the stash added
  `#[cfg(test)] mod mcp_tests;`. Both are kept.
- `iter_data/src/mcp.rs` (the MCP `INSTRUCTIONS` text): resolved. Upstream rewrote the text, and the
  stash's only change was two added sentences ("capability lists or reads the capability documents …;
  locks_list shows who holds which code paths." and "A failed call returns isError with the API's
  status and reason; …"). The result is the upstream text with those two sentences after the
  settings_graph sentence. `iter_data/src/mcp_tests.rs` (from 2026-10-21) was written against the older
  text and may assert on wording that changed. Nobody has run it against this text.
- `iter_data/src/mcp.code.iter.md`: resolved. Upstream rewrote the paragraphs after "How it works", and
  the stash added one paragraph before it (the `instructions` at `initialize`, `isError` results,
  `mcp_tests.rs`). The result is the stash's paragraph followed by upstream's paragraphs.
- `iter_engine/src/sweep.rs`: resolved 2026-11-18 by Stephen's ruling. Upstream's `sweep_tags` is kept and the stash's `container_name`, `key_tags` and their test are dropped, so the file is now identical to upstream `fe6bf9e`. Background: upstream
  replaced `key_tags` with `sweep_tags` and moved the repeat key into `repeat_check` /
  `repeat_container`, so no `container:` tag is on the queue line any more. The stash shortened that
  `container:` tag with `container_name`, and its test (around line 728,
  `the_container_tag_is_the_node_name_not_its_path`) calls `key_tags`, which no longer exists. Upstream
  seems to have fixed the same overflow in another way, which would make `container_name` and its test
  obsolete. Deleting your code is your call, so the markers stay. If you agree, keep the upstream side
  and delete `container_name` and that test.

## `githeal.rs` against `gitsafe.rs` / `gitrepair.rs` on `main`

`githeal.rs` and `githeal_tests.rs` are unchanged from the stash. File by file:

- **`githeal.rs` uses `crate::work::git_lock`** (its `use` line, and `heal` takes the lock), which `main`
  removed. On `main` it must take `crate::gitsafe::guard(topdir)` instead (`let _g = ...`). That guard
  also holds the cross-process `flock` on `.git/iter-engine-git.mutex`, which the agents' git wrapper
  (`gitwrap.rs`) takes too.
- **`githeal::git()` runs `run_shell` directly.** On `main` every engine git call goes through
  `gitsafe::run` (or `gitsafe::retrying`), so it gets `-c rerere.enabled=false -c rerere.autoupdate=false`,
  `ITER_GIT_LOCKED=1`, a stale-lock sweep, retries on lock errors, and the failure class the repair loop
  counts. `githeal::git` should become `gitsafe::run(top, &format!("git {args}"), secs)`.
- **`githeal::dirs` duplicates `gitsafe::dirs`** (`gitsafe::GitDirs { git_dir, common, work_trees }`).
  Use the `gitsafe` one.
- **Stale locks have two rules that disagree.** `githeal::remove_stale_locks` deletes any lock older than
  2 minutes, or written before this boot, without asking whether a git is running. On `main`,
  `gitsafe::clear_stale_locks` deletes a lock only when it is older than 2 minutes AND no live git process
  (by `/proc/<pid>/comm` and cwd / `GIT_DIR`) uses the repository. `gitsafe` also searches
  `.git/worktrees/*/` and skips the object store. A merge should drop `remove_stale_locks` and call
  `gitsafe::clear_stale_locks`. Its "written before this boot" rule is safe to add to `gitsafe`, since no
  process alive now can hold a lock older than the boot: delete such a lock without the liveness check.
- **There are now three holds on a checkout.**
  - `merge::hold_reason`: a merge conflict waits on a human.
  - `gitrepair::hold_reason`: three items in a row failed with one git error class. It runs one repair
    agent session, the engine verifies the result, then either releases work and requeues those items, or
    files one P0 and re-checks every 5 minutes.
  - `githeal::Healer::ready`: the check at startup, or after a crash, finds git broken.
    It writes `.git/iter-git-broken` and re-checks every 5 minutes.

  `githeal` also gates `run_filesync`, ghost repair and replayed closes, not only dispatch. A merge should
  keep `githeal` as the deterministic first step: at startup, and also as the first thing a `gitrepair`
  incident tries before it starts an agent session (it is cheaper and needs no account). Both should
  report one paused state, so an operator sees one reason, not two. `gitrepair::verify` could also run
  `githeal::fsck` (`git fsck --connectivity-only`), which `main`'s check does not.
- **Re-check periods match.** Both use 5 minutes (`githeal::RECHECK_SECS`, `gitrepair::RECHECK`).
- **`engine.rs` (stash):** the `githeal` field, its `Healer::default()`, the gate around `run_filesync`,
  and the `ready` check before per-project work. The edits are next to, not on, `main`'s new lines (the
  graph-sync early return inside `run_filesync`, and the `gitrepair` drive and hold in `dispatch`). They
  should merge with little textual conflict, but the gate is redundant while graph sync is off. Also in
  the stash's `engine.rs`: `tail_path` for lock-path wait reasons, which is unrelated and independent.
- **`main.rs` (stash):** `mod githeal;`, next to `main`'s `mod gitrepair; mod gitsafe; mod gitwrap;`.
- **`loop.code.iter.md`:** both sides add text. The stash adds a "Git self-heal after a crash" section and
  `githeal.rs` to `codedirs`. `main` adds step 6 (graph sync and git pause) and a "Git serialization"
  section. Keep both, and describe the three holds once.
- **`githeal_tests.rs`** builds its repositories with `run_shell` under `std::env::temp_dir()`. On this
  machine the other git tests use `~/.pdy-tmp` and a 300 s setup timeout, because the disk stalls. Its
  `stale_locks_go_and_a_live_one_stays` stands for "a running git" with a lock only seconds old; under
  `main`'s rule it should also cover a real live git process, as `gitsafe_tests.rs` does with a real
  `git hash-object --stdin`.

## Still unresolved, and not built

- No conflict markers remain on this branch (`sweep.rs` was resolved by taking upstream).
- Compile status: see the commit that resolved `sweep.rs` and the `cargo check` result reported with it.
