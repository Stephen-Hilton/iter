---
id: 89cb31a0-ac50-47b5-8bd4-aa1f6d616aad
name: "Agent session"
desc: "An AI agent the engine starts for one work item through a provider (a headless Claude Code session, or the mock provider in tests), running in the project's checkout. It edits code and node files inside the item's lock scope, runs the iter shim (add, ask, wait, doc, runtests, validate) and calls iter_data's MCP tools to read and edit the work queue and the project graph."
creator: "iter migrate5"
teststate: inherit
drives: ["{topdir}/global/usecases/edit_a_node_file_in_the_repo.usecase.iter.md", "{topdir}/global/usecases/edit_a_node_in_the_graph.usecase.iter.md", "{topdir}/global/usecases/a_red_test_becomes_one_work_item.usecase.iter.md"]
touches: ["{topdir}/iter_engine/src/cli.code.iter.md", "{topdir}/iter_data/src/mcp.code.iter.md"]
children:
  codedirs:  []
  codenodes: []
  tests:     []
  reqs:      []
timestamps: {create: "2026-10-02 23:05:35Z", last_modified: "2026-10-02 23:12:30Z", last_tested: ""}
---
# Agent session

Not a person, but it drives use cases like one. The engine builds its
context from the node the item is about (the node file, its children's
`[id, name, desc, path]`, global and local requirements including the
philosophy, and the agentmem file) and dispatches it with the account the
`bills` edge picks.

Where it touches iter5:

- **The checkout** — reads and edits files inside the item's lock scope; node
  file edits reach the graph through file sync.
- **The `iter` shim** (`{topdir}/.iter/bin/iter`, with `ITER_DATA_URL`,
  `ITER_ENGINE_TOKEN`, `ITER_PROJECT`, `ITER_WORKID` set) — file follow-up
  items, ask the human, wait on other items, add doc notes, run tests with
  `--broken` / `--fixed`.
- **MCP** (`<data_url>/mcp`, server `iter`) — work queue, project graph
  (read and edit), test logs and GraphRAG tools under the engine's token.
