---
id: 9c3a79fa-f953-421e-b6fe-f6e9fae195f4
name: "Storage interface"
desc: "Defines the one storage interface the rest of iter_data talks to — get, put, delete, query, scan over (table, partition key, sort key) rows of JSON, plus atomic versioned writes, lock grants and change-counter bumps — and its contract tests, which hold the ArangoDB backend to exactly-one-winner behaviour under concurrent writers on a throwaway database."
creator: "iter migrate5"
teststate: inherit
level: component
owner: bespoke
children:
  codedirs:  ["{topdir}/iter_data/src/storage.rs", "{topdir}/iter_data/src/contract_tests.rs", "{topdir}/iter_data/src/test_db.rs"]
  codenodes: []
  tests:     []
  reqs:      []
timestamps: {create: "2026-10-02 23:05:35Z", last_modified: "2026-10-02 23:10:16Z", last_tested: ""}
---

# Long Description

## Summary

The common socket the database plugs into, with the rules it must keep.

The storage interface sits between iter_data's handlers and the database. iter5 stores everything in ArangoDB; the interface keeps the handlers' calls in one shape and names the operations that must be atomic.

How it works: `iter_data/src/storage.rs: Storage` is an async Rust trait. Every table is modelled as rows keyed by a partition key and a sort key, holding a JSON body: `get`, `put`, `delete`, `query` (all rows for one partition key, in sort-key order) and `scan`. Three operations need the database's own atomicity and get their own methods: `put_versioned` (a create when the expected version is 0, otherwise an update that fails with `StorageError::Conflict` if the version moved), `acquire_lock` (write only when free, expired or already held by the same work item) and `bump_seq` (add one to a per-project, per-table counter that clients poll to see what changed). `arango()` hands the concrete backend to code that needs AQL (project graph, GraphRAG); helpers `body_str` / `body_u64` read row fields.

What goes in and out: the HTTP API, get_next, the settings graph, datasync, GraphRAG and the project graph store call it; the ArangoDB storage backend implements it. `contract_tests.rs` checks the contract — including races between 16 writers — each test on its own throwaway database created and dropped by `test_db.rs` (dev ArangoDB on :8529, databases `iter5_test_*`, overridable with `ITER5_TEST_ARANGO_URL` / `ITER5_TEST_ARANGO_PASSWORD`).

Why it matters: the rules that must be atomic are named once and held to by test.

Example: two engines claiming the same work item both call `put_versioned` with the version they read; exactly one wins and the other gets `Conflict` with the current row.
