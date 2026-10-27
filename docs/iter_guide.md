# iter guide

This guide explains what iter is, how to stand it up, how to create and run a project, and how every part works: work items, agents, engines, the architecture map, tests, GraphRAG search and the MCP server. It ships inside the iter_data server and is searchable from the GraphRAG tab of every project, so asking a question in plain words ("how do I create a new project?") finds the right section.

Each section starts with a question. Commands assume you are in the project's checkout unless it says otherwise.

## What is iter?

### What does iter do?

iter keeps a team of AI coding agents working on a real software project without letting them trip over each other. A person, or an agent, files a **work item**: "make this change, in this part of the code". An **engine** — a program running next to a copy of the code — picks the most urgent item it can start, locks the folders the item will touch, starts a headless Claude Code session (or a plain shell command) to do the work, has a second model check the result against the request (the **close gate**), commits just those files, and moves on.

### What are the parts of iter?

- **iter_data** — the central server. It holds every work item, lock, agent definition, schedule, user, cost record and the architecture map in ArangoDB, answers one HTTP API, serves the web page, and serves the MCP server for agents.
- **iter_engine** — the engine. Run it next to a checkout of your project; it claims work, runs agents, commits and pushes. The same binary is also the `iter` command line (`iter_engine cli <verb>`).
- **iter_core** — the library compiled into both programs (the shared rules, the node-file library and GraphRAG's text and embedding code). The engine's checkout tools live in iter_engine itself.
- **The web page** — four tabs: Intro, Work queue, Project graph, GraphRAG.

### What database does iter use?

ArangoDB Community Edition 3.12, and only ArangoDB. It stores documents (work items and every other record), counters, the architecture map as a graph (collections `node` and `link`, named graph `iter_map`), and the GraphRAG chunks and vectors. SQLite and DynamoDB are not supported.

## Standing up the server

### How do I install and start iter?

With the installers in `installers/` of the iter repository: one script per program and platform, each downloading the prebuilt binary from the latest GitHub Release (no compiler needed). For the server:

```bash
curl -fsSL https://raw.githubusercontent.com/Stephen-Hilton/iter/main/installers/iter_data/linux/install.sh | bash
```

`iter_data/linux` is Debian, Ubuntu or Arch; `iter_data/linux_aws` is EC2 Amazon Linux 2023; `iter_data/aws` stands up a whole AWS stack (CloudFormation); `iter_data/docker` is the all-in-one container (ArangoDB + iter_data) on any Docker host. Each installs ArangoDB beside iter_data (or uses yours with `--arango-url`), serves HTTPS on port 8400, prints the first admin password and the server certificate's fingerprint once, and takes `--help` and `--dry-run`. `installers/upgrade_iter.sh` later upgrades a machine to a newer release.

For development, from the repository: `./deploy.sh docker` builds and starts the container locally, and `./deploy.sh local` runs a release build of iter_data against a dev ArangoDB on :8529.

### Which ports does iter use?

- **8400** — the API and the web page (`ITER_PORT` changes the host port).
- **127.0.0.1:8630** — the ArangoDB web console of the container, localhost only (`ARANGO_HOST_PORT` changes it). Log in as `root` with `ARANGO_ROOT_PASSWORD`; iter's database is `iter_db`.
- **127.0.0.1:8529** — the dev ArangoDB used by `./deploy.sh local`, the unit tests and the end-to-end tests.

### What is the admin password, and how do I log in the first time?

On first start, when there are no users, iter_data creates the user `admin` with role `admin`. The password is `ITER_ADMIN_PASSWORD` from the environment or `.env`; if that is unset, a random 20-character password is generated and printed once in the server log (`docker logs iter`). Open http://127.0.0.1:8400/ and log in as `admin`.

### Which environment variables does the server read?

- `ITER_ADMIN_PASSWORD` — the first admin's password.
- `ITER_JWT_SECRET` — the secret that signs login and API tokens; if unset, iter_data reads or creates `--secret-file`.
- `ARANGO_URL`, `ARANGO_DB`, `ARANGO_USER`, `ARANGO_PASSWORD` — where iter_data finds ArangoDB and who it signs in as (defaults `http://127.0.0.1:8529`, `iter_db`, `iter_user`). That user needs `rw` on that database only, so one ArangoDB can serve other apps too.
- `ARANGO_ROOT_PASSWORD` — the ArangoDB root password, used only when iter's user or database is missing or the user lacks `rw`: root then creates them and grants `rw`. Not needed once they exist.
- `ARANGO_CERT_FINGERPRINT` (`sha256:…`, comma-separated) or `ARANGO_CA_CERT` (a PEM file) — how an `https://` ArangoDB's certificate is checked: pinned, or against your own CA (else the public CAs). Plain `http://` is refused to anything but this machine unless `ARANGO_ALLOW_PLAIN_HTTP=1` (a private network only).
- `ITER_EMBED_MODEL` — the embedding model directory (the container has it at `/opt/iter/models/all-MiniLM-L6-v2`).
- `ITER_ENV_FILE` — which `.env` `deploy.sh` reads (default the repo root `.env`).

The `.env` loader reads plain `KEY=VALUE` lines, never runs them through a shell, and never overrides a variable already set in the environment.

### Where is my data stored?

In one folder, the **iter dir**: `/opt/iter` by default (`--iter-dir` changes it; the AWS stack uses `/data/iter`, the docker installers `~/iter` under Docker Desktop). iter_data keeps its files in `<iter dir>/iter_data/`: `config/data.env` (secrets), `state/` (its signing secret and TLS certificate), `arango/data/` (the database), `backups/` (a backup every 6 hours) and `models/` (the embedding model). An engine keeps its own in `<iter dir>/iter_engine/`: `config/`, `secrets/` (its key and the account tokens), `models/` and `logs/`.

### Does the container run the agents?

No. The container runs ArangoDB and iter_data only. Engines — and the Claude Code sessions they start — run on the machines that hold checkouts of your projects.

### Can I run iter on a server or in the cloud?

Yes. The iter_data installers run on a small VM or an EC2 instance (`installers/iter_data/linux_aws`, or `installers/iter_data/aws` for the whole stack), and engines run on any Linux, macOS or Windows machine (native or in WSL2 or Docker) that can reach the server's `https://` URL (`installers/iter_engine/`).

## Creating a new project

### How do I create a new project?

1. **Run the wizard.** In the web page, go to **Intro** and choose **Design a new project**. Give the name, an optional git remote, the file-naming rule and a daily cost cap. It creates a **designed** project (state Stopped) whose project node and default requirements appear in the Project graph.
2. **Design it** in the Project graph: **+ Node** adds contexts, containers, components, connections, tests, requirements, use cases and actors; draw edges between them. Nothing touches a disk yet.
3. **Have an engine running** on the machine that will hold the code (once per machine; see "How do I install the engine?"). The wizard shows the install command for each platform and watches for the engine to check in.
4. **Build.** The Project graph's **Build…** button: pick the engine and a folder. The engine creates the folder, runs `git init`, writes every node file and commits; optionally the **plan** agent is queued to build the project from its design. For a repository that already exists on that machine, draw a **serves** edge from the engine to the project in the Settings graph instead and set its `topdir` (the checkout's folder); `iter_engine cli init --name <project>` in the checkout writes the project node and three empty requirements if it has none.
5. **Connect accounts.** In the Settings graph, draw a **bills** edge from a Claude account to the project (its switch % and stop % live on that edge). The account's token goes on the engine machine (see "How do Claude accounts and tokens work?").
6. **Start the project.** In the Work queue, press **Running**. The engine only starts work while the project is Running.

