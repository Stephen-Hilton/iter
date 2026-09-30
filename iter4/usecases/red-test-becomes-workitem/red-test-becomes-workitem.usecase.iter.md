---
id: 1e847d34-a056-48b4-8f04-7c29c340d268
name: "A red test becomes one work item"
description: "A scheduled test sweep runs every test group the map allows, records each result on the map, and files exactly one fix item for each failing group, so that a broken test turns into work without anyone filing it."
teststate: inherit
children:
  codenodes:  ["{topdir}/iter_engine/src/mapsync.code.iter.md", "{topdir}/iter_local/src/runner.code.iter.md", "{topdir}/iter_data/src/graph.code.iter.md", "{topdir}/iter_data/src/api.code.iter.md"]
  tests:      []
flowmap:
  summary: "On a schedule the engine runs `iter sweep`. The sweep reads the stored map, decides which test groups may run by walking every chain from main, runs each one, records every result on the map, and files one fix item per failing group, never a second one while the first is still open."
  sequence:
  - '{topdir}/iter_engine/src/loop.code.iter.md'
  - '{topdir}/iter_engine/src/mapsync.code.iter.md'
  - '{topdir}/iter_data/src/graph.code.iter.md'
  - '{topdir}/iter_local/src/runner.code.iter.md'
  - '{topdir}/iter_data/src/api.code.iter.md'
  process_flow:
  - step: 1
    from: '{topdir}/iter_engine/src/loop.code.iter.md'
    to: '{topdir}/iter_engine/src/mapsync.code.iter.md'
    what: "When the project's Test sweep schedule comes due, the engine scheduler loop queues a copy of it and starts that copy at once, outside the agent cap and the usage holds; its shell command is `iter sweep`."
    plain: "The schedule starts a sweep."
    evidence: "iter_engine/src/engine.rs: fire_schedules; iter_engine/src/sweep.rs: sweep_verb"
  - step: 2
    from: '{topdir}/iter_engine/src/mapsync.code.iter.md'
    to: '{topdir}/iter_data/src/graph.code.iter.md'
    what: "The sweep asks the data server for the project's stored map, then walks every chain down from main to see which test groups their `teststate` setting (omit, include, block or inherit) allows to run."
    plain: "The sweep reads the map to see which tests may run."
    evidence: "GET /api/projects/{p}/graph; iter_engine/src/sweep.rs: eligible"
  - step: 3
    from: '{topdir}/iter_engine/src/mapsync.code.iter.md'
    to: '{topdir}/iter_local/src/runner.code.iter.md'
    what: "The sweep hands each allowed test group to the test group runner, which runs its scripts under a time limit and reads green, red or error from their exit codes."
    plain: "Each test group runs."
    evidence: "iter_local/src/runtests.rs: run_group"
  - step: 4
    from: '{topdir}/iter_engine/src/mapsync.code.iter.md'
    to: '{topdir}/iter_data/src/graph.code.iter.md'
    what: "The sweep sends each group's result (green, red or error, with pass and fail counts) to the data server, which records it on that test group's place in the map."
    plain: "Each result is recorded on the map."
    evidence: "POST /api/projects/{p}/graph/nodes/{id}/testresult"
  - step: 5
    from: '{topdir}/iter_engine/src/mapsync.code.iter.md'
    to: '{topdir}/iter_data/src/api.code.iter.md'
    what: "For each red group, the sweep asks the data server to create one fix item for the code agent, locked to the owning part's folders and tagged `check:tests-non-green` + `container:<group>` so that a group with a fix item already open is not filed twice."
    plain: "A failing group becomes one fix-it task."
    evidence: "iter_engine/src/sweep.rs: file_fix_item → POST /api/projects/{p}/workitems"
  data_flow:
  - step: 1
    from: '{topdir}/iter_engine/src/mapsync.code.iter.md'
    to: '{topdir}/iter_data/src/graph.code.iter.md'
    data: "The overall result, the pass/total counts, each labelled group's result with its failing checks, and the id of any fix item filed; they rest on the test group's vertex in the stored map."
    stored: true
    plain: "The verdict rests on the test group in the map."
  - step: 2
    from: '{topdir}/iter_engine/src/mapsync.code.iter.md'
    to: '{topdir}/iter_data/src/api.code.iter.md'
    data: "The fix item: the owning part's folders as its lock scope, the failing checks and their output in the request, the dedup tags and a `sweep` tag; it rests in the work queue."
    stored: true
    plain: "The fix-it task carries the failing output."
---

# A red test becomes one work item

A project wants its tests run regularly and every failure turned into work without a person watching. Every project has one Test sweep schedule, created paused by the engine; a person turns it on with Resume schedule (or `iter sweep --install-schedule --every 4h`). It cannot be deleted, only paused.

When the schedule comes due, an engine runs the sweep at once — no agent slot, no usage hold — as long as the project is Running. It reads the project's stored map and walks it from main, applying each part's test setting, to decide which test groups may run. Each allowed group runs through the test group runner, and its result (green, red or error, with counts) is recorded on the map, where the Project graph shows it.

For every red group the sweep files one fix item for the code agent, locked to the folders of the part that owns the test and carrying the failing output. The item's `check:` and `container:` tags stop a second copy: while a fix item for that group is open, later sweeps file nothing new. A group that errors (the script itself broke) is recorded but not filed, because a broken test tool is not a defect in the code.
