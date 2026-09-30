---
id: dd41bd94-321b-49cb-8f3f-81532241f6a3
name: "Map uploader and test sweep"
description: "Stamps ids on node files and uploads the checkout's architecture map to the data server, and runs the map's test groups and files one fix item per red group, so that the Project graph matches the code and failing tests become work without anyone filing them."
simple_description: "Keeps the online project map in step with the code, and turns failing tests into fix-it tasks automatically."
level: component
owner: bespoke
teststate: inherit
children:
  codedirs:   ["{topdir}/iter_engine/src/sync.rs", "{topdir}/iter_engine/src/sweep.rs"]
  codenodes:  []
  inputs:     ["{topdir}/interfaces/graph-sync/graph-sync.interface.iter.md", "{topdir}/interfaces/graph-testresult/graph-testresult.interface.iter.md", "{topdir}/interfaces/workitem-create/workitem-create.interface.iter.md"]
  outputs:    []
  bizreqs:    []
  techreqs:   []
  tests:      []
---

# Long Description

This part does two jobs over the architecture map, the graph of every `*.iter.md` node file in a checkout and the links between them.

Uploading the map (`iter_engine/src/sync.rs`): `checkout_root` finds the checkout (never the whole repo just because a project name was given) and `conn` finds the server and token (flag, then the engine's `ITER_*` environment, then `.iter/config.json`). `fix_ids` asks the stored map which ids it knows (`GraphLookup`) and has the Node-file id stamper give every file without one an id. `push` asks the Map snapshot builder for the snapshot and sends it with `PUT /api/projects/{p}/graph`. `sync_verb` is `iter sync`; `--read-only` writes nothing into the checkout and uses derived ids instead. `sync_if_changed` is the engine's hook: push only when the snapshot's hash moved. `graph_apply_verb` is `iter graph-apply`: it applies one graph edit by hand through the Graph edit writer and pushes the map.

Running the test sweep (`iter_engine/src/sweep.rs`): `sweep_verb` reads the stored map, `eligible` walks every chain from main and applies each node's `teststate` (omit, include, block or inherit) to decide which test groups run, and each one runs through the Test group runner (`run_group`). Every result is posted to its test group's vertex (`…/graph/nodes/{id}/testresult`). For a red group, `file_fix_item` creates one `code` work item locked to the owning node's folders, carrying the failing output and the `check:`/`container:` tags that stop a second copy. `text_sweep` does the same for unclear map text: each code node that breaks the node-text standard (`validate::node_text_findings`) becomes one `ingest` item, capped per sweep. `install_schedule` makes `iter sweep` a recurring scheduled item.

It is called by the iter command line and the Engine scheduler loop, and calls the Data server client, the id stamper, the snapshot builder, the Graph edit writer and the Test group runner.

Why it matters: without it the Project graph goes stale and a red test waits until a person notices.

Example: a scheduled sweep runs every 4 hours; one group goes red, one fix item is filed; the next sweep finds it still open and files nothing new.
