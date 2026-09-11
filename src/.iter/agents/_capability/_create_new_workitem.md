# Capability: create a work item (`iter add`)

Read this file before you file a work item, every time — not from memory of what it
probably says. Creating an item is how one agent hands work to another: you describe
the work, the engine queues it, and some other agent (or the same type, later) picks
it up with none of your context except what you wrote down.

## The command

    "$ITER_BIN" add --project "$ITER_PROJECT" --file <item.json>

`$ITER_BIN` is the absolute path of the running iter executable and `$ITER_PROJECT`
is the project root that owns the work queue — the engine sets both in your
environment, so this command works from any codepath. Write `<item.json>` into
`$ITER_TEMP/` (see "Scratch files" in the shared instructions); a relative path
mints a stray temp directory wherever your working directory happens to be.

The same fields are available as flags for a one-liner, which is easier when the item
is short:

    "$ITER_BIN" add --project "$ITER_PROJECT" --type plan --usecase "<name>" \
      --title "plan: build out C4 objects for usecase <name>" \
      --mainwork "<where, what, why — the three-tier request text>"

`iter add` prints `added <workid> …` on success. Capture that workid — it is what
`--depends-on` takes.

## The JSON shape

Every key is optional except `type`, `title`, and `mainwork`. This is the full set an
agent may set:

    {
      "type": "code",                       // the target agent type (see below)
      "title": "short line a human scans in a list",
      "mainwork": "the request text — three tiers, see below",
      "codepath": "/abs/or/relative/dir",   // the item's lock scope; narrowest that owns the work
      "codepaths": [],                      // extra directories to lock, when the node declares several codedirs
      "codepath_ignore": ["test/"],         // gitignore-style subtrees carved OUT of that lock
      "priority": 23,                       // 0–99, LOWER = sooner — INHERITED from your item; omit it (see "Priority and usecase")
      "usecase": "<name>",                  // the usecase this item serves — inherited too; set only on a usecase's FIRST item
      "risk": 5,                            // 0–10
      "source": "agent: code",              // "agent: <your agent type>"
      "source_testgroup": "<label>",        // provenance: the testgroup this item exists to turn green
      "source_tests": ["test02"],           // which tests were red when the item was born (informational)
      "depends_on": ["<workid>"],           // ordering gate — see below
      "depends_on_shallow": false,          // wait for the named items only, not their descendants
      "automation": "auto",                 // how items THIS item creates are born; usually leave unset
      "model": "sonnet",                    // per-item model override — see below; omit when unsure
      "question": "",                       // raise the item AS a question instead of work to run
      "context": ["<file>"],                // files the receiving agent must read first
      "testfiles": ["<file>"],              // test files, for test-type items
      "prework": [], "postwork": []         // named prepostwork steps from .iter/prepostwork/
    }

**Never set these** — they are the engine's to write, and anything you put there is
ignored or overridden: `workid`, `state`, `created_by`, `attempts`, `output`,
`lasterror`, `times`, `git_start_commit`, `exec`, `sched`, `source_schedule`.
`iter add` also cannot create a `scheduled` item — schedules are user-created in the
webapp and the command refuses.

## Priority and usecase — inherited, not chosen (decided 2026-09-08)

Priority is 0–99, LOWER = sooner, and it is a property of a LINEAGE, not of an
item: every item you create inherits YOUR item's number exactly, and every
`usecase:<name>` tag your item carries. **Do not set `priority` on items you
create** — a number you pass is ignored and `iter add` says so. Dependencies
(`depends_on`) order the work INSIDE a lineage; the number orders lineages
against each other. The bands, for reading the queue: 0–9 do now · 10–39 one
number per usecase · 40–49 human default · 50–99 maintenance and schedules. The
one place a number is chosen is a ROOT with no creator (a human's item, or the
first item of a usecase): left blank, the engine takes the lowest unused number
in its band so two lineages never compete on one number.

`usecase` names the usecase an item serves and becomes the engine-owned tag
`usecase:<name>` (the webui shows "N of M complete" per usecase from it). You
almost never set it: it is inherited. Set `--usecase <name>` only when you file
the FIRST item of a usecase (the usecase agent does, on the plan item it opens).

## Work items you create: never set `state`

Do not set `state` on work items you create. The engine derives it from YOUR work
item's automation mode, inherited down the whole chain from the original request:
`automation: review` → your items are born `parked` (a human reviews each stage
before it runs); `automation: auto` → born `queued` (fully automated build). A
user-filed item that named no mode of its own takes
`globalsettings.default_automation` (Settings). Any `parked`/`queued` you write is
overridden — the mode, not the prompt, decides. Design every handoff to work in BOTH
modes: the documents and mainwork must stand alone whether a human reads them first
or an agent picks them up seconds later. (Guards outrank automation: `iter reject`,
the non-convergence guard, and failed dependencies land items in `parked` in any
mode, and `--question` lands an item in `question` in any mode.)

