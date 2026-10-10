---
id: 14c76725-6f0b-4907-9e4c-327f4036dcbe
name: "iter_core unit tests"
desc: "Runs cargo test -p iter_core — the work-item rules (dependencies, priorities, accounts, lock shape), the node-file library (round trips, conform idempotence over every *.iter.md in the checkout, legacy migration, globs, path planning, edges), the settings-graph types, the standard test result, schedules, dedup keys, wait graph, cluster hold and question forms — and reports one standard result line counted per test. Fast, no services needed."
creator: "iter migrate5"
teststate: inherit
children:
  codedirs:  []
  codenodes: []
  tests:     ["{thisfiledir}/cargo_test.sh"]
  reqs:      []
timestamps: {create: "2026-10-02 23:05:35Z", last_modified: "2026-10-02 23:15:00Z", last_tested: ""}
---

# iter_core — unit tests

One script, `cargo_test.sh`, runs the crate's whole `cargo test` through `tools/cargo-test-crate.sh iter_core` and prints the standard result JSON (spec §3.5) as its last line: every test in the `normal` bucket (passed → pass, failed → err), one `details` row per test. Exit 0 green, 1 red, 2 could not run (no cargo, did not compile).

## What it covers

- **Work-item model** (`lib.rs`): dependency gate and cycles, priority bands, account ladder, lock shape, close-gate settings.
- **Node-file library** (`nodefile/tests.rs`): parse/render round trip, `conform` idempotence on hand-made messy files, on the iter4 files in `nodefile/fixtures/` and on **every `*.iter.md` in this checkout** (`conform_idempotent_over_repo_tree`) — so a malformed node file anywhere in iter5 turns this red — legacy key migration, glob resolution, designer path planning with sequence / uuid12 collisions, edge derivation, add / remove child.
- **Settings-graph types**, **standard test result** parsing and verdicts, **schedules**, **dedup keys**, **wait graph**, **cluster hold**, **question forms**.

Needs only cargo; runs in well under a minute when the build is cached. Run it with `iter runtests iter_core/test/iter_core.test.iter.md` (add `--no-record` to keep the result out of iter_data). By name or id it is ambiguous while the iter4 fixture `src/nodefile/fixtures/iter_core.tests.iter.md` carries the same id and name.
