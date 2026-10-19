# Capability: run tests and make claims (`iter runtests`)

`iter runtests` is the deterministic test runner: it runs a test node's scripts
(`*.test.iter.md`, scripts from its `children.tests`), prints each script's outcome,
records a `log_header` row on your work item (plus a `log_detail` row with the failing
scripts' output when non-green), and on a full run posts the aggregated result to the
test node in iter_data (its `last_result`, the test log, `timestamps.last_tested`). It is
the only acceptance criterion for "done" — never your own judgment that the code looks right.

    "$ITER_BIN" runtests [<test node>] [--test <script>] [--timeout-min <n>] [--broken | --fixed] [--no-record]

`<test node>` is an id (or a unique tail), a name, a file stem or a path. Left out, it is
your item's node (`$ITER_NODEFILE`): that test node, or every test node it links.
`--test` narrows a NEUTRAL run to one script; a narrowed run never records the node's
result. `--timeout-min` is the node's whole time budget (default 20); a script still
running at the limit is killed and counts as "could not run".

## Three modes: one neutral, two claims

- **Plain (neutral)** — no `--broken`, no `--fixed`. Runs and reports; it never flags
  anything. Run it freely while iterating.
- **`--broken`** — claims "the defect is still present". If the node is actually green
  the claim is false: the command parks your item at once as stale (the reason is recorded
  on it), no matter what you do next. Touch no code and stop. "Could not run" parks it
  too: it is not "the defect reproduces".
- **`--fixed`** — claims "the defect is resolved", and is the completion gate. Any red
  check or script error means the claim is false (recorded as not upheld), and the close
  gate will not complete the item until a later `--fixed` claim on the node is upheld.
  The WHOLE node must be green — a fix that breaks a neighboring check is not done.

A claim needs exactly one test node and always runs all of its scripts.

## Defect items carry their failing test node (red before fix)

A work item may sit queued for hours and then run against a tree that has moved on, so
a defect-shaped item carries the test node that proves the defect. Fix items filed by
the test sweep or by a red run are titled `Tests failing: "<test node name>" …`, quote
the failing scripts' output in their request, and carry the tags
`check:tests-non-green` + `container:<test node name>` (a red result posted from
outside a run files `check:tests-failing` + `container:<last 12 of the test node id>`).
The receiving agent reproduces BEFORE fixing — `--broken` first, then diagnose, then fix
the CODE, then `--fixed`.

A defect claim that could have a test gets the test written first, then the fix item.
Only for genuinely untestable claims (external infrastructure state, credentials) may an
item fall back to prose: state the claim, the check command, and "if this no longer
holds, report stale and stop" in `mainwork`.

## Script errors versus red checks

Exit `1` means the script ran and something was unexpected — a red check, and against
unimplemented code that is expected and fine. Exit greater than `1` means the script
itself broke: a defect in the test, not in the code. Fix script errors where you own the
tests; report them where you do not.

## Run logs, results and fix items

Runs of the SAME test node during one attempt replace that node's previous
`log_header` / `log_detail` rows, so iterate freely. The `testlogs` MCP tool lists the
results recorded for a test node, newest first. A red full run recorded by a `code`,
`refactor` or other building agent files nothing (you are iterating); one recorded by a
`test` item, a human or a schedule files one `code` fix item, deduplicated by its tags —
never file a second one by hand. `--no-record` runs without posting anything.
