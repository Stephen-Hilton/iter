# iter5 — spec and build contract

Source brief: `requirements.md` + `requirements.png` (the architecture diagram).
iter5 is a fork of iter4 (the repo root is its cargo workspace). Everything iter4 does keeps working
unless this document says it changes. Decisions taken with Stephen on 2026-10-02:

- Fork iter4, then rework (not greenfield).
- **Node files are `*.code.iter.md` only** for all four C4 levels; the level is the `level:` key
  (`context | container | component | connection`). Filenames like `data.context.iter.md` in the
  brief's mockup are typos. `actor` is its own node type (behaves like `usecase`).
- Build an iter4 → iter5 file converter (`iter migrate5`), test it on a *copy* of pdy-dev, never run it on live checkouts.
- Automated tests dispatch to a deterministic **`mock` provider**; one opt-in live Claude smoke test.
- Deviations from the diagram are allowed but every one is listed in §12 so the diagram stays single-source.

Names / ports (so iter4 keeps running beside it): container `iter5`, API + webui **:8400**,
Arango console **127.0.0.1:8630**, database `iter5`, dev/test Arango = the existing dev container on
**:8529** with databases `iter5_test_*` / `iter5_e2e_*`, env overrides `ITER5_TEST_ARANGO_URL`,
`ITER5_TEST_ARANGO_PASSWORD`. Browser storage keys `iter5.*`.

---

## 1. Components (who owns what)

| crate | role in iter5 |
|---|---|
| `iter_core` | shared types + **the node-file library** (`nodefile`): parse / render / conform / name / resolve-children. Used identically by iter_data (node side) and iter_engine (file side) so both sides agree byte-for-byte. Also settings-graph types, test-result type. |
| `iter_data` | the server: work items, locks, **server-side get_next**, the **project graph store (nodes = files)**, the **settings graph**, auth/authz, MCP, GraphRAG, webui. Never touches a repo. |
| `iter_engine` | **one per machine**, serves every project it is connected to. Services: agent (get_next → load_context → dispatch → get_usage → workitem_report → test), filescan → conform → sync, schedule, test, build (designer push). |
| `iter_local` | checkout-side helpers used by the engine: file walking (git-ignore aware), test runner, validate. Its iter4 marker scan/graph snapshot is replaced by `iter_core::nodefile`. |
| `iter_rag` | unchanged. |

iter3 DynamoDB migration code (`migrate_ddb.rs`, `ddb.rs`, aws deps) is **removed**.

---

## 2. Node files (`*.iter.md`) — format v5

### 2.1 Filename and type
`<optional_name.><type>.iter.md` — the type is the segment after the last dot before `.iter.md`.
Types (all lowercase):

| type | graph nodetype | notes |
|---|---|---|
| `project` | project | one per project; replaces `main.iter.md` |
| `code` | code | `level: context\|container\|component\|connection` (required) |
| `test` | test | metadata for test scripts (was `tests`/`testgroup`) |
| `bizreq` | bizreq | **many requirements per file**, one `## ` section each (§2.8); one file per attachment point |
| `techreq` | techreq | same as bizreq (§2.8) |
| `philosophy` | philosophy | highest-level guide; body is free prose |
| `usecase` | usecase | keeps `flowmap` |
| `actor` | actor | who drives use cases / where people touch the app |
| `agentmem` | — | engine-written agent memory; **never synced** to the graph (legacy `agentmemory` treated the same) |

Any other `*.iter.md` is a plain context doc: never a node. Non-`.iter.md` files are **never** nodes
(even when referenced by `children.reqs`).

### 2.2 Common frontmatter (required on every node file; empty OK, missing not)
```yaml
---
id: 3f0c...uuid-v4
name: "A short, readable name"
desc: "~100 words: enough for an agent to decide whether to read the whole file."
creator: "stephen"            # user name, or agent.<name> e.g. agent.code, or engine.<name>
teststate: inherit            # inherit | include | omit | block   (iter4 semantics)
children:
  codedirs:  ["{thisfiledir}/**"]   # code dirs read/modified/locked; recursive
  codenodes: []                     # child code.iter.md files (cascade testing / ownership)
  tests:     []                     # test.iter.md: paths to test scripts; others: paths to test.iter.md files
  reqs:      []                     # bizreq/techreq/philosophy files, dirs, or non-iter docs
timestamps: {create: "2026-10-02 14:56:11Z", last_modified: "2026-10-02 14:56:11Z", last_tested: ""}
---
<markdown body>
```
Timestamp format: `YYYY-MM-DD HH:MM:SSZ` (UTC). Key order on render: id, name, desc, creator, teststate,
type-specific keys (alphabetical), children (codedirs, codenodes, tests, reqs, then any extra), timestamps.

Children entries are paths or globs. Placeholders: `{topdir}`, `{thisfiledir}`, `{thisfilestem}`,
`{thisfilename}`, `{projectname}`. A relative path with no placeholder is relative to `{thisfiledir}`.
Only `*.iter.md` matches become graph edges; other matches are kept as plain references
(codedirs, test scripts, non-iter req docs).

