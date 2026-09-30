---
id: 4a557f7e-7e0a-4cb3-9a29-536f25a98f7f
name: "iter_data — data server"
description: "Receives every HTTP request from engines, agents and the web page, checks who is asking, enforces the queue's rules, and reads or writes the matching records in ArangoDB, SQLite or DynamoDB; it also serves the web page and stores the program map."
simple_description: "The server where every request from the workers and the web page arrives and gets answered."
level: container
owner: bespoke
teststate: inherit
children:
  codedirs:   ["{thisfiledir}/"]
  codenodes:  ["{topdir}/iter_data/src/api.code.iter.md", "{topdir}/iter_data/src/auth.code.iter.md", "{topdir}/iter_data/src/storage.code.iter.md", "{topdir}/iter_data/src/arango.code.iter.md", "{topdir}/iter_data/src/graph.code.iter.md", "{topdir}/iter_data/src/migration.code.iter.md", "{topdir}/iter_data/src/datasync.code.iter.md", "{topdir}/iter_data/src/server.code.iter.md", "{topdir}/iter_data/src/rag/rag.code.iter.md", "{topdir}/iter_data/src/mcp.code.iter.md"]
  inputs:     []
  outputs:    []
  bizreqs:    []
  techreqs:   []
  tests:      ["{thisfiledir}/test/*.tests.iter.md"]
---

# Long Description

iter_data is the central server. It is the only program that talks to the database, and it serves the web page. Engines, the agents inside them, and people in the browser all reach iter's state through it.

How it works: `iter_data/src/main.rs` reads the flags, opens its ArangoDB database (the only store iter4 has), creates the first `admin` user if there are none, and starts an axum HTTP server on port 8300. Requests go to the **HTTP API** (`api.rs: router`), which checks the bearer token with **Auth** (`auth.rs`) and then calls the **Storage interface** (`storage.rs: Storage`). Three writes need the database's own atomicity and have their own methods: versioned work-item writes (a write only lands if the version still matches), lock acquire, and change-counter bumps. The **Architecture map** routes (`graph.rs`, `graph_view.rs`) store and answer questions about the program map, and `datasync.rs` holds graph edits until an engine applies them. The page files are compiled into the binary. Run with `--migrate-from dynamodb`, it performs a one-time **Migrations** copy instead of serving.

What goes in and out: iter_engine calls it for heartbeats, claims, locks, results, map pushes and graph-edit pickup; the webui calls it for everything a person sees or changes; the `iter` command line calls it on an agent's behalf. It uses iter_core for the rules and calls only its database.

Why it matters: it is the referee. Without it engines cannot coordinate, people cannot see the queue, and nothing is remembered.

Example: `GET /health` answers `{"ok": true, "backend": "arango", "db": "iter4", …}`; the engine prints that backend on its first line, and the container's health check uses the same call.

Built and tested as the cargo package `iter_data` in the iter4 workspace (`cargo test -p iter_data` from `iter4/`).
