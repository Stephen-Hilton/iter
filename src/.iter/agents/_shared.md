# Shared instructions (all agents)

This file is appended to EVERY agent's context on every run — the store-once place
for rules that apply to all agents, every turn. Keep entries short and universal;
agent-specific guidance belongs in that agent's own file, and mechanics an agent
needs only occasionally belong in a capability file (see the index below). Files
here starting with `_` are helpers, never agent types.

## Capabilities — an index; READ the file when you need the capability

Mechanics used occasionally live in their own files instead of this one, so agents
that never use them never carry them. Each line below names a capability, says when
you need it, and gives the file to read. Use your Read tool (or `cat`) to read that
file AT THE MOMENT you need the capability, and follow what it says — never work
from memory of what it probably contains. The directory is
`$ITER_PROJECT/.iter/agents/_capability/` (`$ITER_PROJECT` is an absolute path the
engine exports into your environment); where the engine serves capabilities
centrally, `"$ITER_BIN" capability <name>` prints the same text. Anywhere a file
names a capability as `_capability/<file>.md` or just `<file>.md`, that is the
file to read — expand it to `$ITER_PROJECT/.iter/agents/_capability/<file>.md`.

- **`_create_new_workitem.md`** — creating a work item with `iter add`: the JSON
  shape, every option, how to write the item's `mainwork`, `depends_on` ordering,
  automation inheritance, `codepath` / `codepath_ignore`, and which `model` to put
  on the item. Read it before every add. Two rules bind you even before you read
  it: **never set `state`** (the engine derives it from the automation mode), and
  write `mainwork` in the same three-tier shape as your own output.
- **`_ask_the_human.md`** — `iter ask`: stopping to put a decision to a human when
  it is not yours to make (what the product should do, which trade-off the project
  lives with), plus the six-part format every question must take and the hard rule
  that a question exists only to obtain a decision that changes what you build —
  confirmations, sign-offs and "should I proceed?" are not questions. Read it the
  moment you are tempted to guess a product decision, or to ask permission.
- **`_critical_review.md`** — `iter critreview`: the synchronous critique
  subprocess. Read it whenever your mainwork asks for a critical review or a
  critique. The review is part of the work, and a nonzero exit means your item has
  already been flagged to fail — stop there. The cap is a live setting (Settings →
  `critreview_max_rounds`), currently
  at most {critreview_max_rounds} review round(s) per work item.
- **`_reject_invalid_work.md`** — `iter reject`: what to do when the problem is the
  WORK ITEM itself (out of scope, goal unclear, premise no longer true, conflicts
  with a `*bizreq.iter.md` invariant) rather than your ability to do it. Rejecting
  is not failing, and it is not asking either. Read it before you fail — or quietly
  complete — a bad item.
- **`_block_cluster_restart.md`** — `iter block --cluster-restart`: what to do when
  your work needs the cluster and it is inside the nightly restart window (02:00–06:00
  PT). Blocking is not failing and not rejecting: the item parks tagged
  `blocked-by-cluster-restart`, your attempt is given back, and the engine requeues it
  once the cluster is back up and healthy. Read it before you retry against a cluster
  that is being rebuilt.
- **`_runtests.md`** — `iter runtests`: the deterministic runner and its three
  modes — neutral runs, `--broken` (claims the defect is still present) and
  `--fixed` (claims it is resolved). A false claim flags your item as failed. Read
  it before any run that carries a claim, and before filing a defect-shaped item.
- **`_testgroup_authoring.md`** — the format law for `*.testgroup.iter.md` files
  and the shell scripts they register: the `iterapp:testgroups` JSONL block, the
  `testlist` entry shape, the exit-code / `ITER_RESULT` script contract, the three
  test flavors, and the registration chain that makes the sweep see a test at all.
- **`_iter_file_authoring.md`** — creating or restructuring a code node or
  requirement `*.iter.md` file: the frontmatter blocks, the required
  `# Long Description`, the quoting rule, and the orphan check.
- **`_interface_contracts.md`** — writing an `*.interface.iter.md`: the fixed
  section format each `kind:` demands, transport neutrality, WHAT-never-WHO-or-HOW,
  and reuse before creating a new id.
- **`_teststate.md`** — `iter teststate`: the Test Loop gate
  (`inherit` | `omit` | `include` | `block`), the nearest-flag-wins rule, and why a
  `block` refusal must never be forced or worked around.
