---
id: 97586f16-d50a-4b7d-bc1d-3a15dbff9f6f
name: "Move iter3's records into iter4"
description: "An operator copies every iter3 record from DynamoDB into iter4's ArangoDB database once, with keys, versions and change counters unchanged, so that iter4 starts with all the old work history."
teststate: inherit
children:
  codenodes:  ["{topdir}/iter_data/src/migration.code.iter.md", "{topdir}/iter_data/src/storage.code.iter.md", "{topdir}/iter_data/src/arango.code.iter.md"]
  tests:      []
flowmap:
  summary: "The operator starts the data server once with `--migrate-from dynamodb`. It opens every iter3 DynamoDB table read-only, reads each row with its keys, and writes it to the matching ArangoDB collection unchanged, skipping rows already copied; the change counters keep their values, and the source and target counts are printed per table."
  sequence:
  - actor:operator
  - '{topdir}/iter_data/src/migration.code.iter.md'
  - '{topdir}/iter_data/src/storage.code.iter.md'
  - '{topdir}/iter_data/src/arango.code.iter.md'
  process_flow:
  - step: 1
    from: actor:operator
    to: '{topdir}/iter_data/src/migration.code.iter.md'
    what: "The operator starts the data server with `--migrate-from dynamodb --migrate-prefix iter3_`, asking it to copy every iter3 table instead of serving requests."
    plain: "The operator starts the copy."
    evidence: "iter_data --backend arango --migrate-from dynamodb --migrate-prefix iter3_ [--migrate-dry-run] [--migrate-overwrite]"
  - step: 2
    from: '{topdir}/iter_data/src/migration.code.iter.md'
    to: '{topdir}/iter_data/src/storage.code.iter.md'
    what: "The migration opens the old DynamoDB tables through the shared storage layer in a read-only mode that cannot create or change tables, and reads every row of each table with its keys."
    plain: "Every iter3 table is read, never written."
    evidence: "iter_data/src/migrate_ddb.rs: DdbBackend::new_readonly, scan_keyed"
  - step: 3
    from: '{topdir}/iter_data/src/migration.code.iter.md'
    to: '{topdir}/iter_data/src/arango.code.iter.md'
    what: "The migration writes each row into the matching ArangoDB collection with its keys and version unchanged (skipping rows already there unless told to overwrite), then sets each change counter to its old value."
    plain: "Each row lands in the new database unchanged."
    evidence: "iter_data/src/migrate_ddb.rs: put_keyed per row, set_seq per counter"
  data_flow:
  - step: 1
    from: '{topdir}/iter_data/src/migration.code.iter.md'
    to: '{topdir}/iter_data/src/arango.code.iter.md'
    data: "Every row (partition key, sort key, version and JSON body) and every change counter; they rest in the ArangoDB collection of the same name."
    stored: true
    plain: "The rows rest in the matching collections."
---

# Move iter3's records into iter4

An operator moving from iter3 to iter4 wants the new system to start with all of the old work history: work items, their detail rows, agents, projects, engines, locks, spend and users.

They start the data server once with `--migrate-from dynamodb --migrate-prefix iter3_`. It opens every `iter3_*` DynamoDB table in a read-only mode that cannot create or change tables, reads each row with its keys, and writes it into the ArangoDB collection of the same name with keys, version and body unchanged. The change counters, which the web page and engines use to notice updates, are copied with their values.

The copy is safe to run again: rows already present are skipped unless `--migrate-overwrite` is given, and `--migrate-dry-run` only reports what would happen. At the end it prints source and target counts for every table, which must match.
