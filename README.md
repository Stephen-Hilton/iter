# iter

A project lives in two places that stay in step: the repo's node files (`*.iter.md`) and the project graph stored in iter_data. One engine per machine serves every project the **Settings graph** assigns to it. Every setting is a node or an edge in that graph. Every model call goes through a provider (`claude`, or the deterministic `mock` that drives the tests). The spec and build contract is [`docs/iter_spec.md`](docs/iter_spec.md), and §12 lists where iter departs from [`docs/requirements.png`](docs/requirements.png).

| dir | what it is |
|---|---|
| `iter_core` | shared types: work items, projects, settings graph types, locks, dedup keys, the test result JSON, `nodefile` (parse / conform / render / edge derivation / path planning for v5 node files) and `rag/` (GraphRAG extraction, chunking and embedding) |
| `iter_data` | the data server: axum API on ArangoDB, auth and authz, get_next (`POST …/next`), the project graph and file sync (`nodes.rs`, `filesync.rs`), the settings graph (`settings.rs`), test results (`testlogs.rs`), MCP (`mcp.rs`) and GraphRAG (`rag/`) |
| `iter_engine` | the engine (one per machine; providers in `provider/`, file sync in `filesync.rs`) the `iter` verbs (`cli.rs`) and the checkout tools in `local/` (node scan, validate, test runner) |
| `iter_data/webui/` | the web page (embedded into iter_data) |
| `installers/` | prebuilt binaries (GitHub Releases) and one install script per program and platform: [`installers/README.md`](installers/README.md) |
| `test/` | every test suite run from outside the crates: the crate test nodes, `test/e2e.sh` and the Playwright suite |
| `docs/` | the spec, the user guide, and `docs/iter_project_docs/` (iter's own project graph: project node, use cases, connections, `map/`) |

iter is a cargo workspace at the repo root (`Cargo.toml`); build and test from the root. Container `iter`, port **:8400**, database `iter_db`.

## Install it

Prebuilt binaries for every platform are published as GitHub Releases; nothing needs Rust. Pick the folder
for the program and the machine in [`installers/`](installers/README.md); each one can be run straight from
GitHub:

```bash
# the server (ArangoDB + iter_data on :8400)
curl -fsSL https://raw.githubusercontent.com/Stephen-Hilton/iter/main/installers/iter_data/docker/install.sh | bash      # any Docker host
curl -fsSL https://raw.githubusercontent.com/Stephen-Hilton/iter/main/installers/iter_data/linux/install.sh | sudo bash  # Debian / Ubuntu / Arch / WSL2
curl -fsSL https://raw.githubusercontent.com/Stephen-Hilton/iter/main/installers/iter_data/linux_aws/install.sh | sudo bash  # EC2 Amazon Linux 2023
curl -fsSL https://raw.githubusercontent.com/Stephen-Hilton/iter/main/installers/iter_data/aws/install.sh | bash -s -- --vpc vpc-… --subnet subnet-… --allowed-cidr 203.0.113.0/24  # a whole AWS stack (CloudFormation)
# one engine per machine
curl -fsSL https://raw.githubusercontent.com/Stephen-Hilton/iter/main/installers/iter_engine/linux/install.sh | bash      # also linux_wsl/, macos/, docker/
```
```powershell
irm https://raw.githubusercontent.com/Stephen-Hilton/iter/main/installers/iter_engine/windows/install.ps1 -OutFile $env:TEMP\iter-install.ps1; powershell -NoProfile -ExecutionPolicy Bypass -File $env:TEMP\iter-install.ps1
```

Add `--version vYYYY.MM.DD` (`-Version` on Windows) for a fixed release; without it the latest is used.
Every script takes `--help` and `--dry-run`. The engine installers ask for the iter_data URL and a name
unique to the machine, make the engine key and wait while you register it in the Engine keys screen, then
start the engine as a service (systemd `iter-engine`, launchd `com.iter.engine`, the Windows logon task
`iter engine`, or the `iter-engine` container). Projects go beside each other: give the engine's `serves`
edge the topdir `~/dev/<project>`, then run the installer's `projects` verb to clone them. Other verbs:
`status`, `update`, `accounts`, `logs`, `uninstall`.

To compile it yourself, clone the repo and run `cargo build --release -p iter_engine -p iter_data`;
`installers/build.sh` builds all seven release binaries and `installers/build.sh --release` publishes them.

## ArangoDB

iter_data keeps everything in ArangoDB, signed in as **its own user** (`iter_user`) with `rw` on **its own
database** (`iter_db`) and nothing else, so one ArangoDB server can also hold other apps' databases. On the
first start, when that user or database is missing (or the user lacks `rw`), iter_data signs in once as
root with `ARANGO_ROOT_PASSWORD`, creates them and grants `rw`. iter's user never gets root's password, and
iter_data creates its own collections and indexes as projects grow.

Every iter_data installer installs ArangoDB beside iter_data unless you point it at one you already run:

```bash
ARANGO_ROOT_PASSWORD=… ./install.sh --arango-url https://arango.example.com:8529 --arango-fingerprint sha256:…
```

| setting | installer option | |
|---|---|---|
| `ARANGO_URL` | `--arango-url URL` | an existing ArangoDB (`local` = install one here) |
| `ARANGO_CERT_FINGERPRINT` | `--arango-fingerprint FP` | pin its self-signed certificate (`sha256:…`, comma-separated for a change-over) |
| `ARANGO_CA_CERT` | `--arango-ca-cert PATH` | or trust the CA (PEM) that signed it; with neither, the public CAs |
| `ARANGO_DB` | `--arango-db NAME` | iter's database [`iter_db`] |
| `ARANGO_USER` | `--arango-user NAME` | iter's user, `rw` on that database only [`iter_user`] |
| `ARANGO_PASSWORD` | environment / `--env-file` | that user's own password (generated and kept when not given) |
| `ARANGO_ROOT_PASSWORD` | environment / `--env-file` | only to create the user and database; not needed once they exist |
| `ARANGO_ALLOW_PLAIN_HTTP` | environment / `--env-file` | `1` allows `http://` to another machine, on a private network only |

ArangoDB on another machine is reached over HTTPS only: plain `http://` goes only to this machine unless
`ARANGO_ALLOW_PLAIN_HTTP=1`. The [linux installer README](installers/iter_data/linux/README.md#arangodb-iters-own-user-and-database-or-an-arangodb-you-already-run)
shows how to serve ArangoDB over HTTPS. Backups (`iter-backup`, every 6 hours) dump iter's database only, as
iter's user, local or remote. `install.sh restore FILE` (linux, linux_aws) loads one back. The CloudFormation
stack takes the same settings as parameters, with the passwords in SSM ([aws/](installers/iter_data/aws/README.md)).

## Signing in to the web app

The first admin (`admin`) is made on an empty database with `ITER_ADMIN_PASSWORD`, or a generated password printed once, and must choose their own password at the first sign-in: until then the API refuses everything else. A password an admin sets for someone (a new user, or a reset) must also be changed at that person's next sign-in. Passwords need 12+ characters; changing one needs the current one and signs out every other session; sign-ins are limited to 5 failures a minute per user and per address. Once the first admin has chosen theirs, iter_data leaves `admin_password_changed` beside its secret file and the installers remove `ITER_ADMIN_PASSWORD` from their env file (and from SSM on AWS).

## Developer loop

```bash
./deploy.sh docker   # build iter_data-linux-<arch> and start the container via installers/iter_data/docker: API + webui on :8400
./deploy.sh local    # native release iter_data on :8400 against the dev Arango container on :8529 (fast dev loop)
```

Secrets (`ITER_ADMIN_PASSWORD`, `ITER_JWT_SECRET`) are read from the repo root `.env`, and `ITER_ENV_FILE` overrides that path. `ITER_PORT` changes the port. The Arango root password is `ARANGO_ROOT_PASSWORD` (default `iterdev`, local only). `deploy.sh local` runs iter_data as `iter_user` on `iter_db`. That user's password is made once and kept in `run/arango_user.pw`, and root creates the user and database on the first start. `deploy.sh` puts its native release binaries in `bin/<os>-<arch>/`.

### Windows

On Windows the server runs in a WSL2 distro, and each engine runs where its project builds best. Unreal-style projects use the native Windows engine (`installers/iter_engine/windows/`). Bash- and Linux-heavy projects use an engine in the distro (`installers/iter_engine/linux_wsl/`), because Git Bash makes every process start expensive.

1. `wsl --install -d Debian`, then in `/etc/wsl.conf`: `[boot] systemd=true`, `[user] default=<you>` (and `[interop] appendWindowsPath=false` to keep Windows tools off the Linux PATH); `wsl --terminate Debian`.
2. In the distro: `installers/iter_data/linux/install.sh` (as root) for the server; put an engine's project checkouts on the distro's disk, e.g. `~/dev/<project>`.
3. On Windows, `deploy.ps1` keeps the distro up and handles a developer build of the native engine:

```powershell
.\deploy.ps1 startup    # start the distro (ArangoDB, iter_data, its engine), wait for :8400, start the Windows engine
.\deploy.ps1 autostart  # run `startup` at every logon (Task Scheduler); also sets WSL memory reclaim
.\deploy.ps1 restart-wsl  # restart the WSL VM (applies ~\.wslconfig / unit changes); refuses while any engine runs work (-Force)
.\deploy.ps1 engine | build | start | stop | status   # developer build of iter_engine.exe from this checkout (needs Rust)
```

- Restart WSL only with `restart-wsl`, never a bare `wsl --shutdown` / `wsl --terminate <distro>`: under a running Docker Desktop that breaks its WSL integration ("WSL integration with distro ... unexpectedly stopped" every ~30 s), and it ends the logon task's keepalive, so the distro is no longer held. `restart-wsl` stops Docker Desktop first, restarts the distro and its keepalive, then starts Docker Desktop again.
- The Windows engine needs Git for Windows and the `claude` CLI on PATH, no Rust. It signs in with `~\.iter\engine.key`, reads account tokens from `~\.iter\.env`, registers under the hostname, and logs to `~\.iter\engine.log`.
- Shell steps (exec items, git postwork, test scripts, the `iter` shim) run under Git for Windows' bash, never WSL's `bash.exe`; `ITER_BASH` overrides. The checkout also gets `.iter\bin\iter.cmd` for cmd and PowerShell.
- Give a `serves` edge a Windows topdir with forward slashes: `C:/Users/me/dev/project`.
- The repo's `.gitattributes` keeps LF in a Windows checkout so the container and Git Bash scripts run.

## Engines

By hand:

```bash
iter_engine key new [--name Engine01]                      # once: makes this machine's key; add its public key in Engine keys
iter_engine key trust --data-url https://iter.example:8400 --fingerprint sha256:…   # optional: pin the server's certificate up front
iter_engine --data-url https://iter.example:8400 [--name Engine01] [--secrets-command '…']
```

- Everything a machine keeps for iter is in `~/.iter` (`$ITER_HOME` moves it): the engine key (`engine.key`), the server certificates it trusts (`server_trust.json`), the env file with each account's token under that account's `token_envar` (`.env`, re-read while the engine runs), the binary, the embedding model and the logs. `--key-file` and `--env-file` point elsewhere.
- **Several engines on one repository.** Each engine has its own clone, so a push can find the remote ahead of it. The engine merges the remote in and pushes again. When the merge conflicts, an agent session resolves it in the checkout: it is told the conflicted files, both sides' commits and the work items behind them. It edits the files, and the engine checks that no conflict marker is left before committing the merge. If the agent cannot combine the two sides, the merge is backed out, so a checkout is never left half-merged, and the work item moves to `question` with what conflicts and why. That engine starts nothing new in that project until you answer; other engines carry on. Your answer is the instruction for the next attempt. The same merge runs before every work item (the git prework).
- **Omarchy laptops.** `installers/iter_engine/linux/install.sh` asks before each change. It turns off suspend and makes closing the lid do nothing. It turns off Omarchy's idle lock and screensaver (in `~/.config/omarchy/shell.json`) and the lock on lid close (in `~/.config/hypr/bindings.lua`; with `o.rebind` on current Omarchy, `hl.unbind` + `o.bind` on older). It also sets up `hypridle` to turn the screen off after 10 minutes. On older Omarchy, its `hypridle.conf` is kept as `hypridle.conf.before-iter`.
- **Engine keys.** An engine proves who it is with a key pair it makes for itself: `iter_engine key new` writes the private key to `~/.iter/engine.key` (owner-only; it never leaves the machine and no command prints it) and prints the public key (`iter_…`; `key show` prints it again). An admin pastes the public key into **Engine keys** (the 🔑 button in the webui header), naming the engine. Nothing secret is copied. The engine signs a one-time login with its key and gets a session token, which it and the agents it starts send with each request; sessions are signed with their own secret, never the one that signs people's tokens. Every request re-checks the key, so **Revoke** stops the engine and its agents at once. **Rotate**: `iter_engine key rotate` makes a new key and has iter_data swap it in, signed by the current key, with no admin step; the old key keeps working for an hour for agents still running. A revoked key can never rotate itself: that engine is set up again by hand. (The screen's *rotate…* does the same with a key made by `key new` on the machine.) `iter_engine key status --data-url URL` shows whether iter_data accepts the keys and which certificate the engine trusts. An engine is a machine, not a user: engines never sign in as people.
- **HTTPS.** iter_data serves HTTPS with a self-signed certificate it makes on first start (`--tls auto`; `--tls-cert/--tls-key` for your own; `--tls off` only on one machine or behind a TLS proxy). Engines pin its fingerprint: at the first signed sign-in, which signs the fingerprint the engine sees so a sign-in relayed through anything else is refused, or up front with `key trust --fingerprint` (shown in Engine keys and by the iter_data installers). Engines refuse plain `http://` to another machine. A new certificate (Engine keys → *prepare a new certificate*) is announced to engines at their sign-ins before you switch to it. Claude sessions reach iter_data's MCP tools through a relay the engine runs on 127.0.0.1, over its pinned connection.
- **Account tokens.** In order: the process environment at startup (systemd `Environment=`, a container's env), the `--secrets-command` (a shell command printing `KEY=value` lines, e.g. from AWS Parameter Store or Secrets Manager, 1Password or the Keychain; run at startup and every `--secrets-every` minutes, values kept in memory only), then `~/.iter/.env`. A missing token is logged by name (never a value), shown in the webui, and that account gets no work. Each Claude session gets only its own account's token; every other account's token is removed from the environment of anything the engine starts.
- If you leave out `--data-url`, the engine uses `$ITER_DATA_URL`. If you leave out `--name`, it uses the short hostname. The name it registers with becomes the engine's id; renaming the engine in the Settings graph changes only its display name, and either one works as `--name` afterwards.
- `/iter_engine_setup.sh`, the older setup script, is retired: it only points at `installers/iter_engine/`.
- Nothing else is read from disk. On its first heartbeat the engine registers itself, and it appears in the Settings graph.

To give the engine work, connect it in the **Settings** tab (the settings graph):

| edge | from → to | settings |
|---|---|---|
| `serves` | engine → project | `topdir` (the checkout), `read_only` |
| `holds` | engine → account | the engine has this credential locally (the token variable is named once, on the account) |
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
- Graph edges are derived from the files: `codenodes`, `tests`, `reqs`, `supplies` / `connects` (connection nodes replace the earlier interfaces), `drives`, `touches`, `uses`.
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

`sync`, `sweep` and `rag sync` connect with `--data-url` / `$ITER_DATA_URL` and `$ITER_ENGINE_TOKEN` (or `$ITER_TOKEN`), else this machine's engine key (`~/.iter/engine.key`).

## MCP

iter_data serves a stateless MCP server at `POST /mcp` (Streamable HTTP, JSON). Send `Authorization: Bearer <token>`. `X-Iter-Project` / `X-Iter-Workid` set the default project and calling item. Every agent session the engine starts gets it, and every call goes through the same rules as HTTP. There are 30 tools:

- **Work queue:** `status`, `workitem_list`, `workitem_get`, `workitem_details`, `workitem_create`, `workitem_ask`, `workitem_reject`, `workitem_wait`, `workitem_doc`, `workitem_block`, `capability`, `locks_list`
- **Project graph:** `graph_stats`, `graph_lookup`, `graph_node`, `graph_neighbors`, `graph_owner`, `graph_usecase`, `graph_node_create`, `graph_node_update`, `graph_edge_add`, `graph_edge_remove`, `testlogs`
- **Settings:** `settings_graph`
- **GraphRAG:** `rag_search`, `rag_status`, `rag_docs`, `rag_doc`, `rag_link_document`, `rag_add_document`

## Tests

```bash
./deploy.sh local              # once: the dev ArangoDB on :8529 that the tests use
cargo test --workspace         # every crate; iter_data's tests each get a throwaway iter_test_* database
test/e2e.sh                       # full stack, mock provider, one engine serving two projects (throwaway iter_e2e_* database)
E2E_ONLY=b,c test/e2e.sh          # setup + only those sections (a0, a, b, c, d, e, f, g, h)
test/e2e.sh --live                # also one tiny item on a real claude account (needs ITER_LIVE_CLAUDE_TOKEN)
test/e2e/playwright/run.sh          # webui suite; screenshots in test/e2e/playwright/screenshots (git-ignored)
```

`ITER_TEST_ARANGO_URL` / `ITER_TEST_ARANGO_PASSWORD` point the tests at another Arango (they need its root password: each test makes its own database, and `test/e2e.sh` also makes its own user).

## Licence

iter is an MIT / open-source project. It runs on ArangoDB Community Edition.
