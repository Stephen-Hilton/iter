---
id: 4a557f7e-7e0a-4cb3-9a29-536f25a98f7f
name: "iter_data — data server"
desc: "Answers every HTTP request from engines, agents, MCP clients and the web page: checks the token and the caller's project rights, enforces the work queue's rules (versioned work-item writes, lease-bound locks, server-side get_next), keeps the settings graph (who serves, bills and runs what) and each project's graph (one node per *.iter.md file, synced with the repo through engines), stores test logs, answers GraphRAG searches and MCP tool calls, and serves the embedded web page. It is the only program that talks to ArangoDB and it never touches a repository."
creator: "iter migrate5"
teststate: inherit
level: container
owner: bespoke
children:
  codedirs:  ["{thisfiledir}/"]
  codenodes: ["{topdir}/iter_data/src/server.code.iter.md", "{topdir}/iter_data/src/api.code.iter.md", "{topdir}/iter_data/src/auth.code.iter.md", "{topdir}/iter_data/src/authz.code.iter.md", "{topdir}/iter_data/src/next.code.iter.md", "{topdir}/iter_data/src/settings.code.iter.md", "{topdir}/iter_data/src/nodes.code.iter.md", "{topdir}/iter_data/src/graph.code.iter.md", "{topdir}/iter_data/src/filesync.code.iter.md", "{topdir}/iter_data/src/testlogs.code.iter.md", "{topdir}/iter_data/src/datasync.code.iter.md", "{topdir}/iter_data/src/rag/rag.code.iter.md", "{topdir}/iter_data/src/mcp.code.iter.md", "{topdir}/iter_data/src/storage.code.iter.md", "{topdir}/iter_data/src/arango.code.iter.md"]
  tests:     ["{thisfiledir}/test/*.test.iter.md"]
  reqs:      ["{topdir}/docs/iter5_spec.md"]
timestamps: {create: "2026-10-02 23:05:35Z", last_modified: "2026-10-02 23:10:16Z", last_tested: ""}
---

# Long Description

## Summary

The server where every request from the engines, the agents and the web page arrives and gets answered.

iter_data is iter5's central server (cargo package `iter_data`, default port 8400, database `iter5`). Engines, the agents running inside them (through the `iter` command line or MCP) and people in the browser all reach iter's state through it. It is the only program that talks to the database, and it never reads or writes a repository: file changes always travel through an engine.

How it works: `iter_data/src/main.rs` (**Server startup**) opens ArangoDB, bootstraps the `admin` user, the GraphRAG `summary` agent and the settings graph, and serves one axum router. Every request passes **Login and tokens** (`auth.rs`, bearer token → user and role) and **Project access rules** (`authz.rs`, a middleware over every `/api/projects/{p}/…` route). The **HTTP API** (`api.rs`) carries users, agents, tooling, projects, engines and heartbeats, work items and their history rows, locks and spend; **Server-side get_next** (`next.rs`) picks and claims the next work item for an engine. The **Settings graph** (`settings.rs`) holds every setting on a node or an edge and answers each engine's assignments. The **Project graph store** (`nodes.rs`), **Project graph API** (`graph.rs`, `graph_view.rs`), **Node file sync and build** (`filesync.rs`, `sync_hooks.rs`) and **Test results and logs** (`testlogs.rs`) keep one node per node file in step with the repo. The **GraphRAG file inbox** (`datasync.rs`), **GraphRAG index** (`rag/`) and **MCP gateway** (`mcp.rs`) complete the surface. Everything is stored through the **Storage interface** (`storage.rs`) on the **ArangoDB storage backend** (`arango.rs`).

What goes in and out: iter_engine calls it for assignments, heartbeats, get_next, locks, results, file sync, builds, test results and GraphRAG jobs; the webui calls it for everything a person sees or changes; MCP clients call `POST /mcp`. It uses iter_core for the shared rules (node-file library, settings types, dependency and lock rules) and calls only ArangoDB.

Why it matters: it is the referee. Without it engines cannot coordinate, nobody can see the queue or the map, and nothing is remembered.

Example: `GET /health` answers `{"ok": true, "backend": "arango", "db": "iter5", "version": …, "ts": …}`; the engine prints that backend on its first line, and the container's health check uses the same call.