### Can I create a project without the web page?

Yes, with the API and an admin token: `PUT /api/projects/<name>` with a project record (state `Stopped`); the engine's key with `POST /api/keys` `{"public_key":"iter_…","engine":"<engine>"}` (the public key from `iter_engine key show` on that machine; the installers ask for this); the checkout with `POST /api/settings/edges` `{"from":"iter_engine:<engine>","to":"project:<name>","settings":{"topdir":"<checkout path>"}}`; an account with `POST /api/settings/nodes` `{"type":"account","name":"<account>","settings":{"provider":"claude","token_envar":"DEV1_TOKEN"}}` and a bills edge `{"from":"account:<account>","to":"project:<name>","settings":{"order":1,"switch":80,"stop":95}}`. Then set the project's state to `Running`.

### What does a project record contain?

Name, description, state (Running, Draining or Stopped), git repository, the agent ladder (`maxagents`, for example `{">98%":0,">95%":1,">90%":2,"else":4}`), a daily cost cap (`maxdailycost`), per-agent overrides (`agents`), the failure policy (`failure`: attempts, first retry, backoff), the engines that serve it, the Claude accounts, `commit_extra_paths`, `pinned_tags`, dedup and cluster-restart settings, and `session_chain_max`. Edit it from the gear next to the project in the left panel (admins can save).

