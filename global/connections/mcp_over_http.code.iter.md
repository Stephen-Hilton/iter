---
id: ef4d98e0-e24d-4524-a1ac-14bc99635a4e
name: "MCP over HTTP"
desc: "iter_data's stateless MCP server at POST /mcp (Streamable HTTP, JSON-RPC 2.0, JSON responses, no sessions): 30 tools over the work queue, the project graph (read and edit), test logs, the settings graph and GraphRAG. The engine writes an MCP config into every agent session it starts, so Claude Code sessions call the tools with the engine's bearer token, X-Iter-Project and X-Iter-Workid; each call is replayed through the HTTP API router with the same authz."
creator: "stephen"
teststate: inherit
connects:
  from: ["{topdir}/iter_data/src/mcp.code.iter.md"]
  to: ["{topdir}/map/external/claude_code/claude_code.code.iter.md", "{topdir}/iter_engine/src/runner.code.iter.md"]
level: connection
children:
  codedirs:  []
  codenodes: []
  tests:     []
  reqs:      []
timestamps: {create: "2026-10-02 23:12:30Z", last_modified: "2026-10-02 23:12:30Z", last_tested: ""}
---
# MCP over HTTP

How agents (and any other MCP client) use iter as tools.

## Protocol

`POST /mcp` with a JSON-RPC 2.0 body (`initialize`, `tools/list`,
`tools/call`); the reply is a single JSON response (no SSE stream: `GET /mcp`
and `DELETE /mcp` answer that streams and sessions are not offered).
Tools: work queue (`status`, `workitem_*`, `capability`, `locks_list`),
project graph (`graph_stats`, `graph_lookup`, `graph_node`,
`graph_neighbors`, `graph_owner`, `graph_usecase`, `graph_node_create`,
`graph_node_update`, `graph_edge_add`, `graph_edge_remove`, `testlogs`),
settings (`settings_graph`, admin), GraphRAG (`rag_*`).

## Auth and addressing

`Authorization: Bearer <token>` — the same tokens as the HTTP JSON API.
`X-Iter-Project` names the default project (or a `project` argument) and
`X-Iter-Workid` the calling work item. Each tool call is replayed through the
API router, so it is authorized exactly like the matching HTTP call.

## Who connects

For every agent turn the work runner writes a temporary MCP config
(`iter_engine/src/work.rs: mcp_config`: server `iter`, type `http`, url
`<data_url>/mcp`, the headers above) and passes it to the Claude Code CLI
with `--mcp-config` — which is why the runner is listed in `connects.to`
beside the CLI. The read-only ELI5 explain session is limited to the read
tools. Any other MCP client with a token can connect the same way.