- **`_usecase_links.md`** — `iter usecase`: editing a use-case file's
  `children.codenodes` link list from any agent, and the rule that links reflect
  what was BUILT, not what was proposed.

## Communicate clearly — a human must get it FAST

Goal: a human developer skimming your output understands the status, the
feedback, and any issues in seconds. Structure EVERY output you write (work
item outputs, observations, reports) as exactly three tiers, in this order:

1. **High-level summary** — a few sentences, no jargon, generous context. A
   reader who knows nothing about this work item must come away knowing
   (a) WHERE in the large codebase this work targets, (b) WHAT changed, and
   (c) WHY.
2. **Details** — everything else worth a human's eyes, as hierarchical
   bullets (nest sub-bullets to show structure). Short but descriptive:
   each bullet ideally fills one line, two lines at most. Numbered lists
   when order matters, bullets when it doesn't.
3. **Agent-level details** — at the BOTTOM: everything only a machine reader
   needs (exact commands run, raw test output, ids, file-by-file minutiae).
   Humans will likely never read this section; keep its content out of the
   two tiers above.

Style rules for all tiers (and everything else you write — commit messages,
docs, the `mainwork` of items you create):

- **Use specific, common words.** No jargon, no invented terms, no implied
  meanings. Call files and things by their exact names (`testgroup.iter.md`,
  not "the manifest").
- **Never leave a bare label — say the thing.** "the lock file was never deleted, so
  every later run waits forever" beats "stale lock issue". (Stated in full under
  "Writing for the human who answers" below; that section governs.)
- **Use an analogy when the concept is abstract.** One good comparison to an
  everyday thing speeds understanding more than a paragraph of precision.
- **Avoid large blocks of dense text** — they slow human readers down. Break
  them up or cut them.

## Writing for the human who answers

The cost this section removes is re-explaining: a human who cannot answer a
question, read a title, or judge an item from the text alone has to send it back and
ask an agent to explain it again. It governs every sentence a human reads —
questions (`iter ask`), work item titles and `mainwork`, your output's summary and
details tiers, reject reasons, observations, commit messages. Dense agent-to-agent
notes remain legal, but only where they are SEPARATED — see the last rule.

**Who you are writing for.** A smart engineer who is NEW to this work item, reading
it once, deciding fast. They own hundreds of moving parts across the project; a term
being their own ruling from last week does not mean they are carrying it in their
head today. Write as though they have never opened the file you are looking at —
for the minute they spend on your text, they have not.

**Never name a finding — state it.** A noun-phrase label is a chapter title the
reader then has to go and look up. A statement says the thing itself.

- Naming it: "the registrar read-route gap."
- Stating it: "the seed job's read calls are being denied, because the network
  route that is supposed to let them through names no registrar."

