---
id: 96833f8c-3001-4029-9797-312e23da8c21
name: "Build, ship and prove"
desc: "Packages iter5 into one container (ArangoDB CE + iter_data on :8400, Arango console on 127.0.0.1:8630), starts it in docker or local mode with deploy.sh, installs iter_data or an engine on any machine from the prebuilt release binaries with the scripts in installers/ (an engine machine makes its own engine key), and proves the whole stack with test/e2e.sh (one engine serving two projects on the mock provider) and the Playwright web suite."
creator: "iter migrate5"
teststate: inherit
level: context
owner: bespoke
children:
  codedirs:  ["{topdir}/installers/", "{topdir}/docs/iter_project_docs/map/delivery/scripts/", "{topdir}/test/"]
  codenodes: ["{topdir}/installers/installers.code.iter.md", "{topdir}/docs/iter_project_docs/map/delivery/scripts/scripts.code.iter.md"]
  tests:     []
  reqs:      []
timestamps: {create: "2026-10-02 23:05:35Z", last_modified: "2026-10-02 23:12:30Z", last_tested: ""}
---
# Build, ship and prove

## Summary

How iter5 is packaged, switched on, put on each machine, and checked before
anyone relies on it.

It holds two containers: **docker — all-in-one container** and
**scripts — deploy, engine setup, e2e and test tools** (which owns the
`test/` component), and **installers** (prebuilt binaries and one install
script per program and platform).

## How it works

- `./deploy.sh docker` builds `iter_data-linux-<arch>` and runs
  `installers/iter_data/docker/install.sh`, which starts container `iter5`: API and web page on **:8400**,
  the Arango console on **127.0.0.1:8630**, database `iter_db` as `iter_user` (`iter5` when it already
  holds one). Every iter_data installer takes `--arango-url` for an ArangoDB you already run.
  `./deploy.sh local` builds release binaries into `bin/` and runs iter_data
  natively against the dev ArangoDB container on **:8529** (the fast loop).
  Both wait for `/health` to say `"ok":true`.
- `iter_data/iter_engine_setup.sh`, served by iter_data at `/iter_engine_setup.sh`,
  sets up the one engine of a machine: checks tools, finds or builds
  `iter_engine`, checks the token, writes `~/.iter/.env` (mode 600) and starts
  the engine, which then appears in the Settings graph.
- `test/e2e.sh` runs a real iter_data (throwaway `iter5_e2e_*` database) and one
  real engine serving **two** copies of `test/e2e/sample5`, all on the `mock`
  provider, and checks work items, file sync both ways, designer build, test
  results, settings edges and MCP. `test/e2e/playwright/run.sh`
  drives the web page. `--live` adds one real Claude item.

## What goes in and out

It builds iter_data (with the web page and the setup script embedded) and
iter_engine from source, runs ArangoDB, and is run by people (operators) and,
through the test nodes' scripts, by the engine's test runner.

## Why it matters

Without it, standing iter5 up means knowing the right flags, ports and secrets
by heart, and there is no whole-system check before a change reaches a real
project.
