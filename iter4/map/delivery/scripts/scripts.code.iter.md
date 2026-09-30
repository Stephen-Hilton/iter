---
id: 87c47715-828b-47ca-9cc5-06adc2c00800
name: "scripts — deploy, e2e and test tools"
description: "Starts iter in docker or local mode, runs the full end-to-end check of a real server and engine against a sample project, and runs one crate's unit tests in the format the test groups read."
simple_description: "The helper commands that switch iter on and check that the whole thing works."
level: container
owner: bespoke
teststate: inherit
children:
  codedirs:   ["{topdir}/deploy.sh", "{topdir}/e2e.sh", "{topdir}/e2e/", "{topdir}/tools/"]
  codenodes:  []
  inputs:     []
  outputs:    []
  bizreqs:    []
  techreqs:   []
  tests:      []
---

# Long Description

These shell scripts start iter4 and prove it works.

How it works: `deploy.sh` takes one mode. `docker` writes `run/docker.env` from the repository's `.env` (admin password and token secret, never sourcing the file) and brings the container up. `local` builds release binaries into `bin/` (remove-then-copy, because macOS kills a binary copied over a running one), starts or reuses an ArangoDB dev container on port 8529, and runs iter_data natively, serving the web page from `webui/` on disk. Every mode waits for `/health` before printing the address. `e2e.sh` copies the sample project in `e2e/sampleV3/` to a scratch folder, starts a real iter_data on a random port and a real engine with a stand-in Claude and a fake usage server, and walks every feature through the API: claims, locks and leases, the close gate, schedules, dedup, accounts, cluster holds, graph edits and datasync. It has 78 named checks today and drops its throwaway ArangoDB database on success and failure alike. `tools/cargo-test-crate.sh <crate>` runs `cargo test -p <crate>` (iter_data's tests need the dev ArangoDB on 8529), and ends with `ITER_RESULT pass= fail= total=` and exit 0 (green), 1 (red) or 2 (could not run).

What goes in and out: it builds iter_data and iter_engine, drives the docker container, and is called by the test groups in the map (through the engine's test runner) and by people.

Why it matters: without the e2e suite, a change that breaks how engine and server work together would only be noticed on a live project.

Example: `./e2e.sh` finishes with every check printed as `[e2e] ok: …`, or stops at the first `FAIL:` and cleans up.
