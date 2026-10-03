---
id: c3862a80-6dd1-4cd1-9939-27638f342a87
name: "MCP gateway"
description: "Serves every agent-facing iter_data call — work items, the map, GraphRAG search — as tools of a stateless MCP server at POST /mcp, replaying each tool call through the real API router with the caller's own token, so agents use iter through MCP without a second copy of any rule."
simple_description: "Lets AI assistants use the project's work queue, map and document search as ready-made tools."
level: component
owner: bespoke
teststate: inherit
children:
  codedirs:   ["{topdir}/iter_data/src/mcp.rs"]
  codenodes:  []
  inputs:     ["{topdir}/interfaces/mcp-call/mcp-call.interface.iter.md"]
  outputs:    ["{topdir}/interfaces/rag-search/rag-search.interface.iter.md", "{topdir}/interfaces/workitem-create/workitem-create.interface.iter.md", "{topdir}/interfaces/graph-neighbors/graph-neighbors.interface.iter.md"]
  bizreqs:    []
  techreqs:   []
  tests:      []
---

# Long Description

The MCP gateway (`iter_data/src/mcp.rs`) speaks the Model Context Protocol (MCP, the standard way AI clients discover and call tools) over plain HTTP. It is stateless: every `POST /mcp` is a complete JSON-RPC request answered with JSON, no session is kept, and any iter_data instance can answer any call.

How it works: `handle` checks the bearer token with the same extractor every API route uses, then `rpc` answers `initialize`, `tools/list` and `tools/call`. Each tool (`tools()`) maps to one or more HTTP API calls; `Mcp::call` builds the request and sends it through the real API router in-process (`tower::ServiceExt::oneshot`) with the caller's Authorization header. Roles, validation and change-counter bumps are therefore exactly the API's. Optional `X-Iter-Project` and `X-Iter-Workid` headers give the default project and the calling work item, as ITER_PROJECT and ITER_WORKID do for the `iter` command line.

The agent command line's multi-step verbs (`add`, `ask`, `reject`, `wait`, `doc`, `block`, `status`, `capability`) are the same call sequences as `iter_engine/src/cli.rs`. Verbs that read the checkout (runtests, validate, sync, sweep, rag sync) and `critreview`, which runs a model session, stay on the engine.

Without it, an agent needs the `iter` shim and a shell. Example: a Claude session configured with `{"type": "http", "url": "http://host:8300/mcp"}` calls `rag_search` and then `workitem_create` with no local tooling.
