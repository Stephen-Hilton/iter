---
id: 9c3a79fa-f953-421e-b6fe-f6e9fae195f4
name: "Storage interface and DynamoDB reader"
description: "Defines the one storage interface the API talks to — get, put, delete, query, scan, plus atomic versioned writes, lock grants and counter bumps — which ArangoDB implements for iter4, and reads iter3's DynamoDB tables through the same interface when they are migrated."
simple_description: "The common socket the database plugs into, plus a read-only plug for the old iter3 data."
level: component
owner: bespoke
teststate: inherit
children:
  codedirs:   ["{topdir}/iter_data/src/storage.rs", "{topdir}/iter_data/src/ddb.rs", "{topdir}/iter_data/src/contract_tests.rs", "{topdir}/iter_data/src/test_db.rs"]
  codenodes:  []
  inputs:     []
  outputs:    []
  bizreqs:    []
  techreqs:   []
  tests:      []
---

# Long Description

The storage interface sits between the HTTP API and the database. iter4 stores everything in ArangoDB (decided 2026-09-29: no SQLite, no DynamoDB serving); the interface keeps the API's calls in one shape, and lets the iter3 migration read DynamoDB through the same calls.

How it works: `iter_data/src/storage.rs: Storage` is a Rust trait. Every table is modelled as rows keyed by a partition key and a sort key, holding a JSON body: `get`, `put`, `delete`, `query` (all rows for one partition key, in sort-key order) and `scan`. Three operations need the database's own atomicity and get their own methods: `put_versioned` (a create when the expected version is 0, otherwise an update that fails with `StorageError::Conflict` if the version moved), `acquire_lock` (write only when free, expired or already held by the same work item) and `bump_seq` (add one to a per-project, per-table counter). Extra methods (`scan_keyed`, `put_keyed`, `all_versions`, `set_seq`) serve migrations. `iter_data/src/ddb.rs: DdbBackend` reads iter3's DynamoDB tables (`<prefix><table>`); it is only ever opened with `new_readonly`, which can create and change nothing.

What goes in and out: the HTTP API, the Architecture map store and Migrations call it; the ArangoDB storage backend implements it for serving, and the DynamoDB reader implements it as a migration source. `iter_data/src/contract_tests.rs` checks the contract on ArangoDB — including races between 16 writers — each test on its own throwaway database (`iter_data/src/test_db.rs`).

Why it matters: the rules that must be atomic (versioned writes, lock grants, counters) are named once, so the ArangoDB implementation is held to them by test, and a migration reads the old store without a second code path.

Example: `iter_data --migrate-from dynamodb --migrate-prefix iter3_` reads every iter3 table through this interface and writes it into ArangoDB through the same interface.
