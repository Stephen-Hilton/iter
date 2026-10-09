# iter4 guide

This guide explains what iter4 is, how to stand it up, how to create and run a project, and how every part works: work items, agents, engines, the architecture map, tests, GraphRAG search and the MCP server. It ships inside the iter_data server and is searchable from the GraphRAG tab of every project, so asking a question in plain words ("how do I create a new project?") finds the right section.

Each section starts with a question. Commands assume you are in the project's checkout unless it says otherwise.

## What is iter4?

### What does iter4 do?

iter4 keeps a team of AI coding agents working on a real software project without letting them trip over each other. A person, or an agent, files a **work item**: "make this change, in this part of the code". An **engine** — a program running next to a copy of the code — picks the most urgent item it can start, locks the folders the item will touch, starts a headless Claude Code session (or a plain shell command) to do the work, has a second model check the result against the request (the **close gate**), commits just those files, and moves on.

### What are the parts of iter4?

- **iter_data** — the central server. It holds every work item, lock, agent definition, schedule, user, cost record and the architecture map in ArangoDB, answers one HTTP API, serves the web page, and serves the MCP server for agents.
- **iter_engine** — the engine. Run it next to a checkout of your project; it claims work, runs agents, commits and pushes. The same binary is also the `iter` command line (`iter_engine cli <verb>`).
- **iter_local** and **iter_core** — libraries compiled into the two programs (checkout tools and the shared rules).
- **The web page** — four tabs: Intro, Work queue, Project graph, GraphRAG.

### What database does iter4 use?

ArangoDB Community Edition 3.12, and only ArangoDB. It stores documents (work items and every other record), counters, the architecture map as a graph (collections `node` and `link`, named graph `iter_map`), and the GraphRAG chunks and vectors. SQLite and DynamoDB serving were removed on 2026-09-29; DynamoDB is only read once, as the source of the iter3 migration.

### How is iter4 different from iter3?

iter4 keeps iter3's work queue and engine, and adds: ArangoDB in one container with the server, a map of the program (the architecture graph) built from `*.iter.md` files, editing the map from the web page, use cases drawn as hierarchies, a test sweep that turns red tests into work, GraphRAG semantic search, and an MCP server for agents.

## Standing up the server

### How do I install and start iter4?

From the `iter4/` directory of the repository:

```bash
./deploy.sh docker   # the all-in-one container: ArangoDB CE + iter_data on http://127.0.0.1:8300
./deploy.sh local    # native iter_data (release build) against a dev ArangoDB container on :8529
```

`docker` is the normal way. It writes `run/docker.env` with the admin password and token secret from your `.env`, downloads the embedding model (`tools/fetch_model.sh`), builds the image and starts it with `docker compose`, then waits for `/health` to answer `"ok":true`. `local` builds `iter_data` and `iter_engine` into `bin/`, starts or reuses the `iter4-arango-dev` container on 127.0.0.1:8529, and runs iter_data natively — the fast loop for development.

### Which ports does iter4 use?

- **8300** — the API and the web page (`ITER_PORT` changes the host port).
- **127.0.0.1:8530** — the ArangoDB web console of the container, localhost only (`ARANGO_HOST_PORT` changes it). Log in as `root` with `ARANGO_ROOT_PASSWORD`; the database is `iter4`.
- **127.0.0.1:8529** — the dev ArangoDB used by `./deploy.sh local`, the unit tests and the end-to-end tests.

### What is the admin password, and how do I log in the first time?

On first start, when there are no users, iter_data creates the user `admin` with role `admin`. The password is `ITER_ADMIN_PASSWORD` from the environment or `.env`; if that is unset, a random 20-character password is generated and printed once in the server log (`docker logs iter4`). Open http://127.0.0.1:8300/ and log in as `admin`.

### Which environment variables does the server read?

- `ITER_ADMIN_PASSWORD` — the first admin's password.
- `ITER_JWT_SECRET` — the secret that signs login and API tokens; if unset, iter_data reads or creates `--secret-file`.
- `ARANGO_ROOT_PASSWORD` — the ArangoDB root password (default `iter4dev`, fine for local use only).
- `ARANGO_URL`, `ARANGO_DB`, `ARANGO_USER`, `ARANGO_PASSWORD` — where iter_data finds ArangoDB (defaults `http://127.0.0.1:8529`, `iter4`, `root`).
- `ITER_EMBED_MODEL` — the embedding model directory (the container has it at `/opt/iter/models/all-MiniLM-L6-v2`).
- `ITER_ENV_FILE` — which `.env` `deploy.sh` reads (default `../.env`).

The `.env` loader reads plain `KEY=VALUE` lines, never runs them through a shell, and never overrides a variable already set in the environment.

### Where is my data stored?

