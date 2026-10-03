---
id: b5280c91-7f58-4790-9078-cb69f4a8ea20
name: "ArangoDB Community Edition"
desc: "The database behind iter_data (ArangoDB 3.12 CE): one database per deployment (iter5 in the container; iter5_test_* and iter5_e2e_* throwaway databases on the dev container at :8529), holding work items, details, locks, projects, engines, users, spend, the project graph (node + link collections), the settings graph (sys_edge), test logs and GraphRAG documents. Reached only by iter_data, over HTTP with basic auth."
creator: "stephen"
teststate: inherit
level: container
owner: "3rdparty"
children:
  codedirs:  []
  codenodes: []
  tests:     []
  reqs:      []
timestamps: {create: "2026-10-02 23:12:30Z", last_modified: "2026-10-02 23:12:30Z", last_tested: ""}
---
# ArangoDB Community Edition

Where it runs:

- **Production / docker mode** — inside container `iter5`, started by
  `docker/iter5-entrypoint.sh`; iter_data reaches it on `127.0.0.1:8529`
  inside the container, the console is published on `127.0.0.1:8630` only.
  Database `iter5`, data in the `arango` named volume.
- **Local mode and tests** — the dev container on `:8529` (`./deploy.sh local`
  starts or reuses it); `cargo test` and `e2e.sh` create throwaway
  `iter5_test_*` / `iter5_e2e_*` databases and drop them afterwards
  (`ITER5_TEST_ARANGO_URL`, `ITER5_TEST_ARANGO_PASSWORD` point elsewhere).

Only iter_data talks to it (see the "ArangoDB HTTP API" connection); engines,
browsers and agents never do. Root password: `ARANGO_ROOT_PASSWORD`.
