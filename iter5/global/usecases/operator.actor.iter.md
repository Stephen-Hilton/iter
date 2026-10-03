---
id: 52993009-49c9-4ea4-954d-be2bbdc179fb
name: "Operator"
desc: "The person who stands iter5 up and keeps it running: starts the server (./deploy.sh docker → container iter5 on :8400 with ArangoDB, or ./deploy.sh local), sets up the one engine of each machine with tools/iter_engine_setup.sh (env file with the engine token and account tokens), keeps engines running and restarts them after upgrades."
creator: "iter migrate5"
teststate: inherit
drives: ["{topdir}/global/usecases/connect_an_engine_to_a_project.usecase.iter.md"]
touches: ["{topdir}/map/delivery/scripts/scripts.code.iter.md", "{topdir}/docker/docker.code.iter.md", "{topdir}/tools/tools.code.iter.md", "{topdir}/iter_engine/src/loop.code.iter.md"]
children:
  codedirs:  []
  codenodes: []
  tests:     []
  reqs:      []
timestamps: {create: "2026-10-02 23:05:35Z", last_modified: "2026-10-02 23:12:30Z", last_tested: ""}
---
# Operator

Works on machines, not in the web page:

- `./deploy.sh docker` (or `local`) on the server host; secrets
  (`ITER_ADMIN_PASSWORD`, `ITER_JWT_SECRET`, `ARANGO_ROOT_PASSWORD`) come
  from the repo `.env`.
- On each machine that should run agents:
  `curl -fsSL <data_url>/iter_engine_setup.sh | bash -s -- --data-url … --engine … --token … --start`,
  then `--status` / `--stop`. The engine is
  `iter_engine --data-url … --env-file … --name …`; adding or rotating an
  account token in the env file takes effect without a restart.

Often the same person as the admin, who then connects the new engine in the
Settings graph.
