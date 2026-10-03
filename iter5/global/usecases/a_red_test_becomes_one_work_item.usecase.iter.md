---
id: 1e847d34-a056-48b4-8f04-7c29c340d268
name: "A red test becomes one work item"
desc: "The project's engine-owned Test sweep schedule runs iter sweep: it reads the project graph, decides by teststate along every chain which test nodes may run, runs each through the test runner (standard result JSON from each script's last stdout line), records every result on its test node and in the test log, and files one code fix item per failing node — deduplicated by its check: and container: tags, so a test that stays red never files a second item while the first is open."
creator: "iter migrate5"
teststate: inherit
actors: ["{topdir}/global/usecases/developer.actor.iter.md", "{topdir}/global/usecases/claude_agent.actor.iter.md"]
flowmap:
  summary: "When the Test sweep schedule comes due, the engine clones it into a run and starts it at once, outside the agent cap; its command is iter sweep. The sweep reads GET …/graph, evaluates teststate along each chain from the project node and every use case or actor, runs each eligible test node's scripts with the test runner, posts each aggregated result to the test node, and for each failing node files one code item locked to the owning node's codedirs. A repeat with the same check: + container: tags is merged into the open item. Could-not-run is recorded but files nothing. The fix item is then worked by an agent like any other."
  sequence: ["{topdir}/iter_engine/src/loop.code.iter.md", "{topdir}/iter_engine/src/sweep.code.iter.md", "{topdir}/iter_data/src/graph.code.iter.md", "{topdir}/iter_local/src/runner.code.iter.md", "{topdir}/iter_data/src/testlogs.code.iter.md", "{topdir}/iter_data/src/api.code.iter.md", "{topdir}/global/usecases/claude_agent.actor.iter.md"]
  process_flow:
    - step: 1
      from: "{topdir}/iter_engine/src/loop.code.iter.md"
      to: "{topdir}/iter_engine/src/sweep.code.iter.md"
      what: "Every project has one paused, undeletable Test sweep schedule (turned on with Resume schedule or `iter sweep --install-schedule --every 4h`). When it comes due on a Running project, the engine queues a copy and starts it at once, outside the agent cap and usage holds; its shell command is `iter sweep`."
      plain: "The schedule starts a sweep."
      evidence: "iter_engine/src/engine.rs: fire_schedules; iter_engine/src/sweep.rs: sweep_verb"
    - step: 2
      from: "{topdir}/iter_engine/src/sweep.code.iter.md"
      to: "{topdir}/iter_data/src/graph.code.iter.md"
      what: "The sweep reads the project graph and works out which test nodes may run: chains start at the project node and every use case or actor and follow codenodes and uses edges; a test node runs when its own teststate is not omit/block and at least one chain reaching an owner says include."
      plain: "The sweep reads the graph to see which tests may run."
      evidence: "GET /api/projects/{p}/graph; iter_engine/src/sweep.rs: eligible"
    - step: 3
      from: "{topdir}/iter_engine/src/sweep.code.iter.md"
      to: "{topdir}/iter_local/src/runner.code.iter.md"
      what: "Each eligible test node goes to the test runner, which runs its scripts with bash under a time limit and turns each last stdout line and exit code into pass, fail or could-not-run, aggregated per node."
      plain: "Each test node runs."
      evidence: "iter_local/src/runtests.rs: run_node"
    - step: 4
      from: "{topdir}/iter_engine/src/sweep.code.iter.md"
      to: "{topdir}/iter_data/src/testlogs.code.iter.md"
      what: "The sweep posts each aggregated result to its test node; the server stores it on the node and in the test log."
      plain: "Each result is recorded."
      evidence: "POST /api/projects/{p}/graph/nodes/{id}/testresult"
    - step: 5
      from: "{topdir}/iter_engine/src/sweep.code.iter.md"
      to: "{topdir}/iter_data/src/api.code.iter.md"
      what: "For each failing node the sweep files one code item locked to the owning node's codedirs, carrying the failing scripts' output and the reproduce / --fixed instructions, tagged with check: and container: keys; if an open item already has those keys the server records a repeat on it and returns it instead."
      plain: "A failing test becomes one fix-it task."
      evidence: "iter_engine/src/sweep.rs: file_fix_item → POST /api/projects/{p}/workitems"
    - step: 6
      from: "{topdir}/global/usecases/claude_agent.actor.iter.md"
      to: "{topdir}/iter_local/src/runner.code.iter.md"
      what: "An agent working the fix item reproduces with `iter runtests <test node> --broken`, fixes the code, and finishes with `iter runtests <test node> --fixed`, which the close gate checks."
      plain: "An agent fixes it and proves it."
      evidence: "iter_engine/src/cli.rs: runtests"
  data_flow:
    - step: 1
      from: "{topdir}/iter_engine/src/sweep.code.iter.md"
      to: "{topdir}/iter_data/src/testlogs.code.iter.md"
      data: "The aggregated standard result per test node; it rests on the test node and in the test log."
      stored: true
      plain: "The verdict rests on the test node."
    - step: 2
      from: "{topdir}/iter_engine/src/sweep.code.iter.md"
      to: "{topdir}/iter_data/src/api.code.iter.md"
      data: "The fix item: lockdirs from the owner's codedirs, the failing output in the request, dedup tags and use-case tags; it rests in the work queue."
      stored: true
      plain: "The fix-it task carries the failing output."
children:
  codedirs:  []
  codenodes: ["{topdir}/iter_engine/src/loop.code.iter.md", "{topdir}/iter_engine/src/sweep.code.iter.md", "{topdir}/iter_local/src/runner.code.iter.md", "{topdir}/iter_data/src/graph.code.iter.md", "{topdir}/iter_data/src/testlogs.code.iter.md", "{topdir}/iter_data/src/api.code.iter.md"]
  tests:     []
  reqs:      []
timestamps: {create: "2026-10-02 23:05:35Z", last_modified: "2026-10-02 23:12:30Z", last_tested: ""}
---
# A red test becomes one work item

A project wants its tests run regularly and every failure turned into work
without a person watching. Every project has one Test sweep schedule, created
paused by the engine; a person turns it on (Resume schedule, or
`iter sweep --install-schedule --every 4h`). It cannot be deleted, only
paused.

When it comes due, an engine runs the sweep at once — no agent slot, no
usage hold — as long as the project is Running. The sweep reads the project
graph, applies each node's teststate along every chain to decide which test
nodes may run, runs them, and records every result on its test node, where
the Project graph shows it.

For every failing test node the sweep files one fix item for the code agent,
locked to the code of the node the test belongs to and carrying the failing
output. Its `check:` and `container:` tags stop a second copy: while a fix
item for that test is open, later sweeps only record a repeat on it. A test
that could not run (the script itself broke) is recorded but not filed,
because a broken test tool is not a defect in the code. The sweep also files
`test` items for leaf code nodes with no tests, coverage top-ups and
node-text fixes, within its limits.
