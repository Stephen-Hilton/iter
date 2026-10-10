---
id: 03903818-853b-4c8b-800a-b5ebaa7811a4
name: "webui Playwright suite"
desc: "Runs e2e/playwright/run.sh — builds iter_data, serves the webui against a throwaway ArangoDB database seeded through the API, and drives Chromium through login, the work queue, the project graph (render, type filters, configure lightbox, create node, drag an edge end, copy/paste an edge), the settings graph and the designer build flow with no console errors — and reports the specs passed and failed as one standard result line. Needs node, cargo and ArangoDB on :8529; the test sweep leaves it out."
creator: "agent.code"
teststate: omit
children:
  codedirs:  []
  codenodes: []
  tests:     ["{thisfiledir}/tests/playwright_suite.sh"]
  reqs:      []
timestamps: {create: "2026-10-02 23:15:00Z", last_modified: "2026-10-02 23:15:00Z", last_tested: ""}
---

# webui Playwright suite

`tests/playwright_suite.sh` runs `e2e/playwright/run.sh` with the list reporter and turns its summary into the standard result JSON (spec §3.5) on the last line: `normal` = specs passed (flaky counts as passed) and failed, `details` = one row per spec. Exit 0 green, 1 red, 2 could not run.

## What it covers

`e2e/playwright/specs/`: `auth` (sign in and out, a bad password refused), `tabs` (every tab loads cleanly, phone width without horizontal scroll), `queue` (every item renders, the detail dialog, park from the Actions menu), `graph-view` (every node type and link drawn, type filter chips and presets in the URL hash, the detail pane), `graph-edit` (configure lightbox for a node and an edge, create a node with its planned path, draw / remove / drag / copy-paste an edge, delete a node), `settings` (the settings graph, configure nodes and edges, tags, dragging an edge end onto a placeholder, copy an edge onto a new engine), `designer` (wizard → designed project → Build → the engine builds it; Build refuses an unregistered engine). Any console error or warning a test does not expect fails it; screenshots land in `e2e/playwright/screenshots/`.

## Needs

node (with `@playwright/test` and a Chromium build; `run.sh` installs or links them), cargo, and the dev ArangoDB on `127.0.0.1:8529`. Missing any → exit 2. `PW_ARGS` passes extra Playwright arguments (e.g. `specs/graph-edit.spec.js`).

`teststate: omit` keeps it out of the test sweep; run it with `iter runtests --node "webui Playwright suite"`.
