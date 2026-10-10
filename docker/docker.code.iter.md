---
id: a9040402-89d2-4e8e-8c61-04c88f0876b8
name: "docker — all-in-one container"
desc: "One Docker image, container iter5, that runs ArangoDB Community Edition and the iter_data server side by side: API and web page on :8400, Arango console on 127.0.0.1:8630 only, database iter5. iter5-entrypoint.sh starts arangod first, then iter_data, and stops the container if either dies, so a whole iter5 server comes up with one command on a laptop or a small VM."
creator: "iter migrate5"
teststate: inherit
level: container
owner: bespoke
children:
  codedirs:  ["{thisfiledir}/"]
  codenodes: []
  tests:     []
  reqs:      []
timestamps: {create: "2026-10-02 23:05:35Z", last_modified: "2026-10-02 23:12:30Z", last_tested: ""}
---
# docker — all-in-one container

## Summary

The package that bundles the database and the server so they start together.

## How it works

`docker/Dockerfile` has two stages. The first (`rust:1-alpine`) compiles a
static `iter_data` with the web page (`webui/`) and the engine setup script
(`tools/iter_engine_setup.sh`) embedded. The second starts from the official
`arangodb:3.12` image and adds that binary, `docker/iter5-entrypoint.sh` and
the GraphRAG embedding model (`models/all-MiniLM-L6-v2`, fetched beforehand by
`tools/fetch_model.sh`). It replaces `/var/lib/arangodb3`,
`/var/lib/arangodb3-apps` and `/var/lib/iter` with links into the one data
folder `/var/lib/iter_data`, then is copied whole into a `scratch` stage: the
Arango image declares its own `VOLUME`s, which a Dockerfile can't remove, and
Docker would otherwise give each an anonymous volume. Defaults: `ARANGO_URL=http://127.0.0.1:8529`,
`ARANGO_DB=iter5`, `ITER_LISTEN=0.0.0.0:8400`.

`iter5-entrypoint.sh` refuses to start without `ARANGO_ROOT_PASSWORD` (or an
explicit no-auth setting), starts `arangod` through the image's own
entrypoint so first-run password setup happens (after creating `arango/`,
`arango-apps/` and `iter/` under `/var/lib/iter_data`) (never with an endpoint
argument: that made the temporary first-run server answer on the real port),
then starts `iter_data --backend arango --listen 0.0.0.0:8400 --secret-file
/var/lib/iter/iter_data.secret`. It watches both processes and stops the
container if either exits, so Docker's restart policy brings them back
together.

`docker/compose.yml` (project and container `iter5`) publishes
`${ITER_PORT:-8400}:8400` and the Arango console on
`127.0.0.1:${ARANGO_HOST_PORT:-8630}:8529`, reads secrets from
`run/docker.env`, and bind-mounts one host folder, `${ITER_DATA_DIR}`
(`deploy.ps1` / `deploy.sh` default it to `~/.iter5/iter_data`), at
`/var/lib/iter_data`: everything that persists, visible on the host. The health
check calls `/health`.

## What goes in and out

`deploy.sh docker` (or `deploy.ps1 docker`) writes `run/docker.env` and runs
compose. Both refuse to run without `ITER_JWT_SECRET`: iter_data would mint a
new secret into the data folder and every existing token would stop
verifying. Engines and
browsers then reach the container on :8400; only the operator's own machine
reaches the Arango console.

## Why it matters

Without it, running iter5 means installing and configuring ArangoDB and wiring
the server to it by hand. It runs beside an iter4 container (:8300 / :8530)
without clashing.
