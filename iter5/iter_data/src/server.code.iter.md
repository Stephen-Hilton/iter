---
id: 8283c45c-f091-4417-a3c1-a210d3602200
name: "Server startup"
desc: "Starts iter_data: loads the .env file, opens ArangoDB, creates the first admin user and the GraphRAG summary agent record, seeds the settings graph (placeholders, providers, work item states, the one-time iter4 edge migration), loads the embedding model and the login secret, then serves the API, MCP, the engine setup script and the embedded web page from one binary on port 8400, so a container needs no files on disk."
creator: "iter migrate5"
teststate: inherit
level: component
owner: bespoke
children:
  codedirs:  ["{topdir}/iter_data/src/main.rs", "{topdir}/iter_data/build.rs"]
  codenodes: []
  tests:     []
  reqs:      []
timestamps: {create: "2026-10-02 23:05:35Z", last_modified: "2026-10-02 23:10:16Z", last_tested: ""}
---

# Long Description

## Summary

Turns the data server on, connects it to its database and puts the web page inside it.

Server startup is iter_data's `main` (`iter_data/src/main.rs`). It parses the command line (`Args`: `--arango-url`, `--arango-db` default `iter5`, `--listen`, `--webui-dir`, `--secret-file`, `--env-file`, `--embed-model`) and loads the `.env` file line by line without shell semantics (`load_env_file`; a variable already set wins).

How it works: `open_arango` connects to ArangoDB (`ARANGO_URL`, `ARANGO_DB`, `ARANGO_USER`, `ARANGO_PASSWORD` override the flags) — the only store iter5 has; any `--backend` other than `arango` exits. Then, in order: `bootstrap_admin` creates `admin` when there are no users (password from `ITER_ADMIN_PASSWORD`, else generated and printed once); `bootstrap_summary_agent` creates the `summary` agent record the engine's GraphRAG Summary worker reads; `settings::bootstrap` seeds the settings graph; `rag::guide::ingest_at_startup` indexes the built-in user guide (compiled in by `build.rs` from `docs/iter4_guide.md`); `auth::load_secret` loads the token-signing secret; `rag::embed` locates the embedding model. The router is `api::router` (which merges the graph, file sync, test log, settings, GraphRAG and datasync routes under the authz layer) plus `mcp::routes`, `/iter_engine_setup.sh` (the setup script from `tools/`, no sign-in) and a permissive CORS layer. It listens on `127.0.0.1:8400` unless `--listen` or `ITER_PORT` says otherwise.

The web page is compiled into the binary (`WEBUI_INDEX`, `WEBUI_FILES`: the page, Project graph viewer and editor, Intro, GraphRAG tab, settings graph, the shared UI kit and the vendored Cytoscape libraries), served by `embedded_index`; `--webui-dir` serves a folder instead, for development.

Why it matters: without it nothing else in iter_data runs.

Example: the container starts `iter_data --listen 0.0.0.0:8400`; this code connects to ArangoDB, bootstraps admin, and prints `[iter_data] listening on 0.0.0.0:8400 (backend: arango)`.