The test: if a line could be the *title* of a chapter, it is wrong — rewrite it as
the sentence that chapter would contain. (Wherever these files say "describe, don't
state", they mean this same rule: never leave a bare label.)

**Define every term in the sentence where it first appears.**

- Specialist words get their meaning right there, not in a later paragraph.
- Every acronym is expanded at first use — "To Be Done (TBD)".
- Every rule ID, work item ID, codename, or internal file name is followed by a
  parenthetical saying what it is: "TECH-065 (the rule that no new code container
  is created without the owner's by-name approval)". A bare ID is a lookup task you
  have handed the reader.

**No house metaphors in human-facing text.** Every project grows an internal
vocabulary — words like "door", "ceremony", "mint", "arm", "fence", "gate",
"seed", "sweep" — that reads as riddles to someone deciding fast. In questions,
titles, and summaries, say the literal thing instead: not "the setup door" but "the
setup command, which prints instructions to the operator"; not "the ceremony minted
the key" but "the key-generation procedure created the key". If a house term must
appear because a file or command is literally named after it, define it in the same
sentence like any other internal name. Plain grammar too: short sentences, one
clause each, active voice. A sentence you would have to read twice is two sentences.

**Before shaping a question, make sure it IS one.** A question exists only to obtain
a decision that changes what you build. If it could be answered "yes, go ahead"
without changing anything, do not ask it. Confirmation, sign-off, approval,
verification and "please review" are not questions — the request you are executing
WAS the sign-off. The full rule, with the four banned shapes ("should I proceed?",
"please confirm this plan", "sign off before I apply this", "is this the right
reading?") and what to do instead of each, is in `_ask_the_human.md`. Everything
from here on describes how to write a question that passes that rule.

**The shape of a question, in this exact order** (`iter ask`, and any `--question`
item):

1. **Situation** — two or three plain sentences: what thing, what happened, why it
   matters. No file paths, no line numbers, no unexplained internal names here.
2. **The decision needed** — ONE sentence, phrased so it can be answered in a word
   or two. One decision per item; two decisions are two items.
3. **Options** — two to four. Each gets a name, one sentence of what it means in
   practice, and one sentence of what it really costs (effort, risk, what it
   forecloses). An option the reader must open a file to understand is not finished.
4. **Recommendation** — one sentence saying which you would take, and why.
5. **How this is never asked again** — MANDATORY. Humans get asked very similar
   questions repeatedly; every question must also recommend the requirement update
   that would let a future agent answer it by reading instead of asking. One short
   paragraph naming the requirement entry (or decision-log row) to update — prefer
   appending to or amending an EXISTING entry over creating a new one; name the file
   and the entry, and draft the one or two sentences you would add. When the answer
   arrives, the agent that executes it writes that update in the same change — the
   answer is not done until the rule that captures it is written down.
6. **How we know** — an appendix, titled exactly that, holding the file paths, line
   numbers, commands, and measurements behind the above.

Evidence goes in the appendix, never in the opening. If the first thing the reader
meets is a path or an ID, the question has failed before it started.

**Titles and summaries obey the same rule.** A title states the thing and its
consequence; it never merely names it. Not "M08 envelope drift"; instead "the
intake module and the ledger disagree on what an envelope's balance is, so
settlement double-counts."

**Dense notes stay legal — separated, never interleaved.** Exact paths, ids,
counters, raw output, and reproduction commands are useful to the next agent. Put
them in a clearly marked trailing section — `### agent notes`, or the "How we know"
appendix — after everything a human reads. Never mixed into the human-facing text.

**The bar this must clear:** the human reads it ONCE and can answer. If they would
have to come back and ask "what does this mean?", the question failed, however
correct it was.

**Worked example — the same question, badly and well.**

BAD (three unexplained names, no situation, and the reader cannot even tell whether
they are being asked about a security change or a filing step):

    Does dropping the ninth-claim token refusal in authenticate need a delta-log
    row? (R044 adoption, TECH-077)

GOOD:

    Situation. The login check currently rejects any access token that carries
    more than eight claims (a claim is one fact a token asserts, such as who the
    holder is). Requirement R044, which we are adopting this week, replaces that
    count limit with its own per-claim validation — so the eight-claim refusal
    goes away. Whether that counts as loosening a security control decides who is
    allowed to sign it off.

    Decision needed. Is removing the eight-claim limit a security REMOVAL (yours
    alone to approve) or a security ADDITION (mine, with a logged row)?

    Options.
      - Call it a removal. The item waits until you approve it. Costs a day or
        two of queue time; costs nothing else if you agree.
      - Call it an addition and log it. R044's per-claim validation is stricter
        than the count it replaces, so the accepted surface narrows rather than
        widens; I write one row in the security change log and keep building.
        Costs nothing now, but if that reading is wrong, a loosened control ships
        unreviewed until the log is reconciled.

    Recommendation. Call it a removal and wait — the narrowing argument depends
    on R044 landing exactly as written, and it has not landed yet.

    How this is never asked again. Whichever way you rule, the deciding fact is
    whether "replaced by something stricter" counts as a removal. TECH-077 (the
    rule that security removals need the owner's approval) does not say. I would
    append one sentence to that entry in the technical requirements file:
    "Replacing a control with a demonstrably stricter one is an addition, not a
    removal, provided the replacement lands in the same change." With that
    sentence in place, the next agent facing this shape reads the rule and keeps
    building.

    How we know.
      - src/authenticate.rs:212 — the claim-count refusal.
      - reqs/techreq.iter.md — TECH-077.

## Task focus — your mainwork is the whole mission

A queue audit on one project found two thirds of the open items were side-quests —
code fixes, decision requests and guard-writing filed by agents that had been sent
to do something else. All of them were deleted unrun. The pattern: an agent notices
something real while working, and "real" feels like license to queue a fix. It is
not. Real-but-unrequested work scattered across the tree adheres to no plan and may
serve no project objective; only the human decides what gets fixed.

- Your work item's `mainwork` defines your entire mission. Done means ITS
  acceptance criteria and tests pass — not that the codepath is free of every
  defect you can see.
- Anything broken, stale, undocumented or untested that your mainwork did not ask
  about is an **observation**, not work. Record it in your output under the heading
  `Observations (not queued)`, one or two sentences each, written to "Writing for
  the human who answers" above. Do not fix it; do not queue it.
- The bar for creating a RUNNABLE item at all: its `mainwork` must begin by
  naming which requirement or test of YOUR current mainwork it exists to serve.
  If you cannot write that first line, it is an observation — this matters
  doubly in `automation: auto` lineages, where anything you add runs unattended.
- Split your own mainwork only when it genuinely cannot be completed in one item,
  and say so in your output.

This project runs test-driven development: work flows from a named set of tests /
verifiable outcomes, and every change exists to make one of them pass. A work item
that names no test or outcome it serves is queue noise, whatever its merits.

## Waiting has two kinds — a prerequisite or a decision

An item blocked on a real PREREQUISITE names it in `depends_on`; the engine holds it
visibly until the dependency closes complete. An item blocked on a DECISION is a
question: `iter ask` when it blocks you now, `--question` on a new item when it
does not (`_ask_the_human.md`). There is no third kind of waiting: never file an
item parked "for review" as a way of handing a decision or a proposal to the human.
A parked pile is a review with no reviewer. Anything addressed to the human — a
title starting "For <name>", work that is "the owner decides/approves/reads", an
item that exists to carry a proposal — is filed with `--question`: the question
state is their inbox. Put the proposal in the question body (six-part shape) and
what to do once it is answered in `mainwork`, so the answer queues the execution
automatically. Never set `state` yourself; the engine derives it (see
`_create_new_workitem.md`).

## Scratch files go in `$ITER_TEMP`

Anything you write for your own use within one work item's lifetime — a work item
draft you feed to `iter add --file`, a question file, review material, a one-off
helper script — goes in `$ITER_TEMP/`, the absolute scratch directory the engine
exports into your environment. **Never write a relative `.iter/temp/...` path**: it
resolves against your working directory and mints a stray temp tree wherever you
happen to be running. Files there are swept after `temp_file_ttl_days` (Settings),
so nothing that someone will want later belongs there.

## Lock scope and codepath_ignore

Your work item's `codepath` is your lock scope: the directory tree you own for
this run. If the item carries `codepath_ignore` patterns (gitignore-style,
relative to the codepath), those subtrees are **carved out of your scope — do not
create, edit, or delete anything under them.** Another work item may own them and
be running there right now; the engine's lock lets you both through on exactly
that promise. Reading is fine anywhere. When you create work items, use the same
mechanism to parallelize them — `_create_new_workitem.md` has the pattern.

## Write fence — the codepath bounds your writes, not just your lock

Your codepath is a write fence, not just a lock: **never create or modify any file
outside it.** The engine's codepath lock only prevents two work items whose
codepaths overlap from running at once; it cannot see the writes you make, so a
file outside your codepath may be mid-edit by another agent at any moment. Two
concurrent read-modify-write appends to the same file silently erase each other —
the lock did not and cannot stop that; only this rule does. Concretely:

- Node and requirement files for your C4 object live inside your codepath; write
  them there. A requirement that belongs project-wide goes in the global context
  files, which sit outside every codepath — changing them is the human's call, so
  raise it as an observation or a question, never an edit.
- A malformed or stale file OUTSIDE your codepath is an observation (see Task
  focus), however easy the fix looks.
- Commit with pathspecs limited to files inside your codepath.

## iter files — the FILENAME declares the nodetype (structureV2)

Every `*.iter.md` file's NAME says what it IS, via the explicit DOT RULE:
`*.nodetype.iter.md` — the nodetype segment must be preceded by a dot unless
the file has no prefix at all, lowercase, case-sensitively. Valid:
`gateway.code.iter.md`, `code.iter.md`. Invalid: `gateway_code.iter.md`,
`gateway.Code.iter.md`. The seven nodetypes:

- `*.main.iter.md` — THE project head (one per project; `$ITER_MAINFILE`)
- `*.code.iter.md` — a code node (C4 object); frontmatter `level:` says which level
- `*.bizreq.iter.md` / `*.techreq.iter.md` — requirement docs
- `*.interface.iter.md` — an interface contract (global object)
- `*.testgroup.iter.md` — test definitions (linked via a node's `children.testgroups`)
- `*.usecase.iter.md` — a use-case thread (global object)

Frontmatter supplies ATTRIBUTES, never identity: a stray `level:` key inside a
usecase file changes nothing — renaming the file is the only way to change its
nodetype. A file matching no nodetype is a plain context doc.

**Nodes join the structure ONLY through explicit `children` links** — paths or
globs in frontmatter; directory nesting alone links NOTHING. A node whose
declared glob (e.g. `{thisfiledir}/test/*.testgroup.iter.md`) later matches a
new file picks it up automatically — that is the declared link doing the work.
Files matching the naming rules but linked from nowhere land in the ORPHANAGE
(visible in `iter markers` output and the webapp) — check it if something you
created is not appearing in the tree.

Paths in links use `{placeholder}` substitution (ONE style, resolved lazily by
the engine): `{thisfiledir}` / `{thisfilestem}` / `{thisfilename}` /
`{thisfilepath}` are relative to the file the pattern appears IN; `{topdir}`
is the project top; `{interfaces}` / `{usecases}` are the global dirs. Ask the
engine, never guess: `"$ITER_BIN" resolve --project "$ITER_PROJECT"
[--node <key>] "<pattern>"` prints the resolved value(s).

**Never write any `*.iter.md` skeleton from memory.**
`"$ITER_BIN" validate --file <path> --template` prints the current
authoritative template for the nodetype named by the filename (the file need
not exist yet; an existing interface file's `kind:` steers which skeleton you
get). Fetch it when you create or restructure a file — the template is the
single deterministic source, so format changes reach every agent at once. The
authoring rules for each nodetype are in `_iter_file_authoring.md` and
`_interface_contracts.md`.

EVERY node file must begin with `---`-fenced frontmatter carrying `name:`,
`description:`, and a `children:` mapping with at least one sub-key (write the
defaults out explicitly; body-only bizreq/techreq may use `reqpaths: []`).

**Project-WIDE context** is `$ITER_MAINFILE` (the main.iter.md project head — the
first file in every agent context) plus every `globalcontextfiles` match,
colon-joined in `$ITER_CONTEXT_FILES`. The engine lists exactly those in your
spin-up context automatically. Component-local requirement files stay linked beside
their component (`children.bizreqs` / `techreqs`); a requirement that spans
components belongs in the global context files.

If a node file INSIDE YOUR CODEPATH has missing or malformed frontmatter, correct it
as part of your change and note the fix in your output — ongoing verification is
every agent's job. A malformed node outside your codepath is an observation (see
Task focus above) — the write fence applies to it like any other file. The
deterministic check is built in — after touching any iter file, run:

    "$ITER_BIN" validate --project "$ITER_PROJECT" --file <the file> --fix

`--fix` applies the safe corrections (fence normalization, quoting prose values
that contain ": "); everything else is reported for you to correct by hand.
Exit 0 = clean (info-level notes are fine); exit 1 = warn/error findings remain —
fix what is inside your codepath before finishing, observe what is not.

## Agent memory — the briefing you leave for the next agent (added 2026-09-08)

Every codepath keeps ONE file `<codepath>/<dirname>.agentmemory.iter.md`: the
briefing the last agent working there left for the next one, so orientation is
paid once instead of on every item. It is listed FIRST in your spin-up when it
exists — read it before the context files and trust it as a map (verify details
that could have moved). The engine gives code, refactor, testwriter and deploy
items a dedicated `agentmemory` step after the mainwork: overwrite the whole
file (never append a log), under 2 KB, in this order — one line on what the
codepath is; `## Where things live` (3–8 lines); `## Build and test` (the exact
commands and testgroup labels); `## Gotchas`; `## Recent changes` (at most five
entries, newest first, `<date> <item id short> — what and why`). Nothing
sensitive in it, ever. The file is an iter file by name only — it is not a node,
carries no frontmatter, and the dot rule does not apply to it.

## Session continuation — a second item may arrive in the same session (added 2026-09-08)

When your item closes complete and a QUEUED item shares your exact codepath and
usecase (and its dependencies are satisfied, and nothing more urgent is queued on
that scope), the engine may hand it to you in this SAME session instead of
starting a fresh agent — at most a few items per session. You will see a "Next
work item — same session" header: it is a different work item with its own
request, close gate, `iter` context (`$ITER_WORKID` changes) and spend record.
Keep what you learned about the codepath; re-read any file the new item changes;
never redo or undo the previous item's work.
