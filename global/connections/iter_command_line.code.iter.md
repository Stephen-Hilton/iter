---
id: e5a6d3f6-a431-437c-92f8-0b5f50591e08
name: "iter command line (agent shim)"
desc: "The iter verbs as a command line: the engine installs {topdir}/.iter/bin/iter as a shim to `iter_engine cli` and puts it on every agent session's PATH with ITER_DATA_URL, ITER_ENGINE_TOKEN, ITER_PROJECT and ITER_WORKID set, so an agent (or a person in the checkout) types iter add / ask / wait / doc / runtests / validate / sync / sweep. Arguments in, text and exit code out; queue verbs forward to the HTTP JSON API with the engine's token."
creator: "stephen"
teststate: inherit
connects:
  from: ["{topdir}/iter_engine/src/cli.code.iter.md", "{topdir}/iter_engine/src/runner.code.iter.md"]
  to: ["{topdir}/map/external/claude_code/claude_code.code.iter.md", "{topdir}/iter_engine/src/provider/provider.code.iter.md"]
level: connection
children:
  codedirs:  []
  codenodes: []
  tests:     []
  reqs:      []
timestamps: {create: "2026-10-02 23:12:30Z", last_modified: "2026-10-02 23:12:30Z", last_tested: ""}
---
# iter command line (agent shim)

## Protocol

A process call: `iter <verb> [flags]`, output on stdout/stderr, exit code
0 = done. The shim (`iter_engine/src/work.rs: write_iter_shim`) is a small
script at `{topdir}/.iter/bin/iter` that runs this engine binary's `cli`
subcommand; `ITER_BIN` names it for prompts.

- **Queue verbs** (`add`, `ask`, `reject`, `block --cluster-restart`,
  `wait --on`, `doc`, `critreview`, `capability`, `status`) work only inside
  an engine-run item: they read `ITER_DATA_URL`, `ITER_ENGINE_TOKEN`,
  `ITER_PROJECT`, `ITER_WORKID` from the environment the engine set and call
  the HTTP JSON API.
- **Checkout verbs** (`init`, `validate`, `sync`, `runtests`, `sweep`,
  `rag sync`, `markers`, `teststate`, `usecase`, `migrate5`) work from any
  shell in a checkout.

## Who supplies it

The command line itself (`iter_engine/src/cli.rs`) and the work runner, which
installs the shim and sets the `ITER_*` environment of every session.

## Who connects

Agent sessions in the Claude Code CLI; the mock provider (`mock: ask` runs
`iter ask` through the shim); and the work runner itself, whose `exec` / test
items run shell commands such as `iter runtests --node <id>` and
`iter sweep`. People use the same verbs as `iter_engine cli <verb>`.

## Auth

None of its own: the queue verbs carry the engine's token, scoped by the
server to the projects the engine serves, and the calling item's id.
