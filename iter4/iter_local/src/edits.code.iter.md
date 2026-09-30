---
id: 51119544-5377-49c3-9946-4ccd37cfdaed
name: "Graph edit writer"
description: "Writes the files behind one Project graph edit (a new part, a link or interface connection added or removed, a new requirement or use case, planned tests, or new body text) into the checkout and links them to their parent, so that a change made on the map becomes real node files in the repo."
simple_description: "Turns a change drawn on the project map into the actual files in the code repository."
level: component
owner: bespoke
teststate: inherit
children:
  codedirs:   ["{topdir}/iter_local/src/graph_edit.rs"]
  codenodes:  []
  inputs:     []
  outputs:    []
  bizreqs:    []
  techreqs:   []
  tests:      []
---

# Long Description

The Graph edit writer is where an edit made in the Project graph turns into files. The data server has no access to the repo, so an engine that serves the project runs this code in its own checkout.

How it works (`iter_local/src/graph_edit.rs`): `apply` takes the project and one operation as JSON and returns the files it wrote. `new_node` makes a context, container or component: `<dir>/<slug>.code.iter.md` plus its `.bizreq`, `.techreq` and `test/<slug>.tests.iter.md` files, and adds the new node to its parent's `children.codenodes` (or to `main.iter.md` for a top-level context). `connect` creates or reuses an interface file and lists it in one part's `inputs` and the other's `outputs`, meaning the first uses what the second provides; `disconnect` removes that pair. `link_child` and `unlink_child` add or remove an ownership link between existing parts (any level may own any level). `new_global` writes a business requirement, technical requirement or use case. `edit_body` replaces the markdown under a file's frontmatter. `define_tests` writes a "Planned tests" list into a node's tests file, and can ask for a `test` agent item to write and run them. Helpers do the careful text work: `slugify` makes file names, `add_child` and `remove_child` edit a `children` list in the file's own style, `set_body` swaps the body, and path resolution refuses anything outside the checkout. `lock_scope` lists every folder an operation may write, so the caller can check locks first.

It is called by the engine's datasync step (`iter_engine/src/datasync.rs: apply_waiting`), which claims a waiting edit, runs `apply`, commits exactly the files returned and pushes the map, and by the iter command line's `iter graph-apply` for a hand-run edit. It reads the project through the Node-file scanner's project loader.

Why it matters: it is the only part that turns the Graph editor's buttons into repo changes; without it the map could be looked at but not built on.

Example: someone adds a container "Ledger API" under the Data context. `apply` writes `ledger_api/ledger_api.code.iter.md` inside the Data context's folder, with its requirement and tests files, and adds that path to the Data context's `children.codenodes`; the engine commits those files and the map redraws.
