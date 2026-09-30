---
id: 9a427159-f4de-4917-ba2e-ce1f674b2f2e
name: "Data migrations"
description: "Copies an existing iter3 installation's records into this server in one run — every DynamoDB table row for row, with versions and change counters — reading the source only and skipping rows already present."
simple_description: "One-time tools that bring an older iter's history into the new one."
level: component
owner: bespoke
teststate: inherit
children:
  codedirs:   ["{topdir}/iter_data/src/migrate_ddb.rs"]
  codenodes:  []
  inputs:     []
  outputs:    []
  bizreqs:    []
  techreqs:   []
  tests:      []
---

# Long Description

Migrations are one-shot imports. They run when iter_data is started with a migration flag, print a report, and exit without serving.

How it works: `iter_data/src/migrate_ddb.rs` handles `--migrate-from dynamodb --migrate-prefix iter3_`. `run_cli` opens the source with `DdbBackend::new_readonly`, which cannot create or change tables, and `copy_all` walks each table: it reads every raw row with its keys and version (`scan_keyed`), skips rows the target already has unless `--migrate-overwrite` is given, and writes the rest unchanged (`put_keyed`). Then it copies each change counter, never moving a target counter backwards. With `--migrate-dry-run` it only counts. The report lists source and target counts per table, which must match. (The V2 import, `--migrate-v2`, which read an old SQLite queue, was retired with SQLite on 2026-09-29.)

What goes in and out: people run it from the command line (`iter_data/src/main.rs`); it writes through the Storage interface into the ArangoDB storage backend.

Why it matters: without it, moving to iter4 would mean losing years of work-item history, locks, agents and spend.

Example: the 2026-09-28 goal run copied pdy-dev's iter3 tables into ArangoDB with matching counts on both sides — 3,047 work items and 18,637 detail rows among them.
