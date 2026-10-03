---
id: e4a21852-a07e-497a-a3e4-dec6a4bc8e07
name: "Run tests and see the results"
desc: "A developer selects a node in the Project graph and presses Run tests; iter_data queues a priority-5 test work item whose shell command is iter runtests --node <id>; an engine serving the project runs it, the test runner executes each script of the test node and reads the standard result JSON from its last stdout line, the result is posted to the test node and appended to the test log, and the graph's detail panel shows the verdict and history."
creator: "stephen"
teststate: inherit
actors: ["{topdir}/global/usecases/developer.actor.iter.md"]
flowmap:
  summary: "Run tests posts graph/run_tests {node}; the server files a queued test item (agent test, priority 5, exec_shell iter runtests --node <id>). The engine claims it through POST …/next and its work runner runs the shell command in the checkout. iter runtests loads the test node, runs its scripts with bash under a time limit, combines each script's last stdout line and exit code into pass / fail / could-not-run, aggregates them, and posts the result to graph/nodes/{id}/testresult. iter_data stores it on the test node (front.last_result, timestamps.last_tested → pending_write), appends a test_log row, and files one deduplicated fix item if it is not green; the detail panel reads testlogs."
  sequence: ["{topdir}/global/usecases/developer.actor.iter.md", "{topdir}/webui/grapheditor.code.iter.md", "{topdir}/iter_data/src/testlogs.code.iter.md", "{topdir}/iter_data/src/next.code.iter.md", "{topdir}/iter_engine/src/runner.code.iter.md", "{topdir}/iter_engine/src/cli.code.iter.md", "{topdir}/iter_local/src/runner.code.iter.md", "{topdir}/iter_data/src/testlogs.code.iter.md", "{topdir}/webui/grapheditor.code.iter.md"]
  process_flow:
    - step: 1
      from: "{topdir}/global/usecases/developer.actor.iter.md"
      to: "{topdir}/webui/grapheditor.code.iter.md"
      what: "The developer selects a test node (or the node that owns it) and presses Run tests."
      plain: "The developer asks for a test run."
      evidence: "webui/graphedit.js: POST graph/run_tests"
    - step: 2
      from: "{topdir}/webui/grapheditor.code.iter.md"
      to: "{topdir}/iter_data/src/testlogs.code.iter.md"
      what: "The server files a queued work item \"Run tests: <node>\" for the test agent at priority 5, tagged tests, with exec_shell `iter runtests --project \"$ITER_PROJECT\" --node \"<id>\"` — it never runs tests itself."
      plain: "The request becomes a work item."
      evidence: "iter_data/src/testlogs.rs: run_tests"
    - step: 3
      from: "{topdir}/iter_engine/src/runner.code.iter.md"
      to: "{topdir}/iter_data/src/next.code.iter.md"
      what: "An engine serving the project gets the item from POST …/next (picked, claimed and locked by the server) and its work runner runs the shell command in the checkout with the iter shim on PATH."
      plain: "An engine picks it up."
      evidence: "iter_engine/src/engine.rs: dispatch → work.rs"
    - step: 4
      from: "{topdir}/iter_engine/src/cli.code.iter.md"
      to: "{topdir}/iter_local/src/runner.code.iter.md"
      what: "iter runtests loads the test node and runs each script in children.tests with bash under the time limit; each script's last stdout line (standard JSON, or legacy ITER_RESULT) and exit code give pass, fail or could-not-run, and the scripts are aggregated into one result."
      plain: "The scripts run and say how they did."
      evidence: "iter_engine/src/cli.rs: runtests → iter_local/src/runtests.rs: run_node; iter_core/src/testresult.rs: evaluate"
    - step: 5
      from: "{topdir}/iter_engine/src/cli.code.iter.md"
      to: "{topdir}/iter_data/src/testlogs.code.iter.md"
      what: "The result is posted to the test node. The server stores it (test, front.last_result, timestamps.last_tested; the node goes pending_write so the file records it too), appends a test_log row, and for a red or could-not-run result files one code item at priority 50 tagged check:tests-failing + container:<last 12 of the test node id>."
      plain: "The verdict is recorded."
      evidence: "POST /api/projects/{p}/graph/nodes/{id}/testresult → iter_data/src/testlogs.rs: testresult"
    - step: 6
      from: "{topdir}/webui/grapheditor.code.iter.md"
      to: "{topdir}/iter_data/src/testlogs.code.iter.md"
      what: "The node's detail panel shows the last result and the history from the test log."
      plain: "The developer sees green or red and the history."
      evidence: "webui/graphedit.js: GET testlogs?node=<id>&limit=10"
  data_flow:
    - step: 1
      from: "{topdir}/iter_local/src/runner.code.iter.md"
      to: "{topdir}/iter_data/src/testlogs.code.iter.md"
      data: "The standard result {name, id, overall_success, normal, longtail, failure, details}."
      stored: true
      plain: "The result rests on the test node and in the test log."
children:
  codedirs:  []
  codenodes: ["{topdir}/webui/grapheditor.code.iter.md", "{topdir}/iter_data/src/testlogs.code.iter.md", "{topdir}/iter_data/src/next.code.iter.md", "{topdir}/iter_engine/src/runner.code.iter.md", "{topdir}/iter_engine/src/cli.code.iter.md", "{topdir}/iter_local/src/runner.code.iter.md"]
  tests:     []
  reqs:      []
timestamps: {create: "2026-10-02 23:12:30Z", last_modified: "2026-10-02 23:12:30Z", last_tested: ""}
---
# Run tests and see the results

A developer wants to know, now, whether one part of the project passes its
tests. Tests are files in the repo: a test node (`*.test.iter.md`) lists its
scripts, and each script prints one standard JSON result as its last line.

Run tests in the Project graph does not run anything on the server — the
server has no checkout. It queues a small test item; an engine that serves
the project runs it like any other item, the test runner executes the
scripts, and the aggregated result is posted back. The test node shows the
verdict, the test log keeps the history, and a red result becomes one fix
item (a repeat failure is merged into the open item by its dedup tags).