In the container, one host folder mounted at `/var/lib/iter_data` (`ITER_DATA_DIR`, default `~/.iter5/iter_data`): `arango/` (the database), `arango-apps/` (Arango's app folder) and `iter/` (iter_data's secret and .env files). With `./deploy.sh local`: the Docker volume `iter4-arango-dev`, plus `run/iter_data.secret`, `run/iter_data.log` and `run/iter_data.pid`.

### Does the container run the agents?

No. The container runs ArangoDB and iter_data only. Engines — and the Claude Code sessions they start — run on the machines that hold checkouts of your projects.

### Can I run iter4 on a server or in the cloud?

Yes. The same `docker/compose.yml` runs on a small VM or an EC2 instance. Engines can run anywhere that can reach the server's URL; see `UBUNTU.md` in the repository for a Linux engine host.

## Creating a new project

### How do I create a new project?

1. **Make the project's repository** (or pick an existing one) and clone it onto the machine that will run the engine. A git repository with a remote is expected: the engine pulls before and pushes after every run. Without a remote it commits locally; without `.git` it skips git.
2. **Run the wizard.** Open the web page, go to **Intro** and choose the new-project wizard (the last slide of either track). It asks for the project (name, description, git URL, checkout path), the Claude accounts and limits, and the engine name and data server URL. An admin's **Create project** creates the project record (state Stopped) and the engine record, then mints the engine's login token and shows it once.
3. **Scaffold the checkout.** In the checkout, run the `init` command the wizard shows:

   ```bash
   iter_engine cli init --name my-project --data-url http://127.0.0.1:8300 --engine Engine01 --desc "What the project is"
   ```

   It writes `main.iter.md`, `.iter/config.json`, `.iter/.gitignore`, `reqs/<slug>.bizreq.iter.md`, `reqs/<slug>.techreq.iter.md`, and the folders `interfaces/` and `usecases/`. Existing files are kept unless you add `--force`. Commit these files.
4. **Give the engine its secrets.** Create `.env` in the checkout (keep it out of git) with the engine token and one token per Claude account:

   ```bash
   ITER_ENGINE_TOKEN=<the token the wizard showed>
   DEV1_TOKEN=<run 'claude setup-token' while logged in to that account>
   ```

5. **Start the engine** in the checkout: `iter_engine --config .iter/config.json`.
6. **Start the project.** In the Work queue, press **Running** for the project. The engine only starts work while the project is Running.

### Can I create a project without the web page?

Yes, with the API and an admin token. `PUT /api/projects/<name>` with a project record, `PUT /api/engines/<engine>` with an engine record whose `projects` maps the project to `{"dirs":{"topdir":"<checkout path>"}}`, `PUT /api/users/<engine>` with `{"role":"engine"}`, and `POST /api/users/<engine>/token` with `{"ttl_days":365}` for the engine token. The wizard's Review step shows the exact request bodies.

### What does a project record contain?

Name, description, state (Running, Draining or Stopped), git repository, the agent ladder (`maxagents`, for example `{">98%":0,">95%":1,">90%":2,"else":4}`), a daily cost cap (`maxdailycost`), per-agent overrides (`agents`), the failure policy (`failure`: attempts, first retry, backoff), the engines that serve it, the Claude accounts, `commit_extra_paths`, `pinned_tags`, dedup and cluster-restart settings, and `session_chain_max`. Edit it from the gear next to the project in the left panel (admins can save).

### What does `iter init` write in `.iter/config.json`?

```json
{
  "data_url": "http://127.0.0.1:8300",
  "token_envar": "ITER_ENGINE_TOKEN",
  "engine_name": "Engine01",
  "env_file": "./.env"
}
```

`data_url` is where iter_data answers; `token_envar` names the environment variable holding the engine token; `env_file` is where the engine reads that token and the account tokens from.

### What goes in `main.iter.md`?

The project's root node: `id`, `projectname`, `projectdescription`, `globalscandirs` (where node files are found), `globalinterfacedir`, `globalusecasedir`, `globalcontextfiles` (requirement files every agent reads) and `children.codenodes` (the top-level contexts of the map). Optionally `actorsfile` (the actors file, default `actors.yaml` at the top of the checkout). The body describes the project in prose.

### Why is my project not starting any work?

Check, in order: the project is **Running** (not Stopped or Draining); an engine is online for it (the status line says "N engines online"); the engine has an account it can use (not "Suspended, no account token" or "no usage left"); the daily cost cap is not reached; the item is `queued`, its dependencies are complete, and no running item holds a lock on its folders (queued items show a `blocked by: …` tag saying why they wait).

## Engines

### How do I install the engine?

