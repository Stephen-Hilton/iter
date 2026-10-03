---
id: 6a737446-4b3b-4b64-babf-ab6706dc4f0b
name: "GraphRAG document writer"
desc: "Applies the GraphRAG document rows waiting for this engine's projects — `store_doc` writes an uploaded document's original into the checkout, `remove_doc` removes it, `gitignore_path` adds or drops a `.gitignore` line — skipping any row whose path a running item has locked, claiming the rest one at a time, committing exactly the files each touched and reporting the outcome, so that documents uploaded in the web page land in the repository without ever touching a node file."
creator: "iter migrate5"
teststate: inherit
level: component
owner: bespoke
children:
  codedirs:  ["{topdir}/iter_engine/src/datasync.rs"]
  codenodes: []
  tests:     []
  reqs:      []
timestamps: {create: "2026-10-02 23:05:35Z", last_modified: "2026-10-02 23:15:33Z", last_tested: ""}
---

# GraphRAG document writer

## Summary

Saves files people upload for GraphRAG search into the project's folder (and keeps them out of git when asked), one at a time and never on top of running work.

## How it works

In iter5 datasync survives only for GraphRAG documents (spec §12.1 item 10); node edits go through the
project graph and Node-file sync, never datasync rows. On each tick the Engine tick loop sends its
heartbeat; when the reply's `datasync_waiting` names a project this engine serves (and the checkout is
not read-only), the loop's `run_filesync` calls `apply_waiting` (`iter_engine/src/datasync.rs`):

1. `GET /api/projects/{p}/datasync?state=claimable` lists the waiting rows.
2. `lock_scope` gives the paths a row writes (`.gitignore` for `gitignore_path`); a row whose path is
   inside a live work-item lock waits for a later tick.
3. `POST …/datasync/{id}/claim` — one engine wins; the others skip the row.
4. `apply_op` writes the base64 original (`store_doc`), removes it (`remove_doc`) or edits
   `.gitignore` (`gitignore_path`); it refuses any `*.iter.md` path and any path leaving the topdir.
5. `filesync::commit_paths` commits exactly the touched files (`iter: <summary>`) under the checkout's
   git lock, then `POST …/datasync/{id}/done {engine, outcome: applied|failed, commit, files, error}`.

## What goes in and out

In: datasync rows from iter_data (GraphRAG uploads). Out: document files and `.gitignore` edits in the
checkout, scoped git commits, done reports.

## Why it matters

iter_data never touches a repository; this is how an upload made in the browser reaches the checkout.
Refusing node files keeps document storage from bypassing the graph's conflict rules.

## Example

A person uploads `runbook.pdf` with "keep out of git". Two rows arrive: `store_doc docs/runbook.pdf` and
`gitignore_path docs/`; the engine writes the PDF, adds `/docs/` to `.gitignore`, commits only those
paths and reports both rows applied.
