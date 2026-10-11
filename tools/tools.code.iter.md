---
id: 8855b650-a6ee-43a3-bdbe-b862f6a41bdb
name: "tools — engine setup and test helpers"
desc: "Helper scripts: iter_engine_setup.sh is retired (served by iter_data at /iter_engine_setup.sh, it only points at install.sh / install.ps1, which set up an engine machine now); cargo-test-crate.sh runs one crate's cargo tests and reports them as a test result line; fetch_model.sh downloads the GraphRAG embedding model."
creator: "stephen"
teststate: inherit
level: component
owner: bespoke
children:
  codedirs:  ["{thisfiledir}/"]
  codenodes: []
  tests:     []
  reqs:      []
timestamps: {create: "2026-10-02 23:12:30Z", last_modified: "2026-10-02 23:12:30Z", last_tested: ""}
---
# tools — engine setup and test helpers

## `iter_engine_setup.sh` — retired

Retired 2026-10-11. An engine machine is set up with the install script at
the repo root (`install.sh` on Linux, `install.ps1` on Windows), which makes
the machine's own engine key. The script stays embedded in iter_data
(`include_bytes!` in `iter_data/src/main.rs`) and served at
`<data_url>/iter_engine_setup.sh` only so an old link prints where to go.

## `cargo-test-crate.sh <crate>`

Runs `cargo test -p <crate>` from the workspace root and ends with the legacy
result line `ITER_RESULT pass= fail= total=` (still accepted by
`iter_core::testresult`), exit 0 green / 1 red / 2 could not run (no cargo, no
dev ArangoDB on :8529 for iter_data, or a build error). The test nodes'
`*/test/cargo_test.sh` scripts call it.

## `fetch_model.sh`

Downloads `sentence-transformers/all-MiniLM-L6-v2` into
`models/all-MiniLM-L6-v2/` (checksum-verified, idempotent); the docker image
copies it in and iter_data / the engine embed with it.