You will rarely set `automation` yourself. Setting it changes how the items your NEW
item creates are born, not how your new item is born. In particular, never pass
`--automation review` to park an item "for review" as a way of handing a decision,
a proposal, or an observation to the human: a parked pile is a review with no
reviewer. Waiting has exactly two kinds — a prerequisite (`depends_on`) or a
decision (`--question`, or `iter ask` when it blocks you now). An observation is
neither: it goes in your output under `Observations (not queued)` (shared rule
"Task focus"). Anything addressed to the human by name is a question, never a
parked item.

## Authoring `mainwork` (request) text on items you create

Each item's `mainwork` is read twice: by a human deciding whether the item should
run, and by the agent that runs it. Author it in the same three-tier shape as your
outputs:

1. Open with a few plain-language sentences: where in the codebase the item
   operates, what must change, and why — which requirement or test of the
   current mainwork it serves. If you cannot write that first sentence, the item
   is an observation for your output, not work to queue (shared rule "Task focus").
2. Then the specifics as hierarchical bullets — acceptance criteria, files,
   constraints — one line each, two max.
3. Put agent-only detail (exact commands, ids, raw listings the human should
   not wade through) at the bottom, clearly last.

The human-facing part — steps 1 and 2 — follows the shared rule "Writing for the
human who answers": state each thing rather than naming it, and say what every ID
and internal name IS the first time you use it. The `title` especially: it states
the thing and its consequence ("the intake module and the ledger disagree on an
envelope's balance, so settlement double-counts"), never just a label for it
("envelope drift").

## `codepath` — the lock scope you are handing over

`codepath` is the directory tree the receiving item owns for its run; the engine
locks it. Narrower = more parallelism. Use `codepath_ignore` to carve subtrees back
out so two items can run at once: e.g. a `code` item on `<component>` with
`"codepath_ignore": ["$ITER_TEST_DIR/"]` alongside a `testwriter` item whose codepath
IS `<component>/$ITER_TEST_DIR`. The test directory name comes from
`globalsettings.test_dir` (exported as `$ITER_TEST_DIR`); never guess it. The engine
also enforces code/testwriter scope disjointness deterministically — but write it
correctly anyway.

## `depends_on` — real ordering constraints, declared not staged

Steps with no ordering constraint get no `depends_on`: they compete on priority and
run in parallel. Declare a dependency only when step B builds on what step A
produces (A makes the tree compile, B adds code to it; A relocates a module, B
imports it from the new home).

- Create A first, capture the workid `iter add` prints, then create B with
  `--depends-on <that workid>` (or `"depends_on": ["<workid>"]`). Chains and fan-ins
  are fine — `--depends-on` repeats, and B may wait on several items.
- Dependencies must NAME EXISTING ITEMS: `iter add` resolves them against the queue
  at add time and refuses unknown ids (exit 2) — which is why a batch is created in
  dependency order. A workid or any unambiguous suffix works; the convention is the
  last 12 characters, what the webapp shows.
- A dependency is satisfied only when the item closes complete AND everything it
  created is closed complete, transitively — so depending on another PLAN item means
  "after everything that plan spawns finishes", which is usually what you want.
  `depends_on_shallow` opts out: wait for the named items' own completion only.
- A gated item never dispatches until every dependency is satisfied; a FAILED
  dependency flips the dependent to `parked` for human review. Ambiguous, unknown, or
  cyclic dependencies refuse (exit 2).
- `depends_on` composes with review-mode gating: deps on a `parked` item are declared
  but dormant — the gate applies from the moment the item is queued. A queued item
  with unmet dependencies is safe: it stays visibly queued and blocked until they
  close complete.

Never hold later slices back to create "when the first wave finishes", and never
build a plan that needs you (or a human) to sequence waves by hand. Create the whole
batch, in dependency order, in one pass.

## `model` — which model the item's agent runs on

`model` overrides the agent type's default model for this one item, at dispatch. The
valid values are `opus`, `sonnet`, `haiku`, `fable`; `iter add` refuses anything else
with exit 2 naming the valid values. The point is to spend the expensive models where
judgment lives and run mechanical work cheap.

- **Simple, well-specified, mechanical work → `"sonnet"`.** The work is fully
  described, there is one obviously right answer, and doing it is typing rather than
  deciding: comment sweeps, repointing documentation at moved files, plumbing a
  rename through the call sites, adding a field that already has a stated shape.
- **Complex work, or fuzzy requirements → `"fable"`.** The item needs the agent to
  weigh options, discover what the requirements actually imply, or design something
  that will be lived with: anything where a wrong-but-plausible answer costs more
  than the run does.
- **Unsure → omit the field.** The agent type's own default then applies, which is
  the tuned setting. Omitting is always safe; guessing wrong is not.

A plan agent filing a programme sets this per child, because it is the one agent that
knows which slices are typing and which are thinking.

## When YOUR item cannot finish until another item lands: `iter wait`

Sometimes the work you are running turns out to need something another item must
build first — an operation in a container outside your lock, a client that a
different lineage generates. Do not re-measure and re-explain that block every
attempt, and do not describe the dependency only in prose. Link it:

    "$ITER_BIN" wait --on <workid-or-suffix> [--on <another>] --reason "<what must land first>"

That makes the named items dependencies of YOUR item (deep: they and everything
they create must close complete). Then finish what you can and end your turn
listing what still waits on them as `NOT DONE:` lines. The close gate sees the
links and queues your item BEHIND them — no bounce is counted and no human is
asked — and the engine re-runs your item once they close, with the open list as
your "previous attempt" feedback. Use `iter add` first when the blocking work has
no item yet, then `iter wait --on` the id it prints.

## `check:` and `container:` tags — the engine merges repeats (built 2026-09-10)

A work item may sit in the queue for hours, and the thing that made you file it can
fire again in every later run or in another agent's session. The engine looks for a
repeat BEFORE it makes a second row, in two stages, and you feed the first one with two
tags:

    "$ITER_BIN" add --project "$ITER_PROJECT" --type code \
      --tag "check:<the rule or check that fired>" --tag "container:<the component or container it fired on>" \
      --title "..." --mainwork "..."

- **`container:<name>`** — the component, service, container or module the item is
  about. SHOULD be set whenever the item is about one identifiable thing.
- **`check:<label>`** — the short fixed label of the rule, test group or check that
  raised it (`check:rollingupdate-guarded-analysis`, `check:tests-non-green`). MAY be
  set when a named rule fired; leave it off for a defect you found by reading.

**Stage 1, exact, at create:** when BOTH tags are present and an OPEN item already
carries the same two, no new item is made. `iter add` prints
`already open: <workid> …` and exits 0: the open item was told (a `doc` row with your
request text, its `repeats` count up by one, its priority halved so repeats climb the
queue, and the `repeated` tag once repeats reach the project's threshold). Treat that
line as success: reference the workid it names, do not retry with different words. A
CLOSED twin does not block — the fault has recurred, and the new item carries a
`doc` row naming the closed one.

**Stage 2, judged, before dispatch:** every new item is compared by a short read-only
Sonnet session against open items that share its container, its check, or its lock
scope. A confident match closes the NEWER item `complete` with the tag
`dup of: <last 12 of the survivor's id>` and books the repeat on the survivor; an
unsure match leaves a "may overlap with <id>" note on both and runs both. So an item
you filed may close within a minute of being created without any agent running it —
that is the merge, not a failure; the survivor's id is in the tag.

## `source_testgroup` — carry the provenance

When you create an item because a testgroup is red — or you are escalating an item
that itself carried a testgroup — put the label on the new item
(`--source-testgroup "<label>"`). Three things depend on it: the sweep's dedup guard
(one open item per group), the webapp's run-history → work item link, and the
engine's non-convergence guard, which counts the loop's laps — the third plan born
from the same testgroup is held in `parked` for human review instead of running.

Carry `source_tests` too when you have it.

## Defect-shaped items carry a test, not prose

A work item you create may sit queued for hours and then run against a tree that has
moved on. A defect claim that could have a test gets the test written first, then the
fix item; name the failing testgroup in `mainwork` so the receiving agent can
reproduce before fixing (see `_runtests.md`). Only for genuinely untestable claims
(external infrastructure state, credentials) may an item fall back to prose: state
the claim, the check command, and "if this no longer holds, report stale and stop" in
`mainwork`.

## Raising a question instead of work

`--question "<the question>"` (JSON `"question"`) files the item in the `question`
state whatever the automation mode says: the text is a decision needed from a human,
and the item queues itself for work once someone answers it in the webapp. Use it for
a decision that does NOT block you right now — keep working and let the answer arrive
later. When the decision DOES block your current item, use `iter ask` instead
(`_ask_the_human.md`). Either way, write the question in the six-part format that
file describes — and first check it IS a question: a decision that changes what you
build, not a confirmation, sign-off or "should I proceed?" (the hard rule in that
file).

## When the add refuses

If the add refuses because the queue is full (`max_open_workitems`), report the
refused items in your output instead of retrying. Same for an exit 2 on an unknown
dependency or an invalid model — fix the call if you can, otherwise say what refused
and why in your output.
