# iter4 — requirements and build plan

Written 2026-09-28 from `iter4/requirements.md`. iter4 is iter3 with a new data backend (ArangoDB Community Edition), a graph of the program built from its `*.iter.md` files, stable ids on every one of those files, and a test sweep that turns red test groups into work items. Everything iter4 lives under `~/dev/iter/iter4/`; iter3 is left untouched.

Terms used below, defined once:
- **node file**: any `*.<nodetype>.iter.md` file (nodetypes `main`, `code`, `interface`, `usecase`, `bizreq`, `techreq`, `testgroup`, per structureV2's dot rule).
- **graph**: the map of the application stored in ArangoDB: one vertex per node file, one edge per `children` link.
- **snapshot**: the engine's JSON rendering of the graph for one checkout (vertices + edges + a content hash).
- **work item**, **close gate**, **lock**, **seq**: unchanged from iter3 (`src/features/iter.v3.md`).

---

## 1. What stays the same

Taken from iter3 as-is (copied into `iter4/` at iter3 commit `56ed0df`):
- The split of work: **iter_data** is the only thing that holds data and serves the API and the webui; **iter_engine** is the only thing with repo access and runs the agents; **iter_core** holds shared types; **iter_local** holds the checkout-side verbs (marker scan, validate, runtests, testgroups).
- The webui design (single embedded `webui/index.html`).
- Work items: states, agents and agent types, versioned writes, detail rows, close gate, locks and lock shape, dedup, schedules, accounts, usage, spend, reopen and closed-item immutability.
- The HTTP API: every iter3 route keeps its path and shape, so the webui and the agent CLI work unchanged.

## 2. What changes

### R1. ArangoDB CE is the default backend
- iter_data gains a third storage backend, `arango`, next to `sqlite` (kept for tests and zero-install runs) and `dynamodb` (kept read-only in practice, as the migration source).
- ArangoDB is reached over its HTTP API (no driver dependency): `--arango-url` (default `http://127.0.0.1:8529`), `--arango-db` (default `iter4`), user/password from `ARANGO_USER` / `ARANGO_PASSWORD` (root with an empty password when unset, the container default).
- On start the backend creates the database, collections, indexes and the named graph if they are missing (additive only, like the DynamoDB backend's table creation).
- **Data mapping** (DynamoDB → ArangoDB):

| iter3 table (DynamoDB) | ArangoDB collection | kind | why |
|---|---|---|---|
| workitem, workitem_detail, agent, agent_tooling, project, project_prepostwork, engine, webui_user, webui, spend, project_structure | same name | document | JSON bodies that are read, filtered and listed by field |
| lock | `lock` | document used as key-value | read and written by key only (atomic acquire) |
| versions | `versions` | key-value | one counter per (project, table), bumped atomically |
| (new) graph vertices | `node` | document (vertex) | one per node file, `_key` = the file's frontmatter `id` |
| (new) graph edges | `link` | edge | one per `children` link, typed by the sub-key |

  Every generic row is stored as `{_key, pk, sk, version, expires, workid, body}`: `pk`/`sk` are the iter3 partition and sort keys, `body` is the unchanged iter3 JSON. `_key` is `pk` + `|` + `sk` with any character ArangoDB forbids written as `%XX`, so `get` is a primary-index lookup. When that key would exceed 254 bytes it is a hash instead. A persistent index on `[pk, sk]` serves `query` (all rows for one pk, ordered by sk).
- The three atomic operations keep their iter3 contracts:
  - versioned writes: an update filtered on `version == expect`, where a write-write conflict (error 1200) counts as a lost race;
  - lock acquire: create if absent, expired, or already held by the same work item;
  - seq bump: an upsert that adds 1.
- **Licence (decided by Stephen, 2026-09-29):** iter is an MIT / open-source project; it runs on ArangoDB Community Edition for now. Background: since 3.12.5, the ArangoDB Community Edition licence allows non-commercial use only, with datasets up to 100 GB. If iter supports commercial work (paydaay), the CE licence may not cover it. Swapping to the Enterprise edition changes no code.

### R2. One container with ArangoDB and iter_data
- `iter4/docker/Dockerfile`, a multi-stage build:
  - a Rust builder stage compiles a static musl `iter_data`;
  - the runtime stage is `arangodb:3.12` (Alpine), with `iter_data` added and an entrypoint that starts `arangod`, waits for it, then runs `iter_data --backend arango --listen 0.0.0.0:8300`.
- `iter4/docker/compose.yml`: one service, ports 8300 (iter_data API and webui) and 8529 (Arango web console, optional), and a named volume for `/var/lib/arangodb3`. The same file works in Docker Desktop and on a small VM or EC2 instance.
- `iter4/deploy.sh docker` builds and starts it; `iter4/deploy.sh local` runs a native iter_data against an Arango container (the fast development loop).
- Health: `GET /health` reports the backend and whether Arango answered.

### R3. The engine can point at any iter_data
- `.iter/config.json` `data_url` stays the default. `ITER_DATA_URL` in the environment and a `--data-url` flag override it, in that order (flag wins).
- The engine's startup line names the iter_data it is using and that server's backend (from `/health`).

### R4. Every node file carries a stable id
- Every node file's frontmatter has an `id:` key holding a UUID (v4), placed as the first key.
- `iter ids [--fix] [--dry-run]` (agent CLI, also run by the engine each tick before a sync):
  - reports every node file without an `id`, and duplicate ids;
  - with `--fix`, gives each missing one an id. First it asks iter_data whether a vertex already exists for that file (`GET /api/projects/{p}/graph/lookup` by path, then by nodetype + name). When exactly one matches, it reuses that vertex's id, so a file whose id line was lost reconnects to its history. Otherwise it mints a new UUID. It writes the id into the file.
  - a duplicate id (for example, a copied file) keeps the id on the file whose path matches the stored vertex; the other file gets a new id.
- `iter validate` gains the same checks as warnings (missing id, duplicate id, id not a UUID).

### R5. Sync of `*.iter.md` metadata from the engine to iter_data
- `iter_local::graph::snapshot(project)` builds the snapshot from the marker scan:
  - **vertices**: one per node file, carrying id, nodetype, name, description, level, owner, teststate (declared and effective), path (as `{topdir}/…`), codedirs, testpaths, the frontmatter as JSON, and a hash of the body (the body itself stays in git);
  - **edges**: `from` id, `to` id, `kind` = the `children` sub-key (`codenodes`, `inputs`, `outputs`, `bizreqs`, `techreqs`, `testgroups`, `usecase_codenodes`), plus `root` edges from `main` to each root node;
  - the snapshot also lists orphans, and links whose target has no id.
- `iter sync [--dry-run]` pushes it with `PUT /api/projects/{p}/graph`. iter_data replaces the project's vertices and edges in one pass: it upserts by id, deletes vertices and edges that disappeared, stores the hash, and bumps the `graph` seq. It answers with counts (added / changed / removed).
- The engine runs `ids --fix` + sync at start and whenever the snapshot hash changes (checked every `graph_sync_ticks` ticks, default 12, which is one minute at the 5-second tick). It never pushes an unchanged snapshot.
- Read API (all project-scoped, viewer role and up):
  - `GET …/graph` returns all vertices and edges;
  - `GET …/graph/nodes/{id}` returns one vertex;
  - `GET …/graph/nodes/{id}/neighbors?depth=N&direction=out|in|any` is an ArangoDB traversal on the `iter_map` graph;
  - `GET …/graph/lookup?path=…|name=…&nodetype=…`;
  - `GET …/graph/owner?path=…` returns the code nodes whose codedirs contain a path (deepest first);
  - `GET …/graph/stats`.
- Backends without a native graph (sqlite, dynamodb) store vertices and edges as ordinary rows and answer the same routes in Rust. ArangoDB answers traversals with AQL.

### R6. Architecture map + test sweep (the iter-prime idea, back)
- Test groups are vertices; their parent code, interface or usecase node is the edge that points at them.
- `iter sweep [--group <label>] [--dry-run]`:
  1. reads the graph from iter_data;
  2. walks each root's chains, honouring `teststate` per chain (omit / include / block / inherit);
  3. runs each eligible group through the existing deterministic runner (`iter runtests`);
  4. posts each result to `POST …/graph/nodes/{id}/testresult` (it lands on the vertex as `lastrun`, `result`, `counts`);
  5. for each red group, files one work item unless an open item already carries the group's `check:testgroup:<id>` dedup tag. The item is agent `code` and locks the parent code node's codedirs. Its request names the group, the failing checks with their output tails, and the owning node, and it carries the parent's `usecase:` tag and priority band.
- A project gets the sweep as a **scheduled** exec work item (`exec_shell: "iter sweep"`) created by `iter sweep --install-schedule [--every 4h]`. Scheduling, skip-don't-backfill, and dedup are the iter3 machinery, unchanged.

### R7. Fixes from pdy-dev, applied in iter4
Every iter-harness fix pdy-dev requested between 2026-09-14 and 2026-09-28 is catalogued in `docs/iter3_fix_catalogue.md`, with its status against iter3 HEAD. Per Stephen's 2026-09-28 direction ("keep everything within the iter4/ directory tree"), the open fixes are applied to iter4's copy of the code, not to iter3. The four known up front:
- close-gate question shape;
- deep-rule sibling deadlock;
- close write lost in an outage;
- locks held by items that are not running (lock deadlock prevention, 2026-09-25).

### R8. Migration (the goal)
`iter_data --backend arango … --migrate-from dynamodb --migrate-prefix iter3_ [--dry-run] [--overwrite]` copies every `iter3_*` DynamoDB table into ArangoDB:
- it copies raw rows with their `pk`/`sk` preserved, and seq counters with their values;
- it only reads DynamoDB, never writes it;
- it is idempotent: existing rows are skipped unless `--overwrite` is given;
- it prints per-table counts from both sides, which must match.

Then iter4 is pointed at its own repo:
1. `~/dev/iter/iter4/` carries its own node files: `main.iter.md`, a context-level code node, container nodes for iter_data / iter_engine / iter_core / iter_local / webui, component nodes where a crate has distinct parts, interface files for the iter_data API families, testgroups that run each crate's `cargo test`, and usecases.
2. `iter ids --fix` gives each of them an id.
3. `iter sync` builds the graph in ArangoDB.

**Done when:**
- `GET /api/projects/iter4/graph/stats` shows every node file as a vertex and every `children` link as an edge;
- a traversal from `main` reaches every non-orphan vertex;
- the per-table DynamoDB and ArangoDB counts match;
- the webui served by the container logs in and lists the migrated pdy-dev work items.

---

## 3. Build plan

| # | Step | Output | Proof |
|---|---|---|---|
| B1 | Copy iter3 crates + webui into `iter4/`, standalone cargo workspace | `iter4/Cargo.toml` | `cargo build --release` in iter4 |
| B2 | Arango backend (`iter_data/src/arango.rs`) behind the Storage trait; `--backend arango` default | backend + unit tests against a live Arango | storage contract tests (get/put/query/scan/versioned conflict/lock contention/seq) pass on sqlite AND arango |
| B3 | Docker image + compose + deploy.sh modes | `iter4/docker/` | `docker compose up`, `/health` shows `arango`, webui login works |
| B4 | Engine `--data-url` / `ITER_DATA_URL` | engine main | unit test of precedence |
| B5 | Frontmatter ids: `iter ids`, validate warnings | `iter_local/src/ids.rs`, cli verb | tests: missing → minted, lookup reuse, duplicate resolution |
| B6 | Snapshot builder + graph storage + API + engine sync | `iter_local/src/graph.rs`, `iter_data/src/graph.rs`, routes | tests on the e2e fixture tree; traversal via AQL on arango |
| B7 | Test sweep + schedule install + testresult route | `iter sweep` | test: a red group files one item, a second sweep files none |
| B8 | pdy-dev fixes (catalogue F1…Fn) | code + tests per fix | each fix's own test |
| B9 | iter4's own node files | `iter4/**/*.iter.md` | `iter validate` clean |
| B10 | DynamoDB → Arango migration tool | `--migrate-from dynamodb` | dry-run counts, then real run, counts match |
| B11 | Goal run: ids + sync of iter4 repo into the container, migration of `iter3_*` | the live container | stats and traversal checks from R8 |
| B12 | webui: graph section (list of nodes by type, neighbors, test results) | index.html | loads against the container |

Order: B1 → B2 → B3 (container first, per Stephen) → B4 → B5 → B6 → B7 → B8 → B9 → B10 → B11 → B12.

## 4. Out of scope for this pass
- pointing the pdy-dev engines at iter4 (production cut-over needs coordination);
- deleting iter3 or its DynamoDB tables;
- vector and search features of ArangoDB (available later without a schema change: an ArangoSearch view over `node` and `workitem_detail` is the natural first use).

---

## 5. Build status (2026-09-28)

| # | Step | Status |
|---|---|---|
| B1 | iter4 workspace | done: `iter4/Cargo.toml`, standalone workspace |
| B2 | ArangoDB backend | done: `iter_data/src/arango.rs`; storage contract (`contract_tests.rs`) green on sqlite and live ArangoDB, including 16-way versioned-write and lock races and 40 concurrent seq bumps |
| B3 | container | done: `docker/` + `deploy.sh docker|local|sqlite`; first-boot race with the image's init server found and fixed (no endpoint argument; `/health` pings iter_data's own database) |
| B4 | engine `--data-url` / `ITER_DATA_URL` | done, with a precedence test; the engine's startup line names the server's backend |
| B5 | frontmatter ids | done: `iter_local/src/ids.rs`, `iter ids [--fix]`, `validate` warns `missing-id` / `malformed-id` |
| B6 | snapshot + graph + sync | done: `iter_local/src/graph.rs`, `iter_data/src/graph.rs`, `iter sync`, engine auto-sync (`sync_maps`, at most once a minute per project); every node file is a vertex (context files get `context` edges, unlinked files are flagged orphans) |
| B7 | test sweep | done: `iter sweep`, `--install-schedule`; teststate per chain; only red groups file items (an erroring script is recorded, not filed); dedup shared with `iter runtests` |
| B8 | pdy-dev fixes | applied, see `docs/iter4_fixes_applied.md`; decisions left open are listed there |
| B9 | iter4's own node files | done: 22 files, `validate` clean |
| B10 | DynamoDB → Arango migration | done: `iter_data --migrate-from dynamodb` (`migrate_ddb.rs`, source opened read-only) |
| B11 | goal run | done: see §6 |
| B12 | webui map | done: "Architecture map" side section + browse dialog (nodes by type, links in/out, test results) |

Deviation from the requirements, by direction: the requirements asked for the pdy-dev fixes to be applied to iter3 as well. Stephen's later instruction ("keep everything within the iter4/ directory tree") took precedence, so iter3 is unchanged and the fixes live only in iter4.

## 6. Goal run (2026-09-28)

- Container `iter4` (image `iter4:latest`, ArangoDB 3.12.12 + iter_data) running on `127.0.0.1:8300`; Arango console on `127.0.0.1:8530`.
- `iter_data --migrate-from dynamodb --migrate-prefix iter3_` into database `iter4`:

  | table | source | target |
  |---|---|---|
  | agent | 8 | 8 |
  | project | 2 | 2 |
  | engine | 2 | 2 |
  | workitem | 3047 | 3047 |
  | workitem_detail | 18637 | 18637 |
  | webui_user | 4 | 4 (admin already bootstrapped by the container, so skipped) |
  | lock | 200 | 200 |
  | agent_tooling | 19 | 19 |
  | spend | 24 | 24 |
  | seq counters | 13 | 13 |

  `project_prepostwork`, `webui` and `project_structure` are empty on both sides. Log: `run/migrate_ddb_*.log`.
- `iter ids --fix` + `iter sync` in `~/dev/iter/iter4` → project `iter4`: 22 vertices (main 1, code 7, interface 5, usecase 3, testgroup 4, bizreq 1, techreq 1) and 39 edges (codenodes 13, root 9, inputs 5, outputs 4, testgroups 4, context 2, bizreqs 1, techreqs 1). All 22 are reachable from main by AQL traversal; no orphans; nothing unresolved.
- `iter sweep` on iter4 against the container: 4 testgroups run off the map, all green (iter_core 53, iter_data 31, iter_engine 71, iter_local 46), results stored on the testgroup vertices.

## 7. Verification (2026-09-28)

- Unit tests: 201 passing (`cargo test`, with the ArangoDB storage contract enabled).
- End-to-end: `./e2e.sh sqlite` 73/73 and `./e2e.sh arango` 73/73 (run one after the other: both modes share the fake probe server's port 8399, so they cannot run in parallel).
- e2e found one real regression from the F17 split of reservation rows (a reservation outlived its holder leaving the queue and blocked the scope for nobody). Fixed in `engine.rs`: the gate honours a reservation only while its holder is still queued. Two e2e assertions were updated to the new evidence wording from F8/F9 (the verifier is told about sibling commits in the scope, labelled as another item's work).
- Web page checked in Chromium (Playwright): login against the container, 3,037 migrated pdy-dev complete items listed, map panel and browse dialog working, no page errors.

---

# Phase 2 (requested 2026-09-28 evening): project graph, test agent, graph editing, intro

Source: Stephen's message of 2026-09-28 ("I'm going to bed, but one more big [bit] left…"). Reference implementation for the viewer: `~/dev/pdy-dev/demos/usecase_map/` (Cytoscape + fcose + dagre, data from `build_usecase_map.py`).

## Requirements

- **R9. Project graph tab.** A native graph viewer of every `*.iter.md` node of the selected project, working like the usecase_map app: layouts (cluster, tiered, rings, sequence, flow), use-case picker with numbered process/data flow steps, edge filters, search, fit/re-layout/hide, and the header / left detail pane / footer legend. Differences from the reference:
  - contexts are drawn as enclosing areas (buckets such as data, comms, auth) holding their containers; containers hold components (crates, packages, libs);
  - edge types are **ownership** (parent → child through `children.codenodes`) and **interface connections** (code node A outputs interface I, code node B inputs I → a connection A→B labelled I);
  - styling matches the dark work-queue webui (same CSS tokens), not the reference's light theme.
- **R10. One app, three tabs, one header.** `[ Intro | Work queue | Project graph ]` in the shared header; the project picker drives both the queue and the graph, and the header shows which project is on screen in both. The viewer lives in iter_data's webui (served by iter_data, embedded in the binary; Cytoscape vendored, no network needed).
- **R11. Read-only mapping of a live checkout.** A checkout whose node files carry no `id:` (pdy-dev) can be mapped without writing into it: `iter sync --read-only` derives each missing id deterministically (UUIDv5 of project + path) and flags the vertex `id_derived`. `iter ids --fix` later writes real ids; the derived id of a file equals what the stored graph knows, so ids stay stable across the switch.
- **R12. Tests through the queue; the `test` agent.** Ongoing testing uses the work queue itself. The `testwriter` agent is renamed **`test`** (singular verb, like plan and code). A `test` item with an `exec_shell` runs deterministically (no LLM) — the 95% case; without one, it is the LLM test-writing agent. The sweep and one-off runs file `test` exec items. `testwriter` is accepted as an alias for existing records.
- **R13. `tests` replaces `testgroup`.** The nodetype is `tests` (`name.tests.iter.md`, children sub-key `tests`); `*.testgroup.iter.md` and `children.testgroups` stay readable as legacy spellings so existing projects (pdy-dev) keep working.
- **R14. Build projects from the graph.** From the webui: create context/container/component nodes (writing `name.code.iter.md`, `name.bizreq.iter.md`, `name.techreq.iter.md`, `name.tests.iter.md`), create a child of an existing node, connect two nodes through a new or existing interface, create and edit global objects (bizreq, techreq, usecase), and queue a one-off test run. iter_data has no repo access, so every edit is a queued `exec` work item (`iter graph-apply`) that an engine applies in its checkout, commits, and syncs back — locks, priorities and history come from the queue for free.
- **R15. Intro tab.** Slide-like pages: what iter is, what it does, the piece parts (iter_engine, iter_data, webui), why it matters, how it works (with links into the iter4 project graph), how to get started, and a new-project wizard (creates the project record, shows and explains every setting, and gives the `iter init` command that writes `main.iter.md` + `.iter/config.json`). Two tracks: a business deck and a technical overview.

## Build plan (Phase 2)

| # | Step |
|---|---|
| P1 | Snapshot carries descriptions (simple + long), use-case flowmaps (full YAML frontmatter), and derived ids (R11) |
| P2 | `GET /api/projects/{p}/graph/view`: the viewer's shape (nodes with levels and parents, contains / interface / flow / usecase_touches edges, use cases with sequences) |
| P3 | Viewer ported into `webui/graph.js` + `graph.css` (dark), Cytoscape vendored under `webui/vendor/`, iter_data serves the extra static files embedded |
| P4 | Tabs + shared header in `index.html`; project picker shared |
| P5 | pdy-dev mapped read-only into the container; Playwright compares the reference app with the new tab, differences explained |
| P6 | `tests` nodetype + `test` agent (R12, R13) |
| P7 | Graph editing (R14): `iter graph-apply` + webui forms |
| P8 | Intro tab + wizard + `iter init` (R15) |
| P9 | iter4's own node files built out (contexts as buckets, components per crate module), synced, verified with Playwright |

## Phase 2 status (2026-09-29, overnight)

| # | Step | Status |
|---|---|---|
| P1 | snapshot: descriptions, flowmaps, derived ids, actors | done — `iter_local/src/graph.rs` (`snapshot_with`, `SnapOpts`), tests `read_only_snapshot_derives_ids_and_reads_actors`, `long_description_and_yaml_front` |
| P2 | `GET /graph/view` | done — `iter_data/src/graph_view.rs`; project settings `graph.hide` / `graph.parents`; component ids `folder/stem` when a folder holds several code nodes |
| P3 | viewer ported, dark, vendored | done — `webui/graph.js` (from usecase_map app.js), `graph.css`, `webui/vendor/` (MIT), all embedded in iter_data |
| P4 | tabs + shared header + project picker | done — `index.html`: `[ Intro | Work queue | Project graph ]`, hash `tab=…&p=…` |
| P5 | pdy-dev mapped read-only + Playwright parity | done — `docs/graph_parity_pdy-dev.md`: node and ownership counts identical; 138/138 shipped-mode views identical; interface-edge differences explained (derived vs audited) |
| P6 | `tests` nodetype, `test` agent | done — `testgroup` still read; `WorkItem::is_shell()`; `test` agent record created in iter4's database (`testwriter` kept as an alias); sweep schedule and one-off runs are `test` shell items |
| P7 | graph editing | done — `iter_local/src/graph_edit.rs` (new_node, connect, new_global, edit_body; `lock_scope`), `POST /graph/edits`, `iter graph-apply`, `webui/graphedit.js`; e2e block "graph edits" drives five edits through a real engine |
| P8 | Intro tab + wizard + `iter init` | done — `webui/intro.js` / `intro.css` (12 business + 13 technical slides, new-project wizard), `iter_engine/src/init.rs` + `iter init` |
| P9 | iter4's own map | done — 62 node files: 5 context buckets under `map/` (Data, Engine, Shared model, Web, Delivery), 7 containers, 31 components (one per module group), 8 interfaces, 4 use cases with flowmaps, 3 actors (`map/actors.yaml`); `validate` 0 findings; synced (62 vertices / 95 edges, all reachable from main) |

Requirements synthesized into `reqs/iter4.bizreq.iter.md` (B6–B11) and `reqs/iter4.techreq.iter.md` (T6–T12).

## Datasync (decided 2026-09-29)

Stephen's direction: a graph edit is accepted at once with a "waiting to sync to engine" state; the engine heartbeat carries a `datasync_waiting` indicator and a new API route lets the engine fetch and apply the change; the fastest engine picks it up within one heartbeat, reconciles with other work, applies it and commits (and pushes) straight away.

- **iter_data** (`iter_data/src/datasync.rs`): table `datasync` (pk project, sk time-ordered id). `POST …/graph/edits` stores a pending row (file-changing ops; `run_tests` stays a `test` work item). Routes: `GET …/datasync[?state=claimable|pending|…]`, `POST …/datasync/{id}/claim {engine}` (versioned write: exactly one engine wins; a claim lapses after 600 s), `POST …/datasync/{id}/done {engine, outcome: applied|failed|retry, commit, files, error}`. The heartbeat reply carries `datasync_waiting: {project: n}` for the projects the engine serves; graph stats carry `datasync_pending`.
- **Engine** (`iter_engine/src/datasync.rs`): on the tick whose heartbeat reply names waiting edits, oldest first — an edit whose paths overlap a live lock waits (a running item's work is not disturbed); otherwise claim, `git pull --no-rebase` when there is a remote, apply (`graph_edit::apply`), commit exactly the files written (never anyone else's), push, report applied with the commit (or failed with the reason), and push the map so open Project graphs redraw.
- **Webui**: the edit dialog says "Accepted — waiting to sync to an engine"; the graph toolbar shows "⟳ n waiting to sync" / "✗ n failed" (click for the list: state, change, engine, commit, note) and redraws when an edit lands.
- An edit waits for an engine that serves the project; for iter4 itself that means an engine running in `~/dev/iter`, which commits and pushes there.
- Tests: `datasync::tests` (both crates), e2e block "graph edits via datasync".

## pdy-dev structure links (2026-09-29, with Stephen's permission: "establish new links (only)")

The map found 131 pdy-dev node files no node linked (129 test-tier testgroup files such as `pdy_core_intake/tests/iter/pdy_core_intake/boot.testgroup.iter.md`, plus `webapp/dev1` bizreq and techreq). Each was appended to its nearest owner's `children.testgroups` / `bizreqs` / `techreqs` (60 files; list entries only, nothing else changed or removed), committed locally in pdy-dev as `a60017cb7` (not pushed; pdy-dev's engines carry it on their next push). After it, all 1234 pdy-dev node files are reachable from main (3468 edges). Correction: the earlier summary said "three use cases name parts their parent files don't list" — measured, no part lacks a parent link; the gap was these unlinked test and requirement files.


---

# Phase 3 (requested 2026-09-29): the map must explain itself — product defects and enhancements

Source: Stephen's review of the iter4 Project graph ("many C4 objects I don't understand, for example 'iter verbs'…") and his follow-up: *"these changes … should be treated as product defects / enhancements and built in such a way as to address future such changes."* So each item below is fixed in the product (rules, automation, UI) for every project, and iter4's own map is then brought up to the rule as the worked example.

| # | Defect / enhancement | Product fix |
|---|---|---|
| D1 | Node text is descriptive, not action-oriented ("The `iter` command agents and people run: add, ask, …"); names are internal code words ("iter verbs") | A written **node-text standard** (`docs/node_text_standard.md`, also the `_iter_file_authoring` capability agents read); `iter validate` warns `description-not-action`, `missing-simple-description`, `thin-long-description`; the test sweep files one `ingest` item per node whose text fails (dedup key `check:node-text` + the node path), so text debt becomes queue work |
| D2 | Long descriptions only repeat the one-liner | same rule (`thin-long-description`: under 60 words, or mostly the description again) |
| D3 | The detail pane shows the `*.iter.md` file but not the code it owns | the snapshot lists each code node's source files (from `codedirs`, capped) and the repository's web URL + path prefix; the detail pane lists them as links |
| D4 | A use case's parts float free of the areas they live in ("iter verbs" next to "Developer", no container or context) | use-case views pull in every touched node's owner chain (component → container → context) with its ownership edges; the use-case-node overlay ties each use case to the top of each chain |
| D5 | No direct manipulation on the map | right-click / ⌘-click context menu: add a child node (any level may own any level), add an edge by drawing it to another node (ownership or interface), define tests (ordered simple → complex) and optionally queue the `test` agent to build them, remove an edge with a recorded reason |
| D6 | TDD from the map | tests defined on the map are written into the node's `*.tests.iter.md` as a planned-tests list; the queued `test` agent writes the scripts (red is expected); the sweep turns red groups into fix items — requirements and tests first, code by agents, all through the queue |

## Phase 3 status (2026-09-29)

| # | Status | Where |
|---|---|---|
| D1 node text | done — standard `docs/node_text_standard.md` (also appended to the `_iter_file_authoring` capability agents read); `iter validate` warns `description-not-action`, `missing-simple-description`, `thin-long-description` (`iter_local/src/validate.rs: node_text_findings`); `iter sweep` files one `ingest` item per failing node (dedup `check:node-text` + node path, `--text-max`, default 10 per sweep); graph-editor forms ask for description / plain summary / long description with the standard as the hint. iter4's 52 nodes + 4 use cases rewritten to the standard ("iter verbs" → "iter command line"); `validate` 0 findings, sweep finds 0 failing nodes |
| D2 long descriptions | done — same rule; iter4's are 150–350 words, written from the source |
| D3 code files | done — the snapshot lists each code node's source files (`code_files`, from codedirs, capped 60) and the repo's `{web, branch, prefix}`; the detail pane's "Code" section links them. Found and fixed on the way: the scanner silently dropped `codedirs` entries that name files (`iter_local/src/markers.rs`) |
| D4 owner chains | done — a use case's views add every touched part's owners (dashed) with ownership edges; the use-case overlay links each use case to the top of each chain; selecting a use case keeps its whole chain lit |
| D5 context menu | done — right-click / ⌘-click a node: add child (any level owns any level), add edge by drawing it (ownership or interface), define tests, run its tests; right-click an edge: remove it (reason required, recorded in the commit). New ops `link_child`, `unlink_child`, `disconnect`, `define_tests`; nodes addressed by their file so components sharing a folder stay distinct |
| D6 TDD | done — planned tests (simplest first) written into the node's `*.tests.iter.md`; `queue_agent` makes the applying engine file a `test` agent item that writes and runs the scripts; red groups become fix items through the sweep, now tagged with the use cases that pass through the red code (spec R6) |
| + coverage | new — code beside what a node's components own but claimed by none is marked `uncovered_files` (detail pane "Code no component owns"; the node-text sweep item carries it). It found `iter_local/src/lib.rs`; iter4 now has none |
| open | the TDD header-level option — Stephen's message was cut off ("there should be a header-level option (for project graph / work queue / intro) that…") |

## Phase 4: hierarchical use cases, every node editable (2026-09-29)

Product changes (they apply to every project, not only iter4's map):

- **H1. Use cases are hierarchical, from tags.** A use case names the parts it needs (`children.codenodes`, plus the code nodes its flowmap steps mention) and nothing else. `iter_local::usecase_map::usecase_map()` runs on every map store (`PUT …/graph`): it walks each named part up its ownership chain to the highest code node and tags every node on the way (`node.usecases: [ucid]`); the use-case vertex records `uc_named` and `uc_tops`. Tags are recomputed from the snapshot on each store, so they never drift from the files.
- **H2. Array index + lookup.** ArangoDB persistent index `project_usecases` on `node [project, usecases[*]]`; `GET /api/projects/{p}/graph/usecases/{ucid}` returns the use case, its tops and every tagged node in one indexed query (contract test checks the plan uses the index).
- **H3. View.** `graph/view` gives each use case `members` and `tops`; `usecase_touches` edges go to the tops only. The Project graph draws a use case as use case → top-level parts → ownership lines; the dashed "ghost" owners are gone. Numbered steps are an overlay ("numbered steps" checkbox) and the Sequence layout.
- **H4. Adding a use case queues the `usecase` agent** (graph edit `new_global` kind usecase, or `name_parts` on an existing one) to list the parts its journey needs; the tags follow on the next sync.
- **H5. Every node type is editable.** New graph-edit ops: `new_actor`, `edit_actor`, `actor_uses`, `actor_unuse`, `usecase_needs`, `usecase_unneed`, `name_parts`. The actors file path travels with the snapshot (`actors_file`, default `{topdir}/actors.yaml`). Right-click menus on code nodes, actors, use cases and the project; a drawn edge may join code↔code, actor↔code (uses) or use case↔code (needs), either end first; every menu has "Hide this node".
- **H6. Interfaces have a plain label.** `label:` in the interface frontmatter ("Files a work item") captions the connection; `validate` warns `missing-interface-label`; the template and the graph editor write one. Default edge grey lightened for the dark background.

iter4's own map: "Data and API" and "Web page" merged into **Orchestration and Data** (`map/orchestration/`, owns `iter_data` and `webui`); Developer and Operator use the new `webui-page` interface, so they connect to the WebUI container.
- **H7. Use-case layouts follow the hierarchy, not the steps.** Top-down and left-to-right place the use case first, then every actor its steps name (together, linked from the use case), then its top-level parts, then each owned part one row/column below its owner (`graph.js: ucBands`; Flow lays out the ownership tree with dagre and gives the actors their own column). Numbered steps are drawn over those places and never move them.
- **H8 (revises H7).** Use-case placement is ordered by (a) the hierarchy, then (b) the numbered steps: the use case, its actors together, then each hierarchy level; inside a level, parts spread over as many ranks as the steps reach them at (hop depth along the primary flow), so the journey reads forward without breaking the hierarchy. Flow uses the same ranks as Top-down, turned on their side. The use case links only to the highest node type present: its actors if any, else its top-level contexts, else containers, and so on (`graph_view.rs`, `usecase.links`).
- **H9. Sequence view.** Column heads stay pinned to the top edge while scrolling; the "N steps across M parts" hint clears after 6 s or on the first pan/zoom; the post-draw settle no longer re-fits a Sequence (it used to zoom a long one down to unreadable); hover highlights clear when the drawing moves.

## ArangoDB only (decided 2026-09-29)

Stephen: "let's stop support for SQLite… move everything 100% to Arango for now." Done:

- `iter_data` serves from ArangoDB only; `--backend` accepts only `arango` (kept so scripts may say so). Removed: `sqlite.rs`, the V2 SQLite import (`migrate.rs`, `--migrate-v2`), DynamoDB serving and the Lambda entry point, and the `rusqlite` and `lambda_http` dependencies. DynamoDB remains only as the read-only source of `--migrate-from dynamodb` (`DdbBackend::new_readonly`).
- The architecture map code has no row fallback any more: every map route runs AQL on the `node`/`link` collections.
- Tests: `iter_data/src/test_db.rs` gives each test its own `iter4_test_*` database on the dev ArangoDB (:8529, `./deploy.sh local`) and drops a previous run's leftovers; `tools/cargo-test-crate.sh iter_data` reports "could not run" (exit 2) without it. `e2e.sh` and `deploy.sh` are ArangoDB-only (`./e2e.sh`, `./deploy.sh docker|local`).

Earlier sections that mention SQLite describe what was built at the time.

## Phase 5: GraphRAG + stateless MCP (2026-09-29)

Stephen's request: a fourth webui tab, **GraphRAG**, over two kinds of document — arbitrary uploads (docx, pdf, md, txt…) and the `*.iter.md` node files of the map — with an ingestion pipeline (text → chapters + chunks → LLM summary per chunk by a new **Summary** agent on Sonnet/Haiku, outside the work queue → embeddings with all-MiniLM-L6-v2 → vectors in ArangoDB, full chunk returned on a hit), a button that creates a scheduled RAG change sweep, the API for all of it, and every agent-facing API call wrapped as a stateless MCP tool.

Decisions (Stephen, 2026-09-29):
- **Two vectors per chunk**: raw text (`vec_raw`, written at ingest, so a document is searchable at once) and LLM summary (`vec_sum`, written when the Summary agent reports). Search scores by the better of the two.
- **Documents are per project.** An upload is chunked, embedded and indexed by iter_data, then the original is handed to an engine (datasync op `store_doc`) that writes and commits it under the project's docs directory — a GraphRAG setting, default `{topdir}/docs/`. Node files already live in the checkout.
- **MCP runs on iter_data** at `POST /mcp` (Streamable HTTP, JSON responses, no sessions).
- **GraphRAG requires a live engine with an LLM account** for summaries; without one, documents are searchable by raw text and the tab says summaries are waiting.

Built:

| Part | Where | What |
|---|---|---|
| G1 embedding | `iter_data/src/rag/embed.rs` | all-MiniLM-L6-v2 in-process via candle (CPU, 384-dim, mean-pooled, L2-normalised, 256-token window); model dir from `--embed-model` / `ITER_EMBED_MODEL` / `models/all-MiniLM-L6-v2` / `/opt/iter/models/…` (container); `tools/fetch_model.sh` downloads it (sha256-checked, git-ignored) |
| G2 extraction | `rag/extract.rs` | pdf (pdf-extract, panic-guarded), docx (`word/document.xml`, headings → `#`), html (tags stripped, h1–h3 kept), UTF-8 text; refuses old binary Office formats and text-less files (scanned PDFs) |
| G3 chunking | `rag/chunk.rs` | chapters = sections at the top heading level (a lone title drops a level); heading-like lines for PDFs/plain text; ~1000-char chunks of whole paragraphs (≤1500), fenced code kept whole, heading path per chunk |
| G4 index + pipeline | `rag/mod.rs` | collections `rag_doc`, `rag_chunk`, `rag_setting`; ingest keeps the summaries of unchanged chunks (text hash); Summary work = claimable chunks (batches ≤8 chunks / 12k chars of one document) then one rollup per document (chapter + document summaries; a one-chunk document needs none); claims lapse after 10 min; 3 failed attempts leave a chunk raw-only |
| G5 search | `rag::run_search` | exact cosine in AQL (both vectors), or ArangoDB vector indexes (`vec_raw`, `vec_sum`, sparse, built once ≥1024 chunks; arangod `--vector-index`, now on in the container and the dev Arango) as a candidate pass re-scored exactly; returns full chunk + chapter + document summaries + the node's map neighbours; optional document ranking by summary vector |
| G6 API | `rag::routes` | `GET …/rag` (status), `GET/PUT …/rag/settings`, `GET/POST …/rag/docs`, `GET/DELETE …/rag/docs/{id}` (`?remove_file=true` queues `remove_doc`), `POST …/rag/docs/{id}/resummarize`, `PUT …/rag/nodes`, `GET …/rag/nodes/hashes`, `POST …/rag/search`, `POST …/rag/work/claim`, `POST …/rag/work/done`, `GET/POST …/rag/schedule`, `GET /api/rag/model`; heartbeat reply carries `rag_waiting` |
| G7 Summary agent | `iter_engine/src/rag.rs: summarize_waiting`, `Engine::start_summaries` | one worker thread per project with work, outside the cap, not while holding; `claude -p --tools "" --no-session-persistence`, model from the `summary` agent record (seeded by iter_data: haiku, 300 s); JSON answers; spend rows tagged `agent: summary` |
| G8 change sweep | `iter rag sync` | sends only node files whose sha256 changed + the full path list (deleted files dropped); the tab's button creates a scheduled exec item (`exec_shell: iter rag sync`, tag `rag-sweep`, one per project) |
| G9 file storage | `iter_local::graph_edit` ops `store_doc` / `remove_doc` | written by the datasync applier, committed like any graph edit; never touches `*.iter.md`, never leaves the checkout |
| G10 webui | `webui/rag.js`, `rag.css` | status strip (engine, model, progress), drag-and-drop upload, document lists (uploaded / node files), detail pane (summary, chapters, chunks, re-summarise, remove), search with filters and map-neighbour chips, docs-dir setting, sweep button, MCP hint |
| G11 MCP | `iter_data/src/mcp.rs` | 23 tools: status, workitem_list/get/details/create/ask/reject/wait/doc/block, capability, locks_list, graph_stats/lookup/node/neighbors/owner/usecase, rag_search/status/docs/doc/add_document; each replays the API through the router with the caller's token; `X-Iter-Project` / `X-Iter-Workid` defaults. Not wrapped: checkout verbs (runtests, validate, sync, sweep, rag sync) and `critreview` (a model session on the engine) |
| G12 map | `rag.code.iter.md` ×2, `mcp.code.iter.md`, `webui/rag.code.iter.md`, interfaces `rag-search`, `rag-work-claim`, `rag-nodes-sync`, `rag-doc-upload`, `mcp-call` | validate clean (75 files, 0 findings) |

An agent reaches the MCP server with, e.g., `.mcp.json`:

```json
{"mcpServers": {"iter": {"type": "http", "url": "http://127.0.0.1:8300/mcp",
  "headers": {"Authorization": "Bearer ${ITER_ENGINE_TOKEN}", "X-Iter-Project": "${ITER_PROJECT}", "X-Iter-Workid": "${ITER_WORKID}"}}}}
```

### Phase 5b: GraphRAG round 2 (2026-09-29, Stephen's review)

Decisions: **split embedding** — the engine does all bulk work (extraction, token-sized chunking, chunk and summary vectors, the Summary agent) on its native hardware; iter_data keeps the model only to embed search questions (and the built-in guide), so search never needs an engine. **OCR through Claude** (pdftoppm renders pages, a Read-only Claude session transcribes them). Summaries' cost is not held to the daily budget (rare; Stephen).

| Part | Where | What |
|---|---|---|
| R1 shared crate | `iter_rag/` | extract (+pptx: a chapter per slide with notes; scanned-PDF detection, <40 chars/page), token-aware `chunk_with` + `Sizer::tokens` (chunk + "title — heading" prefix ≤ 256 word-pieces, so no tail is cut from a raw vector), embedder with model **stamp** (name@sha12), `ensure_model` download to `~/.cache/iter/models`, shared `prepare` |
| R2 engine ingest | `iter_engine/src/rag.rs` | job kinds ingest → chunks → rollup; `ocr_pdf` (≤120 pages, 6 per session); summaries embedded before reporting; `INDEX_VERSION` in node hashes (bump = full re-index, summaries of unchanged chunks kept); invalid-JSON answers salvaged per summary |
| R3 re-index on map change | `Engine::sync_maps` | a map push whose snapshot hash changed starts `rag::sync` (hash-diffed; one per project). Map checks run once a minute, push only on change — not every 5 s heartbeat |
| R4 keyword + fusion | `iter_data/src/rag/search.rs` | ArangoSearch view `rag_view` (text_en on text, summary, heading, title; identity on project/kind/nodetype/doc), BM25; raw+summary vectors merged into ONE meaning ranking (fusing them separately outvoted keyword), RRF with keyword; ≤2 chunks per document; modes hybrid/vector/keyword |
| R5 documents on nodes | `children.documents` (validate allows it on code, interface, usecase, bizreq, techreq) | snapshot → vertex `documents`; index `node[project, documents[*]]`; graph edits `link_document` / `unlink_document`; `POST …/rag/docs/{id}/links`; MCP `rag_link_document`; hits carry `graph.linked_nodes` (file) / `graph.documents` (node) |
| R6 docs .gitignore | setting `docs_gitignore` | queues `gitignore_path` (adds/removes `/<docs_dir>` in the project .gitignore); the datasync applier never commits a git-ignored path (`git check-ignore`) and the tab shows "written" |
| R7 MCP in every session | `work.rs: mcp_config` | `--mcp-config` (system temp dir, 0600, deleted after the run) with the engine token + X-Iter-Project/Workid; ELI5 gets `MCP_READ_TOOLS`. Prompts rewritten to point at the tools (`_shared` "Your tools" section; `_ask_the_human`, `_create_new_workitem`, `_reject_invalid_work`, `_block_cluster_restart`); originals in `run/tooling_backup_20260929.json` |
| R8 close gate sees notes | `gate::Evidence.notes` | the verifier gets the `doc` rows an attempt added (a note-only answer was bounced as unproven) |
| R9 search-quality testgroup | `iter_data/test/rag_eval.{json,sh}`, group `rag-search-quality` | 24 questions → expected node files; hit@5 + MRR per mode; green at hybrid hit@5 ≥ 0.8 |
| R10 built-in guide | `iter_data/build.rs`, `rag/guide.rs` | `docs/iter4_guide.md` (written by another session) compiled in, ingested at startup as scope `_iter`, kind `guide`; every project's search includes it unless `include_guide: false`; engines claim `_iter` summary work when idle |

Measured (iter4's own map, 24 questions, hit@5 / MRR): before — vector only 0.88 / 0.799; three rankings fused naively — hybrid 0.83; final — **hybrid 0.92 / 0.803**, vector 0.88 / 0.778, keyword 0.88 / 0.821. Live: a scanned one-page PDF OCR'd in 8 s and found by its code word (hybrid first; vector alone chose the wrong document); a pptx's speaker notes searchable; document↔node links shown both ways.

Agents and MCP (tested with real `code` agents): both test items filed their answer through MCP `workitem_doc`; neither called `rag_search` — the answers sat under the agent's working directory, where Grep was quicker. The `_shared` pointer now says to search before Grep.

Found: the engine treats a topdir without its own `.git` as "not a repository" — iter4 (inside ~/dev/iter) never pulls, commits or pushes; datasync writes files only. Open decision for Stephen.