### What does `iter init` write?

The project node `global/<slug>.project.iter.md` and three requirement files to fill in, `global/requirements/<slug>_philosophy.philosophy.iter.md`, `…_first_business_requirement.bizreq.iter.md` and `…_first_technical_requirement.techreq.iter.md`, each already in the node-file standard. Existing files are kept unless you add `--force`. It writes no engine configuration: an engine learns its projects, folders and accounts from iter_data (the Settings graph's serves and bills edges).

### What goes in the project node (`*.project.iter.md`)?

The project's root node, one per checkout (`global/<slug>.project.iter.md` from `iter init`): `id`, `name`, `desc` (one paragraph on the project), `creator`, `teststate`, `file_naming` (how a new node file is named when its name is taken), `gitrepo`, `scandirs` (where node files are found) and `children`: `codenodes` (the top-level contexts of the map), `tests`, `reqs` (requirement files every agent reads) and `codedirs`. The body describes the project in prose.

### Why is my project not starting any work?

Check, in order: the project is **Running** (not Stopped or Draining); an engine is online for it (the status line says "N engines online"); the engine has an account it can use (not "Suspended, no account token" or "no usage left"); the daily cost cap is not reached; the item is `queued`, its dependencies are complete, and no running item holds a lock on its folders (queued items show a `blocked by: …` tag saying why they wait).

## Engines

### How do I install the engine?

With its installer, once per machine (one engine serves every project connected to it):

```bash
curl -fsSL https://raw.githubusercontent.com/Stephen-Hilton/iter/main/installers/iter_engine/linux/install.sh | bash   # Debian, Ubuntu, Arch
curl -fsSL https://raw.githubusercontent.com/Stephen-Hilton/iter/main/installers/iter_engine/macos/install.sh | bash   # macOS
```

Windows: `installers/iter_engine/windows/install.ps1` (or `linux_wsl` inside WSL2); any Docker host: `installers/iter_engine/docker`.

Easiest: in the web page, 🔑 Engine keys → **Add an engine** makes a one-time join code and shows the install line for each platform with the code in it (`… | bash -s -- --join iterjoin1_…`, `-Join …` on Windows). Run that line on the machine and nothing else is copied: the code carries the server's URL, which certificate to trust and a secret that registers the machine's key once. The engine's name is the machine's short hostname unless the code names one (`<name>-2` when another engine has it already). It works behind a TLS proxy or load balancer with a public certificate too. Without a code, the script asks for the server's `https://` URL and the fingerprint of its certificate (the iter_data installer printed it; Engine keys shows it) and, after making the key, waits while you paste the public key it prints into **Engine keys** with the engine's name.

Either way the script installs `git`, `jq` and the `claude` command line when missing, downloads the engine and the embedding model, makes the engine's key and starts the engine once the server accepts it. It installs a service (systemd `iter-engine`, launchd `com.iter.engine` on macOS, a logon task on Windows), so the engine runs at boot or login and restarts if it stops.

### How do I start an engine?

The installer starts it as a service, and the service starts it again after a reboot or a crash. `install.sh status`, `install.sh logs` and `install.sh update` (in the installer's folder) show its state, follow its log and upgrade it. By hand, `iter_engine --data-url https://<server>:8400` runs one in the foreground (one engine per machine: stop the service first); `--ticks 20` runs 20 ticks, finishes the work it started and exits.

The first log line names the engine, its version and the server. An engine signs in only with its own key (`<iter dir>/iter_engine/secrets/engine.key`); without a key it says to run `iter_engine key new` and add the public key in Engine keys.

### What does an engine do?

Every tick (5 seconds by default) the engine: renews the leases on its running items' locks, reloads account tokens if `.env` changed, pushes the project's map when `*.iter.md` files changed, picks a Claude account, sends a heartbeat, applies graph edits waiting from the web page, starts GraphRAG summary workers, fires due schedules, and — when the project is Running and the agent cap allows — claims the best queued item, locks its folders, and runs it.

### How much memory may an agent session use?

On Linux, by default at most 40% of the machine's memory for one agent session (the `claude` process and everything it starts, its test scripts included) plus 1 GiB of swap, and at most 30% plus 512 MiB of swap for one test script that `iter runtests` runs inside a session. The engine runs each session and each script in a cgroup (a group of processes the Linux kernel limits and counts together) of its own. When a session reaches its limit the kernel kills that whole session: the item fails with a last error that begins "agent session killed: its processes reached the … memory limit" and names the kernel's `memory.events` counters, the engine log says the same, and every other session keeps running. When a test script reaches its limit the kernel kills only the script's largest process (the runaway test binary); the script goes on and its test run's note says "killed by the memory limit: …".

Why these numbers: on a 15 GiB machine the service as a whole may use 80% (12.4 GiB, the unit's `MemoryMax=80%`). 40% (6.2 GiB) leaves room for a real build or test run in one session while a runaway is stopped with half of the service's memory still free for the rest. Dividing the 80% by the number of sessions (the `maxagents` default is 4, so 3.1 GiB each) would kill ordinary Rust builds. Sessions together can still go over the service's cap; then the kernel kills one whole session (each one's processes are killed as a group), not the engine, because the unit has `OOMPolicy=continue`. The test-script limit sits 10 points under the session's so a runaway test is stopped at its own limit, with only that process killed, before it can take its session down with it. The swap allowances are small so a runaway is killed in seconds instead of thrashing the disk.

Change them with the engine's `--session-memory-max`, `--session-swap-max`, `--test-memory-max` and `--test-swap-max` (the Linux installers' options of the same names keep them in `config/engine.conf` and on the service's command line), or with `ITER_SESSION_MEMORY_MAX`, `ITER_SESSION_SWAP_MAX`, `ITER_TEST_MEMORY_MAX` and `ITER_TEST_SWAP_MAX` in a drop-in in `/etc/systemd/system/iter-engine.service.d/`. A value is a size (`6G`, `512M`), a share of memory (`40%`), `max`, or `off`; a swap limit of `0` means no swap. The limits need cgroup v2 and the unit's `Delegate=yes`, which the installers write. Without either, and on macOS and Windows, the first log lines say "memory limits off: <why>" and sessions run without limits, as before.

### Can several engines serve one project?

Yes. Draw a serves edge from each engine to the project in the Settings graph, each with that machine's checkout folder (`topdir`). Claims are versioned writes (only one engine wins an item), locks are central, schedules and graph edits are claimed the same way, and engines avoid an account another live engine is using.

### How many agents run at once?

The project's `maxagents` ladder sets the cap from the current account usage: the most restrictive `>N%` rule that holds, else the `else` value (default 4). Each agent type has its own `max` too (default 4; a project override wins). Shell items do not count against per-type caps. **Run now** on an item ignores the cap but still waits for dependencies and locks.

### How do Claude accounts and tokens work?

An account is a node in the Settings graph: its name, provider and `token_envar` (the environment variable that holds its token, for example `DEV1_TOKEN`). A **bills** edge from the account to a project lets that project use it, with a `switch` and a `stop` usage percentage and an order. The token itself stays on the engine machine: in `<iter dir>/iter_engine/secrets/.env` (make one with `claude setup-token`; the installer's accounts step asks for it), or from a secret store with `--secrets-command`, and environment variables the service is started with win over both. Each session gets its account's token as `CLAUDE_CODE_OAUTH_TOKEN`; `ANTHROPIC_API_KEY` is removed from the session's environment, because it would bill API credits instead. The engine moves to the next account at `switch`, holds at `stop`, never uses an account whose token is missing, and re-reads the tokens every tick.

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

Log in with `POST /auth/login` and `{"user":"…","password":"…"}` for a 24-hour token. For a long-lived one (an agent or a connector that acts as you), open your settings (the gear at the top right) → **Long-lived token**, choose 1–365 days and copy the token: it is shown once. The same as a call: `POST /api/users/<you>/token` with `{"ttl_days":90}`; an admin may mint one for anyone. Send it as `Authorization: Bearer <token>` on every `/api/` call and on `/mcp`. It has your role, and it ends on its date or when your password changes; the MCP tool `whoami` (or `GET /api/projects/<p>/whoami`) says when.

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

Agent fields: `desc`, `max` (how many run at once), `timeoutsec`, `model`, `flags` (extra Claude Code flags; the webui's new-agent default is `--dangerously-skip-permissions`), `childstate` (the state of items it creates), `closegate` (how its results are checked), `lockshape` (which folders its items may lock) and `promptbody`. A new install starts with the default agents (plan, code, test, usecase, refactor, ingest, explain, summary) and their tooling (`_shared`, `_critic`, the capabilities), added once at iter_data's first start; edit or delete them like any other. A deleted default stays deleted; `iter_data defaults add` on the server brings back any that are missing. An item naming an agent that does not exist fails with "agent '<x>' not defined in iter_data".

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

Items with the same `check:` and `container:` pair are the same work (pass them as tags; the server moves them into the item's `repeat_check` / `repeat_container` fields so they never crowd the queue line): filing one while another is open records a repeat on the open one (and raises its priority) instead of creating a second. Before a new item first runs, a Sonnet judge also compares it with open items and merges clear duplicates.

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

### What is GraphRAG in iter?

Semantic search over a project's knowledge, in the fourth web tab. It indexes two kinds of document per project: files you upload (pdf — scanned PDFs are read with OCR — docx, pptx, html, markdown, text and more, up to 25 MB) and the map's `*.iter.md` node files. This guide is indexed too, product-wide, so its answers appear in every project's search.

### What is the embedding model used?

**all-MiniLM-L6-v2** from sentence-transformers: 384-dimension vectors, a 256-token window, run in-process with candle on the CPU. Engines do the bulk embedding (document chunks and their summaries); iter_data embeds only search questions. Every vector carries a model stamp, so chunks embedded by different weights are flagged. The container has the model built in; `installers/fetch_model.sh` downloads it (about 91 MB) for other setups.

### How does GraphRAG index a document?

A live engine serving the project claims the upload, extracts the text, splits it into chapters (the sections under the top heading level) and chunks of about 200 model tokens, and embeds each chunk twice: its raw text right away, and a short summary written by the **Summary** agent (Haiku by default) as a second vector. Chapter and document summaries follow when every chunk is summarised.

A document is **searchable** once its chunks are stored, which takes one engine tick (5 s) plus extraction and embedding: about 5–30 seconds for a text under 1 MB when an engine is live. Keyword and raw-text search find it from then on, and summary search once it is **ready** (minutes later). With no live engine it stays **queued** until one checks in. The document's `searchable` flag (in the GraphRAG tab, `rag_doc`, `rag_docs`) says which it is.

### How does GraphRAG search work?

Hybrid by default: a keyword ranking (ArangoSearch BM25 over the text, summary, heading and title) is fused with a meaning ranking (the better of the raw and summary vectors). You can choose keywords only or meaning only. A hit returns the full chunk with its chapter and document summaries, and for node files the node's neighbours in the map. At most two chunks of any one document appear in a result list (the `per_doc` search option, default 2; 0 means no limit), so one long document — this guide included — never fills every slot.

### How do I upload a document?

In the GraphRAG tab, drop files on **Upload documents** (right side, above Settings) or choose them. With "store the original in the repo" ticked, an engine writes the original into the project's docs directory (default `{topdir}/docs/`) and commits it; the document then shows **in repo**. It shows **written** instead when the engine wrote the file but could not commit it — because the docs directory is git-ignored, or because the project's checkout is not the root of its own git repository. You can link a document to a map node from its detail pane.

### Does GraphRAG need an engine?

Ingestion and summaries need a live engine with a Claude account. Search works without one.

### How is the index kept up to date?

Node files (`*.iter.md`) are never uploaded: engines index them from the checkout. After every file-sync round that changed something, an engine re-indexes the changed node files, at most once a minute, so a node edited with `graph_node_update` reaches search about a minute after the engine writes its file. The **Create a scheduled RAG change sweep** button makes a scheduled `iter rag sync` item as a backstop; `iter_engine cli rag sync [--force]` does it by hand.

To check that every node file is indexed, run `test/iter_data/rag_node_count.sh <project>`. It compares the distinct `*.iter.md` paths of the map (after the project's `node_types` setting) with the node documents that are searchable, names any that are missing, and exits 0 only when they match. The map is the reference, not `git ls-files`: agent memory and interface files are not map nodes.

## MCP for agents

### Does iter have an MCP server?

Yes: a stateless MCP server (Streamable HTTP, JSON responses) at `POST /mcp` on iter_data. Send `Authorization: Bearer <token>`, and optionally `X-Iter-Project` and `X-Iter-Workid` to set the default project and calling item. Every tool call goes through the same API rules as HTTP. Clients that send `MCP-Protocol-Version: 2025-06-18` must send one message per request (no batches), as that version of the specification says.

Each tool's description has six parts, **Use**, **Arguments**, **Example**, **Returns**, **Errors** and **Limits**, and each tool declares the shape of its result, so an agent can call it without guessing.

Who filed or changed a work item is set by the server from the token and the calling work item (`X-Iter-Workid`): `user:<name>` for a person, `agent:<agent>@<engine>` for an agent working on its own in-progress item, `engine:<engine>` otherwise. Whatever `requestedby` or `createdby` a request sends is ignored, and named in the reply's `warnings`.

### Which MCP tools are there?

- **Work queue:** `status`, `workitem_list`, `workitem_get`, `workitem_details`, `workitem_create`, `workitem_ask`, `workitem_reject`, `workitem_wait`, `workitem_doc`, `workitem_block`, `capability`, `locks_list`.
- **Map:** `graph_stats`, `graph_lookup`, `graph_node`, `graph_neighbors`, `graph_owner`, `graph_usecase`, and the edit tools `graph_node_create`, `graph_node_update`, `graph_edge_add`, `graph_edge_remove`.
- **Requirements:** `req_create`, `req_update`, `req_move`, `req_delete` (one `## ` section of a bizreq or techreq file each).
- **Other:** `testlogs` (test results of the test nodes), `settings_graph` (engines, projects, accounts, agents and their edges).
- **GraphRAG:** `rag_search`, `rag_status`, `rag_docs`, `rag_doc`, `rag_add_document`, `rag_link_document`.
- **With a person's token only** (the webui's queue controls, for you or an agent acting as you): `workitem_answer` (answer a question item; it queues again), `workitem_set_state` (park, unpark, pause, resume, close, run now), `workitem_stop`, `workitem_priority`, `workitem_reopen` (retry a failed item), `workitem_edit`, `workitem_clone` (copy, follow-up, run a schedule now), `workitem_duplicate_of`, `workitem_explain`, `workitem_delete`, `rag_delete_document`, `rag_resummarize_document`, and `whoami`. The agents iter runs itself (engine sessions) see only the item they work on, so they neither list nor call these. To update a document, upload it again under the same filename.

`workitem_list` reaches every item of a project, closed ones included: filter on any field (`{"field": "ts.receive", "op": "gte", "value": "2026-10-01"}`; a missing field reads as false or empty), sort on any field (newest first by default), and page with `limit` (up to 500) by passing each reply's `next_cursor` back as `cursor` until it is `null`.

`rag_add_document` takes `wait_sec` (up to 60): the call returns as soon as the document is searchable, or when the wait runs out, with its state and a message saying what to do next. It refuses `*.iter.md` files: change a node with `graph_node_update` instead.

The checkout verbs (validate, runtests, teststate, usecase, markers, critreview) stay on the `iter` command line.

### How do I connect an agent to the MCP server?

Every agent session an engine starts gets it automatically, with its calling work item in `X-Iter-Workid`.

For a claude.ai chat, add a custom connector: the URL is `https://<your iter_data>/mcp`, Authentication **No sign-in**, and one request header, `Authorization` = `Bearer <your long-lived token>` (see "How do I get an API token?"). The agent then acts as you, with your role and the people-only tools.

For your own Claude Code session, add to `.mcp.json`:

```json
{"mcpServers": {"iter": {"type": "http", "url": "https://127.0.0.1:8400/mcp",
  "headers": {"Authorization": "Bearer ${ITER_ENGINE_TOKEN}", "X-Iter-Project": "${ITER_PROJECT}",
              "X-Iter-Workid": "${ITER_WORKID}"}}}}
```

Leave `X-Iter-Workid` out when you are not working on an item: then you act as yourself (`user:<name>`).

### Can a web page call the MCP server?

Only browser-based clients (a page on another site, like the MCP Inspector) are affected by this. Agents and Claude Code are not browsers and are never affected. By default every site is allowed: iter_data trusts only the bearer token, which a browser never adds by itself. An admin can limit which sites may call it under the iter_data settings, **Allowed web origins** (one `https://host[:port]` per line). An empty list allows every site, and the webui on its own server is always allowed, whatever the server is called. A change takes effect within 15 seconds.

## The iter command line

### Where is the `iter` command?

`iter` is the engine binary in command-line mode: `iter_engine cli <verb>`. Inside agent sessions the engine puts a shim named `iter` on the PATH, so agents type `iter <verb>`. The global flag `--project` (default `$ITER_PROJECT`) names the project or its checkout path.

### Which verbs does `iter` have?

- **Work items** (need `ITER_DATA_URL` and `ITER_ENGINE_TOKEN`): `add` (create an item), `ask` (ask a person), `reject` (park the calling item as invalid), `block --cluster-restart` (park until a cluster restart), `wait --on <id>` (wait for another item), `doc` (append a note), `critreview` (a synchronous review), `capability` (read a capability doc), `status` (open work in run order).
- **Checkout:** `init` (scaffold a project), `validate [--fix] [--template]` (check node files), `ids [--fix]` (node ids), `sync` (push the map), `graph-apply --file op.json` (apply a graph edit by hand), `markers` (the scan as JSON), `teststate`, `usecase --file … --add/--remove/--list`.
- **Tests:** `runtests --group <label> [--broken|--fixed]`, `sweep [--install-schedule --every 4h]`.
- **GraphRAG:** `rag sync [--force]`.

### Which engine flags are there?

`--data-url`, `--name` (default: the machine's short hostname), `--key-file`, `--env-file`, `--secrets-command` / `--secrets-every`, `--ticks N`, `--accounts` (which account tokens are set), `--probe` (live usage per account), `--adduser <name>` (an approval key), `--approve <id>` (sign an approval), `--doc <id> --text|--file`, `--question-widget <file>` (check a question widget) and `--version`. Key commands: `iter_engine key join | new | show | rotate | status | trust`.

## Settings and customisation

### What can I customise per project?

In the project's gear: description, state, git repository, main file, default context files, the agent ladder, the daily cost cap, `fix_on_test_failure` (file fix items when tests fail), `session_chain_max` (how many items one session may take in a row, default 3), per-agent overrides (model, flags, timeout, max, close gate, lock shape), the failure policy, cluster restart, dedup, pinned tags, engines, accounts and `commit_extra_paths` (extra paths every commit includes). The Project graph can hide folders (`graph.hide`) and move nodes (`graph.parents`).

### Which settings does the iter_data server itself have?

The iter_data settings (⚙ on the iter_data node) show its host, database and environment, read only, and, for admins, **Allowed web origins**: the sites whose web pages may call this server from a browser, one `https://host[:port]` per line. Leave it empty to allow every site. Agents that are not browsers are never affected, and the webui is always allowed. It is the same as `PUT /api/server/origins {"allowed_origins": [...]}`; there is no flag or environment variable, and no installer setting.

### What is agent tooling?

Shared prompt pieces an admin edits under **Agent Tooling**: `shared` (added to every agent's prompt), `capability` (listed by name; read in full with `iter capability <name>`), `source` (instructions chosen by who requested the item: user, agent or error), `prepost` (extra turns before or after the main work) and `critic` (the reviewer used by `iter critreview`).

### How is an agent's prompt put together?

The agent's `promptbody`, then the shared tooling, the capability index, the project's context files, the close-gate paragraph, the source instructions, the work item (title, id, folders, priority), any previous attempt's error, the agent's memory and the item's context files; then the main work turn, any pre/post turns, a memory turn and a self-check.

## Troubleshooting

### The engine cannot sign in. What do I do?

Engines have no token: each signs in with its own key. "no engine key" means the machine has none yet: run `iter_engine key new` (or the installer), then add the public key it prints (`iter_engine key show`) in the web page's 🔑 Engine keys screen with the engine's name. A key that is not accepted yet, or was revoked, shows in `iter_engine key status`. "the engine saw a TLS certificate that is not iter_data's" means the URL answers with another certificate than iter_data's own, and not one a public certificate authority issued for one of iter_data's names: use the URL the engine was set up with. A TLS proxy or load balancer with a public certificate in front of iter_data is fine once iter_data knows the name: make a join code for that URL (Engine keys → Add an engine), which also sets the engine up. A join code that is refused says why: used already, cancelled, expired, or made by another server.

### An item keeps bouncing at the close gate. Why?

The verifier found the result does not meet the request. Read the gate feedback in the item's history; make the request's "done when" concrete, or answer the question the item moved to. Each agent's `closegate` setting controls the bounce limit and verifier model.

### A graph edit stays "waiting to sync". Why?

No engine serving the project is online, or a running item holds a lock on the folders the edit touches (it applies when that run ends). Failed edits show the reason in the sync table.

### GraphRAG says the embedding model is not found.

Run `installers/fetch_model.sh` (or `./deploy.sh docker`, which runs it), or point `ITER_EMBED_MODEL` or `--embed-model` at the model directory.

### Where are the logs?

iter_data: `journalctl -u iter-data` on Linux, `docker logs iter` for the container. An engine: `<iter dir>/iter_engine/logs/engine.log` and `engine.err.log` (`install.sh logs` follows them). Each work item's history in the Work queue holds its runs, questions, verdicts and costs.

## Words you will meet

### What do the iter terms mean?

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
