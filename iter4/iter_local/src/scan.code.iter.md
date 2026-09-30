---
id: 6238b88c-3234-4f2a-83a0-0555a2016f36
name: "Node-file scanner"
description: "Finds every `*.iter.md` file in the checkout, works out each one's type from its file name, expands its path placeholders and follows its `children` links into one tree, so that every other part sees the same picture of how the project is put together."
simple_description: "Reads all the project's description files and works out how the parts fit together."
level: component
owner: bespoke
teststate: inherit
children:
  codedirs:   ["{topdir}/iter_local/src/markers.rs", "{topdir}/iter_local/src/placeholders.rs", "{topdir}/iter_local/src/project.rs", "{topdir}/iter_local/src/lib.rs"]
  codenodes:  []
  inputs:     []
  outputs:    []
  bizreqs:    []
  techreqs:   []
  tests:      []
---

# Long Description

The Node-file scanner reads the checkout's node files and builds the tree of the project from them. Everything that reasons about structure, such as the map, the checker, the test sweep and agent prompts, starts from its result.

How it works: `iter_local/src/project.rs: Project::load` reads the head file `main.iter.md` (or `$ITER_MAINFILE`) into a `ProjectConfig`: project name, the folders to scan (`globalscandirs`), the global interface and use-case folders, and the context files every agent gets. `iter_local/src/markers.rs: scan` gathers every matching file under those folders and classifies it by the dot rule: the file name says what it is (`name.code.iter.md`, `name.interface.iter.md`, …; `role_of`), and frontmatter adds attributes, never identity (`parse_front`). It resolves each node's `children` links (`codenodes`, `inputs`, `outputs`, `bizreqs`, `techreqs`, `tests`) to real files; explicit links are the only way nodes join, since folder nesting alone links nothing. Context-level code nodes, interfaces and use cases hang from the root; other nodes attach where a parent lists them; anything unlinked goes to the Orphanage (a list of files nothing links), and a link that would make a cycle is dropped and noted. `effective_teststate` works out each node's test setting through its parents. `iter_local/src/placeholders.rs: Vars` expands path placeholders such as `{topdir}` and `{thisfiledir}` at each use, never rewriting the files; a list-valued placeholder expands to one path per entry.

The Map snapshot builder, the Node-file checker, the Node-file id stamper and the iter command line (`iter markers`, `iter teststate`, `iter usecase`) all call it.

Why it matters: one scanner means the map, the tests and the agents agree on what the project contains.

Example: `webui/webui.code.iter.md` lists `{topdir}/webui/queue.code.iter.md` under `codenodes`; the scanner expands `{topdir}`, finds the file and attaches the queue as a child of the webui.
