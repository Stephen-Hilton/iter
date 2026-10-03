---
id: d24c167b-19e6-4f5d-8e0b-801ae555cfa7
name: "Project graph editor"
desc: "Adds editing to the Project graph: create a node attached to another, configure any node or edge in a lightbox, draw, remove, drag the end of, copy and paste edges, move or delete a node's file and run its tests — each edit goes straight to iter_data and shows at once, while an engine writes the file; for a designed project (no repository yet) it shows the Designed banner and the Build dialog."
creator: "iter migrate5"
teststate: inherit
level: component
owner: bespoke
children:
  codedirs:  ["{topdir}/webui/graphedit.js"]
  codenodes: []
  tests:     []
  reqs:      []
timestamps: {create: "2026-10-02 23:05:35Z", last_modified: "2026-10-02 23:15:00Z", last_tested: ""}
---

# Project graph editor

## Summary

Lets a person design and change the project right on the map; the server records the change at once and an engine writes the file.

## How it works (`webui/graphedit.js`)

`IterGraphEdit` decorates the Project graph viewer. A viewer (read-only role) sees only the sync state. Editors get:

- **Toolbar**: sync state (how many nodes wait for an engine to write or delete their file, or are only designed), **+ Node**, **Connect**, **Paste edge**, **Run tests**, and a conflicts button when file sync kept a losing version.
- **Node menu** (right-click, long-press or ⌘-click): Configure…, Add node here…, Connect from here, Paste edge here, Run tests, Move file…, Hide, Delete… (asks for a reason).
- **Edges**: select one and drag either end onto another node (`POST …/graph/edges/move`); right-click for Configure…, Copy edge, Remove… (asks for a reason). ⌘C / ⌘V copy and paste an edge onto the selected node.
- **Keys**: N new node, C connect, E configure, T run tests, Del delete / remove.

Each action is one call under `/api/projects/{p}`: `POST graph/nodes` (the server plans the file path by the folder rules and adds the attaching edge to the parent's file), `PATCH` / `DELETE graph/nodes/{id}`, `POST graph/nodes/{id}/move`, `POST` / `DELETE graph/edges`, `POST graph/edges/move`, `POST graph/run_tests` (a test work item). The node is redrawn at once with a "⟳ pending sync" badge until the engine has written and committed the file.

**Designer → Build**: when every node of the project is `designed` (no repository yet) a banner says "Designed — not built yet" with **Build…**: pick a registered engine and a folder, optionally queue the plan agent; `POST …/build` records it and the banner follows `GET …/build` until the engine reports the repository built. The project node's detail shows its build record and file-sync state.

Dialogs, menus, the Configure… lightbox and the edge handles come from the webui kit.

## What goes in and out

Calls only iter_data's project-graph routes; the engine serving the project picks pending writes up on its next heartbeat (`files/pending` → write → commit → `files/ack`).

## Why it matters

It turns the map from a picture into the way to grow — or start — a project, with every change landing as an ordinary committed file.

## Example

On the Data context a developer chooses Add node here…, picks container, types "Ledger API" and saves. The node appears inside Data with "⟳ pending sync"; seconds later the engine has written `ledger_api/ledger_api.code.iter.md`, added it to Data's `children.codenodes`, committed both, and the badge clears.
