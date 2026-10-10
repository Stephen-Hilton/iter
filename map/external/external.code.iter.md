---
id: 794588bf-1ed5-4776-8057-1bf78db378dc
name: "Outside systems"
desc: "The systems iter5 depends on but does not build: ArangoDB Community Edition, the only database (iter_data talks to it over its HTTP API), and the Claude Code CLI, the agent runtime the claude provider starts for every model call. They appear in the map so the connection nodes can show who supplies and who consumes each connection type."
creator: "stephen"
teststate: inherit
level: context
owner: "3rdparty"
children:
  codedirs:  []
  codenodes: ["{topdir}/map/external/arangodb/arangodb.code.iter.md", "{topdir}/map/external/claude_code/claude_code.code.iter.md"]
  tests:     []
  reqs:      []
timestamps: {create: "2026-10-02 23:12:30Z", last_modified: "2026-10-02 23:12:30Z", last_tested: ""}
---
# Outside systems

Third-party software iter5 runs against. Nothing here is iter5 code (no
`codedirs`); each node exists so the map can show where iter5 meets it.

- **ArangoDB** — documents, counters and graph collections for every record
  iter_data keeps. In production it runs inside the `iter5` container; in
  development and tests it is the dev container on :8529.
- **Claude Code CLI** — the `claude` program the engine's claude provider runs
  headless for each work turn, close-gate check, explain, dedup judge, critic
  and GraphRAG summary. Inside it, the agent reads and edits the checkout, runs
  the `iter` shim and calls iter_data's MCP tools.
