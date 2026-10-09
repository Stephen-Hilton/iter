# iter5

iter5 is a fork of iter4. A project lives in two places that stay in step: the repo's node files (`*.iter.md`) and the project graph stored in iter_data. One engine per machine serves every project the **Settings graph** assigns to it. Every setting is a node or an edge in that graph. Every model call goes through a provider (`claude`, or the deterministic `mock` that drives the tests). The spec and build contract is [`docs/iter5_spec.md`](docs/iter5_spec.md), and §12 lists where iter5 departs from `requirements.png`.

| dir | what it is |
|---|---|
| `iter_core` | shared types: work items, projects, settings graph types, locks, dedup keys, the test result JSON, and `nodefile` (parse / conform / render / edge derivation / path planning for v5 node files) |
| `iter_data` | the data server: axum API on ArangoDB, auth and authz, get_next (`POST …/next`), the project graph and file sync (`nodes.rs`, `filesync.rs`), the settings graph (`settings.rs`), test results (`testlogs.rs`), MCP (`mcp.rs`) and GraphRAG (`rag/`) |
| `iter_engine` | the engine (one per machine; providers in `provider/`, file sync in `filesync.rs`) and the `iter` verbs (`cli.rs`) |
| `iter_local` | checkout-side tools: node scan, validate, test runner, `migrate5` |
| `iter_rag` | GraphRAG extraction, chunking and embedding |
| `webui/` | the web page (embedded into iter_data) |
| `docker/` | the all-in-one image: ArangoDB CE + iter_data |

iter5 is its own cargo workspace (`iter5/Cargo.toml`). Build and test from `iter5/`. It runs beside iter4: container `iter5`, port **:8400**, database `iter5`.

## Run it

```bash
./deploy.sh docker   # build and start the container: API + webui on :8400, Arango console on 127.0.0.1:8630
./deploy.sh local    # native release iter_data on :8400 against the dev Arango container on :8529 (fast dev loop)
```

Secrets (`ITER_ADMIN_PASSWORD`, `ITER_JWT_SECRET`) are read from `../.env`, and `ITER_ENV_FILE` overrides that path. `ITER_PORT` changes the port. The Arango root password is `ARANGO_ROOT_PASSWORD` (default `iter4dev`, local only). Release binaries land in `bin/<os>-<arch>/` (`bin/linux-x86_64/`, `bin/windows-x86_64/`, …).

### Windows (native)

The engine runs natively on Windows; `deploy.ps1` is the PowerShell twin of `deploy.sh`:

```powershell
.\deploy.ps1 docker   # the container, same as ./deploy.sh docker (Docker Desktop)
.\deploy.ps1 engine   # cargo build --release -> bin\windows-x86_64\iter_engine.exe, then (re)start it
.\deploy.ps1 start | stop | status
```

- Needs Rust (`winget install Rustlang.Rustup`, MSVC toolchain), Git for Windows and the `claude` CLI on PATH.
- The engine uses `~\.iter5\.env`, registers under the hostname, and logs to `~\.iter5\engine.log` (`-EnvFile`, `-Name`, `-DataUrl` override).
- Shell steps (exec items, git postwork, test scripts, the `iter` shim) run under Git for Windows' bash, never WSL's `bash.exe`; `ITER_BASH` overrides. The checkout also gets `.iter\bin\iter.cmd` for cmd and PowerShell.
- Give a `serves` edge a Windows topdir with forward slashes: `C:/Users/me/dev/project`.
- Windows Smart App Control can block cargo's unsigned debug build scripts (`os error 4551`); `cargo test --release` builds past it.
- The repo's `.gitattributes` keeps LF in a Windows checkout so the container and Git Bash scripts run.

## Start an engine (once per machine)

```bash
iter_engine --data-url http://127.0.0.1:8400 --env-file ~/.iter5/.env [--name Engine01]
```

