# iter4

iter3 with a new data backend, a map of the program, and a test sweep. Spec and build plan: [`docs/iter4_spec.md`](docs/iter4_spec.md).

| dir | what it is |
|---|---|
| `iter_core` | shared types: work items, projects, engines, agents, locks, widgets, schedules, dedup keys |
| `iter_data` | the data server: axum API + ArangoDB storage (the only store; DynamoDB is read once, as the iter3 migration source), auth, locks, versioned writes, the architecture map (`graph.rs`), the iter3 migration (`migrate_ddb.rs`) |
| `iter_engine` | the local engine + the `iter` verbs, including `ids`, `sync`, `sweep` |
| `iter_local` | checkout-side tools: node-file scan, stable ids (`ids.rs`), map snapshot (`graph.rs`), testgroups, runner, validate |
| `webui/` | the single-file web page (embedded into iter_data) |
| `docker/` | the all-in-one image: ArangoDB CE + iter_data |

iter4 is its own cargo workspace (`iter4/Cargo.toml`), separate from the root iter/iter3 workspace. Build and test from `iter4/`.

## Run it

```bash
./deploy.sh docker   # build + start the container: API + webui on :8300, Arango console on 127.0.0.1:8530
./deploy.sh local    # native release iter_data against a dev Arango container on :8529 (fast dev loop)
```

Secrets (`ITER_ADMIN_PASSWORD`, `ITER_JWT_SECRET`) are read from `../.env` (override with `ITER_ENV_FILE`). The Arango root password is `ARANGO_ROOT_PASSWORD` (default `iter4dev`, local only). The same compose file runs on a small VM or EC2 instance.

## Point an engine at any iter_data

`.iter/config.json` `data_url` is the default; `ITER_DATA_URL` overrides it; `iter_engine --data-url …` overrides both. The engine's first line names the server and its backend.

## The web app: Intro | Work queue | Project graph | GraphRAG

One header for four tabs; the project picker in the header drives the queue and the graph (URL: `#tab=graph&p=<project>`).

- **Intro** — business and technical slides, and a new-project wizard (creates the project and engine records, mints an engine token, prints `iter init`).
- **Work queue** — the iter3 queue, unchanged.
- **Project graph** — the architecture map, ported from pdy-dev's usecase_map viewer (Cytoscape; cluster, tiered, rings, sequence and flow layouts; use-case steps; search; detail pane; legend), dark. Build from it: `+ Context`, a child from any node, `Connect` (an interface between two parts), `+ Global object` (bizreq / techreq / use case), `Edit a global object`, `Run tests`. Each change is accepted at once as "waiting to sync"; the first engine serving the project picks it up on its next heartbeat (`datasync_waiting`), applies it, commits just those files, pushes, and the map redraws.

- **GraphRAG** — semantic search over the project's documents: uploads (pdf incl. scans via OCR, docx, pptx, html, md, txt; the engine writes the original under the docs directory, default `{topdir}/docs/`, and commits it unless the directory is git-ignored) and the map's `*.iter.md` node files. Engines extract, chunk (in the model's tokens) and embed; each chunk has two vectors (raw text, Summary-agent summary). Search fuses keyword (BM25) and meaning rankings and returns the full chunk, its summaries and its map context (neighbours; documents linked by `children.documents`). The built-in user guide is searchable in every project. "Create a scheduled RAG change sweep" files a scheduled `iter rag sync`. Summaries need a live engine with an account. Details: `docs/iter4_spec.md` Phase 5.

Per-project picture rules live on the project record: `graph.hide` (folders left out) and `graph.parents` (placement overrides).

Right-click (or ⌘/ctrl-click) a node: **Add new child node** (any level may own any level; the form takes the name, an action-first description, a plain summary, the long description, requirements and planned tests), **Add new edge** (draw it to another node; ownership or an interface connection), **Define tests** (simplest first; optionally queue the `test` agent to build and run them — red is expected, and red groups become work items), **Run its tests**. Right-click an edge to remove it (a reason is required and lands in the commit). A use case's views include each part's owner chain (component → container → context), and the detail pane lists and links the code each node owns.

Node text follows `docs/node_text_standard.md`; `iter validate` flags text that breaks it and `iter sweep` turns each failing node into an `ingest` work item.

## The architecture map

```bash
iter ids              # report node files missing an id (or sharing one)
iter ids --fix        # give each an id (reuses the stored graph's id when it knows the file)
iter sync             # fix ids, then push the map: one vertex per *.iter.md, one edge per children link
iter sync --read-only --actors path/actors.yaml   # map a checkout without writing into it (derived ids)
iter graph-apply --file op.json                   # apply one graph edit by hand
iter init --project <name>                         # scaffold main.iter.md + .iter/config.json + reqs
iter sweep            # run the map's testgroups (teststate per chain), record results; file a code item per red group, a test item per untested node, an ingest item per unclear node
iter sweep --install-schedule --every 4h   # turn on the project's engine-owned Test sweep (created paused; user token required)
```

Outside an engine these read `--data-url`/`ITER_DATA_URL` (or `.iter/config.json`) and a token from `ITER_ENGINE_TOKEN`/`ITER_TOKEN`. A running engine syncs each project's map on its own, at most once a minute when the tree changed.

Read routes: `GET /api/projects/{p}/graph`, `…/graph/stats`, `…/graph/lookup?path=|name=&nodetype=`, `…/graph/owner?path=`, `…/graph/nodes/{id}`, `…/graph/nodes/{id}/neighbors?depth=N&direction=out|in|any` (AQL traversal on ArangoDB).

## GraphRAG and MCP

```bash
tools/fetch_model.sh     # the embedding model (all-MiniLM-L6-v2, ~91 MB) into models/ — deploy.sh docker runs it
iter rag sync            # re-index the map's node files whose text changed (what the scheduled sweep runs)
iter rag sync --force    # re-send every node file
```

Routes: `…/rag` (status), `…/rag/docs`, `…/rag/search`, `…/rag/settings`, `…/rag/schedule`, `…/rag/nodes`, `…/rag/work/claim|done` (engines).

**MCP**: iter_data serves a stateless MCP server at `POST /mcp` (Streamable HTTP, JSON). Send `Authorization: Bearer <token>`; `X-Iter-Project` / `X-Iter-Workid` set the default project and calling item. 24 tools cover the work queue (`status`, `workitem_*`, `capability`, `locks_list`), the map (`graph_*`) and GraphRAG (`rag_*`). Each call goes through the same API rules as HTTP. Every agent session the engine starts has it (`--mcp-config`), and each call is logged as `[iter_data] mcp <user> <tool> …`.

## Migrating iter3's DynamoDB records

```bash
iter_data --env-file ../.env --migrate-from dynamodb --migrate-prefix iter3_ [--migrate-dry-run] [--migrate-overwrite]
```

Reads every `iter3_*` table (never writes it; the source connection cannot create tables), copies rows with pk/sk/version preserved and seq counters with their values, skips rows already present, and prints source vs target counts per table.

## Tests

```bash
./deploy.sh local     # once: the dev ArangoDB on :8529 that the tests use
cargo test            # all crates; iter_data's tests each get a throwaway iter4_test_* database
./e2e.sh              # full stack on a throwaway Arango database
```

## Licence

iter is an MIT / open-source project and runs on ArangoDB Community Edition (decided 2026-09-29). CE 3.12.5+ is licensed for non-commercial use and datasets up to 100 GB; swapping to the Enterprise image changes no code.
