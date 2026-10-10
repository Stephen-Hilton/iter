# Capability: create a work item (`workitem_create`)

Read this file before you file a work item, every time — not from memory of what it
probably says. Creating an item is how one agent hands work to another: you describe
the work, the engine queues it, and some other agent (or the same type, later) picks
it up with none of your context except what you wrote down.

## The tool

Call `workitem_create` on the `iter` MCP server. Its arguments are the fields below
(`title`, `agent`, `request`, `codepaths`, `depends_on`, `context`, `model`, `tags`,
`usecase`, `question`, `priority`, `depends_on_shallow`); the `iter add --file` names
are accepted too (`type` for `agent`, `name` for `title`, `mainwork` for `request`,
`codepath` / `lockdirs` for `codepaths`, `blockedby` for `depends_on`). Inside a work
item it knows the calling item already — the new item becomes its child.

It answers with the new item's `id` on success. Keep that id — it is what `depends_on`
and `workitem_wait` take. (Fallback without the MCP server: `"$ITER_BIN" add --file
<item.json>` with the same fields.)

## The fields

Every field is optional except `title` and `request` (`mainwork`) — a `question` item
may leave `request` empty — and `agent` (`type`) defaults to `code`. This is the full
set an agent may set (shown with the `iter add --file` names):

    {
      "type": "code",                       // the target agent type (see below)
      "title": "short line a human scans in a list",
      "mainwork": "the request text — three tiers, see below",
      "codepath": "/abs/or/relative/dir",   // the item's lock scope; narrowest that owns the work
      "codepaths": [],                      // several directories to lock (instead of codepath), when the node declares several codedirs
      "priority": 23,                       // 0–99, LOWER = sooner — INHERITED from your item; omit it (see "Priority and usecase")
      "usecase": "<name>",                  // the usecase this item serves — inherited too; set only on a usecase's FIRST item
      "depends_on": ["<workid>"],           // ordering gate — see below
      "depends_on_shallow": false,          // wait for the named items only, not their descendants
      "model": "sonnet",                    // per-item model override — see below; omit when unsure
      "question": "",                       // raise the item AS a question instead of work to run
      "context": ["<file>"],                // files the receiving agent must read first
      "tags": [{"text": "check:<label>"}, {"text": "container:<name>"}]  // see "check: and container: tags" below
    }

(`tags` in a `--file` are objects with `text` and an optional `color`; `workitem_create`
and `iter add --tag` take plain strings instead, e.g. `tags: ["check:<label>"]`.)

That is the WHOLE set. `iter add --file` refuses any other key (it names the key and
the keys it reads), and `workitem_create` silently drops it — so the older fields
(`codepath_ignore`, `risk`, `source`, `source_testgroup`, `source_tests`, `automation`,
`testfiles`, `prework`, `postwork`, `node`) are not accepted. **Never set the engine's own
fields** — `id`, `state`, `createdby`, `attempt`, `lasterror`, `ts`, `lease`,
`exec_shell`, `sched`, `source_schedule`. `workitem_create` cannot create a `scheduled`
item — schedules are user-created in the webapp and the call is refused.

## Priority and usecase — inherited, not chosen (decided 2026-09-08)

Priority is 0–99, LOWER = sooner, and it is a property of a LINEAGE, not of an
item: every item you create inherits YOUR item's number exactly, and every
`usecase:<name>` tag your item carries. **Do not set `priority` on items you
create** — a number you pass is ignored. Dependencies
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

Do not set `state` on work items you create — you cannot. The birth state comes from
YOUR agent type's `childstate` setting (the agent record, or the project's override
of it), and every agent's `childstate` today is `queued`: the item runs as soon as
its dependencies, locks and the agent cap allow. The one exception is `question`: an
item filed with `question` is born in the `question` state whatever `childstate`
says. There is no `todo` state and no per-item automation mode. Design every
handoff to stand alone anyway: the documents and mainwork must make sense whether a
human reads them first or an agent picks them up seconds later.

## Authoring `mainwork` (request) text on items you create

Each item's `mainwork` is read twice: by a human deciding whether the item should
run, and by the agent that runs it. Author it in the same three-tier shape as your
outputs:

1. Open with a few plain-language sentences: where in the codebase the item
   operates, what must change, and why — which requirement or test of the
   current mainwork it serves.
2. Then the specifics as hierarchical bullets — acceptance criteria, files,
   constraints — one line each, two max.
3. Put agent-only detail (exact commands, ids, raw listings the human should
   not wade through) at the bottom, clearly last.

## `codepath` — the lock scope you are handing over

`codepath` (or `codepaths`) is the directory tree the receiving item owns for its run;
the engine locks it, and no other item whose lock overlaps it can start until the run
ends. It also chooses the receiving agent's node — the code node that owns the first
codepath — and so which requirements it is given (its node's and its ancestors'; see
`_shared`, "Requirements"): name the part the work is about, not a folder above it. Narrower = more parallelism. There is no carve-out: a lock on `<component>`
covers `<component>/tests` too, so a `code` item and a `test` item (old name
`testwriter`) run side by side only when their paths do not overlap — e.g. the `code`
item locks the component's source directories by name and the `test` item locks
`<component>/$ITER_TEST_DIR` (the engine exports the test directory name as
`$ITER_TEST_DIR`; it is `tests`). Each agent type's lock shape is enforced when the
item is created: a path the receiving agent may not lock can be refused with the rule
named (a project may narrow an agent's shape, for example where its `plan` items may
lock), and a path that is over-wide, or outside a `test` item's tests folders, comes
back as a warning — read it.

