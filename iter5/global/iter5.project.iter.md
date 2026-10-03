---
id: dd366398-147d-450e-a792-59ef7ae020bc
name: "iter5"
desc: "The iter harness, version 5: one ArangoDB-backed data server (iter_data) and one engine per machine (iter_engine) that runs AI agents on every project the settings graph assigns to it. A project's node files (*.iter.md) and its project graph stay in step within seconds, so the graph is also a designer: draw a project, press Build, and an engine creates the repo. Connections replace interfaces, every setting is a node or edge in the settings graph, and every model call goes through a provider (claude, or the deterministic mock)."
creator: "iter migrate5"
teststate: inherit
file_naming: sequence
gitrepo: ""
scandirs: ["{topdir}/"]
children:
  codedirs:  []
  codenodes: ["{topdir}/map/orchestration/orchestration.code.iter.md", "{topdir}/map/engine/engine.code.iter.md", "{topdir}/map/shared/shared.code.iter.md", "{topdir}/map/delivery/delivery.code.iter.md", "{topdir}/map/external/external.code.iter.md"]
  tests:     ["{topdir}/e2e/e2e.test.iter.md", "{topdir}/e2e/playwright.test.iter.md"]
  reqs:      ["{topdir}/reqs/"]
timestamps: {create: "2026-10-02 23:05:35Z", last_modified: "2026-10-02 23:12:30Z", last_tested: ""}
---
# iter5

iter keeps AI coding agents working on real software projects without letting
them trip over each other. A person, or an agent, files a **work item**: "make
this change, in this part of the code". An **engine** asks the data server for
the next item of one of its projects (`POST /api/projects/{p}/next`); the server
picks the most urgent item that can run, claims it and locks the folders it
will touch in one step. The engine builds the agent's context from the node the
item is about, starts the agent through a **provider** (`claude`, or the
deterministic `mock` the tests use), has a second model check the result
against the request (the **close gate**), commits just those files, and moves on.

## What version 5 changes

- **One engine per machine.** An engine starts with only a server URL and an
  env file (`iter_engine --data-url … --env-file …`). Which projects it serves,
  where their checkouts are and which accounts it may bill come from the server
  (`GET /api/engines/{name}/assignments`); every call names its project.
- **Nodes are files.** Each `*.iter.md` file in a checkout is one node of the
  project graph, keyed by its frontmatter `id`. The engine scans, conforms and
  syncs changed files every tick (`POST …/files/sync`); an edit made in the
  graph is written back to its file and committed by the engine
  (`GET …/files/pending` → write → commit → `POST …/files/ack`). The engine is
  the only writer to a repo; the server never touches one.
- **The graph is a designer.** A project can be created with no engine and no
  repo, drawn in the Project graph, and then built: the engine runs `git init`,
  writes every designed file, makes the first commit, and the server can queue a
  `plan` item to build the code.
- **Connections replace interfaces.** A `level: connection` code node names a
  *type* of connection (HTTP JSON API, MCP, agent dispatch, …); `connects.from`
  lists the parts that supply it and `connects.to` the parts it reaches. There
  are few of them, widely connected — see `global/connections/`.
- **The settings graph.** Every setting is a node (engine, project, account,
  provider, agent, tooling, user, work-item state) or an edge (`serves`,
  `holds`, `bills`, `of`, `member`, `runs`, …) in the Settings tab. Each type
  has an undeletable `_deactivated` placeholder, so an edge can be parked
  without losing its settings.
- **Multi-provider dispatch.** Every model call goes through
  `dispatch_agent(provider, model, context, settings)`, and usage through
  `get_usage`; the provider comes from the account's `of` edge.
- **Standard test results.** A test node's scripts print one JSON result as
  their last stdout line; results land on the test node and in the test log,
  and a red result files one deduplicated fix item.

## The map of this repository

This repository is iter5's own checkout, and iter5 maps itself. This file is
the root: its `children.codenodes` are the five context nodes —

- **Orchestration and data** (`map/orchestration/`) — `iter_data`, the server
  (API, project graph, settings graph, get_next, MCP, GraphRAG) and the `webui`
  it serves.
- **Engines and checkout tools** (`map/engine/`) — `iter_engine`, one per
  machine, which also is the `iter` command line, and `iter_local`, which walks,
  validates and tests a checkout.
- **Shared rules and types** (`map/shared/`) — `iter_core` (records, queue
  rules, the node-file library both sides use) and `iter_rag`.
- **Build, ship and prove** (`map/delivery/`) — the all-in-one container,
  `deploy.sh`, the engine setup script, `e2e.sh` and Playwright.
- **Outside systems** (`map/external/`) — ArangoDB and the Claude Code CLI,
  which iter uses but does not build.

The connection types between them are in `global/connections/`; the actors
and use cases in `global/usecases/`. `children.reqs` points at `reqs/`, the
project-wide requirements every agent is given.
