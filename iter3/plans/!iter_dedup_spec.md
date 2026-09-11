# Work item for the iter engine: stop the queue from filling with copies of one finding

Written 2026-09-10 by Stephen's session in pdy-dev, from a measured incident. Send as-is to the
iter agent. File references into the iter engine are from a read of `~/dev/iter/iter3` that day;
verify each before relying on it.

## What happened, and why it matters

A release script in pdy-dev goes red in dev when it cannot measure a money-path container, and
under that project's rules it records the red as a work item and carries on. The script remembers
what it has already filed in a shell variable that dies when the run ends, so every later release
of the same container filed a fresh copy. Between 05:57Z and 07:23Z on 2026-09-10 one unchanged
condition produced thirteen priority-1 work items, nine of them word-for-word twins (pdy-dev items
ending b4959893208a, e0ddf7af1226, cb243325a20c, 516288bc9664, 50f09f2bde80, 41668fa38feb,
cede178cc68f, d4368e3a175b, c4ab5179f70f). Each twin cost an agent a full session to measure that
the premise had gone stale, then cost Stephen a reading of a parked item. He closed them by hand.

The engine accepted every copy because nothing in it looks for an existing item before creating a
new one. This item adds that, in two stages, and turns a repeat into a signal instead of clutter.

## What exists today (measured 2026-09-10, `~/dev/iter/iter3`)

- **Create checks nothing.** `POST /api/projects/{name}/workitems` runs `workitem_create`
  (`iter_data/src/api.rs` ~1003): `place_new_item` picks a priority number, `normalize_new_item`
  (~923) assigns a fresh UUID, `lockshape_findings` (~951) warns about lock overlap. None of them
  compares `name`, tags or anything else against existing items. Two POSTs with byte-identical
  names make two rows.
- **`iter add` checks nothing** (`iter_engine/src/cli.rs` `add()` ~757): it reads existing items
  only to resolve `--depends-on`. `--source-testgroup` (cli.rs:76) is accepted and never used.
- **Two narrow gates exist and neither is general.** The test-failure auto-fix path
  (`cli.rs` ~430-438, `open_dup`) refuses to file when an open item has the exact same
  `Tests non-green: ...` title. Schedules do not fire while a clone is open
  (`iter_engine/src/engine.rs` ~472, `iter_core/src/sched.rs` `is_open_state` ~156).
- **The gate only warns an agent about its own children.** `gate::feedback_section`
  (`iter_engine/src/gate.rs` ~264) lists items whose `createdby` is the running item and says "do
  not file these again", on attempt 2 or later. It cannot see a sibling's items.
- **The record has no place for a key.** `WorkItem` (`iter_core/src/lib.rs` ~499) has `name`,
  `agent`, `lockdirs`, `createdby`, `requestedby`, `tags`, `source_schedule`, `ts`. No
  fingerprint field, no `duplicate` state in `STATES` (lib.rs ~35). V2's `source_testgroup`
  survives only inside a `v2` passthrough detail row (`iter_data/src/migrate.rs` ~202).
- **The design already asks for this.** `src/features/iterloop.md` ~596 lists "dedup of
  near-identical items (same type + codepath + similar title)" as open question 2.
  `src/features/TDD.md` ~250 specifies a one-open-item-per-testgroup guard the V3 record cannot
  carry.

## The identity of a repeat: check plus container

Every automated red in pdy-dev's scripts carries a short fixed label for the rule that fired. It
is the last line of each finding's request text, for example:

```
check: rollingupdate-guarded-analysis
```

