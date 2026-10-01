# Capability: request a critical review (`iter critreview`)

When your mainwork asks for a critical review (or "critique"), get one
synchronously — no work items involved — BEFORE acting on the reviewed result
(e.g. a plan agent reviews its plan before creating the follow-on items):

1. Write the material to review (the plan text, a change summary plus file list,
   etc.) to a temp file OUTSIDE the checkout, e.g.
   `"${TMPDIR:-/tmp}/critique-$ITER_WORKID.md"`. (Never a relative `.iter/temp/...`
   path: it resolves against your working directory, which is inside your lock
   scope, so the end-of-run commit would sweep it in. The file need not survive the
   run — `critreview` copies the material into the item's review record.)
2. Run — and set the Bash tool call's timeout high (up to 1800000 ms); the review
   takes minutes:

       "$ITER_BIN" critreview --project "$ITER_PROJECT" --file <material.md> --context <requirements.md> ...

   `--context` repeats: give the critic every requirements file it must judge
   against, or it will judge against taste. The critic receives only the file
   NAMES and opens them itself from the project's top directory, so pass absolute
   paths (or paths relative to `$ITER_TOPDIR`).
3. The critic's verdict and numbered feedback arrive on stdout, and the round is
   recorded on your work item as a `review` row (stderr names its round number).
   Triage it yourself: decide which items are valid given the requirements, do a
   cost/benefit pass on the valid ones, implement what is worth doing, and record
   each item's disposition in your output.
4. **Report the round's outcome back — the close gate requires it.** Once you have
   acted on a round, run:

       "$ITER_BIN" critreview --project "$ITER_PROJECT" --disposition <revised|rejected|no-findings> --round <n>

   (`revised` — you changed the material; `rejected` — you judged the findings not
   worth acting on, reasons in your output; `no-findings` — nothing material came
   back. Without `--round`, the latest round.) A `review` row without a disposition
   holds the item open at the close gate: it cannot complete until every round has one.
5. After major revisions, request another review of the revised material. There is
   a cap on review rounds per work item — the shared instructions' "Requesting a
   critical review" section states the live number (an engine constant, not a
   setting); obey that number, not a remembered one. Stop earlier the moment a
   review comes back with no material findings — rounds are a budget, not a target.

Exit codes: **0** — feedback on stdout, triage it. **Any nonzero exit** — the
review could not be delivered and nothing was recorded on the item. (It tries once
by default; `--max-retry <n>` gives it more tries — one rerun with `--max-retry 2` is
reasonable.) The engine does NOT fail the item for you, so STOP yourself: do not
create work items, do not proceed without the review, and end your turn with a
`NOT DONE: critical review — critreview failed: <its error line>` line, so the close
gate sends the item back for another attempt. A requested review is part of the
work — work without it is not done.