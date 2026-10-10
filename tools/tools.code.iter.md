---
id: 8855b650-a6ee-43a3-bdbe-b862f6a41bdb
name: "tools — engine setup and test helpers"
desc: "Three helper scripts: iter_engine_setup.sh sets up the one iter5 engine of a machine (served by iter_data at /iter_engine_setup.sh; checks git/curl/claude, finds or builds iter_engine into ~/.iter5/bin, checks the engine token, writes ~/.iter5/.env mode 600, starts the engine and waits for its check-in; --status / --stop); cargo-test-crate.sh runs one crate's cargo tests and reports them as a test result line; fetch_model.sh downloads the GraphRAG embedding model."
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

## `iter_engine_setup.sh` — one engine per machine

iter5 runs one engine per machine, serving every project the settings graph
connects to it, so setup needs no project. The script is embedded in
iter_data (`include_bytes!` in `iter_data/src/main.rs`) and served at
`<data_url>/iter_engine_setup.sh`, so nothing is copied by hand:

```bash
curl -fsSL http://127.0.0.1:8400/iter_engine_setup.sh | bash -s -- \
  --data-url http://127.0.0.1:8400 --engine mbp --token <engine token> --start
```

In order, each step saying what it found and overwriting nothing: check the
tools (git, curl, the `claude` CLI the agents run in); find `iter_engine`
(`--bin`, `$ITER_ENGINE_BIN`, `~/.iter5/bin`) or build it from GitHub with
cargo; check the server answers and the token signs in; write the env file
(default `~/.iter5/.env`, mode 600) with `ITER_ENGINE_TOKEN` plus one token per
account the engine holds; with `--start`, run
`iter_engine --data-url URL --env-file FILE --name NAME` in the background and
wait for its first heartbeat. Then, in the Settings tab, draw `serves` edges
(engine → project, with the checkout as `topdir`) and `holds` edges (engine →
account). `--status` and `--stop` check or stop the engine it started.

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
