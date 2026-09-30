---
id: 96833f8c-3001-4029-9797-312e23da8c21
name: "Build, ship and prove"
description: "Packages iter into a runnable container, starts it in one of three modes, and drives a real stack through every feature end to end, so a release can be stood up on a laptop or a small server with one command and trusted."
simple_description: "How iter is packaged, switched on, and checked before anyone relies on it."
level: context
owner: bespoke
teststate: inherit
children:
  codedirs:   ["{topdir}/docker/", "{topdir}/map/delivery/scripts/"]
  codenodes:  ["{topdir}/docker/docker.code.iter.md", "{topdir}/map/delivery/scripts/scripts.code.iter.md"]
  inputs:     []
  outputs:    []
  bizreqs:    []
  techreqs:   []
  tests:      []
---

# Long Description

The Delivery area turns the source into something running and proves it works. It holds two containers: **docker — all-in-one container** (the image that runs ArangoDB and iter_data together) and **scripts — deploy, e2e and test tools**.

How it works: `deploy.sh` is the switch. `./deploy.sh docker` writes a secrets file and starts the container from `docker/compose.yml`; `./deploy.sh local` runs a native iter_data against a separate ArangoDB container on port 8529 (the fast development loop). Each mode waits until `/health` answers `"ok":true`. The end-to-end script `e2e.sh` copies a sample project (`e2e/sampleV3/`), starts a real iter_data and a real engine with a fake Claude, and walks every feature — queueing, locks, the close gate, schedules, dedup, accounts, graph edits — checking each result through the API. `tools/cargo-test-crate.sh` runs one crate's unit tests and prints the result line the test groups expect.

What goes in and out: it builds the Data and API and Engines containers from source and serves the Web page inside iter_data; the test groups in the map call `tools/cargo-test-crate.sh`, and the engine's test sweep records those results on the map.

Why it matters: without it, standing iter up means knowing the right flags, ports and password handling by heart, and there is no whole-system check before a change reaches a real project.

Example: `./e2e.sh arango` creates a throwaway ArangoDB database `iter4_e2e_<random>`, runs the whole suite against it, and drops the database whether the run passes or fails.
