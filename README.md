# iter

**iter** is a long-running AI build harness. A central queue of work items is worked, around the clock, by purpose-built headless Claude Code agents that an engine spawns on whatever machine has the checkout. Agents plan, write code, write tests, deploy, and file follow-up work for each other; humans steer through a web UI: they file requests, answer questions, rule on decisions, and watch usecases move toward done.

The current generation is **V3** (`iter3/`): a central API on DynamoDB, one or more engines per project, and a thin web UI. Specification and decision log: [`src/features/iter.v3.md`](src/features/iter.v3.md). The earlier V2 engine (`src/`, file-based queue, single binary) is kept for reference; new work goes to V3.

## How it works

1. A **project** is a git checkout with an `.iter/` folder and a head file (`main.iter.md`). Its structure is declared by `*.iter.md` node files: code nodes (C4 objects), requirements, interface contracts, usecases and testgroups, linked by explicit `children` entries (the "structureV2" DAG).
2. **Work items** live centrally. Each names an agent type, a codepath (its lock scope), a priority, dependencies, and a request. Items are born `queued`, run `in-progress`, and close `complete`, or land in `question` when a human decision is needed.
3. An **engine** polls the queue, claims the best runnable item (dependencies satisfied, no lock overlap, usage caps), acquires central locks for its codepath, and runs one `claude -p` session with the agent's prompt, the shared rules, the project context and the item's request. Every run is committed and pushed. A **close gate** verifies the result before the item may complete.
4. Agents hand work to each other with the `iter` CLI (`iter add`, `iter ask`, `iter reject`, `iter doc`, `iter runtests`, `iter critreview` …), which the engine puts on their PATH.
5. **Test-driven**: every code node, usecase and interface declares testgroups; `iter runtests` is the only acceptance criterion, and a scheduled Test Loop keeps everything honest.

Priorities are 0–99, lower = sooner, and belong to a *lineage*: everything an item creates inherits its number, so a usecase runs as one block ordered inside by dependencies. Every item under a usecase carries a `usecase:<name>` tag and the UI reports "N of M complete" per usecase.

## Layout

| path | what |
|---|---|
| `iter3/iter_core` | shared types: work items, projects, engines, agents and tooling, tags, priority bands, the dependency gate, lock shapes, question widgets |
| `iter3/iter_data` | the central API (axum): storage trait with `sqlite` and `dynamodb` backends, JWT auth and roles, versioned writes, central locks, spend accounting, migrations; also serves the web UI; runs locally or as a Lambda |
| `iter3/iter_engine` | the engine binary and the agent-facing `iter` CLI: tick loop, account ladder and usage caps, prompt assembly, session continuation, close gate, spend rows |
| `iter3/iter_local` | project-local logic the engine and CLI share: testgroup parsing and the deterministic test runner, `*.iter.md` validation, marker scan |
| `iter3/webui/` | the thin static client |
| `iter3/e2e.sh` | the end-to-end suite (sqlite or dynamodb) with a fake `claude` |
| `src/.iter/agents/` | the agent prompts, shared rules and capabilities (the source for the central tooling records) |
| `src/features/` | specifications: `iter.v3.md` is the live one |
| `sampleV3/` | a small sample project |
| `src/`, `sampleV1/`, `tests/` | the V2 engine, kept for reference |

## Run it

Local, zero config (sqlite):

```bash
iter3/deploy.sh sqlite        # builds release binaries into iter3/bin, starts iter_data on :8300
open http://127.0.0.1:8300/   # admin / ITER_ADMIN_PASSWORD from .env
```

Production (DynamoDB behind a Lambda + API Gateway; AWS creds, `ITER_ADMIN_PASSWORD` and `ITER_JWT_SECRET` in the repo `.env`):

```bash
pip3 install cargo-lambda     # once
iter3/deploy_lambda.sh        # build, create/update the function, smoke-test /health
```

An engine on any machine that holds a project checkout:

```bash
# <project>/.iter/config.json names the data URL and engine; <project>/.iter/.env holds
# ITER_ENGINE_TOKEN plus one setup-token per Claude account
cd ~/dev/myproject && ~/dev/iter/iter3/bin/iter_engine --config .iter/config.json
```

Then assign the project to the engine in the web UI (engine gear → projects served). Ubuntu notes: [`iter3/UBUNTU.md`](iter3/UBUNTU.md). Detailed V3 operations: [`iter3/README.md`](iter3/README.md).

## Tests

```bash
cargo test --workspace        # unit tests
iter3/e2e.sh sqlite           # the full loop with a fake claude: locks, dependencies, close gate, stop, drain, usage caps
```

## Notes

- Agent prompts ship with `--dangerously-skip-permissions` for autonomous runs; the agents' write fence is the item's codepath and the engine's locks, not Claude's permission prompts.
- Every prompt, capability and prepost step is a markdown file; central copies live in the `agent_tooling` records so every engine assembles the same prompt.
- This repository was named `iterapp` until 2026-09-08.