Build it from the repository: `cargo build --release -p iter_engine` gives `target/release/iter_engine` (`./deploy.sh local` also copies it to `bin/`). The engine machine also needs `git` (with push credentials for your repository), `bash`, and the `claude` command line (Claude Code) on the PATH.

### How do I start an engine?

In the project's checkout:

```bash
iter_engine --config .iter/config.json            # run until stopped
iter_engine --config .iter/config.json --ticks 20 # run 20 ticks, finish running work, exit
```

The first log line names the server and its database backend. The server URL comes from `--data-url`, else `ITER_DATA_URL`, else `data_url` in the config. Without an engine token the engine exits with a message saying how to mint one. On a server, run it under systemd with `Restart=always`.

### What does an engine do?

Every tick (5 seconds by default) the engine: renews the leases on its running items' locks, reloads account tokens if `.env` changed, pushes the project's map when `*.iter.md` files changed, picks a Claude account, sends a heartbeat, applies graph edits waiting from the web page, starts GraphRAG summary workers, fires due schedules, and — when the project is Running and the agent cap allows — claims the best queued item, locks its folders, and runs it.

### Can several engines serve one project?

Yes. List them in the project's `engines` and give each engine record the project's checkout path. Claims are versioned writes (only one engine wins an item), locks are central, schedules and graph edits are claimed the same way, and engines avoid an account another live engine is using.

### How many agents run at once?

The project's `maxagents` ladder sets the cap from the current account usage: the most restrictive `>N%` rule that holds, else the `else` value (default 4). Each agent type has its own `max` too (default 4; a project override wins). Shell items do not count against per-type caps. **Run now** on an item ignores the cap but still waits for dependencies and locks.

### How do Claude accounts and tokens work?

The project lists accounts by name and the environment variable that holds each token (`token_envar`), with a `switch` and a `stop` usage percentage. The tokens themselves live in the engine's `.env` (make one with `claude setup-token`). Each session gets its account's token as `CLAUDE_CODE_OAUTH_TOKEN`; `ANTHROPIC_API_KEY` is removed from the session's environment, because it would bill API credits instead. The engine moves to the next account at `switch`, holds at `stop`, never uses an account whose token is missing, and re-reads `.env` without a restart. With no accounts configured, the machine's own Claude login is used.

### What does "Suspended" mean on an engine?

The engine is holding: every account is at its stop percentage ("no usage left"), no account has a token set ("no account token"), or the daily cost cap was reached. It keeps heartbeating and reports when the next account frees up; queued items carry a `blocked by:` tag with the reason.

### What do Running, Draining and Stopped mean?

They are the project's commanded state. **Running**: engines start work. **Stopped**: engines start nothing new (and fire no schedules). Pressing Stopped while items run sets **Draining**: running items finish, then the project flips to Stopped by itself; **force stop** skips the wait.

## Users, roles and tokens

### What user roles are there?

- **admin** — everything, including users, agents, projects and tokens.
- **user** — creates and changes work items, answers questions, edits the map.
- **engine** — what engines log in as; can claim and update work, but cannot create schedules.
- **viewer** — reads everything, changes nothing but their own profile.

### How do I add a user?

As an admin: the gear at the top right → All users → a name, password and role → add user. Or `PUT /api/users/<name>` with `{"role":"user","password":"…"}`.

### How do I get an API token?

Log in with `POST /auth/login` and `{"user":"…","password":"…"}` for a 24-hour token, or have an admin mint a long-lived one with `POST /api/users/<name>/token` and `{"ttl_days":365}`. Send it as `Authorization: Bearer <token>` on every `/api/` call and on `/mcp`.

### How do I revoke a token?

Bump the user's **token version** (`tokenver`) in the user settings. Every token issued before is refused from then on.

## Work items

### What is a work item?

One piece of work with a name, a request (what to do and when it is done), an agent that does it, a priority, the folders it may change (`lockdirs`), optional dependencies (`blockedby`), tags, context files and a state. Its history — the request, runs, questions, answers, verifier verdicts, costs and notes — is kept as detail rows.

### What types of work items can I create?

The `agent` field decides what does the work. Built-in and conventional types:

- **exec** — a shell command (`exec_shell`), run with bash in the checkout. No Claude, no close gate.
- **test** — writes tests with Claude, or, when `exec_shell` is set, runs tests deterministically (for example `iter runtests --group <label>`). The test sweep and GraphRAG change sweep are scheduled `test` items.
- **code** — the default: change code with Claude. The sweep's fix items for red tests are `code` items.
- **usecase** — names the parts of the architecture map a use case needs; queued automatically when a use case is added in the Project graph.
- **ingest** — brings a map node's text up to the node-text standard; filed by the sweep.
- **plan**, **refactor**, **deploy** — conventional Claude agents (they exist when the project has agent records for them).

