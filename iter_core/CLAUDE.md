# iter_core: the shared library

iter_core is the one library compiled into both programs: `iter_engine` and `iter_data` each list it as a
path dependency (`iter_core = { path = "../iter_core" }`). Nothing is copied: cargo compiles the current
`iter_core/` source into each binary at build time. Never copy iter_core source files into `iter_engine/`
or `iter_data/`; a second copy would drift from this one.

It holds the work-item rules, the settings-graph types, `nodefile` (the node-file library both sides must
agree on byte for byte), the standard test result, and `rag/` (GraphRAG text extraction, chunking and
embedding).

## After changing anything in iter_core/

A change here changes both programs, so finish with all of these, from the repo root:

1. `cargo test -p iter_core` (or `test/iter_core/cargo_test.sh`)
2. `cargo build --release -p iter_engine -p iter_data`: both, never only one.
3. `cargo test -p iter_engine` and `cargo test -p iter_data` (iter_data's tests need the dev ArangoDB on
   127.0.0.1:8529; start it with `docker start iter-arango-dev`). A change that compiles in iter_core can
   still break a caller.
4. Say in your report that both binaries must be redeployed: one rebuilt alone leaves the other on the old
   iter_core rules (see the repo-root `CLAUDE.md`), and that `installers/` needs a rebuild and release.
