---
id: b5703965-cba5-4f2a-a434-adff26013938
name: "Orchestration and data"
desc: "The one data server, iter_data, and the web page it serves. It keeps every record iter5 holds — work items, locks, the project graph (one node per node file), the settings graph (engines, projects, accounts, providers, agents, users and the edges between them), test logs, spend and GraphRAG documents — in ArangoDB behind one authorized HTTP API and MCP endpoint. It picks and claims the next work item for an engine, but never touches a repository."
creator: "iter migrate5"
teststate: inherit
level: context
owner: bespoke
children:
  codedirs:  ["{topdir}/iter_data/", "{topdir}/webui/"]
  codenodes: ["{topdir}/iter_data/iter_data.code.iter.md", "{topdir}/webui/webui.code.iter.md"]
  tests:     []
  reqs:      []
timestamps: {create: "2026-10-02 23:05:35Z", last_modified: "2026-10-02 23:12:30Z", last_tested: ""}
---
# Orchestration and data

## Summary

The one place iter5's information is kept, the only door to it, and the page
people use to watch, steer and design the work.

It holds two containers: **iter_data — data server** (`iter_data/`) and
**webui — web page** (`webui/`, compiled into the iter_data binary).

## How it works

iter_data is an axum HTTP server on ArangoDB (`iter_data/src/arango.rs`, the
only store). Every request is authenticated (users log in for a JWT; engines
use a long-lived engine token bound to an owning user) and authorized per
project (`authz.rs`): an engine may act on a project only through an active
`serves` edge, a user only through admin rights or a `member` edge.

- **Work queue.** Work items, details, locks, schedules and spend, as in iter4,
  plus server-side **get_next** (`next.rs`): `POST /api/projects/{p}/next` picks
  by priority and receive time, honours dependencies, waits, cluster holds and
  lock availability, and claims the item and its locks in one versioned write.
- **Project graph** (`nodes.rs`, `graph.rs`, `graph_view.rs`, `filesync.rs`):
  one stored node per node file, edges derived with `iter_core::nodefile`.
  Engines push file changes (`files/sync`, newer `last_modified` wins a
  conflict); edits made in the graph are stored at once as `pending_write`
  and pulled by an engine (`files/pending`, `files/ack`). A designed project
  (no repo yet) is built through `build` / `build/done`.
- **Settings graph** (`settings.rs`): nodes and `sys_edge` edges for every
  setting; it answers each engine's `assignments` (projects + topdirs from
  `serves`, accounts from `bills` ∩ `holds`, providers from `of`).
- **Tests** (`testlogs.rs`): standard result JSON on test nodes, the test log,
  and "Run tests" as a test work item.
- **MCP** (`mcp.rs`, `POST /mcp`) and **GraphRAG** (`rag/`).

The web page (Intro, Work queue, Project graph, GraphRAG, Settings) calls the
same API with the user's token; it holds no rules of its own.

## What goes in and out

Engines and the `iter` command line call the HTTP JSON API (heartbeat,
assignments, next, locks, results, file sync, build, GraphRAG work); agent
sessions call MCP; browsers load the page. iter_data itself calls only
ArangoDB. It has no checkout: anything that must change a file waits as a
pending node until an engine that serves the project writes it.

## Why it matters

With a single door, rules such as "only one engine wins an item", "a closed
item cannot be edited" or "this engine does not serve that project" hold no
matter who asks, and the graph shown to people is the same graph the files
describe.