Any other name works once an agent record with that name exists. Separate from work items, iter also runs: **explain** (the ELI5 button), **dedup** (the duplicate judge), **summary** (GraphRAG summaries) and the close-gate verifier.

### Can I create new work item types?

Yes. A work item type is an **agent record**; create one and any item can name it. As an admin, in the left panel choose **Agents +**, give it a name (letters, digits, `_`, `-`), and fill in its prompt (`promptbody`) and settings. Or `PUT /api/agents/<name>` with, for example:

```json
{"desc": "Writes release notes", "max": 1, "timeoutsec": 1800, "model": "sonnet",
 "promptbody": "# Agent Definition: notes\n\nYou are the notes agent. …"}
```

Agent fields: `desc`, `max` (how many run at once), `timeoutsec`, `model`, `flags` (extra Claude Code flags; the webui's new-agent default is `--dangerously-skip-permissions`), `childstate` (the state of items it creates), `closegate` (how its results are checked), `lockshape` (which folders its items may lock) and `promptbody`. A fresh install has only the `summary` agent; the others come from the iter3 migration or are created this way. An item naming an agent that does not exist fails with "agent '<x>' not defined in iter_data".

### How do I create a work item?

- **Web page:** Work queue → **+ New workitem**. Fields: name, agent, exec shell (for `exec`), priority (blank = automatic), state on save, lockdirs, blocked by, context files, model, tags, use case, request.
- **API:** `POST /api/projects/<project>/workitems`:

  ```json
  {"name": "Add retry to the S3 uploader", "agent": "code",
   "request": "Wrap upload() in src/s3 with 3 retries and exponential backoff. Done when src/s3's tests pass.",
   "lockdirs": ["{topdir}/src/s3"], "tags": [{"text": "storage", "color": "#3b6fb6"}], "usecase": "upload-files"}
  ```

- **Inside an agent session:** `iter add --type code --title "…" --request "…" --codepath src/s3` (the new item becomes a child of the calling item).
- **MCP:** the `workitem_create` tool.

### What states can a work item be in?

`queued` (waiting to run), `in-progress` (running), `question` (waiting for a person's answer), `paused` (held by a person), `parked` (set aside: rejected, stopped, or waiting for a cluster restart), `scheduled` (a recurring template), `complete` and `failed` (closed). "Blocked" is not a state: a queued item that cannot start carries an engine-owned `blocked by: <reason>` tag, rewritten every tick.

### How do priorities work?

Lower numbers run sooner, 0–99, in bands: **0–9** do now, **10–39** use cases (one number per use case), **40–49** people's requests, **50–99** maintenance. An item with no priority gets the lowest unused number in its band. A child item inherits its parent's priority exactly, and **Set priority** changes a whole lineage.

### What are lockdirs and locks?

`lockdirs` are the folders (or files) an item may change, as `{topdir}/…` paths. When an item starts, the engine takes a central lock on each; no other item touching an overlapping path can start until the run ends. Locks carry the run's lease and last 10 minutes, renewed every minute while the run lives; a lost lock is re-taken, or the run is stopped and requeued if another item took the path. The end-of-run commit includes only the item's lock scope plus `commit_extra_paths`, so another agent's unfinished files are never committed under this item.

### How do dependencies work?

List the items that must finish first in `blockedby`. Dependencies are deep: the blocker and every item it created must close complete (`blockedby_shallow` opts out). A failed blocker keeps its dependents waiting. A dependency loop is refused, and the error names the loop.

### What is the close gate?

When an agent says it is done, iter checks before closing: deterministic checks first (the session succeeded; review rows are resolved; a claimed test fix really turned green; a required commit exists), then a verifier model (Haiku by default, read-only) compares the result with the request, including any notes (`doc` rows) the attempt added — so an item whose answer is recorded as a note can pass. A failed check sends the item back to `queued` with the feedback, or to `question` after the bounce limit. A declared open blocker makes the item wait instead of bouncing. Shell items skip the gate. Each agent's `closegate` setting tunes it.

### How do questions and answers work?

An agent that needs a decision runs `iter ask` (or the `workitem_ask` MCP tool); the item moves to `question` and shows a question widget in the Work queue. Answer it (Answer… on the item), and the item queues again with the answer in its next run.

### Can I change a closed work item?

A closed item (complete or failed) is immutable except for tags and appended notes (`doc`, `explained` and `spend` rows). To work on it again, **Reopen…** it with a reason (users only); it goes back to `queued`.

### How do scheduled work items work?

A scheduled item is a template in state `scheduled` with a `sched` of kind `every` (every N minutes), `daily`, `weekly` or `stale`. When due, an engine claims the firing and clones the template into a queued run; a missed daily or weekly slot is skipped, never back-filled, and a template does not fire while its last run is still open. Only users (not engine tokens) can create schedules, with one exception: the test sweep, which every engine makes sure each project has (see "What is the test sweep?"). The test sweep and the GraphRAG change sweep are schedules; the Work queue offers Run now, Pause and Resume for them.

### What is dedup?

Items tagged with the same `check:` and `container:` pair are the same work: filing one while another is open records a repeat on the open one (and raises its priority) instead of creating a second. Before a new item first runs, a Sonnet judge also compares it with open items and merges clear duplicates.

### What happens when a run fails?

The item is retried up to the project's `failure.maxattempts` (default 5), waiting 10 seconds before the first retry and doubling each time (capped at 24 hours), then it closes `failed`. The last error is shown on the item and passed to the next attempt.

### What is ELI5?

The **ELI5** action on any item asks the `explain` agent, read-only and outside the agent cap, to explain the item in plain words for someone who has never seen the code. The answer is added to the item's history, usually within a minute.

### How much does a work item cost?

Each run records a `spend` row (dollars, tokens, turns). The Work queue status line shows today's spend against the project's `maxdailycost`; when the cap is reached, engines start nothing more that day.

## The architecture map

### What is the architecture map?

A graph of the program built from `*.iter.md` node files in the checkout: the project, its **contexts** (top-level areas), **containers** (deployable programs or packages) and **components** (modules), plus requirements, interfaces (connections between parts), tests, use cases and actors. Engines push it to iter_data whenever the files change; the Project graph tab draws it.

### How is a node file named?

By the dot rule: the part after the last dot before `.iter.md` is the node type — `main`, `code`, `bizreq`, `techreq`, `interface`, `tests`, `usecase`. For example `ledger_api.code.iter.md`. The frontmatter carries the attributes, never the type.

### What goes in a code node's frontmatter?

`id` (a UUID; `iter ids --fix` adds it), `name`, `description` (one sentence that says what it does), `simple_description` (one plain sentence for a business reader), `level` (context, container or component), `teststate`, and `children`: `codedirs` (the code it owns), `codenodes` (the nodes it owns), `inputs` and `outputs` (interfaces it uses and provides), `bizreqs`, `techreqs`, `tests`, `documents`. The body holds a `# Long Description`. Nodes join the map only through these explicit links; a file nobody links is not on the map.

### What is the node text standard?

Names are 2–6 words. A description starts with an action ("Stores work items and answers every read…"), never "The …" or a label with a list. The simple description is one plain sentence. The long description is 150–350 words: what it does, how, what goes in and out, why it matters, one example. `iter validate` warns when text breaks the standard, and `iter sweep` files an `ingest` item for each failing node.

### What is an interface file?

A contract between two parts, one operation per file, with a `label` (2–5 plain words shown on the map edge, like "Files a work item") and a `kind`: request-reply, event, stream or dataset. The body shows the data that crosses: the request, the success reply, the failure reply, and at most two worked examples in JSON. A code node lists it in `outputs` (it provides it) or `inputs` (it uses it).

### What is a use case?

A journey through the product, in `usecases/<name>/<name>.usecase.iter.md`. It lists the parts it needs in `children.codenodes` (the most specific part that is true) and may carry a `flowmap` with numbered process and data steps. When the map is stored, iter tags each listed part and every owner above it with the use case, and the Project graph draws the use case as a hierarchy: the use case → its actors (if any), else its top-level parts → ownership lines down.

### What are actors?

The people and outside programs at the edge of the map (a developer, an operator, a payment provider), in the actors file (`actors.yaml`): an id, a name, a description and `uses` patterns naming the interfaces they call. The map draws an edge from the actor to the part that provides each interface.

### How do I push the map to the server?

A running engine does it by itself, at most once a minute, when node files change. By hand: `iter_engine cli sync` (it also fixes missing ids). `iter_engine cli sync --read-only --actors path/actors.yaml` maps a checkout without writing into it.

## The Project graph tab

### How do I read the Project graph?

Pick **Whole repository** or a use case at the top. Layouts: **Cluster** (boxes inside boxes), **Top-down tiered**, **Inside-out rings**, **Sequence** (a use case's numbered steps as a lifeline diagram) and **Flow (left to right)**. Edge toggles show interfaces, libraries, ownership (contains), not-built links (red dashed) and use-case nodes. Click a node for its detail pane: description, file, the code it owns (linked to the repository), what contains it, interfaces in and out, and the use cases that pass through it.

### How are use cases laid out?

By the hierarchy first, then the numbered steps: the use case first, then all its actors together, then its top-level parts, each owned part a level further; inside a level, parts reached later by the steps sit further along. Tick **numbered steps** to draw the steps over the hierarchy. In the Sequence view the part names stay pinned at the top while you scroll.

### How do I edit the map from the web page?

Right-click (or ⌘/ctrl-click) a node:

- **code node** — Add new child node, Add new edge (draw it to another node), Define tests, Run its tests, Hide this node.
- **actor** — Add new edge (to the part it uses), Edit actor, Hide.
- **use case** — Add new edge (to a part it needs), Queue the usecase agent to name its parts, Edit its text, Hide.
- **project** — + Context, + Actor, + Global object.
- **an edge** — Remove edge (a reason is required and goes into the commit).

The toolbar has + Context, + Actor, + Global object (business or technical requirement, or use case), Edit a global object, Connect and Run tests.

### What happens after I edit the map?

The edit is accepted at once as **waiting to sync to engine**. The first engine serving the project picks it up on its next heartbeat, applies it in its checkout, commits exactly the files it wrote (with your reason, for a removal), pushes, and pushes the map, so the graph redraws. An edit whose folders a running item has locked waits until the lock is released. The sync indicator on the toolbar shows edits waiting or failed.

### How do I add a use case?

Choose **+ Global object** → use case (or the project's right-click menu), give it a name and description, and optionally pick parts it needs. When the edit lands, the `usecase` agent is queued to name every part the journey needs; the map then tags them and their owners.

## Tests and test-driven work

### How are tests organised?

In `*.tests.iter.md` files (usually `tests/<name>.tests.iter.md` beside the node; files in an older `test/` folder still count) listing **test groups**; each group lists test scripts. A script passes with exit 0, fails with exit 1, and anything else (including a timeout) is an error; it may end its output with `ITER_RESULT pass=X fail=Y total=Z`.

### How do I run tests?

`iter_engine cli runtests --group <label>` runs one group. Inside an agent session, `iter runtests --broken` proves a bug is reproduced (red) and `--fixed` proves it is fixed (green); the close gate will not complete an item whose `--fixed` claim was false.

### How much testing is enough?

As many tests as the code's input space calls for — how many practical permutations it accepts, not how long it is. A function taking one boolean needs about two tests; one taking an open JSON document needs a representative collection. Every test group covers four kinds: **golden** (the expected paths, always at least one), **malformed** (allowable malformed, incomplete or missing inputs), **longtail** (rare but valid inputs) and **failure** (what the code must refuse). The `test` agent judges the input space and records it on the group's line in the testgroup block: `input_space` (what the code accepts and roughly how many permutations) and `coverage` (tests needed per kind, 0 = cannot apply), and gives every test a `kind`. The sweep only compares those targets with the registered tests: a group that is unassessed, has unclassified tests, or is short of a target gets a top-up item.

### What is the test sweep?

`iter_engine cli sweep` runs every test group the map allows (`*.tests.iter.md`, and the older `*.testgroup.iter.md`), records each result on the map, and files work for what it finds, never a second item while the first is open:

- one `code` fix item per red group (tagged with the use cases it touches); when the fix is very complex or risky, that item files a `plan` item instead and waits for it (`iter wait --on`), then proves the fix; a group whose script itself broke is recorded but files nothing;
- one `test` item per leaf code node the map includes that has no tests at all — no tests file, or one with no test registered — which writes its first tests and links them from the node (at most 5 new per sweep, `--tests-max`);
- one `ingest` item per node whose text breaks the standard (at most 10 new per sweep, `--text-max`).
- one `test` top-up item per node whose tests fall short of their coverage (at most 5 new per sweep, `--coverage-max`) — see "How much testing is enough?".

Every project has exactly one **Test sweep** schedule, owned by the engine: the first engine to serve the project creates it **paused**, every 4 hours. Resume it in the Work queue to turn it on; Pause turns it off. It cannot be deleted or closed, and Pause & edit changes its interval or command flags. When due it runs at once on its timer — it takes no agent slot and ignores the usage and budget holds, since a shell run spends no model time — but only while the project is Running. It writes nothing into the checkout (results live on the map), so its runs make no commit; neither do `iter rag sync` runs.

Test scripts write any output files only under `$ITER_TEST_OUT`: a folder per test group outside the checkout, emptied at the start of every run, so it holds the last run and never a history. `iter_engine cli sweep --install-schedule --every 4h` turns it on from the command line (needs a user token).

### What is teststate?

A node's `teststate` decides whether its tests run in the sweep: `omit` skips them, `include` brings them back under an omitted parent, `block` parks them (only a person lifts it), `inherit` (the default) follows the parents. `iter_engine cli teststate --list` shows every node's effective state. A test group can also carry its own `teststate: omit`: the sweep then skips that one group whatever its owners say. Use it for groups that are run by hand only, such as live-site tests that need real credentials, and link them like any other group so the map shows them instead of reporting them as unlinked.

### How do I work test-first from the map?

Right-click a node → **Define tests**: list what passing means, simplest first, and tick "queue the test agent". The planned tests are written into the node's tests file, the `test` agent writes one script per test and runs them — red is expected — and the sweep turns each red group into a `code` item, so the code follows the tests.

## GraphRAG search

### What is GraphRAG in iter4?

Semantic search over a project's knowledge, in the fourth web tab. It indexes two kinds of document per project: files you upload (pdf — scanned PDFs are read with OCR — docx, pptx, html, markdown, text and more, up to 25 MB) and the map's `*.iter.md` node files. This guide is indexed too, product-wide, so its answers appear in every project's search.

### What is the embedding model used?

**all-MiniLM-L6-v2** from sentence-transformers: 384-dimension vectors, a 256-token window, run in-process with candle on the CPU. Engines do the bulk embedding (document chunks and their summaries); iter_data embeds only search questions. Every vector carries a model stamp, so chunks embedded by different weights are flagged. The container has the model built in; `tools/fetch_model.sh` downloads it (about 91 MB) for other setups.

### How does GraphRAG index a document?

The text is extracted, split into chapters (the sections under the top heading level) and chunks of about 200 model tokens, and each chunk is embedded twice: its raw text at once (so a document is searchable immediately), and a short summary written by the **Summary** agent (Haiku by default) as a second vector. Chapter and document summaries follow when every chunk is summarised.

### How does GraphRAG search work?

Hybrid by default: a keyword ranking (ArangoSearch BM25 over the text, summary, heading and title) is fused with a meaning ranking (the better of the raw and summary vectors). You can choose keywords only or meaning only. A hit returns the full chunk with its chapter and document summaries, and for node files the node's neighbours in the map. At most two chunks of any one document appear in a result list (the `per_doc` search option, default 2; 0 means no limit), so one long document — this guide included — never fills every slot.

### How do I upload a document?

In the GraphRAG tab, drop files on **Upload documents** (right side, above Settings) or choose them. With "store the original in the repo" ticked, an engine writes the original into the project's docs directory (default `{topdir}/docs/`) and commits it; the document then shows **in repo**. It shows **written** instead when the engine wrote the file but could not commit it — because the docs directory is git-ignored, or because the project's checkout is not the root of its own git repository. You can link a document to a map node from its detail pane.

### Does GraphRAG need an engine?

Ingestion and summaries need a live engine with a Claude account. Search works without one.

### How is the index kept up to date?

Engines re-index node files after every map push. The **Create a scheduled RAG change sweep** button makes a scheduled `iter rag sync` item; `iter_engine cli rag sync [--force]` does it by hand.

## MCP for agents

### Does iter4 have an MCP server?

Yes: a stateless MCP server (Streamable HTTP, JSON responses) at `POST /mcp` on iter_data. Send `Authorization: Bearer <token>`, and optionally `X-Iter-Project` and `X-Iter-Workid` to set the default project and calling item. Every tool call goes through the same API rules as HTTP.

### Which MCP tools are there?

- **Work queue:** `status`, `workitem_list`, `workitem_get`, `workitem_details`, `workitem_create`, `workitem_ask`, `workitem_reject`, `workitem_wait`, `workitem_doc`, `workitem_block`, `capability`, `locks_list`.
- **Map:** `graph_stats`, `graph_lookup`, `graph_node`, `graph_neighbors`, `graph_owner`, `graph_usecase`.
- **GraphRAG:** `rag_search`, `rag_status`, `rag_docs`, `rag_doc`, `rag_add_document`, `rag_link_document`.

The checkout verbs (validate, runtests, teststate, usecase, markers, critreview) stay on the `iter` command line.

### How do I connect an agent to the MCP server?

Every agent session an engine starts gets it automatically. For your own Claude Code session, add to `.mcp.json`:

```json
{"mcpServers": {"iter": {"type": "http", "url": "http://127.0.0.1:8300/mcp",
  "headers": {"Authorization": "Bearer ${ITER_ENGINE_TOKEN}", "X-Iter-Project": "${ITER_PROJECT}"}}}}
```

## The iter command line

### Where is the `iter` command?

`iter` is the engine binary in command-line mode: `iter_engine cli <verb>`. Inside agent sessions the engine puts a shim named `iter` on the PATH, so agents type `iter <verb>`. The global flag `--project` (default `$ITER_PROJECT`) names the project or its checkout path.

### Which verbs does `iter` have?

- **Work items** (need `ITER_DATA_URL` and `ITER_ENGINE_TOKEN`): `add` (create an item), `ask` (ask a person), `reject` (park the calling item as invalid), `block --cluster-restart` (park until a cluster restart), `wait --on <id>` (wait for another item), `doc` (append a note), `critreview` (a synchronous review), `capability` (read a capability doc), `status` (open work in run order).
- **Checkout:** `init` (scaffold a project), `validate [--fix] [--template]` (check node files), `ids [--fix]` (node ids), `sync` (push the map), `graph-apply --file op.json` (apply a graph edit by hand), `markers` (the scan as JSON), `teststate`, `usecase --file … --add/--remove/--list`.
- **Tests:** `runtests --group <label> [--broken|--fixed]`, `sweep [--install-schedule --every 4h]`.
- **GraphRAG:** `rag sync [--force]`.

### Which engine flags are there besides `--config`?

`--data-url`, `--ticks N`, `--accounts` (which account tokens are set), `--probe` (live usage per account), `--adduser <name>` (an approval key), `--approve <id>` (sign an approval), `--doc <id> --text|--file` and `--question-widget <file>` (check a question widget).

## Settings and customisation

### What can I customise per project?

In the project's gear: description, state, git repository, main file, default context files, the agent ladder, the daily cost cap, `fix_on_test_failure` (file fix items when tests fail), `session_chain_max` (how many items one session may take in a row, default 3), per-agent overrides (model, flags, timeout, max, close gate, lock shape), the failure policy, cluster restart, dedup, pinned tags, engines, accounts and `commit_extra_paths` (extra paths every commit includes). The Project graph can hide folders (`graph.hide`) and move nodes (`graph.parents`).

### What is agent tooling?

Shared prompt pieces an admin edits under **Agent Tooling**: `shared` (added to every agent's prompt), `capability` (listed by name; read in full with `iter capability <name>`), `source` (instructions chosen by who requested the item: user, agent or error), `prepost` (extra turns before or after the main work) and `critic` (the reviewer used by `iter critreview`).

### How is an agent's prompt put together?

The agent's `promptbody`, then the shared tooling, the capability index, the project's context files, the close-gate paragraph, the source instructions, the work item (title, id, folders, priority), any previous attempt's error, the agent's memory and the item's context files; then the main work turn, any pre/post turns, a memory turn and a self-check.

## Migrating from iter3

### How do I move iter3's data into iter4?

Run iter_data once in migration mode against the iter3 DynamoDB tables:

```bash
iter_data --env-file ../.env --migrate-from dynamodb --migrate-prefix iter3_ [--migrate-dry-run] [--migrate-overwrite]
```

It reads every `iter3_*` table (it cannot create or change them), copies rows with their keys and versions, copies change counters without moving them backwards, skips rows already present unless `--migrate-overwrite`, prints source and target counts per table, and exits. This is also how iter3's agent records arrive.

## Troubleshooting

### The engine says "no engine token". What do I do?

Mint one as an admin (`POST /api/users/<engine>/token`, or the wizard's Set up step) and put it in the checkout's `.env` as `ITER_ENGINE_TOKEN=<token>`, or in the variable `token_envar` names.

### An item keeps bouncing at the close gate. Why?

The verifier found the result does not meet the request. Read the gate feedback in the item's history; make the request's "done when" concrete, or answer the question the item moved to. Each agent's `closegate` setting controls the bounce limit and verifier model.

### A graph edit stays "waiting to sync". Why?

No engine serving the project is online, or a running item holds a lock on the folders the edit touches (it applies when that run ends). Failed edits show the reason in the sync table.

### GraphRAG says the embedding model is not found.

Run `tools/fetch_model.sh` (or `./deploy.sh docker`, which runs it), or point `ITER_EMBED_MODEL` or `--embed-model` at the model directory.

### Where are the logs?

The container: `docker logs iter4`. Native iter_data: `run/iter_data.log`. Engines log to their console (or journal under systemd). Each work item's history in the Work queue holds its runs, questions, verdicts and costs.

## Words you will meet

### What do the iter4 terms mean?

- **Work item** — one piece of work for an agent or a shell command.
- **Agent** — a named role (a prompt plus settings) that does a type of work.
- **Engine** — the program next to a checkout that runs work items.
- **Checkout** — the engine's copy of the project repository; paths are written `{topdir}/…`.
- **Lockdirs** — the folders an item may change; locked while it runs.
- **Close gate** — the checks and verifier that decide whether an item is really done.
- **Node file** — a `*.iter.md` file describing one part of the program.
- **Context / container / component** — the three levels of code nodes, largest to smallest.
- **Interface** — a contract between two parts, one operation per file.
- **Use case** — a journey through the product and the parts it needs.
- **Datasync** — graph edits from the web page waiting for an engine to apply them.
- **Sweep** — the project's engine-owned Test sweep schedule: runs every test group on a timer and files work for what it finds.
- **GraphRAG** — the semantic search index over documents and node files.
- **MCP** — the Model Context Protocol server agents use to call iter.