### 2.3 Type-specific keys
| type | keys |
|---|---|
| project | `scandirs: ["{topdir}/"]` (where marker files are looked for), `file_naming: sequence\|uuid12` (default sequence), `gitrepo: ""` (optional remote). `children.codenodes` = the root context nodes; `children.reqs` = **global** requirements (always given to every agent). |
| code | `level` (required), `owner: bespoke\|oss\|3rdparty` (optional). **level=connection** also has `connects: {from: [paths], to: [paths]}` — the code nodes that *supply* this connection type and the ones it *connects to* (directional: from → connection → to). |
| test | `children.tests` = the scripts (`["{thisfiledir}/tests/{thisfilestem}*.sh"]`). `last_result:` the last standardized result JSON (§5). |
| usecase | `children.codenodes` = the parts it uses (required, may be empty), `actors: [paths to actor files]`, `flowmap: {summary, sequence, process_flow, data_flow}` (iter4 shape). |
| actor | `drives: [usecase paths]`, `touches: [code node paths]` (where people interact). |
| bizreq / techreq | file-level `status: draft\|agreed\|done` (default draft; kept for compatibility — each requirement's own status is on its marker line, §2.8). Body = optional preamble + one `## ` section per requirement (§2.8). |
| philosophy | none. |

### 2.4 Graph edges derived from a file (from = this node)
| kind | from | to | source |
|---|---|---|---|
| `codenodes` | any | code | `children.codenodes` |
| `tests` | any but test | test | `children.tests` |
| `reqs` | any | bizreq/techreq/philosophy | `children.reqs` |
| `supplies` | code (supplier) | connection | connection's `connects.from` (edge stored *from supplier to connection*) |
| `connects` | connection | code | `connects.to` |
| `drives` | actor | usecase | `drives` (+ usecase `actors` → also `drives` actor→usecase) |
| `touches` | actor | code | `touches` |
| `uses` | usecase | code | usecase `children.codenodes` (kind `uses`, not `codenodes`) |

Edge identity = `(from_id, kind, to_id)`. Edges are recomputed whenever a node changes: resolve each
glob against the **set of node paths** of the project (server and engine use the same function, so
the server can derive edges without the filesystem).

### 2.5 Folder rules (designer: node → file path)
Used when a node is created in the graph (no file yet). `slug(name)` = lowercase, `[a-z0-9_-]`,
runs of other chars → `_`, trimmed, max 60, empty → the type.

| node | path |
|---|---|
| project | `{topdir}/global/<slug>.project.iter.md` |
| usecase | `{topdir}/global/usecases/<slug>.usecase.iter.md` |
| actor | `{topdir}/global/usecases/<slug>.actor.iter.md` |
| bizreq/techreq attached to the project (global) | `{topdir}/global/requirements/<projectslug>.<type>.iter.md` — ONE per type (§2.8) |
| bizreq/techreq attached to a code node (any level, incl. connection) | `<nodedir>/reqs/<nodeslug>.<type>.iter.md` — ONE per type (§2.8) |
| bizreq/techreq with no parent | `{topdir}/global/requirements/<slug>.<type>.iter.md` |
| philosophy attached to the project / no parent | `{topdir}/global/requirements/<slug>.philosophy.iter.md` |
| philosophy attached to a code node | `<nodedir>/reqs/<slug>.philosophy.iter.md` |
| code, level context, no parent | `{topdir}/src/<slug>/<slug>.code.iter.md` |
| code with parent code node P | `<dir of P>/<slug>/<slug>.code.iter.md` |
| code, level connection | `{topdir}/global/connections/<slug>.code.iter.md` |
| test attached to node N | `<dir of N>/<slug>.test.iter.md`, scripts default `{thisfiledir}/tests/{thisfilestem}*.sh` |

`<nodeslug>` / `<projectslug>` = the parent file's stem (`api.code.iter.md` → `api`), else `slug(parent name)`. A bizreq/techreq
with a code/project parent never collides: when the parent's `children.reqs` already names a file of that type (any
path), that file is returned, else the path above — existing or not (the requirement goes into that file).

On a path collision: `file_naming: sequence` → `<slug>01`, `<slug>02`, … ; `uuid12` → `<slug>_<last 12 of id>`.
Renaming a node never moves its file (the id is the key); moving is an explicit `move` op.

### 2.6 Conformance (file side)
`iter_core::nodefile::conform(path, text, now, creator) -> Conformed { text, changed, findings }`:
- missing frontmatter → add one; missing `id` → new uuid v4; malformed id → new id (finding)
- missing common keys → add empty (`desc: ""`, `creator: ""`, `teststate: inherit`, children with the 4 keys, timestamps with `create`=`last_modified`=now)
- legacy keys renamed: `description→desc`, `bizreqs/techreqs/reqpaths→reqs`, `testpaths/testgroups→tests`, `projectname→name`, `projectdescription→desc`; `simple_description`/`long_description` folded into the body (Summary / Long description sections); `inputs/outputs/interface*` dropped (finding)
- code without valid level → finding (not auto-set) — except missing → `component` + finding
- `timestamps.last_modified` bumped when the content (minus timestamps) changed
- output re-rendered in canonical key order; body untouched (except legacy folding)
- idempotent: conform(conform(x)) == conform(x)

The node side (iter_data) always stores conformed data; every node write goes through the same
`render` so the file the engine writes is exactly what `conform` would produce.

