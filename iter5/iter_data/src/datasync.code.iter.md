---
id: cf0ab2a4-b264-4123-8920-f0f44dedefea
name: "GraphRAG file inbox"
desc: "Holds the repository file operations GraphRAG asks for — store an uploaded document under the docs directory, remove it, add or drop a .gitignore line — as pending rows waiting for an engine, tells each engine on its heartbeat how many are waiting per project, and lets exactly one engine claim each (a versioned write; a claim lapses after ten minutes) and report it applied, failed or to retry. Node file edits never pass through here: they travel through node file sync."
creator: "iter migrate5"
teststate: inherit
level: component
owner: bespoke
children:
  codedirs:  ["{topdir}/iter_data/src/datasync.rs"]
  codenodes: []
  tests:     []
  reqs:      []
timestamps: {create: "2026-10-02 23:05:35Z", last_modified: "2026-10-02 23:10:16Z", last_tested: ""}
---

# Long Description

## Summary

Keeps GraphRAG's file changes until an engine writes them into the project.

iter_data never touches a repository, yet a document uploaded on the GraphRAG tab should end up committed under the project's docs folder. The GraphRAG file inbox (`iter_data/src/datasync.rs`) is the hand-over point: it accepts such operations at once and keeps them until an engine serving the project applies them, without queueing them behind agent work.

How it works: `create` stores a row in the `datasync` table (pk project, sk a time-ordered id: `{id, op, lockdirs, summary, state, created, by, engine, claim_expires, attempts, …}`) and refuses any op but `store_doc`, `remove_doc` and `gitignore_path` (`OPS`). `waiting` counts claimable rows per project for the heartbeat reply's `datasync_waiting`. The routes are `GET /api/projects/{p}/datasync?state=` (list), `POST …/datasync/{id}/claim` (a versioned write, so exactly one engine wins; `claimable` treats a claim older than `CLAIM_SEC` = 600 s as free again) and `POST …/datasync/{id}/done` with `applied`, `failed` or `retry` plus the commit and files. The engine applies a claimed op once no running item's lock overlaps it, commits just those files and pushes.

What goes in and out: GraphRAG (`rag/mod.rs`: uploads, deletes, the `docs_gitignore` setting) creates rows; the HTTP API's heartbeat reports the counts; iter_engine's datasync service claims and finishes them.

Why it matters: without it uploaded documents would live only in the database and never reach the repository.

Example: a person uploads `runbook.pdf`; a `store_doc` row appears, the next heartbeat tells the serving engine one op is waiting, it claims it, writes `docs/runbook.pdf`, commits, and reports `applied` with the commit hash.
