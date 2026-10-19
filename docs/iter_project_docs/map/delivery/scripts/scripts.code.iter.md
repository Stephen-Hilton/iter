---
id: 87c47715-828b-47ca-9cc5-06adc2c00800
name: "scripts — deploy, engine setup, e2e and test tools"
desc: "The shell entry points around the binaries: deploy.sh (docker or local mode on :8400), test/e2e.sh (the whole stack — iter_data on a throwaway iter_e2e_* database and one engine serving two projects on the mock provider), the Playwright web suite under test/e2e/playwright, and the test/ helpers (one crate's tests as a test result)."
creator: "iter migrate5"
teststate: inherit
level: container
owner: bespoke
children:
  codedirs:  ["{topdir}/deploy.sh", "{topdir}/test/e2e.sh", "{topdir}/test/e2e/"]
  codenodes: ["{topdir}/test/test.code.iter.md"]
  tests:     []
  reqs:      []
timestamps: {create: "2026-10-02 23:05:35Z", last_modified: "2026-10-19 23:02:12Z", last_tested: ""}
---
# scripts — deploy, engine setup, e2e and test tools

## Summary

The commands that switch iter on and prove the whole thing works.

## How it works

- **`deploy.sh <mode>`.** `docker` builds `iter_data-linux-<arch>` with
  `installers/build.sh` and runs `installers/iter_data/docker/install.sh`,
  which writes `<iter dir>/iter_data/config/data.env` (iter dir `~/iter`) from the repo `.env`
  (`ITER_ADMIN_PASSWORD`, `ITER_JWT_SECRET`; read key by key, never sourced)
  and brings up container `iter`. `local` builds
  release `iter_data` and `iter_engine` into `bin/` (remove-then-copy: macOS
  kills a binary copied over a running one), starts or reuses the dev
  ArangoDB container on :8529 and runs iter_data natively. Every mode waits
  for `/health`. `ITER_PORT` changes the port (default 8400).
- **`test/e2e.sh [--live] [--keep]`.** Has iter_data create database `iter_e2e_<random>`
  and a user of the same name on the dev ArangoDB (root's password, as on a fresh install), starts iter_data and ONE engine, copies `test/e2e/sample5` twice
  into scratch git repos (two projects, both served by that engine), and
  checks, section by section: work-item round trips on the `mock` provider,
  file edit → node, node edit → file + commit, create / delete both ways,
  conflicts, designer → build → repo + plan item, standard test results → test
  node + log, account switch/stop through the `bills` edge, deactivating a
  `serves` edge and MCP tool calls.
  The fixture's node files are stored as `*.iter.md.fixture` so iter's own
  scan never reads them. The database is dropped on success and failure.
  `E2E_ONLY=b,c` runs only some sections.
- **`test/e2e/playwright/run.sh`.** The web suite: login, queue, project graph,
  type filters, configure lightbox, edge drag / copy / paste, settings graph,
  designer build flow; screenshots in `test/e2e/playwright/screenshots/` (git-ignored).
- **`test/`** — see its component node; installers live in `installers/` (its own node).

## What goes in and out

Builds the binaries, drives the container and the dev ArangoDB, and talks to
a running iter_data over its HTTP API exactly as engines and browsers do.

## Why it matters

Without the e2e suite, a change that breaks how the engine, the server and
the files work together would only be noticed on a live project.
