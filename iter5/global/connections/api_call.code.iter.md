---
id: 026e2318-87c0-4c00-af34-42c0afb61b7f
name: "HTTP JSON API"
desc: "iter_data's request/reply API over HTTP (axum, JSON bodies, port 8400): /api/... for work items, locks, get_next, engines (heartbeat, assignments), the project graph and file sync, designer build, test results and logs, the settings graph and GraphRAG, plus /auth/login, /health, the embedded web page and /iter_engine_setup.sh. Every /api call carries Authorization: Bearer <JWT> (a user's login token or an engine token) and names its project in the path; authz checks serves/member edges per call."
creator: "iter migrate5"
teststate: inherit
connects:
  from: ["{topdir}/iter_data/src/server.code.iter.md", "{topdir}/iter_data/src/api.code.iter.md", "{topdir}/iter_data/src/next.code.iter.md", "{topdir}/iter_data/src/settings.code.iter.md", "{topdir}/iter_data/src/graph.code.iter.md", "{topdir}/iter_data/src/filesync.code.iter.md", "{topdir}/iter_data/src/testlogs.code.iter.md", "{topdir}/iter_data/src/datasync.code.iter.md", "{topdir}/iter_data/src/rag/rag.code.iter.md"]
  to: ["{topdir}/iter_engine/src/client.code.iter.md", "{topdir}/iter_engine/src/loop.code.iter.md", "{topdir}/iter_engine/src/assign.code.iter.md", "{topdir}/iter_engine/src/mapsync.code.iter.md", "{topdir}/iter_engine/src/datasync.code.iter.md", "{topdir}/iter_engine/src/rag.code.iter.md", "{topdir}/iter_engine/src/sweep.code.iter.md", "{topdir}/iter_engine/src/cli.code.iter.md", "{topdir}/iter_engine/src/dedup.code.iter.md", "{topdir}/iter_engine/src/runner.code.iter.md", "{topdir}/webui/webui.code.iter.md", "{topdir}/webui/queue.code.iter.md", "{topdir}/webui/projectgraph.code.iter.md", "{topdir}/webui/grapheditor.code.iter.md", "{topdir}/webui/settings.code.iter.md", "{topdir}/webui/rag.code.iter.md", "{topdir}/webui/intro.code.iter.md", "{topdir}/tools/tools.code.iter.md"]
level: connection
children:
  codedirs:  []
  codenodes: []
  tests:     []
  reqs:      []
timestamps: {create: "2026-10-02 23:05:35Z", last_modified: "2026-10-02 23:12:30Z", last_tested: ""}
---
# HTTP JSON API

The main connection type of iter5: plain request/reply HTTP with JSON bodies,
served by iter_data (default `0.0.0.0:8400`; `ITER_PORT` / `ITER_LISTEN`).
`connects.from` lists the iter_data parts whose routes make up the API;
`connects.to` the parts that call it.

## Protocol

- `GET|POST|PUT|PATCH|DELETE /api/...`, `Content-Type: application/json`.
  Errors come back as a non-2xx status with `{"error": "..."}`.
- Project-scoped routes all live under `/api/projects/{project}/...`: one
  engine serves many projects, so every call names the project it is about.
- Groups (one connection type, many routes):
  - work queue — `workitems`, `next` (server-side pick + claim + lock),
    `locks/*`, `status`, `spend`, `versions`;
  - engines — `PUT /api/engines/{name}` (register),
    `/api/engines/{name}/heartbeat` (reply carries `files_waiting`,
    `build_waiting`, `rag_waiting`, stop requests), `/assignments`;
  - project graph — `graph`, `graph/view`, `graph/nodes`, `graph/edges`,
    `graph/conflicts`, `files/sync`, `files/pending`, `files/ack`,
    `build`, `build/done`, `graph/nodes/{id}/testresult`, `testlogs`,
    `graph/run_tests`;
  - settings graph — `/api/settings/graph`, `/api/settings/nodes`,
    `/api/settings/edges` (+ `/copy`);
  - users, agents, tooling, GraphRAG (`rag/*`, `datasync` for document files);
  - unauthenticated: `POST /auth/login`, `GET /health`, the web page's static
    files and `GET /iter_engine_setup.sh`.

## Auth

`Authorization: Bearer <token>`, HS256 JWTs signed with `ITER_JWT_SECRET`
(`iter_data/src/auth.rs`). People get one from `/auth/login` (the web page
keeps it for the session); an engine uses `ITER_ENGINE_TOKEN` from its env
file, minted by an admin (`POST /api/users/<user>/token`) and bound to the
owning user. Authorization per call (`authz.rs`): admins see everything; an
engine token reaches a project only through an active `serves` edge from one
of its user's engines; a user through a `member` edge (`user` or `viewer`).
Settings-graph writes are admin only.

## Consumers

The engine (every service, through its one HTTP client), the `iter` command
line (`sync`, `sweep`, `rag sync`, `runtests` recording, and the queue verbs
agents run), the web page (every tab) and the engine setup script (health and
token check).
