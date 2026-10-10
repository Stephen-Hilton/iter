---
id: effc23a0-100e-479b-ae15-c8b020d4fccb
name: "ArangoDB storage backend"
desc: "Stores all of iter's records and every project graph in ArangoDB over its plain HTTP API: creates the database, one collection per table, their indexes, the node and link collections with the named graph, and the GraphRAG and project-graph collections on start; performs versioned writes, lock grants and counter bumps each as one atomic AQL statement; and offers raw AQL and stream transactions to the project graph store and GraphRAG."
creator: "iter migrate5"
teststate: inherit
level: component
owner: bespoke
children:
  codedirs:  ["{topdir}/iter_data/src/arango.rs"]
  codenodes: []
  tests:     []
  reqs:      []
timestamps: {create: "2026-10-02 23:05:35Z", last_modified: "2026-10-02 23:10:16Z", last_tested: ""}
---

# Long Description

## Summary

Where iter saves everything it knows, in the ArangoDB database.

The ArangoDB backend is the only implementation of the Storage interface in iter5. It keeps every table as an ArangoDB collection and every project graph as documents and edges, all in one database (default `iter5`).

How it works: `iter_data/src/arango.rs: ArangoBackend::new` connects over ArangoDB's HTTP API with `reqwest` — no driver — and runs `ensure_schema`, which creates the database, one document collection per table in `iter_core::TABLES`, a `[pk, sk]` index on each, the `versions` counter collection, the project-graph collections `node` (vertices) and `link` (edges) with their project indexes and the named graph `iter_map`, then asks GraphRAG (`rag::ensure_schema`) and the Project graph store (`nodes::ensure_schema`: `node_conflict`, `test_log`) for theirs. Creation is additive, so a restart changes nothing. Each row is stored as `{_key, pk, sk, version, expires, workid, body}`; `doc_key` builds `_key` from pk and sk with forbidden characters `%`-escaped (or a SHA-256 hash when longer than 254 bytes), so a lookup is a primary-index hit. The three atomic operations are single AQL statements: `put_versioned` updates only when `version` still equals the expected value; `acquire_lock` inserts only when the row is absent, expired or already this item's; `bump_seq` is an upsert adding one. ArangoDB's write-write conflict (1200) and unique-key violation (1210) are read as "someone else won". `aql`, `aql_retry`, and the stream-transaction trio `begin` / `aql_in` / `commit` serve the project graph store and GraphRAG directly; `ping` backs `/health`.

What goes in and out: everything in iter_data reaches it through the Storage interface, and the project graph store, file sync, test logs and GraphRAG also through `Storage::arango`. It calls only ArangoDB.

Why it matters: the queue's safety — one winner per lock, no lost updates — rests on these statements being atomic.

Example: two engines request the lock on `{topdir}/iter_data/` at the same instant; one AQL statement succeeds, the other returns `free: false` with the winner's row, which the API sends back as a 409.
