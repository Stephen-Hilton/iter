---
id: d33af173-f275-4eba-9f38-cc1daf025bd8
name: "Node-file walker"
desc: "Finds every `*.iter.md` file whose filename names a node type under a project's scan dirs, never entering `.git`, `target`, `node_modules`, `.iter` or `.claude`, dropping whatever git ignores and never following symlinks; offers one full walk for validate, migrate5 and the test sweep, plus the stat / list / walk pieces the engine's incremental filescan is built from, and maps checkout paths to and from the `{topdir}/…` form, so that every part agrees on which files are nodes and no build output copy ever becomes one."
creator: "agent.code"
teststate: inherit
level: component
owner: bespoke
children:
  codedirs:  ["{topdir}/iter_local/src/walk.rs", "{topdir}/iter_local/src/lib.rs"]
  codenodes: []
  tests:     []
  reqs:      []
timestamps: {create: "2026-10-02 23:10:52Z", last_modified: "2026-10-02 23:10:52Z", last_tested: ""}
---

# Node-file walker

## Summary

Finds the project's description files on disk, quickly and without being fooled by build output or ignored folders.

## How it works

`iter_local/src/walk.rs` (iter5 spec §3.2):

- `is_node_name` asks `iter_core::nodefile::type_of` whether a filename is a node file (plain
  `*.iter.md` context docs are not).
- `scan_roots` turns the project node's `scandirs` into directories under the topdir.
- `node_files(topdir, roots)` is one full walk: sorted node-file paths, git-ignored ones dropped.
- For the engine's incremental filescan: `walk(root, ignored)` returns `Walked` (node files plus every
  directory entered with its mtime), `list_dir` re-lists one directory, and `stat` gives a file's
  `(mtime, size)` as a `Stat`, so a tick only re-lists directories whose mtime moved and only re-reads
  files whose stat moved.
- `topdir_path` turns an absolute path into `{topdir}/rel`; `real_path` turns a stored path back into
  an absolute one and refuses anything leaving the topdir.

`iter_local/src/lib.rs` holds the shared pieces: `SKIP_DIRS` / `is_skip_dir`, `GitIgnored` (one
`git ls-files --others --ignored` listing per walk, `contains`), `drop_git_ignored` (`git check-ignore`)
and `now_iso`. Outside a git repository nothing is dropped.

## What goes in and out

In: a topdir and its scan dirs. Out: node-file paths, directory stats and path conversions for
Node-file sync, validate, the test runner, the test sweep and migrate5.

## Why it matters

Build tools such as AWS CDK copy whole source folders (node files included) into ignored output folders;
without the git-ignore rule each deploy would add stray copies of nodes to the graph. The stat-based
pieces keep a tick cheap on a large checkout.

## Example

A checkout has `cdk.out/` in `.gitignore` holding a copy of `api/api.code.iter.md`; `node_files` returns
only the real `api/api.code.iter.md`.
