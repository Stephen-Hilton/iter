---
id: 7b63d1a2-84f0-405e-9dbf-691237a04a6a
name: "webui — web page"
desc: "The one browser page people use to see and steer iter5: the Intro story and new-project designer wizard, the Work queue, the Project graph (view, design and edit every node file and its links), GraphRAG search and the Settings graph. Static HTML, CSS and JavaScript compiled into iter_data; it sends every request to the data server, which makes every decision."
creator: "iter migrate5"
teststate: omit
level: container
owner: bespoke
children:
  codedirs:  ["{thisfiledir}/"]
  codenodes: ["{topdir}/webui/queue.code.iter.md", "{topdir}/webui/projectgraph.code.iter.md", "{topdir}/webui/grapheditor.code.iter.md", "{topdir}/webui/settings.code.iter.md", "{topdir}/webui/kit.code.iter.md", "{topdir}/webui/intro.code.iter.md", "{topdir}/webui/rag.code.iter.md"]
  tests:     ["{topdir}/e2e/playwright.test.iter.md"]
  reqs:      []
timestamps: {create: "2026-10-02 23:05:35Z", last_modified: "2026-10-02 23:15:00Z", last_tested: ""}
---

# webui — web page

## Summary

The page in the browser where people see and steer the work, design projects and wire engines, accounts and agents together.

webui is static HTML, CSS and JavaScript with no server code of its own. At build time `iter_data/src/main.rs` compiles every file (`index.html`, the scripts and stylesheets, the Cytoscape libraries in `vendor/`) into the server binary, so the container needs no web folder; `./deploy.sh local` serves them from disk for fast editing. It is served on iter_data's port (:8400).

## How it is built

`index.html` holds the login, the shared header (project picker, the project's status line: what stops it, engine dot, running, spend) and five tabs, switched by `setTab` and the keys 1–5, with the state in the URL hash (`#tab=graph&p=<project>`):

| tab | file(s) | component |
|---|---|---|
| Intro | `intro.js`, `intro.css` | Intro story and designer wizard |
| Work queue | inside `index.html` | Work queue page |
| Project graph | `graph.js`, `graph.css`, `vendor/` + `graphedit.js` | Project graph viewer + Project graph editor |
| GraphRAG | `rag.js`, `rag.css` | GraphRAG tab |
| Settings | `settings.js` | Settings graph |

`kit.js` / `kit.css` (component *webui kit*) supply the dialogs, menus, Configure… lightbox, edge-end drag handles and edge clipboard both graph views share. Each tab script mounts itself (`IterIntro`, `IterGraph` / `IterGraphEdit`, `IterRag`, `IterSettings`) with a small `ctx` (the `api` helper, project, role, navigation callbacks). Colours are `:root` tokens in `index.html`; the page is dark and phone-width tolerant.

## What goes in and out

It calls only iter_data's HTTP API: `/auth/login`, work items and their details, engines, projects, `/api/projects/{p}/graph*` (view, nodes, edges, build, run_tests, conflicts), `/testlogs`, `/rag*`, `/api/settings/*`. Nothing calls it. iter4's interface views and the `/graph/edits` / datasync edit queue are gone: graph edits change nodes directly and the engine writes the files.

## Why it matters

It is how a human files work, answers an agent's question, designs a project before it has a repository, wires engines and accounts to projects, and sees which engine is running what. It holds no rules: a stale tab can ask for something, but the server still refuses what the rules refuse.

## Tests

The Playwright suite (`e2e/playwright.test.iter.md`, run by `e2e/playwright/run.sh`) drives every tab in Chromium against a throwaway server. `teststate: omit` keeps the page out of the test sweep, which runs only unit-test nodes.

## Example

Opening `http://127.0.0.1:8400/#tab=graph&p=iter5` logs in, selects project iter5 and draws its graph: contexts holding containers holding components, connection nodes between them, and the test and requirement nodes around them.
