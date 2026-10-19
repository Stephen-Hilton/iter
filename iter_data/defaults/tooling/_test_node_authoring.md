# Capability: author test nodes and test scripts

Read this before you create or edit a test node (`*.test.iter.md`) or any script it
runs. Two different jobs use this file: DEFINING what must be proven (the plan and
usecase agents) and WRITING the scripts that prove it (the `test` agent).

## What a test node is

A test node is metadata for a set of test scripts. Its `children.tests` lists the
scripts (paths or globs); every script it matches IS a test — there is no separate
registry or test list. The node that is tested links the test node from its own
`children.tests`. Default layout (follow the project's own layout where its requirements
or the siblings show one, e.g. tier folders):

    <node dir>/<stem>.test.iter.md         the test node (stem = the tested node file's stem)
    <node dir>/tests/<stem>*.sh            its scripts: children.tests = ["{thisfiledir}/tests/{thisfilestem}*.sh"]
    {topdir}/.iter/tests/*                 shared test programs ($ITER_TESTS_SHARED), optional

Frontmatter (common keys per `_iter_file_authoring`, plus):

    teststate: inherit          # omit for a set that must only ever run by hand (see `_teststate`)
    input_space: "what the code accepts and roughly how many practical permutations"
    coverage: {normal: 3, longtail: 2, failure: 3}
    children:
      tests: ["{thisfiledir}/tests/{thisfilestem}*.sh"]

`last_result` and `timestamps.last_tested` are written by the runner — never by hand.
The body says exactly what the set must prove: a `## Planned tests` list, simplest first
(expected paths, refusals, edge cases), and a `## Gaps` list for anything the
requirements leave undecided. This is the most review-critical artifact in the flow: it
is what "done" means for the code.

**Defining tests is not writing them.** A plan or usecase agent writes the test node
and its `## Planned tests` with no scripts, and files the `test` item that writes them.
The test sweep files a `test` item by itself only for an included leaf code node with no
runnable tests (at most a few per sweep).

## The script contract

- A script may run anything (cargo test, pytest, curl, a mix) and may report one check or
  many. It runs with `bash` in the test node's folder.
- **Last stdout line** — the standard result JSON, one line:

      {"name":"<test node name>","id":"<test node id>","overall_success":true,
       "normal":{"total":3,"pass":3,"err":0},"longtail":{"total":1,"pass":1,"err":0},
       "failure":{"total":2,"pass":2,"err":0},
       "details":[{"name":"rejects empty account","bucket":"failure","pass":true,"msg":""}]}

  `$ITER_TEST_NODE_NAME` and `$ITER_TEST_NODE_ID` give the name and id. `details` is
  optional but makes a red run readable. (A legacy `ITER_RESULT pass= fail= total=` line is
  still accepted and counted as `normal`; write the JSON in anything new.)
- **Exit code**: `0` = ran, everything as expected (an expected refusal counts as a pass —
  that logic lives INSIDE the script); `1` = ran, something unexpected; anything else =
  the script itself broke (no toolchain, build failure, timeout) — recorded as "could not
  run", never as a pass or a code defect.
- stderr is free-form diagnostics: make failures loud and specific there.
- Deterministic: same inputs, same result. No timing dependence, no live network, no
  ordering assumptions. Resolve your own paths and toolchain (`~/.cargo/bin` is not on a
  non-login shell's PATH).
- **Output files go only under `$ITER_TEST_OUT`** — a folder per test node outside the
  checkout, emptied at the start of every run. Never write into the tree and never commit
  output.
- All scripts of one node share one time budget (`iter runtests --timeout-min`, default 20).

## Size the tests by the input space

How many checks a test node needs follows from what the code ACCEPTS, not from how long
it is: one boolean needs about two; an open JSON document needs a representative
collection. Three buckets, the ones the result JSON counts:

- **normal** — the expected paths, plus the malformed, incomplete or missing input the
  code must tolerate; always at least one;
- **longtail** — rare but valid input (limits, sizes, unusual encodings, odd combinations);
- **failure** — what the code must refuse, and how it refuses.

Write `input_space` and `coverage` (checks needed per bucket; 0 only for a bucket that
cannot apply, with the reason in `input_space`). The sweep compares `coverage` with the
last result's bucket totals and files a top-up item for a node that is unassessed or short.

## Three flavors, by what links the test node

- **code node**: unit/component tests of that part's behavior;
- **use case**: end-to-end journey tests that walk the real parts in order;
- **connection**: checks that every supplier of the connection meets its requirements.

## Make sure the link is complete

A test node nobody links never runs in the sweep. If the tested node's `children.tests`
does not name your test node, add it — for a `test` agent this is the one sanctioned
write outside your codepath: exactly that `children.tests` entry, nothing else in the
file. Then prove every new script LAUNCHES with a neutral `iter runtests <test node>`
(capability `_runtests`): exit 1 against unimplemented code is expected; exit >1 is yours
to fix now. Finish with `iter validate --file <test node> --fix` clean.
