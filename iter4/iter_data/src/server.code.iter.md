---
id: 8283c45c-f091-4417-a3c1-a210d3602200
name: "Server startup"
description: "Starts iter_data: reads the command line and environment, opens the chosen storage backend, creates the first admin user, embeds and serves the web page, and runs the one-shot migrations, so one binary serves both the API and the page."
simple_description: "Turns the data server on and connects it to its database."
level: component
owner: bespoke
teststate: inherit
children:
  codedirs:   ["{topdir}/iter_data/src/main.rs"]
  codenodes:  []
  inputs:     []
  outputs:    []
  bizreqs:    []
  techreqs:   []
  tests:      []
---

# Long Description

Server startup is iter_data's `main` (`iter_data/src/main.rs`). It parses the command line (`Args`: ArangoDB address and database, listen address, secret file, env file, migration flags), loads the `.env` file line by line without shell semantics (`load_env_file`), and opens ArangoDB (`open_arango`, which honours `ARANGO_URL`, `ARANGO_DB`, `ARANGO_USER`, `ARANGO_PASSWORD`) — the only store iter4 serves from; any other `--backend` is refused.

When a migration flag is given it runs that job instead of serving: `--migrate-from dynamodb` copies every iter3 table (`migrate_ddb.rs`). Otherwise it creates the `admin` user on first start (`bootstrap_admin`, password from `ITER_ADMIN_PASSWORD` or generated and printed once), loads the signing secret, builds the HTTP router (`api::router`, which merges the map and datasync routes), and serves.

The web page is compiled into the binary (`WEBUI_INDEX` and `WEBUI_FILES`: the page, the Project graph viewer and editor, the Intro, and the vendored Cytoscape libraries), so the Docker container and a Lambda deployment need no files on disk; `--webui-dir` serves them from a folder instead, for development.

Without it nothing else in iter_data runs. Example: the container's entrypoint starts `iter_data --backend arango --listen 0.0.0.0:8300`, and this code connects to ArangoDB, bootstraps admin, and begins answering on port 8300.
