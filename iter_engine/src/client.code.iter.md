---
id: da8a51ee-f804-4fc2-b560-371dd469a897
name: "Data server client"
desc: "Sends each engine request to the data server over HTTP with the engine's token and turns the answer into JSON or a typed error, so that no other part of the engine handles the network itself."
creator: "iter migrate5"
teststate: inherit
level: component
owner: bespoke
children:
  codedirs:  ["{topdir}/iter_engine/src/client.rs"]
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

It is built in three places: `iter_engine/src/main.rs` for the engine itself (address from `--data-url`, else `$ITER_DATA_URL`; token `ITER_ENGINE_TOKEN` from the env file), `env` in `cli.rs` for queue verbs run inside an agent session (`ITER_DATA_URL`, `ITER_ENGINE_TOKEN`), and `sync::conn` for checkout verbs run from a shell (`iter sync`, `sweep`, `rag sync`). Its users are the Engine tick loop, the Work runner, the Duplicate work judge, Node-file sync, the Test sweep, the GraphRAG worker and the iter command line. It calls nothing but the network.

Why it matters: one small, blocking client keeps the engine's worker threads simple, and the status-0 rule lets the Work runner's `with_retry` and `is_transient` ride out an outage (retrying with backoff, then saving a close to disk) instead of losing a finished run. For tests, `client::fake::serve` starts a real throwaway HTTP server on 127.0.0.1 so engine code can be tested without iter_data.

Example: when a run ends, the Work runner calls `api.put("/api/projects/<p>/workitems/<id>", record)`. If another writer got there first, the server answers 409 and the runner re-reads the record and tries once more; if the network is down, the error has status 0 and the write is retried.
