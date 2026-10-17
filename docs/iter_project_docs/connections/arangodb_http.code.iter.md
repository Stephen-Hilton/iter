---
id: 72607ae2-6782-49c3-bd71-131a6699d9c2
name: "ArangoDB HTTP API"
desc: "iter_data's only storage connection: ArangoDB's REST API over HTTP on this machine or HTTPS to another (default http://127.0.0.1:8529, database iter_db), AQL queries through /_db/<db>/_api/cursor plus document, collection and stream-transaction calls, with HTTP basic auth as iter's own user (iter_user, rw on its database only; root only to create that user and database). Supplied by the ArangoDB server; consumed by iter_data's ArangoDB backend, which every other storage user goes through."
creator: "stephen"
teststate: inherit
connects:
  from: ["{topdir}/docs/iter_project_docs/map/external/arangodb/arangodb.code.iter.md"]
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
`/_api/version` to wait for the server (any answer, even a refusal, counts),
then every call under `/_db/<db>/...` — `_api/cursor` for AQL
(`{query, bindVars, batchSize}`), document and collection endpoints, and stream
transactions for multi-document writes. `ARANGO_URL` (default
`http://127.0.0.1:8529`) and `ARANGO_DB` (`iter_db`; `iter5` on installs from
before 2026-10-17) choose the server and database.

To another machine it is HTTPS: the certificate is pinned
(`ARANGO_CERT_FINGERPRINT`), checked against a CA file (`ARANGO_CA_CERT`) or
the public CAs. Plain `http://` goes only to this machine unless
`ARANGO_ALLOW_PLAIN_HTTP` says the network is private.

## Auth

HTTP basic auth as `ARANGO_USER` (default `iter_user`) with `ARANGO_PASSWORD`.
That user holds `rw` on its own database only. When it cannot sign in, or lacks
`rw`, iter_data signs in once as `root` with `ARANGO_ROOT_PASSWORD` to create
the user and database and grant `rw`, then goes on as the user. The user never
gets root's password. A local ArangoDB is never published beyond localhost
(`127.0.0.1:8630` for the console in docker mode).

## Who connects

Only iter_data's ArangoDB backend, behind the `Storage` trait
(`storage.rs`): every iter_data component that keeps data — queue, locks,
project graph, settings graph, test logs, GraphRAG — goes through it. Test
tooling (`test/cargo-test-crate.sh`) only probes `/_api/version`.
