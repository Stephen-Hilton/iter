---
id: d189ff3e-7930-4fff-9092-ec1a0a80a006
name: "iter_local — checkout tools"
description: "Reads and writes the project's files on disk — finds every *.iter.md node file, gives each a stable id, builds the map snapshot, applies graph edits, runs test groups and validates the structure — without needing the server or an engine."
simple_description: "The toolkit that reads and edits the project's own description files and runs its tests."
level: container
owner: bespoke
teststate: inherit
children:
  codedirs:   ["{thisfiledir}/"]
  codenodes:  ["{topdir}/iter_local/src/scan.code.iter.md", "{topdir}/iter_local/src/ids.code.iter.md", "{topdir}/iter_local/src/snapshot.code.iter.md", "{topdir}/iter_local/src/edits.code.iter.md", "{topdir}/iter_local/src/runner.code.iter.md", "{topdir}/iter_local/src/validate.code.iter.md"]
  inputs:     []
  outputs:    []
  bizreqs:    []
  techreqs:   []
  tests:      ["{thisfiledir}/test/*.tests.iter.md"]
---

# Long Description

iter_local is a Rust library for everything iter does against the checkout on disk. It needs no server, no queue and no engine: give it a project folder and it answers from the files.

How it works: a project here is a top folder plus its `main.iter.md` (`iter_local/src/project.rs`). The **node-file scan** (`markers.rs`, with `placeholders.rs` expanding `{topdir}` and `{thisfiledir}`) walks the tree, skipping `.git`, `target`, `node_modules`, `.iter` and `.claude`, and parses each `*.iter.md` file's frontmatter and `children` links. **Stable ids** (`ids.rs`) finds files with no `id:` or a duplicate one and, with `--fix`, writes a UUID into each. **Map snapshot** (`graph.rs: snapshot_with`) turns the scan into vertices, edges and a content hash. **Graph edits** (`graph_edit.rs: apply`) writes the new or changed node files a web-page edit asks for. The **test runner** (`runtests.rs`, `testgroups.rs`) runs a test group's commands and grades them. **Validate** (`validate.rs`) reports broken links, missing ids and malformed files.

What goes in and out: iter_engine — local engine is its only caller: the engine loop uses it to sync the map and apply edits, and the `iter` command line exposes it as `iter ids`, `sync`, `graph-apply`, `runtests` and `validate`. It reads and writes files only; it never calls the network.

Why it matters: the program map, the test sweep and graph editing all start from what these functions read in the files. If the scan misread a `children` link, the map would show a part as orphaned and the sweep would skip its tests.

Example: `iter validate` on this repository walks every node file and prints "0 finding(s)" when every link resolves and every file has a well-formed id.

Built and tested as the cargo package `iter_local` in the iter4 workspace (`cargo test -p iter_local` from `iter4/`).