- The env file holds `ITER_ENGINE_TOKEN` (the engine's iter_data credential, minted by an admin with `POST /api/users/<user>/token`), plus each account's token under that account's `token_envar`. The engine re-reads the file while it runs.
- If you leave out `--data-url`, the engine uses `$ITER_DATA_URL`. If you leave out `--name`, it uses the short hostname. The name it registers with becomes the engine's id; renaming the engine in the Settings graph changes only its display name, and either one works as `--name` afterwards.
- Or let the server walk you through it (checks tools, builds the binary if missing, writes the env file with mode 600, starts the engine and waits for its check-in; files live in `~/.iter5/`):
  `curl -fsSL http://127.0.0.1:8400/iter_engine_setup.sh | bash -s -- --data-url http://127.0.0.1:8400 --engine Engine01 --token <token> --start` (`--status` / `--stop` later).
- Nothing else is read from disk. On its first heartbeat the engine registers itself, and it appears in the Settings graph.

To give the engine work, connect it in the **Settings** tab (the settings graph):

| edge | from → to | settings |
|---|---|---|
| `serves` | engine → project | `topdir` (the checkout), `read_only` |
| `holds` | engine → account | optional `token_envar` override (the engine has this credential locally) |
| `bills` | account → project | `order` (the account's priority: P0 is used first, no negatives; a tie goes to the account whose 7-day window resets soonest), `switch` / `stop` (usage %), optional `model` (the default model: used when the agent names none, or one the account's provider cannot run) |

An account bills a project through this engine only when the engine also holds that account. The engine reads all of this from `GET /api/engines/{name}/assignments`. To stop a project on one engine, deactivate its `serves` edge: drag the endpoint onto the `_deactivated` placeholder.

**Switches.** The server, each project, each engine and each account is **Active** or **Stopped**, set in the Work queue's side panel. Stopped starts nothing new; work already running finishes. The server's switch is the master switch (admin only); an engine's switch belongs to its owner or an admin; an account's to an admin (`POST /api/switch {"target": "iter_data:self" | "iter_engine:<id>" | "account:<id>", "active": false}`). A project's switch is its `state` (Running | Draining | Stopped). With the server or an engine Stopped, the engine's assignments report its Running projects as Stopped (`stopped_by`); a Stopped account is never picked, and with every account Stopped the project holds ("accounts switched off") rather than fall back to the machine's own login.

**Agent permissions.** Agents run headless (`claude -p`), so an agent needs a permission mode in its `flags` (the agent record, or the project's per-agent override): `--dangerously-skip-permissions`, `--permission-mode …` or `--allowedTools …`. Without one, Claude Code refuses every write, every command and every read outside the agent's folder. The engine therefore never starts such an agent: its items stay queued, tagged "blocked by: agent '<name>' has no permission flags", and nothing is spent. An agent meant to run read-only says so with `readonly: true` (explain and summary).

Other engine flags: `--accounts` / `--probe` (list the assigned accounts' envars, with live usage for `--probe`), `--adduser`, `--approve`, `--doc`, `--ticks N` (tests).

## The web app

Tabs: **Intro | Work queue | Project graph | GraphRAG | Settings**. The header's project picker drives the queue and the graph.

- **Project graph**: nodes are the node files. Node-type filter chips, a "Network map" preset (code + connections), and a configure lightbox (double-click) for every node and edge. You can create nodes and edges, drag an edge's endpoint, copy and paste an edge, and remove an edge (a reason is required). An edit shows at once and waits to be written. The engine writes the file and commits it. A sync badge counts the edits still waiting.
- **Designer → Build**: the wizard's "New project" creates a project with no engine and no repo. It is seeded with a project node and default philosophy / bizreq / techreq. Design it in the graph, then press **Build** and pick the engine and topdir. The engine creates the repo (`git init`, `.gitignore`), writes every designed file and makes the first commit. Optionally the server then queues a `plan` work item (priority 5) to build it.
- **Settings**: the settings graph (engines, projects, accounts, providers, agents, tooling, users, work item states). A tagged edge always shows its tag; an untagged edge shows its type on hover, when selected, or zoomed in. Every node has a fixed **id** (used by edges, tokens, work items and URLs) and a **name** you can change with **Rename…** (detail pane or right-click) or in Configure; a name in a URL path or at sign-in resolves to the id.
- **Phones**: the header is two rows (tabs + a ⋯ menu for user, timezone, help, my settings and logout; then project, its status, engine dot + running count; the Active | Stopped switches are in the side panel).

## Node files (format v5)

The full format is in spec §2. In short:

- The filename is `<name>.<type>.iter.md`. Types: `project`, `code` (with `level: context|container|component|connection`), `test`, `bizreq`, `techreq` (one file per code node holding many `## KEY — title` requirement sections, §2.8), `philosophy`, `usecase`, `actor`, and `agentmem` (agent memory, never synced). Any other `*.iter.md` is a plain doc. One `global/<slug>.project.iter.md` replaces `main.iter.md`.
- Common frontmatter: `id`, `name`, `desc`, `creator`, `teststate`, `children` (`codedirs`, `codenodes`, `tests`, `reqs`; paths or globs with `{topdir}` / `{thisfiledir}` / …), `timestamps`. Then the markdown body.
- Graph edges are derived from the files: `codenodes`, `tests`, `reqs`, `supplies` / `connects` (connection nodes replace iter4's interfaces), `drives`, `touches`, `uses`.
- `iter_core::nodefile::conform` repairs and canonicalises a file (missing keys, legacy keys, key order). It is idempotent, and the server renders every node through the same code.

## File sync

Each tick, for every served project, the engine runs **filescan → conform → sync**:

1. **filescan** walks `scandirs`, git-ignore aware, and reads only the files whose mtime or size changed.
2. **conform** writes back any file it changes.
3. **sync** posts to `POST /api/projects/{p}/files/sync`.

If a file and its node both changed, the newer `timestamps.last_modified` wins. A hand edit is stamped with the file's mtime. The loser is kept and listed by `GET /api/projects/{p}/graph/conflicts`.

Graph edits go the other way. The heartbeat reply names projects with `files_waiting`. The engine then fetches `files/pending`, writes the files, commits only those files (`iter: graph edit — …`), pushes if there is a remote, and acks. The engine is the only writer to the repo.

## Tests in a project

A test node (`*.test.iter.md`) lists its scripts in `children.tests`. Each script prints the standard result as its **last stdout line**:

```json
{"name":"…","id":"…","overall_success":true,"normal":{"total":3,"pass":3,"err":0},"longtail":{"total":0,"pass":0,"err":0},"failure":{"total":0,"pass":0,"err":0},"details":[{"name":"t1","bucket":"normal","pass":true,"msg":""}]}
```

- Exit code: 0 = pass, 1 = fail, anything else = could not run. A legacy `ITER_RESULT pass= fail= total=` last line is still accepted.
- Results are posted on the test node (`POST …/graph/nodes/{id}/testresult`) and appended to the test log (`GET …/testlogs`).
- A red or could-not-run result files a `code` work item at priority 50 tagged `check:tests-failing` + `container:<last 12 of id>`. The tags dedup it, so a repeat failure does not file a second item.

## Providers

Each account's provider comes from its `of` edge (account → provider); the default is `claude`. The `mock` provider runs no model. On a work turn it runs the directive lines in the work item's request, one per line, top to bottom:

```
mock: write <relpath> <<<single-line content>>>
mock: append <relpath> <<<text>>>
mock: run <shell>          # non-zero exit = the turn fails
mock: say <text>           # the response text (last one wins)
mock: fail <msg>
mock: ask <question>       # via the iter shim
mock: sleep <ms>
```

- With no directives, the response is `mock: done`.
- The close-gate verifier answers complete unless the item carries `mock: gate incomplete`.
- Usage comes from `ITER_MOCK_USAGE[_<ACCOUNT>]=<5h%>,<7d%>`.
- A mock failure message never starts with `mock:`, so a retry prompt that quotes it is not re-run as a directive.

## CLI: `iter_engine cli <verb>`

The engine installs `{topdir}/.iter/bin/iter` as a shim to `iter_engine cli`, so agents and people type `iter <verb>`. `--project` is global.

Queue verbs work inside an engine-run item only (they need `ITER_DATA_URL` / `ITER_ENGINE_TOKEN` / `ITER_PROJECT`):

| verb | what it does |
|---|---|
| `add` | create a work item (`--file` or flags: `--type`/`--agent`, `--title`, `--request`, `--codepath`, `--priority`, `--depends-on`, `--context`, `--tag`, `--usecase`, `--question`, …) |
| `ask` | ask the human a question; the calling item moves to `question` |
| `reject` | park the calling item as invalid |
| `block --cluster-restart` | park the calling item until the cluster is back |
| `wait --on <id>` | make the calling item wait for other items |
| `doc` | append a doc note to an item |
| `critreview` | a synchronous critical review |
| `capability [name]` | read a capability doc |
| `status` | open work, in run order |

Checkout verbs work from a shell in the checkout:

| verb | what it does |
|---|---|
| `init --name <p> [--desc] [--creator] [--force]` | scaffold a v5 project (`global/<slug>.project.iter.md` + `global/requirements/`) |
| `validate [--file F] [--fix]` | conform-check node files |
| `sync [--read-only] [--data-url]` | one file-sync round now |
| `runtests [node] [--test S] [--broken\|--fixed] [--timeout-min N] [--no-record]` | run a test node's scripts |
| `sweep [--node] [--dry-run] [--no-file] [--text-max] [--tests-max] [--coverage-max]` | run the project's test nodes and file fix / test / coverage / node-text items |
| `sweep --install-schedule [--every 4h]` | turn on the engine-owned test sweep |
| `rag sync [--force] [--dry-run]` | re-index changed node files for GraphRAG |
| `markers` | every node file, parsed, as JSON |
| `teststate --omit\|--include\|--block\|--clear <path> \| --list` | set or list the test gate on node files |
| `usecase --file F [--add\|--remove <path>] [--list]` | edit a use case's parts |
| `migrate5 --from <iter4 checkout> --to <dir> [--dry-run] [--json]` | convert iter4 → iter5 in a copy, never in place |

`sync`, `sweep` and `rag sync` connect with `--data-url` / `$ITER_DATA_URL` and `$ITER_ENGINE_TOKEN` (or `$ITER_TOKEN`).

## MCP

iter_data serves a stateless MCP server at `POST /mcp` (Streamable HTTP, JSON). Send `Authorization: Bearer <token>`. `X-Iter-Project` / `X-Iter-Workid` set the default project and calling item. Every agent session the engine starts gets it, and every call goes through the same rules as HTTP. There are 30 tools:

- **Work queue:** `status`, `workitem_list`, `workitem_get`, `workitem_details`, `workitem_create`, `workitem_ask`, `workitem_reject`, `workitem_wait`, `workitem_doc`, `workitem_block`, `capability`, `locks_list`
- **Project graph:** `graph_stats`, `graph_lookup`, `graph_node`, `graph_neighbors`, `graph_owner`, `graph_usecase`, `graph_node_create`, `graph_node_update`, `graph_edge_add`, `graph_edge_remove`, `testlogs`
- **Settings:** `settings_graph`
- **GraphRAG:** `rag_search`, `rag_status`, `rag_docs`, `rag_doc`, `rag_link_document`, `rag_add_document`

## Tests

```bash
./deploy.sh local              # once: the dev ArangoDB on :8529 that the tests use
cargo test --workspace         # every crate; iter_data's tests each get a throwaway iter5_test_* database
./e2e.sh                       # full stack, mock provider, one engine serving two projects (throwaway iter5_e2e_* database)
E2E_ONLY=b,c ./e2e.sh          # setup + only those sections (a0, a, b, c, d, e, f, g, h)
./e2e.sh --live                # also one tiny item on a real claude account (needs ITER_LIVE_CLAUDE_TOKEN)
e2e/playwright/run.sh          # webui suite; screenshots in e2e/playwright/screenshots
```

`ITER5_TEST_ARANGO_URL` / `ITER5_TEST_ARANGO_PASSWORD` point the tests at another Arango.

## Converting iter5's own node files

iter5's own node docs (`*.code.iter.md`, `*.tests.iter.md`, `main.iter.md`, … in this directory) are still in iter4 format. Do not convert them in place. `iter migrate5` only writes a copy (`--from` / `--to`).

When you do convert them, leave out `iter_core/src/nodefile/fixtures/`. Those are test fixtures with duplicate ids, and they must stay iter4-shaped. `migrate5` has no exclude flag (it skips only git-ignored paths and build folders), so remove that directory from the copy you convert from.

Whether and when to convert is pending a decision.

## Licence

iter is an MIT / open-source project. It runs on ArangoDB Community Edition.
