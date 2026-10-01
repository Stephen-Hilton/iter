# Capability: author testgroups and test scripts

Read this before you create or edit any tests file (`*.tests.iter.md`; older
projects name it `*.testgroup.iter.md`, and both are read), or any test script
registered in one. Two different jobs use this file: DEFINING groups (what must be
proven — the plan and usecase agents) and WRITING the scripts that prove them (the
`test` agent; `testwriter` on projects that still use the older name).

## The testgroup file

Testgroups live INSIDE the object they test. The iter4 default, which the Project
graph, the test sweep's items and the code-node template all use, is a `tests/`
folder beside the node file (`$ITER_TEST_DIR`; files in an older `test/` folder
still count):

    <node dir>/tests/<slug>.tests.iter.md               the group definitions
    <node dir>/tests/*.sh                               one script per test
    {topdir}/.iter/tests/*                              shared test programs (optional; the
                                                        runner exports $ITER_TESTS_SHARED = {topdir}/.iter/tests)

A project that already keeps another layout keeps it — any location works as long
as the owner's `children.tests` (or `children.testgroups`) link finds the file. If the
project's context files fix a test layout (tier folders, registry names, a label scheme),
follow it; otherwise follow the layout the object's siblings already use.

Script paths in a `testlist` are relative to the tests file, so an entry reads
`"shell": "t01_golden_create_account.sh"` (or `"<subfolder>/t01_….sh"` when the
scripts sit in a subfolder). There is no `runs/` directory any more — run output goes to the work item
(see "Run logs" below). The tests file holds three things:

1. **Frontmatter** — `id`, `name`, `description` and `children.testpaths`
   (`["{thisfiledir}/*.sh"]`), plus `teststate: omit` only for a group that is run
   by hand (see `_teststate`).
2. **Markdown prose describing each group** — exactly what the group must prove:
   golden paths, expected errors, edge cases (a `## Planned tests` list, simplest
   first, is what the Project graph's Define tests writes). This is the most
   review-critical artifact in the flow, because it is what "done" means for the
   code.
3. **An `iterapp:testgroups` JSONL block** — one line per group, with keys
   `label`, `desc`, `auto_fix` (default `false`), `lastrun`, `result`, `counts`,
   `input_space`, `coverage` and `testlist` (see "Size the tests by the input
   space" below).

Get the current skeleton from the engine rather than writing one from memory:

    "$ITER_BIN" validate --file <path>.tests.iter.md --template

**Defining a group is not writing its tests.** A plan or usecase agent writes the
prose and the JSONL line with an EMPTY `testlist`. The test sweep files a `test`
item by itself only for an included LEAF code node (one that owns no child code
nodes) with no registered test, at most a few per sweep; a group declared by a
use case, an interface or a parent node gets no such item, so whoever defines it
files the `test` item that writes its scripts.

## Registering a test in a group's `testlist`

Each script becomes a structured entry in its group's `testlist`:

    {"id": "test02", "name": "invalid accounts",
     "desc": "rejects a set of invalid accounts", "shell": "test02.sh",
     "kind": "failure"}

Registration is what makes a test exist to the engine's sweep. An unregistered
script never runs, and a registered script that does not exist is an `iter validate`
error (`missing-test-script`). **Never delete existing tests.** An entry may carry
`"gates": false` for an inventory-style check that must be run and logged but must
not turn the group red.

## Size the tests by the input space

How many tests a group needs follows from what the code under test ACCEPTS — how
many practical permutations — not from how long the code is: a function taking one
boolean needs about two tests; one taking an open JSON document needs a
representative collection of each kind. Every group covers four kinds:

- **golden** — the expected paths; always at least one;
- **malformed** — allowable malformed, incomplete or missing inputs;
- **longtail** — rare but valid inputs (limits, sizes, odd combinations);
- **failure** — what the code must refuse, and how it refuses.

On the group's line write `input_space` (what the code accepts and roughly how many
permutations) and `coverage`, the number of tests each kind needs, e.g.
`{"golden":3,"malformed":4,"longtail":2,"failure":3}` — 0 only for a kind that
cannot apply, with the reason in `input_space`. Give every test its `kind`. The
sweep compares these targets with the registered tests and files a `test` top-up
item for a group that is unassessed, has unclassified tests, or falls short.

## The test contract (every test is a shell script)

- One script per test, in the component's test directory. The script may invoke
  anything — pytest, cargo test, curl, a mix.
- **Exit code**: `0` = ran, everything as expected (an expected-error test exits 0
  when the app correctly rejects!). `1` = ran, something unexpected. Anything
  else = the script itself broke. Never encode "expected failure" in the exit
  code — that logic lives INSIDE the script.
- **Last stdout line**: `ITER_RESULT pass=X fail=Y total=Z`.
- stderr is free-form diagnostics — make failures loud and specific there.
- Deterministic: same inputs, same result, every run. No timing dependence, no
  live network, no ordering assumptions.
- **Output files go only under `$ITER_TEST_OUT`** — a folder per group outside the
  checkout, emptied at the start of every run. Never write into the tree: nothing
  would commit it, and another item's commit could sweep it up.
- The group shares one time budget (`iter runtests --timeout-min`, default 20
  minutes); a script killed by it counts as a script error.

## Three test FLAVORS, by what declares the group

- **code node file (C4 object)**: unit/component tests of that object's behavior.
- **use-case file**: end-to-end JOURNEY tests — scripts that walk the actual user
  journey through the real participants, in order.
- **interface file**: CONTRACT-enforcement tests — scripts that assert the real
  providers' inputs/outputs against the contract's example in the interface file
  body, so drift turns red instead of silently accumulating.

## The registration chain — make sure it is complete

The DECLARING file — a `*.code.iter.md`, `*.usecase.iter.md`, or
`*.interface.iter.md` (the sweep walks all three) — must LINK its tests via
`children.tests` (older files: `children.testgroups`; paths or globs resolved
relative to the declaring file, e.g. `["{thisfiledir}/tests/*.tests.iter.md"]`). A
tests file nobody links never runs in the sweep, and the map reports it as unlinked.
With no `tests` key at all, the defaults look in `{thisfiledir}/tests/` (then `test/`) for a code
node and in `{thisfiledir}/{thisfilestem}/` for a use case or interface. A link
matching nothing does not make a leaf code node "deliberately untested": the sweep
still files a `test` item for it; only `teststate` keeps it out.

- If the declaring file's tests link would not match your new file: name the
  tests file so the existing glob finds it, or add/extend the `children.tests`
  entry (keep `children.testgroups` if the file already uses that spelling). For a
  `test` agent this is the one sanctioned write outside your codepath — you may edit
  exactly that children sub-key on your work item's declaring file, and touch
  nothing else in it. (`tests: []` declared empty is the deliberate opt-out for
  use-cases/interfaces — set it only when the item asks you to.)
- If the declared tests file does not exist: CREATE it, per the shape above.

## Run logs and fix items (decided 2026-09-08)

`iter runtests` writes NO log files under the tree (on a full run it only updates
the group's `lastrun`, `result` and `counts` in the JSONL block). Every run appends a
`log_header` row to the running work item (date, group, per-test pass/fail, no
bodies); a non-green run also appends a `log_detail` row with the failing scripts'
output (tail-capped) for whoever follows up. When the project setting
`fix_on_test_failure` is on — or a group sets `"auto_fix": true` — a non-green
FULL `iter runtests` run from an exec item or a human files a `code` item to
investigate and fix, inheriting the run's priority and usecase; an agent session
iterating on its own tests never files one. The test sweep is separate: it records
its results on the map (it does not rewrite `lastrun`/`result`/`counts` in the
file) and files one `code` item per red group whatever these settings say.

## Prove every new script LAUNCHES

Run the group once via a neutral `iter runtests` (see `_runtests`, read with
`"$ITER_BIN" capability _runtests` — neutral runs never flag anything). Failures
against unimplemented code are expected and fine (exit 1); script errors (exit
greater than 1) are yours to fix now.