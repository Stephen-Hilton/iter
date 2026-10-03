---
id: dd41bd94-321b-49cb-8f3f-81532241f6a3
name: "Node-file sync and build"
desc: "Keeps a checkout's node files and iter_data's project graph in step, both ways, every tick: walks the scan dirs for changed `*.iter.md` files (stat-based, full walk every 5 minutes), conforms them with `iter_core::nodefile` and writes the conformed text back, posts them to `files/sync` with the version last acked, writes the server's rewrites, then fetches pending graph edits and writes, moves or deletes those files, commits only the touched paths, pushes and acks; it also builds a designed project into a new repo and is `iter sync` from a shell."
creator: "iter migrate5"
teststate: inherit
level: component
owner: bespoke
children:
  codedirs:  ["{topdir}/iter_engine/src/filesync.rs", "{topdir}/iter_engine/src/filesync_tests.rs", "{topdir}/iter_engine/src/sync.rs"]
  codenodes: []
  tests:     []
  reqs:      []
timestamps: {create: "2026-10-02 23:05:35Z", last_modified: "2026-10-02 23:15:33Z", last_tested: ""}
---

# Node-file sync and build

## Summary

Makes sure a change to a description file shows up in the project graph within seconds, and a change made in the graph lands in the files and in git.

## How it works

`iter_engine/src/filesync.rs: tick` runs for every served project on every engine tick (iter5 spec §3),
with per-checkout state in `.iter/filesync.json` (`FileSyncState`: each file's id, content hash, semantic
hash, acked version and stat):

1. **filescan** (`scan`, using iter_local's walker): the project node's `scandirs`, git-ignore aware;
   only files whose `(mtime, size)` moved are re-read and only directories whose mtime moved are
   re-listed; a full walk on the first tick and every `FULL_EVERY` (5 minutes). Agent memory files are
   never synced.
2. **conform**: `nodefile::conform_against` with the last synced semantic hash, so a hand edit bumps
   `timestamps.last_modified` (stamped with the file's mtime); a changed text is written back unless the
   file sits inside a live work-item lock (then deferred) or the checkout is read-only.
3. **sync**: `POST /api/projects/{p}/files/sync {engine, full, files:[{path, text, hash, base_version}],
   deleted}`; `full` only for a checkout with no saved state. Applied versions are remembered,
   `rewrite` texts written (lock-aware), conflicts logged; conform write-backs and rewrites are
   committed together as `iter: conform <n> node files` (`commit_conform`, `commit_paths`).
4. **pending** (when the heartbeat reply's `files_waiting` names the project): `GET files/pending`,
   write / delete / move each file (`apply_pending`; paths leaving the topdir are refused, paths inside a
   live lock wait), commit only the touched files `iter: graph edit — …`, push in the background when a
   remote exists (`push_async`), then `POST files/ack`.

After a round that changed something, `maybe_rag` re-indexes GraphRAG (at most once a minute).
`build` is the designer push (§3.4, `build_waiting`): `mkdir -p`, `git init` (+ `origin` from the project's
`gitrepo`), `.gitignore` with `.iter/`, every pending file written, one commit `iter: build from design`,
acks, `POST build/done`.

`iter_engine/src/sync.rs` is the connection the checkout verbs share (`checkout_root`, `project_file_in`,
`project_name`, `conn`: `--data-url` / `$ITER_DATA_URL`, `$ITER_ENGINE_TOKEN`, project from
`--project` / `$ITER_PROJECT` / the project node) and `iter sync` itself (`sync_verb`): one immediate
round of the same `tick`.

## What goes in and out

In: node files on disk, pending node writes from iter_data. Out: `files/sync` batches, acks,
`build/done`, rewritten node files, scoped git commits and pushes.

## Why it matters

The engine is the only writer to the repository; this is the only path by which graph edits become files
and file edits become graph nodes. Lock awareness keeps it from rewriting a file an agent is editing.

## Example

A developer edits `api/api.code.iter.md` by hand. Within one tick the file is re-read, conformed (its
`last_modified` set to the edit time), posted with its base version, and the graph shows the new desc;
the conform rewrite is committed as `iter: conform 1 node files`.
