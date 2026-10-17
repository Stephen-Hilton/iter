---
id: da8a51ee-f804-4fc2-b560-371dd469a897
name: "Data server client"
desc: "Sends each engine request to the data server over HTTP with the engine's token and turns the answer into JSON or a typed error, so that no other part of the engine handles the network itself."
creator: "iter migrate5"
teststate: inherit
level: component
owner: bespoke
children:
  codedirs:  ["{topdir}/iter_engine/src/client.rs", "{topdir}/iter_engine/src/key.rs"]
  codenodes: []
  tests:     []
  reqs:      []
timestamps: {create: "2026-10-02 23:05:35Z", last_modified: "2026-10-02 23:15:33Z", last_tested: ""}
---

# Long Description

## Summary

The engine's line to the central server that holds all the work items and the project map.

The Data server client carries every request the engine makes to iter_data, the central data server: assignments, heartbeats, `next` claims, work-item writes, file-sync batches and acks, test results, spend and usage.

How it works: `iter_engine/src/client.rs: Api` holds the server's base address, the bearer token and one blocking `reqwest` client with a 30-second timeout. `Api::new` builds it; `get`, `put`, `post` and `delete` each add the `Authorization: Bearer` header and send the request. `handle` turns a 2xx answer into JSON (an empty body becomes null) and anything else into `ApiError { status, body }`. When no answer comes back at all, the error carries status 0, which is how callers tell a network outage from a refusal.

It is built in three places: `iter_engine/src/main.rs` for the engine itself (address from `--data-url`, else `$ITER_DATA_URL`; signs in with the machine's key), `env` in `cli.rs` for queue verbs run inside an agent session (`ITER_DATA_URL`, `ITER_ENGINE_TOKEN`), and `sync::conn` for checkout verbs run from a shell (`iter sync`, `sweep`, `rag sync`). Its users are the Engine tick loop, the Work runner, the Duplicate work judge, Node-file sync, the Test sweep, the GraphRAG worker and the iter command line. It calls nothing but the network.

Why it matters: one small, blocking client keeps the engine's worker threads simple, and the status-0 rule lets the Work runner's `with_retry` and `is_transient` ride out an outage (retrying with backoff, then saving a close to disk) instead of losing a finished run. For tests, `client::fake::serve` starts a real throwaway HTTP server on 127.0.0.1 so engine code can be tested without iter_data.

How the engine proves who it is (2026-10-11): with the machine's own Ed25519 key (`iter_engine key new`, `key.rs`). `Api::with_key` signs in (`POST /auth/engine`) for a session token. It sends that token as its bearer, signs in again well before the session ends, and does so at once on a 401. When `key new` left a next key beside the key file (`engine.key.next`), `adopt_next` tries it once a minute from the tick, and at once when the current key is refused. As soon as iter_data accepts it, every clone of the client signs in with it and the next key becomes the key file. Agent sessions get the session token the engine holds when they start (`Api::new`). Every connection follows `trust.rs` (2026-10-17): iter_data's certificate pinned (or CA-checked with `key trust --ca`), plain http only to this machine, and no call carrying a token before the certificate is trusted; the sign-in signs the fingerprint it sees and pins what iter_data confirms. `rotate_key` sends a replacement key signed by the current one (`iter_engine key rotate`).

Example: when a run ends, the Work runner calls `api.put("/api/projects/<p>/workitems/<id>", record)`. If another writer got there first, the server answers 409 and the runner re-reads the record and tries once more; if the network is down, the error has status 0 and the write is retried.
