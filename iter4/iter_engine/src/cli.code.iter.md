---
id: 01e11745-c2f8-403a-a620-7753bdf0d2ea
name: "iter command line"
description: "Reads the command a person or agent typed (`iter sync`, `iter add`, `iter runtests`, …), checks its arguments and hands the work to the part that does it, so that every change to the queue or the checkout goes through one checked front door."
simple_description: "The typed commands people and AI agents use to add work, ask a human a question, run tests and refresh the project map."
level: component
owner: bespoke
teststate: inherit
children:
  codedirs:   ["{topdir}/iter_engine/src/cli.rs", "{topdir}/iter_engine/src/init.rs"]
  codenodes:  []
  inputs:     ["{topdir}/interfaces/workitem-create/workitem-create.interface.iter.md"]
  outputs:    []
  bizreqs:    []
  techreqs:   []
  tests:      []
---

# Long Description

The iter command line turns a typed command into an action. Agents running inside the engine use it to file follow-up work (`iter add`), ask the human a question (`iter ask`), give up on an invalid item (`iter reject`), wait for other items (`iter wait --on`) and run tests (`iter runtests`). Developers use the same commands in a terminal inside their checkout to map the repo (`iter sync`), stamp ids (`iter ids --fix`), check node files (`iter validate`) or start a new project (`iter init`).

How it works: `iter_engine/src/main.rs` sees the `cli` subcommand and calls `cli::run` in `iter_engine/src/cli.rs`. clap parses the words into the `Verb` enum, one variant per command. `run` serves the file-only commands first (`Runtests`, `Validate`, `Markers`, `Teststate`, `Usecase`), which need no server, then the map commands (`Ids`, `Sync`, `Sweep`, `GraphApply`), which find their checkout and server with `sync::checkout_root` and `sync::conn`. The queue commands (`Add`, `Ask`, `Reject`, `Block`, `Wait`, `Doc`, `Critreview`, `Status`) read the connection the engine sets for every agent session (`ITER_DATA_URL`, `ITER_ENGINE_TOKEN`, `ITER_WORKID`; see `env`). `iter init` is `iter_engine/src/init.rs: init_project`, which writes `main.iter.md`, `.iter/config.json`, a business and a technical requirement under `reqs/`, and the `interfaces/` and `usecases/` folders, never overwriting a file without `--force`. Old V2 commands get a plain "retired" message (`retired`).

It calls the Data server client for queue writes, the Map uploader and test sweep for `ids`, `sync`, `sweep` and `graph-apply`, and the local Test group runner, Node-file checker and Node-file scanner for the file commands. The Close gate later reads what these commands recorded, such as an `iter wait` blocker or an `iter runtests --fixed` claim. Inside a run, agents reach it through the `iter` shim the Work runner writes to `.iter/bin/iter`.

Why it matters: agents never write to the database directly. A bad request is refused with the offending key named (`check_add_file`, `check_add_request`) instead of being stored half-empty.

Example: a developer types `iter sync`. `Verb::Sync` resolves the checkout and server and calls `sync::sync_verb`, which stamps missing ids, pushes the map and exits 0 on success.
