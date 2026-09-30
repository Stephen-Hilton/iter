---
id: d24c167b-19e6-4f5d-8e0b-801ae555cfa7
name: "Project graph editor"
description: "Adds editing controls to the Project graph (new context or child part, connect two parts, new or edited requirement or use case, run a test group) and sends each change to the data server as a queued edit, so that a project can be built from the map without opening the repo."
simple_description: "Lets a person add and connect parts of the project right on the map, and the system writes the files for them."
level: component
owner: bespoke
teststate: inherit
children:
  codedirs:   ["{topdir}/webui/graphedit.js"]
  codenodes:  []
  inputs:     ["{topdir}/interfaces/graph-edits/graph-edits.interface.iter.md"]
  outputs:    []
  bizreqs:    []
  techreqs:   []
  tests:      []
---

# Long Description

The Project graph editor is the "build it here" half of the Project graph tab. It lets a signed-in editor change the project from the map; it never writes files itself.

How it works (`webui/graphedit.js`): after each drawing the page calls `IterGraphEdit.attach(root, G, ctx)`, where `G` is the map view and `ctx` gives the page's API helper, project and role. A viewer sees "read-only". Everyone else gets a toolbar (`+ Context`, `+ Global object`, `Edit a global object`, `Connect`, `Run tests`) and, in a selected node's panel, `+ Container` / `+ Component`, `Connect from here` and `Run its tests`. `open(kind, node)` shows a dialog for the chosen action; on save it builds one operation as JSON (`new_node`, `connect`, `new_global`, `edit_body` or `run_tests`) and sends it with `POST /api/projects/{p}/graph/edits`. The dialog then says "Accepted — waiting to sync to an engine". `refreshSync` and `showSync` read `GET …/datasync` and show "⟳ n waiting to sync" or "✗ n failed" in the toolbar, with a list of each edit's state, engine and commit.

What happens next is outside the page: the data server stores a file-changing edit as a waiting row (a `run_tests` request becomes a `test` work item instead), the first engine serving the project picks it up on its next heartbeat, the Graph edit writer writes the files, the engine commits and pushes them and uploads the new map, and the Project graph viewer redraws.

It depends on the Project graph viewer (which draws the map it decorates) and on the data server's graph-edit and datasync routes.

Why it matters: it turns the map from a picture into a way to grow the project, with the queue's locks and history applied to every change.

Example: on the Data context a developer clicks `+ Container`, types "Ledger API" and saves. The toolbar shows "⟳ 1 waiting to sync"; a few seconds later the engine has written and committed the files, and "Ledger API" appears inside Data.
