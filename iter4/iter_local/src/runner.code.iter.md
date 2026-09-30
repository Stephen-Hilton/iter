---
id: f85558b1-6a4b-4575-b032-50c4db370c9c
name: "Test group runner"
description: "Runs a test group's scripts with a time limit and reads green, red or error from each script's exit code, then stamps the result into the group's file, so that test results are decided by the scripts alone and a broken script is never mistaken for a failing test."
simple_description: "Runs a set of automated tests and records whether they passed, failed or could not run."
level: component
owner: bespoke
teststate: inherit
children:
  codedirs:   ["{topdir}/iter_local/src/runtests.rs", "{topdir}/iter_local/src/testgroups.rs"]
  codenodes:  []
  inputs:     []
  outputs:    []
  bizreqs:    []
  techreqs:   []
  tests:      []
---

# Long Description

The Test group runner runs tests the same way every time, with no AI involved. A test group is a named list of shell scripts registered in a `<!-- iterapp:testgroups … -->` block inside a node file; each script is one test.

How it works: `iter_local/src/testgroups.rs` reads and writes that block. `parse` returns the `TestGroup`s (label, tests, last run, result, counts), `update` writes them back, and `find_files` locates the files that hold groups. `iter_local/src/runtests.rs: locate_group` finds a group by label, and `run_group` runs each script with `bash` in the group's folder under one shared time budget (default 20 minutes, `DEFAULT_GROUP_TIMEOUT_MIN`). Each script leads its own process group, so a timeout kills everything it started. The exit code alone decides the `Outcome`: 0 is green (passed), 1 is red (failed), anything else is error (the script itself broke, for example a missing tool or a timeout). A last output line of `ITER_RESULT pass=X fail=Y total=Z` gives per-test counts. `stamp_group` records the last run, result and counts in the file under a lock file, and `log_header` and `log_detail` format the run for the work item's history. Shared test programs live in `{topdir}/.iter/tests` (`shared_tests_dir`).

It is called by the iter command line (`iter runtests`, where `--broken` or `--fixed` records a claim the close gate later checks) and by the Map uploader and test sweep for each group the map allows.

Why it matters: the red/error split keeps an infrastructure problem from being filed as a code defect, and the fixed contract means an agent's claim "I fixed it" can be checked by running the same scripts.

Example: `iter runtests --group ledger --fixed` runs the ledger scripts; one exits 1, so the group is red, the claim is not upheld, and the item cannot close complete.