The same rule words its complaint differently depending on the stage it fires from ("the release
of pdy_core_clearing (guarded profile, money path) could not be measured: ..." versus
"pdy_core_clearing is on the guarded profile and its release cannot be measured before it is
built: ..."), and the rest of each sentence carries run-specific detail. So the title is not the
identity. The pair **rule label + container name** is: it collapses the nine clearing items to one
and keeps pdy_core_authority's item, which tripped the same rule, separate.

Represent the key as two tags on the record, `check:<label>` and `container:<name>`. Tags need
no schema change, the list endpoint already returns them so any caller can pre-narrow, and tags
are the one field a closed item still accepts.

**May or must (Stephen, 2026-09-10).** The API keeps the tags optional, because every existing
caller (agents, the command line, the web app) files without them today and must not break. Who
must send them is decided per caller: pdy-dev's finding scripts always know both and MUST send
both; an agent filing an item about one container SHOULD send `container:` and MAY send `check:`
when a named rule fired; the `_create_new_workitem` prompt row says so. Items that arrive with no
key are the reason stage 2 exists.

## Stage 1: exact match at create, deterministic, no model

When a create request carries both key tags, the service looks for an **open** item (state not
`complete` or `failed`) carrying the same two tags in the same project.

- Found: do not create. Return the existing item with `"already_open": true` and HTTP 200, and
  run the merge bookkeeping below (a repeat was observed, even though no second row exists).
- Not found: create as today.
- No key tags on the request: create as today. Nothing changes for callers that send none.
- A **closed** twin with the same key does not block: the fault has recurred. Create the new item
  and append a `doc` row to it: "recurrence of <closed id>, which closed <date>".

Mirror the same check in `iter add` so the command line reports "already open: <id>" instead of
"added". Retire the special-purpose `open_dup` title match by giving the test-failure filer the
tags `check:tests-non-green` and `container:<testgroup label>`.

## Stage 2: nuanced overlap before dispatch, Sonnet, only over a narrowed set

Repeats that stage 1 cannot see: same fault, different rule label; an agent-written defect with
no label; two agents describing one problem in their own words. For these the engine runs a short
triage on each newly created item before it is dispatched.

1. **Narrow deterministically.** Candidates are open items in the same project that share the
   new item's `container:` tag, or its `check:` tag, or whose `lockdirs` overlap the new item's.
   Cap at 20, newest first. No candidates: dispatch as today, no model call.
2. **Judge with Sonnet.** One call, model `sonnet`, run the way the `explain` agent is run (an
   engine-internal agent type, name it `dedup`, read-only, short timeout). Input: the new item's
   name and request row, and each candidate's id, name and request row. Ask it first to write a
   one-line summary of each candidate, then for each candidate give exactly one of `same_fault`,
   `different`, `unsure`, with one sentence of reason. Output as JSON so the engine parses a
   verdict, never prose.
3. **Act only on confidence.** `same_fault` on exactly one candidate: merge the new item into it.
   `same_fault` on several: merge into the oldest open one and note the others on it. `unsure`:
   merge nothing; append a `doc` row to both items, "may overlap with <id>: <reason>", and
   dispatch as today. `different` everywhere: dispatch as today. A judge that fails or times out
   never blocks dispatch; log it and carry on.

The model call is never in the create path. A shell script filing from a release machine with no
model key gets stage 1 and is done in one request.

## What a merge does, in both stages

The **duplicate** (always the newer item; never an item that is in-progress, and never one that is
closed) is closed `complete` with the tag `dup of: <last 12 of the survivor id>` (for example
`dup of: f89259cb05e0`), so the working item is one click away from any list, and a `doc` row:
"Duplicate of <survivor id>: <one sentence from the judge, or 'same check and container' for
stage 1>".

The **survivor** gets, every time:

- a `doc` row "seen again <UTC time> by <requestedby or source of the repeat>", carrying the
  repeat's request text so nothing the repeat observed is lost. A repeat is evidence: nine copies
  in ninety minutes told us how often the release ran and that none of those runs fixed it;
- `repeats` incremented (new integer field on the record, default 0; use a `repeats:<n>` tag if
  a schema field is refused);
- **priority halved, rounding down, floor 1.** Smaller is more urgent in this queue. 77 goes
  77, 38, 19, 9, 4, 2, 1 over six repeats: two repeats are a nudge, five are an alarm. Stephen's
  rule, 2026-09-10. Write the halved value directly. Priority numbers are not required to be
  unique among open items; the placement code keeps them unique only as a convention that lets
  related work start and finish together, and an unrelated number slipping in affects nothing
  (Stephen, 2026-09-10);
- the tag `repeated` once `repeats` reaches the threshold (default 3, project setting
  `dedup.repeated_threshold`). An item already at priority 1 cannot move by halving, so for it
  this tag and the `repeats` count are the whole signal. (This incident's nine were at 1 because
  pdy-dev's finding helper requests priority 0 for every finding; that is being narrowed on the
  pdy-dev side to the bring-up-blocking checks the rule actually names, so most findings will
  arrive in the maintenance band and the halving will move them.)

## Tests, and proof that each test can fail

Each numbered case below is an automated cargo test in the crate that owns the behaviour. Two
rules apply to every one of them:

- **Prove the test can fail.** After it passes, deliberately break the behaviour it covers (for
  example, comment out the halving), run the test, watch it fail, then restore the code. A test
  that has never been seen failing may be passing for a reason unrelated to the feature, and
  there is no way to tell from a green run alone. Say in the closing response how each test was
  broken and what it reported.
- **No test calls a model.** For stage 2 the engine's call to Sonnet is behind a small interface,
  and the tests supply a stand-in that returns a scripted verdict (`same_fault`, `unsure`,
  `different`, or an error). That lets the merge logic be tested exactly and for free; whether
  Sonnet itself judges well is measured separately, by hand, on real items.

1. Same check and container, open twin: create returns the twin, `already_open` true, no new
   row, twin's `repeats` is 1 and priority halved.
2. Same check, different container: a new row.
3. Same key, twin closed `complete`: a new row carrying the recurrence `doc` row.
4. No key tags: a new row, no lookup, no change to the response shape.
5. Halving: 77 → 38 → 19 → 9 → 4 → 2 → 1 → 1. Priority 1 stays 1.
6. `repeated` tag appears at the threshold and not before.
7. Stage 2 narrowing: an item sharing nothing is not a candidate; container, check and lockdir
   overlap each make one; the cap holds.
8. Stand-in judge says `same_fault` on one candidate: newer merged into older, both rows carry
   the right `doc` rows and tags. `unsure`: both open, overlap notes on both. `different`:
   untouched. Judge error: dispatched as today.
9. Never merges into an in-progress or closed survivor; never closes an in-progress duplicate.
10. `iter add` prints "already open: <id>" for case 1 and exits 0.

## Done when

- Stages 1 and 2 are in the service and engine as described, every test above passes and was
  seen failing, and the response says how each was broken.
- The test-failure filer uses the tags instead of its title match.
- The engine spec (`src/features/iterloop.md` open question 2, `TDD.md` ~250) is updated to
  describe what was built, and the `_create_new_workitem` prompt row tells agents to send
  `check:`/`container:` tags when they have them and that the engine merges repeats.
- The closing response names the pdy-dev consumer that becomes a one-line change once this lands:
  pdy-dev item 3449ec72-c7ce-4a35-8c47-2239eebbe607 (`devops/script/dev_finding.sh`,
  `pdy_dev_file_workitem`), which today plans to do the stage-1 lookup itself.

## Do not

- Do not put a model call in the create endpoint or in `iter add`.
- Do not drop a repeat silently; every merge leaves the `doc` rows above.
- Do not add a `duplicate` state; the closed state stays `complete` and the `dup of:` tag says
  why and where.
- Do not merge on `unsure`, and do not let a judge failure hold an item back from dispatch.
- Do not touch existing items in any project's queue as part of this work.
