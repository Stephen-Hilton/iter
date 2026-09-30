---
id: a9040402-89d2-4e8e-8c61-04c88f0876b8
name: "docker — all-in-one container"
description: "Builds one Docker image that runs the ArangoDB database and the iter_data server side by side, starts them in the right order, and stops the container if either dies, so a whole iter server comes up with one command on a laptop or a small cloud VM."
simple_description: "The package that bundles the database and the server so they can be started together."
level: container
owner: bespoke
teststate: inherit
children:
  codedirs:   ["{thisfiledir}/"]
  codenodes:  []
  inputs:     []
  outputs:    []
  bizreqs:    []
  techreqs:   []
  tests:      []
---

# Long Description

The docker folder builds and runs iter's server as one container: ArangoDB Community Edition and iter_data together.

How it works: `docker/Dockerfile` has two stages. The first compiles a static (musl) `iter_data` with the web page embedded. The second starts from the official `arangodb:3.12` image and adds that binary and `docker/iter4-entrypoint.sh`. The entrypoint refuses to start unless `ARANGO_ROOT_PASSWORD` (or an explicit no-auth setting) is given, starts `arangod` through the image's own entrypoint so the first-run password setup happens, waits up to two minutes for ArangoDB to answer, then starts `iter_data --backend arango` on port 8300. It watches both processes and stops the container if either exits, so Docker's restart policy brings them back together. `docker/compose.yml` publishes 8300 (API and web page), puts the ArangoDB console on `127.0.0.1:8530` only, and keeps data in two named volumes (the database, and iter_data's secret file).

What goes in and out: `deploy.sh docker` (in scripts — deploy, e2e and test tools) writes `run/docker.env` with the admin password and token secret and runs compose. Engines and browsers then reach the container on port 8300. The image's health check calls `/health`.

Why it matters: without it, running iter means installing and configuring ArangoDB and wiring the server to it by hand. A startup-order bug was found here: passing an endpoint argument made ArangoDB's temporary first-run server answer on the real port, and iter_data created its database on that throwaway server.

Example: `./deploy.sh docker` builds the image, starts container `iter4`, waits for `/health` to say ok, and prints the web page address and the console address.
