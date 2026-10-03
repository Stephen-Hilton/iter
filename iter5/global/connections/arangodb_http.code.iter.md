---
id: 72607ae2-6782-49c3-bd71-131a6699d9c2
name: "ArangoDB HTTP API"
desc: "iter_data's only storage connection: ArangoDB's REST API over HTTP (default http://127.0.0.1:8529, database iter5), AQL queries through /_db/<db>/_api/cursor plus document, collection and stream-transaction calls, with HTTP basic auth as root (ARANGO_ROOT_PASSWORD). Supplied by the ArangoDB server; consumed by iter_data's ArangoDB backend, which every other storage user goes through."
creator: "stephen"
teststate: inherit
connects:
  from: ["{topdir}/map/external/arangodb/arangodb.code.iter.md"]
  to: ["{topdir}/iter_data/src/arango.code.iter.md"]
level: connection
children:
  codedirs:  []
  codenodes: []
  tests:     []
  reqs:      []
timestamps: {create: "2026-10-02 23:12:30Z", last_modified: "2026-10-02 23:12:30Z", last_tested: ""}
---
# ArangoDB HTTP API

## Protocol

HTTP/JSON to ArangoDB's REST API (`iter_data/src/arango.rs`, reqwest):
`/_db/_system/_api/version` to wait for the server, then every call under
`/_db/<db>/...` — `_api/cursor` for AQL (`{query, bindVars, batchSize}`),
document and collection endpoints, and stream transactions for multi-document
writes. `ARANGO_URL` (default `http://127.0.0.1:8529`) and `ARANGO_DB`
(`iter5`) choose the server and database.

## Auth

HTTP basic auth, user `root`, password `ARANGO_ROOT_PASSWORD` (the container
refuses to start without it). The port is never published beyond localhost
(`127.0.0.1:8630` for the console in docker mode).

## Who connects

Only iter_data's ArangoDB backend, behind the `Storage` trait
(`storage.rs`): every iter_data component that keeps data — queue, locks,
project graph, settings graph, test logs, GraphRAG — goes through it. Test
tooling (`tools/cargo-test-crate.sh`) only probes `/_api/version`.
