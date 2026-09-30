---
id: da8a51ee-f804-4fc2-b560-371dd469a897
name: "Data server client"
description: "Sends each engine request to the data server over HTTP with the engine's token and turns the answer into JSON or a typed error, so that no other part of the engine handles the network itself."
simple_description: "The engine's line to the central server that holds all the work items and the project map."
level: component
owner: bespoke
teststate: inherit
children:
  codedirs:   ["{topdir}/iter_engine/src/client.rs"]
  codenodes:  []
  inputs:     []
  outputs:    []
  bizreqs:    []
  techreqs:   []
  tests:      []
---

# Long Description

The Data server client carries every request the engine makes to iter_data, the central data server: reading the queue, claiming an item, writing its result, pushing the project map, sending a heartbeat.

How it works: `iter_engine/src/client.rs: Api` holds the server's base address, the bearer token and one blocking `reqwest` client with a 30-second timeout. `Api::new` builds it; `get`, `put`, `post` and `delete` each add the `Authorization: Bearer` header and send the request. `handle` turns a 2xx answer into JSON (an empty body becomes null) and anything else into `ApiError { status, body }`. When no answer comes back at all, the error carries status 0, which is how callers tell a network outage from a refusal.

It is built in three places: `iter_engine/src/main.rs` for the engine itself (address from `--data-url`, else `$ITER_DATA_URL`, else `.iter/config.json`; see `resolve_data_url`), `env` in `cli.rs` for commands run inside an agent session, and `sync::conn` for map commands run from a shell. Its users are the Engine scheduler loop, the Work runner, the Duplicate work judge, the Map uploader and test sweep, and the iter command line. It calls nothing but the network.

Why it matters: one small, blocking client keeps the engine's worker threads simple, and the status-0 rule lets the Work runner's `with_retry` and `is_transient` ride out an outage (retrying with backoff, then saving a close to disk) instead of losing a finished run. For tests, `client::fake::serve` starts a real throwaway HTTP server on 127.0.0.1 so engine code can be tested without iter_data.

Example: when a run ends, the Work runner calls `api.put("/api/projects/<p>/workitems/<id>", record)`. If another writer got there first, the server answers 409 and the runner re-reads the record and tries once more; if the network is down, the error has status 0 and the write is retried.
