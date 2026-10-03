---
id: 090b870c-dea5-4b5b-8ec1-74ce1dbf6d15
name: "Node files in a git checkout"
desc: "The repository itself as a connection: *.iter.md node files (and the code they describe) on disk in a git checkout. The engine is the only iter5 writer — conform write-backs, graph edits pulled from files/pending, the designer build (git init + first commit), scoped commits after each work item, GraphRAG document originals — and agent sessions edit files inside an item's lock scope. Readers: the engine's file scan and prompt builder (node context, agent memory) and iter_local's git-ignore aware walk, validate, test runner and migrate5."
creator: "stephen"
teststate: inherit
connects:
  from: ["{topdir}/iter_engine/src/mapsync.code.iter.md", "{topdir}/iter_engine/src/runner.code.iter.md", "{topdir}/iter_engine/src/datasync.code.iter.md", "{topdir}/map/external/claude_code/claude_code.code.iter.md"]
  to: ["{topdir}/iter_local/src/walk.code.iter.md", "{topdir}/iter_local/src/validate.code.iter.md", "{topdir}/iter_local/src/runner.code.iter.md", "{topdir}/iter_local/src/migrate5.code.iter.md", "{topdir}/iter_engine/src/prompt.code.iter.md", "{topdir}/iter_engine/src/mapsync.code.iter.md"]
level: connection
children:
  codedirs:  []
  codenodes: []
  tests:     []
  reqs:      []
timestamps: {create: "2026-10-02 23:12:30Z", last_modified: "2026-10-02 23:12:30Z", last_tested: ""}
---
# Node files in a git checkout

Nodes are files: each `*.<type>.iter.md` file is one node of the project
graph, keyed by its frontmatter `id` (format v5, `iter_core::nodefile`).

## Protocol

- **Format**: YAML frontmatter in canonical key order (id, name, desc,
  creator, teststate, type-specific keys, children, timestamps) + a markdown
  body; `conform` makes any hand-edited file canonical and is idempotent.
- **Location**: the project's `topdir` (from the `serves` edge), under the
  project node's `scandirs`; git-ignored paths, `.git`, `target`,
  `node_modules`, `.iter` and `.claude` are never read as nodes.
  `*.agentmem.iter.md` files are engine memory, never synced.
- **Writes** (`iter_engine/src/filesync.rs`): conform write-backs and
  server-won rewrites (`iter: conform <n> node files`), graph edits from
  `files/pending` (`iter: graph edit — …`), the designer build
  (`iter: build from design`); paths outside the topdir are refused and a path
  inside a live work-item lock waits. Commits name only the touched files;
  push only when a remote exists. Agents write code and node files inside
  their item's lock scope; the runner commits that scope at the end. The
  GraphRAG document writer (`iter_engine/src/datasync.rs`) stores uploaded
  documents' originals and `.gitignore` lines.
- **Reads**: the engine's per-tick filescan (stat first, re-read only what
  changed, full walk every few minutes; state in `<topdir>/.iter/filesync.json`)
  (with iter_local's git-ignore aware walk), the prompt builder (the node
  file, its children list and the agentmem file for an agent's context), and
  iter_local's validate, test runner and migrate5.

## Auth

Filesystem permissions of the engine's user; git remotes use whatever
credentials that user's git has. A `read_only` serves edge means the engine
scans and syncs but never writes.