## `depends_on` — real ordering constraints, declared not staged

Steps with no ordering constraint get no `depends_on`: they compete on priority and
run in parallel. Declare a dependency only when step B builds on what step A
produces (A makes the tree compile, B adds code to it; A relocates a module, B
imports it from the new home).

- Create A first, keep the `id` `workitem_create` returns, then create B with
  `depends_on: ["<that id>"]`. Chains and fan-ins are fine — B may wait on several items.
- Dependencies must NAME EXISTING ITEMS: `workitem_create` resolves them against the queue
  at create time and refuses unknown ids — which is why a batch is created in
  dependency order. A workid or any unambiguous suffix works; the convention is the
  last 12 characters, what the webapp shows.
- A dependency is satisfied only when the item closes complete AND everything it
  created is closed complete, transitively — so depending on another PLAN item means
  "after everything that plan spawns finishes", which is usually what you want.
  `depends_on_shallow` opts out: wait for the named items' own completion only.
- A gated item never dispatches until every dependency is satisfied; a FAILED
  dependency keeps the dependent waiting until a human reopens the failed item or
  removes the link. Ambiguous, unknown, or cyclic dependencies are refused (the
  refusal names the loop).
- A queued item with unmet dependencies is safe: it stays queued, carrying an
  engine-written `blocked by: …` tag, until they close complete.

Never hold later slices back to create "when the first wave finishes", and never
build a plan that needs you (or a human) to sequence waves by hand. Create the whole
batch, in dependency order, in one pass.

## `model` — which model the item's agent runs on

`model` overrides the agent type's default model for this one item, at dispatch. The
valid values are `opus`, `sonnet`, `haiku`, `fable`. Nothing checks the value when the
item is created, so use exactly one of those four words. The point is to spend the expensive models where
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

## A red test node — carry its dedup tags

When you create an item because a test node is red — or you are escalating an item
that itself carried one — tag the new item `check:tests-non-green` and
`container:<test node name>`, the same pair the test sweep and `iter runtests` put on
the fix items they file. That pair is what keeps it to one open item per red test node
(see "`check:` and `container:` tags" below). Name the test node (its path) and the
failing scripts in `mainwork` too.

## Defect-shaped items carry a test, not prose

A work item you create may sit queued for hours and then run against a tree that has
moved on. A defect claim that could have a test gets the test written first, then the
fix item; name the failing test node in `mainwork` so the receiving agent can
reproduce before fixing (see the `_runtests` capability). Only for genuinely untestable claims
(external infrastructure state, credentials) may an item fall back to prose: state
the claim, the check command, and "if this no longer holds, report stale and stop" in
`mainwork`.

## Raising a question instead of work

`question` files the item in the `question`
state whatever the creating agent's `childstate` says: the text is a decision needed from a human,
and the item queues itself for work once someone answers it in the webapp. Use it for
a decision that does NOT block you right now — keep working and let the answer arrive
later. When the decision DOES block your current item, use `workitem_ask` instead
(the `_ask_the_human` capability). Either way, write the question in the shape that
capability describes (its four parts, plus the two the shared rules add).

## When the add refuses

A create is refused for: no title; no request (and no question); a request that is a
placeholder ("PLACEHOLDER…"); an unknown or ambiguous dependency id; a dependency loop;
a lock path the receiving agent's lock shape does not allow; a `scheduled` item; and,
for `iter add --file`, a key it does not read. Fix the call if you can, otherwise say
what refused and why in your output — do not retry the same call. A create can also
SUCCEED with `warnings` (a priority you passed was ignored, a codepath is an area
covering several nodes): read them and correct the next call.

## When YOUR item cannot finish until another item lands: `workitem_wait`

Sometimes the work you are running turns out to need something another item must
build first — an operation in a container outside your lock, a client that a
different lineage generates. Do not re-measure and re-explain that block every
attempt, and do not describe the dependency only in prose. Link it:

    workitem_wait — on: [the ids or last-12 suffixes], reason: what must land first

That makes the named items dependencies of YOUR item (deep: they and everything
they create must close complete). Then finish what you can and end your turn
listing what still waits on them as `NOT DONE:` lines. The close gate sees the
links and queues your item BEHIND them — no bounce is counted and no human is
asked — and the engine re-runs your item once they close, with the open list as
your "previous attempt" feedback. Use `workitem_create` first when the blocking work has
no item yet, then `workitem_wait` on the id it returns.

## `check:` and `container:` tags — the engine merges repeats (built 2026-09-10)

A work item may sit in the queue for hours, and the thing that made you file it can
fire again in every later run or in another agent's session. The engine looks for a
repeat BEFORE it makes a second row, in two stages, and you feed the first one with two
tags:

    workitem_create — agent: code, title, request,
      tags: ["check:<the rule or check that fired>", "container:<the component or container it fired on>"]

- **`container:<name>`** — the component, service, container or module the item is
  about. SHOULD be set whenever the item is about one identifiable thing.
- **`check:<label>`** — the short fixed label of the rule, test or check that
  raised it (`check:frontmatter-not-yaml`, `check:tests-non-green`). MAY be
  set when a named rule fired; leave it off for a defect you found by reading.

**Stage 1, exact, at create:** when BOTH tags are present and an OPEN item already
carries the same two, no new item is made. `workitem_create` answers
`already_open: true` with the open item's `id`: the open item was told (a `doc` row with your
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