### 2.7 Library API (`iter_core::nodefile`) — the contract
```rust
pub enum NodeType { Project, Code, Test, Bizreq, Techreq, Philosophy, Usecase, Actor, Agentmem }
pub fn type_of(filename: &str) -> Option<NodeType>;          // None = not a marker / plain doc
pub fn is_synced(t: NodeType) -> bool;                       // false for Agentmem
pub struct NodeDoc {                                         // serde Serialize/Deserialize
  pub id: String, pub nodetype: NodeType, pub name: String, pub desc: String, pub creator: String,
  pub teststate: String, pub level: Option<String>,
  pub children: Children,          // codedirs, codenodes, tests, reqs, extra: BTreeMap<String, Vec<String>>
  pub timestamps: Timestamps,      // create, last_modified, last_tested, extra
  pub front: serde_json::Map<String, Value>,   // every other type-specific key (connects, flowmap, actors, drives, touches, status, scandirs, file_naming, gitrepo, owner, last_result…)
  pub body: String,
  pub path: String,                // "{topdir}/rel/path.x.iter.md"
}
pub fn parse(path: &str, text: &str) -> Result<NodeDoc, NodeErr>;      // tolerant of legacy keys
pub fn render(doc: &NodeDoc) -> String;                                // canonical text
pub fn conform(path: &str, text: &str, now: &str, creator: &str) -> Conformed;
pub fn content_hash(text: &str) -> String;                             // sha256 hex of the text
pub fn semantic_hash(doc: &NodeDoc) -> String;                         // ignores timestamps
pub fn slug(name: &str) -> String;
pub fn plan_path(doc: &NodeDoc, parent: Option<&NodeDoc>, attach: Attach, existing_paths: &HashSet<String>, naming: Naming) -> String;
pub fn resolve(entry: &str, this_path: &str, node_paths: &[String]) -> Vec<String>;  // glob/path → node paths
pub fn edges_of(doc: &NodeDoc, node_paths_by_id: &[(String,String)]) -> Vec<Edge>;   // Edge{from,kind,to}
pub fn add_child(doc: &mut NodeDoc, kind: EdgeKind, target_path: &str);   // writes the right key (children.x / connects.from|to / drives / touches / actors)
pub fn remove_child(doc: &mut NodeDoc, kind: EdgeKind, target_path: &str) -> Result<(), NodeErr>; // refuses when only a glob matches
pub fn now_ts() -> String;
```
`iter_core` gains deps `serde_yaml`, `glob`/`globset`, `sha2`.

---

### 2.8 Requirement files hold many requirements; `req` nodes (2026-10-02, Stephen)
**Files.** Requirements are grouped per attachment point: every code node (any level, incl. connection) has at most
ONE bizreq file and ONE techreq file, in its `reqs/` folder, named after the node:
`<nodedir>/reqs/<nodeslug>.bizreq.iter.md`, `<nodedir>/reqs/<nodeslug>.techreq.iter.md`. The project has one global pair
in `global/requirements/<projectslug>.{bizreq,techreq}.iter.md` (philosophy files stay separate). A folder with
requirements but no code node (e.g. a deploy folder) gets the same pair named after the folder. The code node's
`children.reqs` names its two files explicitly (`["{thisfiledir}/reqs/<slug>.bizreq.iter.md", "{thisfiledir}/reqs/<slug>.techreq.iter.md"]`);
the project node names the global pair. (Agent context still also picks them up by the folder convention, §8.)

**Body format.** Frontmatter as for every node (`status` is per requirement now, not per file). Body:
```
<optional preamble — free markdown, before the first "## ">

## PDY-TECH-034 — JWT identifies; only Ed25519 authorizes money
<!-- req: id=6f2c4c9e-…-uuid status=agreed -->
Every caller presents a JSON Web Token … (markdown; use ### or lower for sub-headings)

## Second requirement title
<!-- req: id=… status=draft -->
…
```
- One `## ` heading = one requirement. Heading = `KEY — title` when the part before the first " — " is a key
  (`[A-Za-z0-9_.\-]+`), else just `title` (key empty).
- The marker line carries `id` (uuid v4, permanent: survives edits and moves between files) and `status`
  (`draft|agreed|done`, default `draft`). Conform adds a missing marker/id, de-duplicates ids within a file, and
  normalises the marker; text is kept verbatim. Order of sections is meaningful and preserved.

**Library** (`iter_core::nodefile::reqs`): `ReqItem {id, key, title, status, text}`;
`parse_reqs(body) -> (preamble, Vec<ReqItem>)`; `render_reqs(preamble, &[ReqItem]) -> body`;
`conform_reqs(body) -> (body, findings)` (called from `conform` for bizreq/techreq); `req_node_name(item)`.
Also: `ReqItem.extra` (unknown `k=v` marker pairs, kept and re-rendered), `ReqItem::new(key, title, text, status)` (fresh id),
`item.heading()`, `item.marker()`, `first_sentence(text)`, `is_req_key(s)`, `req_edges(file_doc) -> Vec<(file_id, req_id)>`.
A body without any `## ` section (outside code fences) is all preamble and is never changed. Canonical render: preamble
(trimmed) + blank line, then per section `## heading` / marker / text (leading blank lines + trailing whitespace trimmed),
a blank line between sections, LF endings. An empty title with a key renders `## KEY —`. Conform finding codes:
`req-marker-missing`, `req-id-missing`, `req-id-malformed`, `req-id-duplicate`, `req-status-missing`,
`req-status-invalid` (kept, not replaced), `req-marker-normalised`, `req-marker-junk`. A marker anywhere in the section
(outside fences) is found and moved to the line under the heading.

**Graph.** iter_data derives, for every bizreq/techreq file node, one node per section:
`{nodetype: "req", id: <req uuid>, name: "KEY — title" | "title", desc: <first sentence of text>, key, title, status,
text, order, file: <file node id>, file_type: bizreq|techreq, path: "<file path>#<req uuid>", file_state: <file's>}`
and an edge `{from: <file node id>, kind: "contains", to: <req id>}`. Req nodes are not files; they are rebuilt
whenever their file node changes. A req id appearing in two files: the first (by path) keeps it, the other gets a
new id via conform on the next write.

