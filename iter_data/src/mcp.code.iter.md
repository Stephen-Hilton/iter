---
id: c3862a80-6dd1-4cd1-9939-27638f342a87
name: "MCP gateway"
desc: "Serves every agent-facing iter_data call — work items and the agent command line's multi-step verbs, locks, the project graph (read and edit), test logs, the settings graph and GraphRAG search — as tools of a stateless MCP server at POST /mcp, replaying each tool call through the real API router with the caller's own token, so agents use iter through MCP with exactly the API's role checks, project access rules and validation and no second copy of any rule."
creator: "iter migrate5"
teststate: inherit
level: component
owner: bespoke
children:
  codedirs:  ["{topdir}/iter_data/src/mcp.rs"]
  codenodes: []
  tests:     []
  reqs:      []
timestamps: {create: "2026-10-02 23:05:35Z", last_modified: "2026-10-02 23:10:16Z", last_tested: ""}
---

# Long Description

## Summary

Lets AI assistants use the project's work queue, map and document search as ready-made tools.

The MCP gateway (`iter_data/src/mcp.rs`) speaks the Model Context Protocol (MCP, the standard way AI clients discover and call tools) over plain HTTP. It is stateless: every `POST /mcp` is a complete JSON-RPC request answered with JSON, no session is kept (GET and DELETE answer that there is no stream or session), and any iter_data instance can answer any call.

How it works: `handle` checks the bearer token with the same `AuthUser` extractor every API route uses, then `rpc` answers `initialize`, `tools/list` and `tools/call`. Each tool in `tools()` maps to one or more API calls; `Mcp::call` builds the request and sends it through the real API router in-process (`tower::ServiceExt::oneshot`) with the caller's Authorization header, so the project access rules, roles, validation and change-counter bumps are exactly the API's. The tools: `status`, `workitem_list` / `get` / `details` / `create`, the agent verbs `workitem_ask`, `workitem_reject`, `workitem_wait`, `workitem_doc`, `workitem_block`, `capability`, `locks_list`; the graph tools `graph_stats`, `graph_lookup`, `graph_node`, `graph_neighbors`, `graph_owner`, `graph_usecase`, `graph_node_create`, `graph_node_update`, `graph_edge_add`, `graph_edge_remove`; `testlogs`; `settings_graph`; and GraphRAG's `rag_search`, `rag_status`, `rag_docs`, `rag_doc`, `rag_link_document`, `rag_add_document`. Project-scoped tools take the project from their arguments or the `X-Iter-Project` header; `X-Iter-Workid` names the calling work item, as ITER_PROJECT and ITER_WORKID do for the `iter` command line.

Verbs that read the checkout (runtests, validate, sync, sweep, rag sync) and `critreview`, which runs a model session, stay on the engine.

What goes in and out: agent sessions started by iter_engine (and any MCP client) call it; it calls only the HTTP API router.

Example: a Claude session configured with `{"type": "http", "url": "http://host:8400/mcp"}` calls `rag_search`, then `graph_node_update` to fix a node's desc, with no local tooling.
