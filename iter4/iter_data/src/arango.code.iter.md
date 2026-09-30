---
id: effc23a0-100e-479b-ae15-c8b020d4fccb
name: "ArangoDB storage backend"
description: "Stores iter's records and the program map in ArangoDB over its plain HTTP API, creating the database, collections, indexes and graph on start, and performing versioned writes, lock grants and counter bumps each as one atomic statement."
simple_description: "The default way iter saves its information, in the ArangoDB database."
level: component
owner: bespoke
teststate: inherit
children:
  codedirs:   ["{topdir}/iter_data/src/arango.rs"]
  codenodes:  []
  inputs:     []
  outputs:    []
  bizreqs:    []
  techreqs:   []
  tests:      []
---

# Long Description

The ArangoDB backend is the default implementation of the Storage interface. It keeps every table as an ArangoDB collection and the program map as an ArangoDB graph, all in one database (default `iter4`).

How it works: `iter_data/src/arango.rs: ArangoBackend::new` connects over ArangoDB's HTTP API with `reqwest` — no driver — and runs `ensure_schema`, which creates the database, one document collection per table in `iter_core::TABLES`, a `[pk, sk]` index on each, the `versions` counter collection, and the graph `iter_map` (vertex collection `node`, edge collection `link`). Creation is additive, so a restart changes nothing. Each row is stored as `{_key, pk, sk, version, expires, workid, body}`; `doc_key` builds `_key` from pk and sk with forbidden characters `%`-escaped (or a SHA-256 hash when longer than 254 bytes), so a lookup is a primary-index hit. The three atomic operations are single AQL statements: `put_versioned` updates only when `version` still equals the expected value; `acquire_lock` inserts only when the row is absent, expired or already this item's; `bump_seq` is an upsert adding one. ArangoDB's write-write conflict (1200) and unique-key violation (1210) are read as "someone else won". The map routes use `begin`/`aql_in`/`commit` stream transactions to replace a project's vertices and edges in one go, and AQL traversals for neighbour queries.

What goes in and out: the HTTP API and Architecture map call it through the Storage interface (the map also reaches it directly via `Storage::arango`); Migrations writes into it. It calls only ArangoDB.

Why it matters: the queue's safety — one winner per lock, no lost updates — rests on these statements being atomic.

Example: two engines request the lock on `{topdir}/iter_data/` at the same instant; one AQL statement succeeds, the other returns `free: false` with the winner's row, which the API sends back as a 409.