**Routes** (under `/api/projects/{p}`, all write through the owning file: render → node_version+1 → pending_write):
| method | path | body |
|---|---|---|
| POST | `graph/reqs` | `{file?: <file node id>, node?: <code node id>, type?: bizreq\|techreq, key?, title, text, status?, after?: <req id>}` — with `node`+`type` instead of `file`, the node's file is created (path above) and added to its `children.reqs` if missing → `{req, file}` |
| PATCH | `graph/reqs/{id}` | `{key?, title?, text?, status?, expect_version?}` (expect_version = the file node's) |
| DELETE | `graph/reqs/{id}` | `{reason}` |
| POST | `graph/reqs/{id}/move` | `{to_file: <file node id>, after?: <req id>}` — removes the section from its file, inserts it in the target (same id); both files pending_write |
| POST | `graph/edges/move` | with `kind: "contains"` and `new_from` = another req file → same as move |
MCP: `req_create`, `req_update`, `req_move`, `req_delete`.

**Web UI.** `req` is a node type in the filter chips, hidden by default. A bizreq/techreq node's detail pane shows its
requirements as a table (key, title, status, text; inline edit, add, delete, move-to…). Unhidden, req nodes hang off
their file by `contains` edges; dragging that edge's file end onto another req file moves the requirement.

## 3. Project graph ⇄ repo sync (nodes = files)

### 3.1 Stored node (Arango collection `node`, project-scoped, key `doc_key(project,id)`)
`{project, id, nodetype, level, name, desc, creator, teststate, children, timestamps, front, body, path,
 node_version (int, +1 on every server-side change), file_version (node_version last written to / read from the file),
 file_hash (content hash of the file as last seen), file_state: synced|pending_write|pending_delete|designed,
 deleted (bool), test (last test result), updated_by}`
Edges collection `link`: `{project, from, kind, to}` — fully derived, rebuilt per changed node.
`file_state=designed` = node exists only in the designer (project has no repo yet).

### 3.2 File → node (engine filescan → conform → sync)
Engine service per served project, every tick (≤ few seconds):
1. **filescan**: walk `scandirs` (git-ignore aware, skip `.git`, `target`, `node_modules`) for `*.iter.md`;
   compare `(mtime, size)` with the last scan; re-hash changed ones. Full rescan every 5 min.
2. **conform**: run `nodefile::conform`; if it changed the text, write the file back (the engine commits
   conformance rewrites with the next sync commit, message `iter: conform <n> node files`).
3. **sync**: `POST /api/projects/{p}/files/sync {engine, full: bool, files: [{path, text, hash, base_version}], deleted: [path]}`
   - `base_version` = the `file_version` the engine last acked for that id (0 if unknown).
   - server: parse; upsert by id. If the node has `node_version > base_version` (server-side edit not
     yet written) **and** the file changed → conflict: newer `timestamps.last_modified` wins (tie: server);
     the loser is kept as a `node_conflict` row and a `doc` detail; reply tells the engine to rewrite the file if server won.
     A test result (`front.last_result` + `timestamps.last_tested`) is the server's alone: a newer one on the node is
     carried onto whichever content wins (and the file rewritten), and a pending edit that is only test results is no conflict.
   - id collision (two files, one id) → second file gets a new id via `rewrite`.
   - reply `{applied: [{id, path, node_version}], rewrite: [{path, text, node_version}], removed: [id], conflicts: [...]}`.
   - `full: true` → nodes of this project whose path wasn't sent are marked deleted.
4. Edges recomputed server-side.

### 3.3 Node → file (graph edits, designer)
- Webui/MCP/API edits nodes directly (§6.2). Each edit: conform + render on the server, `node_version += 1`,
  `file_state = pending_write` (or `designed` if the project has no active engine/repo), edges recomputed.
  The edit is visible in the graph **immediately**.
- Heartbeat reply lists `files_waiting: [project]`. Engine: `GET /api/projects/{p}/files/pending` →
  `[{id, op: write|delete|move, path, old_path?, text, node_version}]`; writes/removes files (refusing
  paths outside topdir), `git add` + commit (`iter: graph edit — <summary>`; only the touched files),
  push if a remote exists, then `POST …/files/ack {engine, acks: [{id, node_version, path, hash, commit}]}`
  → `file_state=synced`, `file_version=node_version`.
- Lock-aware: if a pending path is inside a live work-item lock, the engine waits (iter4 behaviour).

### 3.4 Designer → build
- A project can be created with no engine (`POST /api/projects` via settings graph or the wizard).
  The server creates its project node (path `{topdir}/global/<slug>.project.iter.md`, `file_state=designed`)
  and default global reqs (one philosophy, one bizreq, one techreq, empty bodies).
- Everything can be designed in the project graph (§9).
- **Build**: `POST /api/projects/{p}/build {engine, topdir, queue_plan: bool, plan_note?}`
  → creates/activates the `serves` edge (engine→project, settings.topdir), sets `project.build = {state: requested, engine, at}`,
  flips all `designed` nodes to `pending_write`. Engine on next heartbeat (`build_waiting: [p]`):
  `mkdir -p topdir`, `git init` if not a repo (+ `git remote add origin <gitrepo>` if set), writes `.gitignore`
  (`.iter/`), writes all pending files, initial commit `iter: build from design`, acks, `POST …/build/done {engine, commit}`.
  If `queue_plan`, the server queues a `plan` work item at priority 5 on the project node: "Build the project from its design".
- Re-pushing is just more pending writes.

### 3.5 Tests
`*.test.iter.md` scripts (`children.tests`) print, as the **last line of stdout**, the standard result:
```json
{"name":"<test node name>","id":"<test node id>","overall_success":true,
 "normal":{"total":50,"pass":50,"err":0},"longtail":{"total":0,"pass":0,"err":0},"failure":{"total":0,"pass":0,"err":0},
 "details":[{"name":"t1","bucket":"normal","pass":true,"msg":""}]}
```
`details` optional. Exit code 0 = pass, 1 = fail, other = could-not-run. Legacy `ITER_RESULT pass= fail= total=`
last line is still accepted (mapped to `normal`). Results: `POST /api/projects/{p}/graph/nodes/{id}/testresult {result}`
→ stored on the test node (`test`, `front.last_result`, `timestamps.last_tested` → pending_write) and the test log
(`test_log` rows, diagram's `iter_data::test_logs`), `GET …/testlogs?node=&limit=`. A failed run files a work item
(iter4 sweep behaviour, `new_workitem` on fail in the diagram).

---

## 4. One engine per machine; every call names the project

### 4.1 Startup
`iter_engine --data-url <url> --env-file <path> [--name <engine name>]`. Name defaults to the hostname (short).
`.env` holds `ITER_ENGINE_TOKEN` (iter_data credential) + LLM tokens (`token_envar`s) + other secrets.
Nothing else is read from disk. `--config` is removed. The engine registers itself (`PUT /api/engines/{name}`) if missing.

### 4.2 Assignments (everything else comes from the server)
`GET /api/engines/{name}/assignments` →
```json
{"engine":"mbp","projects":[{"project":"pdy","topdir":"~/dev/pdy","read_only":false,"state":"Running",
  "accounts":[{"name":"main","provider":"claude","token_envar":"CLAUDE_TOKEN_MAIN","order":1,"switch":80,"stop":95,"model":""}],
  "edge_tag":"pdy_default"}],
 "accounts":[{"name":"main","provider":"claude","token_envar":"CLAUDE_TOKEN_MAIN"}]}
```
derived from active settings-graph edges (§7). A project with no active `serves` edge to this engine is not run.

### 4.3 Server-side get_next
`POST /api/projects/{p}/next {engine, lease_ttl_sec, agents_allowed?: [..], max: 1}` →
`{item: WorkItem|null, reason: "none-queued"|"blocked"|"locked"|"project-stopped"|"not-served"}`.
Server: refuses unless the engine serves p; picks among queued items by (priority, receive time) honouring
`dependency_status`, cluster-restart tags, wait/blockedby, and lock availability; claims it with a versioned
write (state in-progress, engine, attempt+1, lease, start ts), acquires its locks (lockdirs, kind lock) under
the same lease, rolls back the claim if the locks can't be taken. Returns the claimed item. Engine-side
caps (max agents, budget, account choice) stay in the engine and are checked **before** calling next.

### 4.4 Authz
- Engine tokens (role `engine`): the engine record's `user` = token `sub`. Project-scoped routes accept an
  engine token only if one of that user's engines has an active `serves` edge to the project.
  Engines may only heartbeat/PUT their own engine records.
- Users: admin sees all. Non-admin users see/act on projects where they have an active `member` edge
  (role on the edge: `user|viewer`). The project creator gets a member edge automatically.
- MCP calls use the same rules.

---

## 5. Multi-provider dispatch (engine `provider` module)
```rust
pub struct AgentContext { pub prompt: String, pub cwd: PathBuf, pub env: Vec<(String,String)>, pub mcp_config: Option<PathBuf>,
                          pub resume: Option<String>, pub allowed_tools: Option<String>, pub extra_args: Vec<String> }
pub struct DispatchSettings { pub account: String, pub token: Option<String>, pub timeout: Duration, pub max_turns: Option<u32>, pub stop: StopCheck }
pub struct DispatchOut { pub text, subtype, num_turns, cost_usd, input/output/cache tokens, session_id, raw: String /* provider raw output for get_usage */ }
pub fn dispatch_agent(provider: &str, model: &str, ctx: &AgentContext, s: &DispatchSettings) -> Result<DispatchOut>;
  // "claude" → dispatch_agent_claude, "mock" → dispatch_agent_mock, else Err(unknown provider)
pub fn get_usage(provider: &str, account: &str, token: Option<&str>, last: Option<&DispatchOut>) -> Option<UsageSnapshot>;
  // claude: parse rate_limit_event from last.raw, else probe; mock: deterministic from ITER_MOCK_USAGE or 0%
```
**Every** model call (work turns, close-gate verifier, explain, dedup judge, RAG summaries/OCR, critic,
nudge) goes through `dispatch_agent`. Provider comes from the account (`account.provider`, default claude).

**Mock provider** (deterministic; drives all e2e tests): the prompt is scanned for directives in the work
item's description, one per line:
```
mock: write <relpath> <<<single-line content>>>     write a file (relative to cwd)
mock: append <relpath> <<<text>>>
mock: run <shell>                                  run a shell command in cwd
mock: say <text>                                   the response text
mock: fail <msg>                                   return subtype error_during_execution
mock: ask <question>                               call `iter ask` (question widget) via the iter shim
mock: sleep <ms>
```
Default response "mock: done". Verifier prompts (close gate) answer `VERDICT: complete` unless the item
contains `mock: gate incomplete`. Cost 0, tokens counted as prompt length/4, session id `mock-<uuid>`.
Usage: `ITER_MOCK_USAGE=<five_hour_pct>,<seven_day_pct>` env (for gating tests).

Live smoke: `e2e.sh --live` queues one tiny item to a real `claude` account (skipped unless set).

---

## 6. iter_data API changes (summary; iter4 routes not mentioned stay)

### 6.1 Removed
`/graph` PUT snapshot sync, `/graph/edits` op list, `/datasync*`, interface everything, iter3 migration.

### 6.2 Project graph (under `/api/projects/{p}`)
| method | path | body / result |
|---|---|---|
| GET | `graph` | `{nodes:[Node], edges:[Edge], stats}` (deleted nodes excluded) |
| GET | `graph/view` | webui shape: nodes with `level` (code level or nodetype), `parent` (owner chain), `usecases`, file_state; edges; usecases with flowmaps; summary |
| GET | `graph/nodes/{id}` / `graph/nodes/{id}/neighbors?depth&direction` / `graph/lookup?path=|name=&nodetype=` / `graph/stats` | as iter4 |
| POST | `graph/nodes` | `{nodetype, level?, name, desc?, body?, attach_to?: id, attach_kind?: codenodes|tests|reqs|uses|drives|touches|supplies|connects, front?, children?}` → creates (path via §2.5), adds the attaching edge on the parent node, returns `{node, touched:[ids]}` |
| PATCH | `graph/nodes/{id}` | `{expect_version?, name?, desc?, body?, teststate?, level?, front?, children?}` → node |
| DELETE | `graph/nodes/{id}` | `{reason}` → pending_delete (also removes it from every parent's children lists) |
| POST | `graph/nodes/{id}/move` | `{path}` → move op |
| POST | `graph/edges` | `{from, to, kind}` → updates the owning node (see §2.4: for supplies/connects the connection node owns it) |
| DELETE | `graph/edges` | `{from, to, kind, reason}` |
| POST | `graph/edges/move` | `{from, to, kind, new_from?, new_to?}` (drag an endpoint) |
| POST | `graph/nodes/{id}/testresult` | standard result JSON |
| GET | `testlogs?node=&limit=` | test log rows |
| POST | `files/sync`, GET `files/pending`, POST `files/ack` | §3 |
| POST | `build`, POST `build/done`, GET `build` | §3.4 |
| POST | `graph/run_tests` | `{node}` → test work item (iter4 run_tests) |

### 6.3 get_next — §4.3. Engines — `GET /api/engines/{name}/assignments`; heartbeat reply adds `files_waiting`, `build_waiting` (and keeps `rag_waiting`).

### 6.4 Settings graph — §7.

### 6.5 MCP
Remove interface nodetypes. Add tools `graph_node_create`, `graph_node_update`, `graph_edge_add`, `graph_edge_remove`,
`testlogs`, `settings_graph` (read; admin). Every tool requires a project (arg or `X-Iter-Project`) and goes
through the same authz.

---

## 7. Settings graph
Every setting lives on a node or an edge. Stored in Arango: node records stay in their iter4 tables
(`project`, `engine`, `agent`, `agent_tooling`, `webui_user`) plus new tables `account`, `provider`,
`workitem_type`; edges in a new collection `sys_edge`
`{id, type, from, to, tag, settings, active, created, updated}`; node id = `<type>:<id>`.

**Ids and names.** Every node record has a stable `id` and a display `name`. The `id` is the record's storage key,
fixed when the record is created: every edge endpoint, sign-in token (`sub`), work item (`project`, `agent`,
`engine`), lock and URL uses it. The `name` is what people read, and it can be renamed at any time without
touching any of those. A record written before ids existed reads with `id` = its key and `name` = the same until
renamed (iter_data stamps both on every read and write). A new node's id is minted once from the name it is created
with (letters, digits, `.`, `-`, `_` kept, anything else `-`, a `-2`… suffix when taken), or given explicitly;
a record created by `PUT /api/<collection>/{key}` uses the path key as its id. Names are 1–100 characters, no `/`,
unique within their type, and may not equal another record's id of that type, so a name in a URL path
(`/api/projects/<name>/…`, `/api/engines/<name>/…`, `/api/users/<name>/…`, `/api/agents/…`, `/api/tooling/…`,
`/api/settings/nodes/<type>:<name>`), an edge endpoint, MCP's `project` and the login form all resolve to the id.
Work-item states keep their names.

Node types: `iter_data` (singleton `iter_data:self`; settings = server info, read-mostly), `iter_engine`, `project`,
`workitem_type` (one per state: queued, in-progress, question, parked, paused, failed, complete, scheduled),
`agent`, `agent_tools` (incl. `_shared`), `user`, `account`, **`provider`** (claude, mock — new).
Every type has a non-deletable placeholder `<type>:_deactivated`. Moving an edge endpoint onto the
placeholder keeps the edge (and its settings) but makes it inactive.

| edge type | from → to | settings | effect |
|---|---|---|---|
| `serves` | iter_engine → project | `topdir`, `read_only` | engine runs the project (replaces `Engine.projects` + `Project.engines`) |
| `bills` | account → project | `order` (priority, 0 = first, no negatives; ties: soonest 7-day reset first), `switch`, `stop` (percent), `model` default (used when the item, the project's agent override and the agent name none, or name one the provider cannot run) | project may use the account (replaces `Project.accounts`; switch/stop moved here) |
| `holds` | iter_engine → account | `token_envar` override | engine has the credential locally |
| `of` | account → provider | — | provider for dispatch |
| `member` | user → project | `role: user\|viewer` | visibility / write |
| `owns` | user → iter_engine | — | whose token the engine uses |
| `runs` | agent → project | iter4 per-project agent override (model, flags, lockshape, closegate, timeout…) | agent enabled on project; absent edge = agent disabled |
| `handles` | agent → workitem_type | — | default: every agent → `queued` |
| `allows` | project → workitem_type | per-state policy: `failed` holds the failure/retry policy; `scheduled` holds schedules on/off; `question` holds notify settings | created for every new project |
| `uses` | agent_tools → agent | — | tooling the agent can look up; `_shared` → all agents (suffixed at startup) |
| `hosts` | iter_data → project / iter_engine | — | display only |

API:
- `GET /api/settings/graph` → `{nodes:[{id,type,key,name,deactivated,placeholder,settings,summary}], edges:[…]}` (`id` = `<type>:<key>`; `key` the stable id; `name` the display name)
- `POST /api/settings/nodes {type, name, id?, settings}` (the id is minted from the name unless given); `PATCH /api/settings/nodes/{id} {name?, settings?}` (a `name` renames: the id never changes); `DELETE /api/settings/nodes/{id}` (refused for placeholders and `iter_data:self`; edges move to the placeholder)
- `POST /api/settings/edges {type?, from, to, tag?, settings?}` (type inferred from endpoint types); `PATCH /api/settings/edges/{id} {from?, to?, tag?, settings?, active?}`; `DELETE …/{id}`; `POST …/{id}/copy {from?, to?}` (copy/paste)
- Edge type validation: endpoints must match the table (placeholder of the right type counts).
- Admin only for writes; users read the parts touching their projects.
- On startup the server seeds placeholders, `iter_data:self`, providers, workitem_types, and migrates iter4-shaped
  records (Engine.projects → serves, Project.accounts → account nodes + bills, Project.agents overrides → runs edges) once.

---

## 8. Agent context (engine `load_context`)
For a work item with `node: <id>` (new optional work item field; else the nearest node owning the first lockdir):
1. the whole node file (path given; agent reads it),
2. `[id, name, desc, path]` of every child node (codenodes, tests, reqs, uses…) — listed, agent reads at discretion,
3. requirements — by **folder convention**, no `children.reqs` entry needed. A directory's requirements are the
   req nodes (bizreq / techreq / philosophy) directly in it and anywhere under its `reqs/` or `requirements/` folder:
   - **global, always:** the project node's `children.reqs`, its own folder (`global/requirements/`), and the checkout
     root's `reqs/` / `requirements/`;
   - **local, always (nearest first):** this node's folder, what its file references in `children.reqs`, then every
     ancestor directory's folder up to the root, and every code ancestor's references;
   - **descendants, optional:** every requirement below this node's directory and every descendant code node's
     references — listed (name, desc, path) under "read only the ones your change affects".
   Example: work on `data/postgres18/` (container) gets global + `data/postgres18/reqs/*` + `data/reqs/*`, and lists
   `data/postgres18/encryption_extension/reqs/*`; nothing from sibling branches. (Test:
   `prompt::tests::requirement_folders_global_self_ancestors_then_descendants`.)
4. philosophy guidance sentence: "Use the philosophy file(s) to infer missing requirements and settle conflicts."
5. agentmem file for the node dir (unchanged behaviour).

---

## 9. Web UI
Tabs: Intro | Work queue | Project graph | GraphRAG | **Settings** (the settings graph, same Cytoscape engine;
also reachable from the graph view's picker as "Settings graph").
- **Node-type filter**: chips for every nodetype/level (project, context, container, component, connection, test,
  bizreq, techreq, philosophy, usecase, actor) to hide/show; state in the URL hash. Connection view preset ("Network map":
  code + connection only).
- **Connections**: styled as small diamond/hexagon nodes; `supplies` edges into, `connects` edges out.
- **Edge editing**: select an edge → endpoint handles appear; drag an endpoint onto another node → `edges/move`.
  Copy an edge (⌘C / menu), select a node, paste (⌘V) → new edge with the same kind/settings from/to that node
  (side chosen in a small prompt). Settings graph edges show their `tag` as label.
- **Configure lightbox** for every node and edge (double-click or menu "Configure…"): an editable key/value list
  built from the node/edge fields (frontmatter keys + body for project nodes; settings for settings-graph items).
- **Designer**: "New project" (wizard) creates a designed project; graph shows a "Designed — not built" banner with
  **Build** (pick engine + topdir, optional "queue plan agent"). Pending/designed nodes show a sync badge.
- File-sync indicator: count of pending_write nodes; turns green when all synced.
- Interfaces removed everywhere. Remove-edge asks for a reason (kept).
- Must look polished in dark mode (existing look), phone-width tolerant, no console errors.

---

## 10. Converter `iter migrate5`
`iter migrate5 --from <iter4 checkout> --to <dir> [--dry-run]` (never in place):
copies the tree, then: `main.iter.md` → `global/<slug>.project.iter.md`; `*.tests.iter.md`/`*.testgroup.iter.md` →
`*.test.iter.md` (registry testlist → `children.tests` script paths); bizreq/techreq → the §2.8 one-file-per-attachment-point layout: one `## ` section per requirement (multi-bullet
files: one per top-level bullet, `**ID**` → key, ids minted, status agreed), file placed at
`<nodedir>/reqs/<nodeslug>.<type>.iter.md` (code node beside it or above its `reqs/` folder), else
`global/requirements/<projectslug>.<type>.iter.md` (named by `main.iter.md`'s globalcontextfiles), else
`<folder>/reqs/<folderslug>.<type>.iter.md`; several files landing on one path are merged (first by path keeps its id);
originals removed, references rewritten to the new file, owners' `children.reqs` name their files; description/simple/long →
desc + body; interfaces: files removed, one `connection` node per interface **kind** (request-reply → "API call",
event → "Event", stream → "Stream", dataset → "Shared dataset") with `connects.from/to` = old producers/consumers;
`actors.yaml` → `global/usecases/<slug>.actor.iter.md`; `*.agentmemory.iter.md` → `*.agentmem.iter.md`; then
conform every file. Prints a report. Tested on a scratch copy of pdy-dev.

---

## 11. Tests (definition of done)
- `cargo test` green in every crate (iter_data tests on Arango :8529, db `iter5_test_*`).
- `iter_core::nodefile` property-ish tests: parse/render round trip, conform idempotent, legacy migration, glob resolution, path planning collisions, edge derivation.
- iter_data API tests: node CRUD + edges + pending/ack, files/sync conflict cases, settings graph CRUD/validation/placeholders, get_next ordering/locks/deps/authz, engine token authz, MCP tools.
- `e2e.sh` (rewritten, mock provider, one engine, **two projects**): engine serves both; work item round trips in each;
  file edit → node within seconds; node edit → file + commit; create node in graph → file; delete both ways; conflict;
  designer project → build → repo created + files + plan item; tests standard JSON → test node + log; account switch/stop
  via bills edge; deactivate serves edge → project stops; MCP tool calls; `iter migrate5` on a fixture.
- Playwright (`e2e/playwright/`): login, queue, project graph renders, type filters, configure lightbox node + edge,
  create node via graph, drag edge endpoint, copy/paste edge, settings graph, designer build flow, screenshots
  reviewed for polish (light/dark not required — the app is dark), no console errors.
- Live smoke (`--live`) opt-in.
- At final pass (2026-10-02) all three suites were green: `cargo test --workspace` 375 tests, `./e2e.sh` 184/184 checks,
  `e2e/playwright/run.sh` 31/31 specs.

---

## 12. Diagram review (`requirements.png`)

### 12.1 Deviations (update the diagram to match)
1. **get_next claims and locks on the server** in one call (`::get_next` + `::lock` merged into `POST …/next`); the engine
   still decides *whether* to ask (caps, budget, account) before calling.
2. **`::sync` is two-way and the engine is the only repo writer**: iter_data never pushes files; it marks nodes
   pending and the engine pulls them (`files/pending` → write → `files/ack`). The diagram's server→engine sync arrow is a pull.
3. **`::conform` runs before sync on the engine only**; iter_data conforms its node side with the same library (no separate service).
4. **`::schedule`** feeds the work queue (scheduled templates clone into queued items) as in iter4, not `iter_engine::test` directly;
   the engine-owned test sweep is one such schedule.
5. **`iter_data::test` (webui "run tests")** creates a test work item (the engine runs it) — it doesn't run tests server-side.
6. **`workitem_type` nodes are work-item states** with per-state policy on the `allows` edge; agents connect to *projects*
   (`runs`, holding the per-project override) and to `workitem_type` only as the default `handles queued` edge.
7. **New node type `provider`** (claude, mock) between account and dispatch.
8. **Folder layout**: the code tree is not forced under `src/`; only designer-created nodes default there.
9. **The plan work item is queued at `build/done`, not at `build`**: `POST …/build` only records the request; the server
   files the `plan` item (priority 5, on the project node) when the engine reports `build/done` without an error, so the
   plan never runs before the repo exists (`iter_data/src/filesync.rs::build_done`).
10. **`/datasync` is kept only for GraphRAG file ops** (`store_doc`, `remove_doc`, `gitignore_path`); node edits go through
    the project graph and `files/pending` (§3.3), never datasync rows.
11. **Sync conflicts are listed via `GET /api/projects/{p}/graph/conflicts`** (the `node_conflict` rows; an admin dismisses
    them with `DELETE` on the same path, `?id=` for one node's); no work-item `doc`
    detail row is written (§3.2 step 3 said "and a `doc` detail").
12. **Test results are accepted on test nodes only** (`…/graph/nodes/{id}/testresult` refuses any other nodetype). A red or
    could-not-run result files a `code` work item at priority 50 tagged `check:tests-failing` + `container:<last 12 of the
    test node id>`, so the dedup check files a repeat failure once.
13. **The project node and default reqs are server-seeded only while the project has no `serves` edge** (a designed
    project). When the repo's own project node arrives by file sync it replaces the seeds that were never edited.
14. **The engine asks `next` with a `run_now` hint** (when its cap is full and a run-now item waits) rather than the server
    tracking caps. Three claims still happen on the engine as a versioned PUT, not through `next`: session-chain neighbours
    (`work.rs`), test-sweep runs (outside the cap, `claim_direct`) and close-gate "accept" closes (`close_accepted`).
15. **An engine's assignments list, per project, only the billing accounts the engine also holds** (`holds` edge); an
    account that bills the project but is not held is left out. The iter4 → iter5 settings migration creates the `holds`
    edges (engine that served a project → each of its accounts) alongside `serves` / `bills`.
16. **A hand edit is stamped with the file's mtime**, not the scan time, so conflict resolution compares real edit times.
17. **Mock provider failure messages never start with `mock:`** ("mock run failed …"): a retry's prompt quotes the previous
    response, and a `mock:` line there would be parsed as a directive and run again.

### 12.2 Missing from the diagram (added in iter5)
- **Auth/authz**: login/JWT, engine tokens bound to an owning user, project membership — every request is authorized for its project.
- **Heartbeat** (engine → iter_data) and its reply (`files_waiting`, `build_waiting`, `rag_waiting`, stop requests).
- **Close gate** (verifier between `::workitem_report` and complete) and **session chaining**.
- **Dedup triage**, **explain (ELI5)**, **question widget / answers**, **approval signatures** — all existing iter4 flows.
- **Spend/usage reporting** to iter_data (per project, per account) and the account `switch/stop` decision before get_next.
- **Git commit/push** after work and after file sync (scoped commit).
- **GraphRAG engine work** (embedding, OCR, summaries) — the engine side of `::add_doc`.
- **Build** (designer → repo) path from iter_data to the engine.
- **Settings graph** store (sys edges) beside the project graph.
- **Lock renewal/release and stale-lock sweep**.

---

## 13. Build plan (parallel teams)
Phase 1: `iter_core` nodefile + settings types (team CORE). In parallel: settings graph + authz + get_next (DATA-SYS),
engine multi-project + provider/mock (ENG-RUN), webui scaffolding (UI).
Phase 2 (after CORE): project graph store + file sync API + designer + MCP (DATA-GRAPH); filescan/conform/sync service,
pending writer, build, test JSON runner, migrate5, interface removal in iter_local (ENG-FILES).
Phase 3: e2e.sh + Playwright + fixes until green (TEST), docs/README.
Status (2026-10-02): all three phases built; final pass green — `cargo test --workspace` 375, `./e2e.sh` 184/184,
`e2e/playwright/run.sh` 31/31.
