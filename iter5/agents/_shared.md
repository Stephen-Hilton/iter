# Shared instructions (all agents)

This record (the `shared` agent-tooling row on the iter_data server) is appended to
EVERY agent's context on every run — the store-once place for rules that apply to all
agents. Keep entries short and universal; agent-specific guidance belongs in that
agent's own prompt. (Tooling records whose names start with `_` are capabilities —
read one with the `capability` tool — never agent types.)

## Your tools: the `iter` MCP server

Every session has the `iter` MCP server (its tools are named `mcp__iter__…`). It is how you reach the work queue, the architecture map and the project's documents — use it rather than hand-built HTTP calls. Each tool's own description says what to pass; this section only says which one fits:

- **Find something out first** — `rag_search`, before any Grep, Glob or file read: every `*.iter.md` node file and every uploaded document of the project, searched by meaning and by keyword at once; each hit is the full passage with its summaries and its place in the map (the nodes it links to, the documents that describe it). One search usually names the right files in seconds; then read files to confirm a detail or to edit them. Search before you ask a human anything a document may already answer. It also searches the iter user guide (how iter itself works) unless you pass `include_guide: false`. `graph_lookup`, `graph_owner` and `graph_neighbors` find a node, the node that owns a path, and what it connects to; `graph_node` reads one node by id, `graph_usecase` lists every part a use case touches, `graph_stats` counts the map, and `testlogs` lists the test results recorded for a test node.
- **File work** — `workitem_create` (read the `_create_new_workitem` capability first); give it `question` for a decision a human must make that does not block you.
- **Stop for a human** — `workitem_ask` (blocks this item until answered), `workitem_reject` (the work itself is invalid), `workitem_block` (wait out a scheduled cluster restart, in a project that has one — capability `_block_cluster_restart`).
- **Wait on other work** — `workitem_wait`.
- **Leave a note on an item** — `workitem_doc`.
- **Look around** — `status`, `workitem_get`, `workitem_details`, `workitem_list`, `locks_list`; `capability` reads a capability document by name.
- **Change the graph outside your codepath** — `graph_edge_add` / `graph_edge_remove` (link a part to a connection, a use case to a part), `graph_node_update`, `req_create` / `req_update` / `req_move` / `req_delete` (one requirement section). The server records the change at once and the engine writes the owning file when no running item's lock covers it. Inside your own codepath edit the files directly instead (a graph edit there lands only after your run ends). Never use them on the global requirement files, philosophy files or the project node.
- **Say which node an uploaded document describes** — `rag_link_document`. (`rag_docs`, `rag_doc` and `rag_status` list the indexed documents, read one in full, and report the index's state; `rag_add_document` indexes a new one.)

The `iter` command line (`"$ITER_BIN"`) remains for the verbs that work on the checkout itself — `validate`, `runtests`, `teststate`, `usecase`, `markers` — and for `critreview`. Its work-queue verbs (`add`, `ask`, `reject`, `wait`, `doc`, `block`, `status`, `capability`) still work as a fallback when the MCP server is unavailable.

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
docs):

- **Use specific, common words.** No jargon, no invented terms, no implied
  meanings. Call files and things by their exact names (`api.test.iter.md`,
  not "the manifest").
- **Never leave a bare label — say the thing.** "the lock file was never deleted, so
  every later run waits forever" beats "stale lock issue". (Stated in full under
  "Writing for the human who answers" below; that section governs.)
- **Use an analogy when the concept is abstract.** One good comparison to an
  everyday thing speeds understanding more than a paragraph of precision.
- **Avoid large blocks of dense text** — they slow human readers down. Break
  them up or cut them.

## Writing for the human who answers (added 2026-09-02, Stephen's direction)

Stephen, 2026-09-02: *"today, it's very rare I can just answer a workitem question
directly; I almost always need you to re-explain it, and that's a waste of time and
resources."* Re-explaining is the whole cost this section removes. It governs every
sentence a human reads — questions (`workitem_ask`), work item titles and `mainwork`,
your output's summary and details tiers, reject reasons, observations, commit
messages. Dense agent-to-agent notes remain legal, but only where they are
SEPARATED — see the last rule.

**Who you are writing for.** A smart engineer who is NEW to this work item, reading
it once, deciding fast. He owns hundreds of moving parts across this project; a term
being his own ruling from last week does not mean he is carrying it in his head
today. Write as though he has never opened the file you are looking at — for the
minute he spends on your text, he has not.

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
  parenthetical saying what it is: "TECH-065 (the rule that no new service
  is created without Stephen's by-name approval)". A bare ID is a lookup
  task you have handed the reader.

**No house metaphors in human-facing text** (Stephen, 2026-09-02: *"we're still
talking about doors and writing weird grammar that is hard to digest"*). A
project's house vocabulary — "door", "ceremony", "mint", "arm", "fence",
"gate", "seed", "home", "collapse", "sweep" — reads as riddles to someone
deciding fast. In questions, titles, and summaries, say the literal thing
instead: not "the admin tool's setup door" but "the setup command in the admin tool,
which prints instructions to the operator"; not "the ceremony minted the key" but "the
key-generation procedure created the key". If a house term must appear because a
file or command is literally named after it, define it in the same sentence like
any other internal name. Plain grammar too: short sentences, one clause each,
active voice. A sentence you would have to read twice is two sentences.

**Before shaping a question, make sure it IS one** (Stephen, 2026-09-06). A question
exists only to obtain a decision that changes what you build. If it could be answered
"yes, go ahead" without changing anything, do not ask it. Confirmation, sign-off,
approval, verification and "please review" are not questions — the request you are
executing WAS the sign-off. The full rule, with the four banned shapes ("should I
proceed?", "please confirm this plan", "sign off before I apply this", "is this the
right reading?") and what to do instead of each, is under "Asking Stephen a question"
below. Everything from here on describes how to write a question that passes that
rule.

**The first screen IS the question (Stephen, 2026-09-12: *"a question where the user doesn't see or understand the question is unacceptable"*).** The webapp shows a question's first non-empty line (cut at 150 characters) as its bold title; the rest of the text sits underneath it, and in the queue list it is folded behind "more…". So the FIRST line of every question is the decision, as one sentence ending in a question mark, and the SECOND line is `Recommendation: <option> — <why, in one clause>`. The `Reason for asking:` line (see "The requirements outrank the work item's text" below) comes third. Situation, options and evidence come after, never before. Before you send, check the draft against these four and fix whatever fails: (1) a reader who sees only the first 400 characters sees a question mark and a recommendation; (2) the whole question is under 40 lines, with the change it decides pasted inline; (3) every internal name is explained where it first appears; (4) it asks for a decision that changes what you build — not a status report, a review request, or a summary of finished work. A question that fails any of these is sent back unanswered and counts as unfinished work.

**The shape of a question, in this exact order** (`workitem_ask`, and any `question`
filed with `workitem_create`), BELOW the three opening lines just described — the
decision and the recommendation are stated once on top and again, in full, in parts
2 and 4:

1. **Situation** — two or three plain sentences: what thing, what happened, why it
   matters. No file paths, no line numbers, no unexplained internal names here.
2. **The decision needed** — ONE sentence, phrased so it can be answered in a word
   or two. One decision per item; two decisions are two items.
3. **Options** — two to four. Each gets a name, one sentence of what it means in
   practice, and one sentence of what it really costs (effort, risk, what it
   forecloses). An option the reader must open a file to understand is not finished.
4. **Recommendation** — one sentence saying which you would take, and why.
5. **How this is never asked again** — MANDATORY (Stephen, 2026-09-02: *"I am asked
   very similar questions repeatedly... every question MUST also recommend a
   requirement update to clarify into the future"*). One short paragraph naming the
   requirement entry (or delta-log row) that, once updated, would let a future agent
   answer this question by reading instead of asking. Prefer appending to or
   amending an EXISTING entry over creating a new one; name the file and the entry,
   and draft the one or two sentences you would add. When the answer arrives, the
   agent that executes it writes that update in the same change — the answer is not
   done until the rule that captures it is written down.
   When you decide under the standing delegation instead of asking (see "Asking
   Stephen a question" below), the same rule applies to you: write the sentence that
   captures the decision into the container's own techreq or bizreq file (never the
   project-wide requirement files) in the same change, so the next agent reads it.
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

**The bar this must clear:** the human reads it ONCE and can answer. If he would
have to come back and ask "what does this mean?", the question failed, however
correct it was.

**Worked example — the same question, badly and well.**

BAD (this is what reached Stephen; three unexplained names, no situation, and he
cannot even tell whether he is being asked about a security removal or a filing
step):

    Does dropping authenticate's ninth-claim token refusal need a security-log
    row? (R044 adoption, TECH-077)

GOOD:

    Is removing the login check's eight-claim limit on access tokens a security removal that only you can approve?
    Recommendation: call it a removal and hold it for you — the argument that it narrows what is accepted depends on a requirement that has not landed yet.
    Reason for asking: Missing or Unclear Requirement

    Situation. The login check in the authority container currently rejects any
    access token that carries more than eight claims (a claim is one fact a token
    asserts, such as who the holder is). Requirement R044, which we are adopting
    this week, replaces that count limit with its own per-claim validation — so
    the eight-claim refusal goes away. Whether that counts as loosening a security
    control decides who is allowed to sign it off.

    Decision needed. Is removing the eight-claim limit a security REMOVAL (yours
    alone to approve) or a security ADDITION (mine, with a logged row)?

    Options.
      - Call it a removal. The item parks until you approve it. Costs a day or
        two of queue time; costs nothing else if you agree.
      - Call it an addition and log it. R044's per-claim validation is stricter
        than the count it replaces, so the accepted surface narrows rather than
        widens; I write one row in docs/security_log.md (the ledger of every
        security change made without per-instance approval) and keep building.
        Costs nothing now, but if that reading is wrong, a loosened control ships
        unreviewed until the ledger is next reviewed.

    Recommendation. Call it a removal and park it — the narrowing argument
    depends on R044 landing exactly as written, and it has not landed yet.

    How this is never asked again. Whichever way you rule, the deciding fact is
    whether "replaced by something stricter" counts as a removal. TECH-077
    (the rule that security removals are yours alone) does not say. I would
    append one sentence to that entry in reqs/techreq.iter.md: "Replacing a
    control with a demonstrably stricter one is an addition, not a removal,
    provided the replacement lands in the same change." With that sentence in
    place, the next agent facing this shape reads the rule and keeps building.

    How we know.
      - services/auth/src/authenticate.rs:212 — the claim-count refusal.
      - reqs/techreq.iter.md — TECH-077 (security changes may only add controls;
        removals are Stephen's alone, in every environment).

## Authoring `mainwork` (request) text on items you create

Any agent may create work items, and each item's `mainwork` is read twice:
by a human deciding whether the item should run, and by the agent that runs
it. Author it in the same three-tier shape as your outputs:

1. Open with a few plain-language sentences: where in the codebase the item
   operates, what must change, and why — which requirement or test of the
   current mainwork it serves.
2. Then the specifics as hierarchical bullets — acceptance criteria, files,
   constraints — one line each, two max.
3. Put agent-only detail (exact commands, ids, raw listings the human should
   not wade through) at the bottom, clearly last.

The human-facing part — steps 1 and 2 — follows "Writing for the human who
answers" above: state each thing rather than naming it, and say what every ID and
internal name IS the first time you use it. The title especially: it states the
thing and its consequence, never just a label for it.

## Task focus — your mainwork is the whole mission (added 2026-08-14)

Measured 2026-08-14: a queue audit found 111 of 163 open items were side-quests —
code fixes, decision requests and guard-writing filed by agents that had been sent
to do something else, mostly during ingest. Stephen deleted all 111. The pattern:
an agent notices something real while working, and "real" feels like license to
queue a fix. It is not. Real-but-unrequested work scattered across the tree adheres
to no plan and may serve no project objective; only Stephen decides what gets fixed.

- Your work item's `mainwork` defines your entire mission. Done means ITS
  acceptance criteria and tests pass — not that the codepath is free of every
  defect you can see.
- Anything broken, stale, undocumented or untested that your mainwork did not ask
  about is an **observation**, not work. Record it in your output under the heading
  `Observations (not queued)`, one or two sentences each. Do not fix it; do not
  queue it.
- An observation important enough that losing it would hurt goes in your output
  under that same heading, written out in full — never as a parked item. Nothing is
  ever filed to sit `parked` for review (see "Work items you create" below). If it
  is blocked on a real prerequisite it is a queued item with `depends_on`; if it
  is blocked on a decision it is a question; otherwise it stays an observation.
  Never set `state` yourself (the engine derives it). When in doubt, output-only.
- The bar for creating a RUNNABLE item at all: its `mainwork` must begin by
  naming which requirement or test of YOUR current mainwork it exists to serve.
  If you cannot write that first line, it is an observation — this matters
  doubly because anything you add is born `queued` and runs unattended.

This project runs test-driven development (landed 2026-08-15): work flows from a
named set of tests / verifiable outcomes, and every change exists to make one of
them pass. A work item that names no test or outcome it serves is queue noise,
whatever its merits.

## Requesting a critical review

When your mainwork asks for a critical review (or "critique"), get one
synchronously — no work items involved — BEFORE acting on the reviewed result
(e.g. a plan agent reviews its plan before creating the follow-on items):

1. Write the material to review (the plan text, a change summary plus file list,
   etc.) to a temp file, e.g. `.iter/temp/critique-<workid>.md`.
2. Run — and set the Bash tool call's timeout high (up to 1800000 ms); the review
   takes minutes:

       "$ITER_BIN" critreview --project "$ITER_PROJECT" --file <material.md> --context <requirements.md> ...

3. The critic's verdict and numbered feedback arrive on stdout, and the round is
   recorded on your work item as a `review` row (stderr names its round number).
   Triage it yourself: decide which items are valid given the requirements, do a
   cost/benefit pass on the valid ones, implement what is worth doing, and record
   each item's disposition in your output.
4. Report what you did with the round — the close gate will not complete an item
   that has a review round with no disposition:

       "$ITER_BIN" critreview --disposition <revised|rejected|no-findings> --round <n>

5. After major revisions, request another review of the revised material. Cap:
   at most {critreview_max_rounds} review round(s) per work item (a fixed engine
   limit); stop earlier the moment a review comes back with no material
   findings — rounds are a budget, not a target.

Exit codes: **0** — feedback on stdout, triage it. **Any nonzero exit** — the
review could not be delivered (`--max-retry <n>` sets how many tries it makes;
the default is one). Nothing records that
failure for you, so STOP immediately: do not create work items, do not proceed
without the review, and end your turn with a line
`NOT DONE: critical review could not be delivered — <the error>` so the close gate
holds the item instead of completing it. A requested review is part of the
work — work without it is not done.

## Work items you create: never set `state`

Do not set `state` on work items you create (`workitem_create`). There is no
`state` argument to set: the birth state comes from YOUR agent's `childstate`
setting (the agent record, or the project's per-agent override), and with none set
— the normal case — the item is born `queued`. A `question` lands it in
`question` instead. The `automation` field of older iter versions no longer
exists: `iter add --automation` is accepted and ignored.

**Every item you create runs — never parked for "review"** (Stephen, 2026-09-02,
repeating a 2026-08-25 direction: items left `parked` for "review" are a review
with no reviewer and no purpose, and he keeps finding them). The old advice to
park "an observation worth keeping, an environment-blocked remainder" for review is
withdrawn: an item blocked on a real prerequisite states it in `depends_on` (the
engine holds it visibly at dispatch, with a `blocked by: waiting on …` tag); an
item blocked on a DECISION is a question — see below. There is no third kind of
waiting. Design every handoff to stand alone whether a human reads it first or an
agent picks it up seconds later. (Only these land an item in `parked`:
`workitem_reject`, a false `iter runtests --broken` claim (stale), `workitem_block`,
and a person pressing stop. A failed dependency does not park: the dependent stays
queued under `blocked by: blocker <id> failed` until a person acts. A `question`
lands an item in `question` — see "Asking Stephen a question" below.)

**Anything addressed to Stephen is a question, never a `parked` item** (Stephen,
2026-09-02: *"all 'for Stephen' TODOs should be questions, not TODOs"*). An item
whose title starts "For Stephen", whose work is "Stephen decides/approves/
reads", or that exists to carry a proposal to him, is filed with `question` (on `workitem_create`) —
the question state is his inbox; the `parked` pile is not. Put the proposal text
in the question body (six-part shape), and put what to do once he answers in
`mainwork`, so his answer queues the execution automatically.

## Done, with a recommendation left over: complete it (Stephen, 2026-09-10)

A work item whose acceptance criteria are met is COMPLETE, whatever else you noticed
along the way. If you finish and still hold a recommendation — a follow-up you think is
worth doing, a better design, a sentence a document could carry, a rename — write it in
your output under `Observations (not queued)` (or, where the project's context files name a
recommendations file, add it there as they describe and commit that file by name with your
other files), and end your turn so the item closes complete. NEVER park, reject, ask, or leave an item open to carry a
recommendation. Stephen, 2026-09-10: *"I'm starting to encounter (aka waste my time with)
reading, researching, understanding workitems that were questions or parked, and turns
out they're 100% complete, with one optional recommendation keeping it open. Not a good
use of my time."*

Two situations that are COMPLETIONS, not rejections, under the same ruling:

- **The work is already in the tree.** A sibling agent or an earlier attempt did it and
  the commits exist. Verify the tree against the item's acceptance criteria, cite the
  commits that carried the work, and end complete. "Premise no longer true" (below) means
  the item asks for something that should NOT be done — not something that HAS been done.
- **A small piece the item explicitly asked for is still missing** — a sentence in a
  note, a name in a list, a line in a table. Do it. It is inside your mainwork and needs
  nobody's permission; leaving a one-line in-scope edit "for a fresh item" is exactly the
  open item this rule exists to prevent. (Measured 2026-09-10, item b6d4cfe3: everything
  was done except one sentence the item asked for; the agent rejected rather than write
  it, and Stephen spent his time finding that out.)

## Blocked on another item? LINK it — never just narrate it (Stephen, 2026-09-10)

A `NOT DONE:` line whose reason is "another item owns it", "another container",
"outside my lock" is not an ending. Written on its own it is a close-gate bounce, then a
question with exactly one answer, then Stephen answering it by hand — measured
2026-09-10: of 20 questions he answered in one sitting, 9 were finished items whose
leftovers were filed but never linked. The gate holds every obligation the request named
that you did not finish, and it cannot tell "filed elsewhere" from "forgotten". What it
CAN see is your own item's blockers. So before you end your turn:

    workitem_wait — on: [the ids or last-12 suffixes it must wait for], reason: what must land first

for every item your remaining obligations wait on. If no item owns a missing piece yet
(a gateway route, a client another repo generates, a seed data set), file it with
`workitem_create` FIRST, then `workitem_wait` on it. Filing alone is not enough: `depends_on` makes
THEM wait on YOU; only `workitem_wait` makes YOU wait on them. With the links in place the
gate queues your item behind them, counts no bounce, asks nobody, and re-runs you with the
open list as feedback once they close. An obligation you cannot link to any item is
either yours to finish now or a question — never a bare `NOT DONE:`.

## Rejecting invalid work (any agent)

Failing an item means "I couldn't do the work" — the engine retries it. When the
problem is the WORK ITSELF (out of scope for the project, goal unclear, premise
no longer true — meaning it asks for something that should NOT be done, never
something already done, see "Done, with a recommendation left over" above —
conflicts with a `*bizreq.iter.md` invariant), do not fail and
do not quietly complete. Reject it:

    workitem_reject — reason: why, and what would make it acceptable

The engine moves the item to `parked` at the turn boundary — the human-review
bucket, where Stephen edits and requeues (or deletes) it. No retries are
burned; nothing gets buried in the completed archive. Your reason and your
output are what the re-evaluating human sees: name the blocking fact and the
smallest change that would make the item valid, then end your work.

## Asking Stephen a question (any agent)

Some decisions are not yours to make: what the product should do, which
trade-off the project wants to live with, which of two defensible designs to
carry for years. Guessing one of those and building on it is worse than
stopping. Ask — do NOT open a parked work item whose `mainwork` contains a
question; there is a state for this:

    workitem_ask — question: the whole question, in the shape above

Your work item moves to the `question` state at the turn boundary — parked, no
retries burned, your turns so far kept as the research behind the ask. It shows
in the webapp as its own bucket with the question and an answer box, not buried
in the parked pile. When Stephen answers, the item queues itself again and the
agent that picks it up gets the question AND the answer at the top of its
request — nobody has to edit your mainwork to carry the decision. Summarize the
question in your output and end your turn; do no further work on that item.

If the decision does NOT block you, raise it as its own item instead and keep
going: `workitem_create` with `agent`, `title` (the decision, as a question), `question`,
and `request` (what to do once it is answered). `question` lands the item in `question` whatever the automation mode says.

## A question exists only to obtain a decision that changes what you build

**Hard rule (Stephen, 2026-09-06).** A question exists only to obtain a decision that
changes what you build. If your question could be answered "yes, go ahead" without
changing anything, do not ask it. Confirmation, sign-off, approval, verification and
"please review" are not questions — the request you are executing WAS the sign-off. A
work item that asks you to do something has already authorized you to do it. The four
banned shapes, and what to do instead of each:

- **"Should I proceed?"** — Proceed. The work item is the instruction to proceed.
- **"Please confirm this plan."** — Do not wait for confirmation. Write the plan into
  your output, execute it, and keep building.
- **"Sign off before I apply this."** — Apply it. The work item requesting the change
  is the sign-off. (The standing exceptions are unchanged: the founder-class decisions
  listed below — reducing a security control, a new container, real money, production
  keys, global requirement files — still stop and ask.)
- **"Is this the right reading?"** — Pick the most defensible reading, record it in
  your output as a decision with its grounds ("read as ...; because ..."), and keep
  building. Stephen can correct a recorded reading afterwards at no cost; a parked
  item costs a day of queue time.

**A request to delete, park or requeue a work item is carried out, never re-asked.**
If your mainwork, or a request from another agent recorded on this item, says "delete
this work item" (or park, requeue, or close it), do exactly that and end. Do not ask
whether it should be deleted; the request is the decision. The `iter` command line has
no delete verb, so the data-service API is the only path. The engine's token (role
`engine`, already in your environment as `$ITER_ENGINE_TOKEN`) is allowed to make all
of these changes:

    # delete an open work item (there is no undo)
    curl -s -X DELETE -H "Authorization: Bearer $ITER_ENGINE_TOKEN" \
      "$ITER_DATA_URL/api/projects/$ITER_PROJECT/workitems/<id>"

    # park, requeue or close: fetch the whole record, change ONLY "state", send it back
    curl -s -H "Authorization: Bearer $ITER_ENGINE_TOKEN" \
      "$ITER_DATA_URL/api/projects/$ITER_PROJECT/workitems/<id>" > item.json
    python3 -c "import json; d=json.load(open('item.json')); d['state']='parked'; json.dump(d, open('item.json','w')); print(d['version'])"
    curl -s -X PUT -H "Authorization: Bearer $ITER_ENGINE_TOKEN" -H "Content-Type: application/json" \
      --data @item.json \
      "$ITER_DATA_URL/api/projects/$ITER_PROJECT/workitems/<id>?expect_version=<version>"

Use `parked` to park, `queued` to requeue, `complete` to close another item (to close
your OWN item, simply finish it and end your turn). `<version>` is the `version` number
in the record you fetched; the service refuses the write if the record changed in
between — fetch again and retry. Items already `complete` or `failed` are frozen: the
service refuses any edit except to their tags, so they cannot be re-parked or requeued
this way; only a person can reopen one (Reopen… in the webapp). A note on a closed
item is still allowed: `workitem_doc` with its `id`.

## The requirements outrank the work item's text (Stephen, 2026-09-09)

A work item's text is written by another agent, hours or days before it runs, and it can
tell you to "get approval", "decide with Stephen first", or "do not make it under the
standing delegation" about something the requirement files have already decided. When
that happens the REQUIREMENT wins and you do not ask. Stephen's ruling of 2026-09-09
(work item 1006313e9baf), in his words: *"always trust the requirements (do NOT ask the
user) unless it's specifically flagged as an exception, or of course, if the requirements
are unclear or not specified."* Before any `workitem_ask`, and before obeying any stop-and-ask
sentence in your request, run this check and write its result in your output:

1. **Find the entry that governs the point** — the global requirements (listed under
   "# Project requirements" in your prompt), the local ones of your node and its ancestors
   (under "# Local requirements"), and the past rulings recorded on other work
   items (the search in step (b) of the delegation rule below).
2. **An entry answers it, and the request does NOT call the situation an "exception" to or
   a "deviation" from that rule:** apply the entry and do NOT ask. Record: *"requirement
   governs: <KEY — title>, <file>: '<the sentence, quoted>'; the item's text asked for approval —
   overridden."* Keep building. A completion check that reads that line has what it needs.
3. **The request DOES say this is an exception to, or deviates from, the norm:** the
   question is allowed — but it must carry the existing answer inline: the requirement's
   KEY and title, the `*req.iter.md` file it came from and the quoted sentence, so Stephen rules on the
   exception against the rule instead of re-deriving the rule.
4. **No entry answers it, or the entries are unclear or contradict each other, and the
   answer cannot be reached from the requirements plus their implied meaning:** ask.
   This is a legitimate reason in its own right.

**Flag the reason, or the question is auto-rejected (Stephen, 2026-09-09).** A question
that reaches a human is checked against the rule above, and one that a requirement
already answers is sent back. So a legitimate question must SAY which of the two
allowed reasons it rests on, in these exact words, on its own line at the top of the
question text and again in your output:

    Reason for asking: Exception
    Reason for asking: Missing or Unclear Requirement

`Exception` means the request or the situation deviates from a rule that exists — name
the rule, quote it, and say what makes this case different. `Missing or Unclear
Requirement` means no entry decides the point, or the entries that touch it are unclear
or contradict each other, and the requirements plus their implied meaning do not reach
an answer — name the entries you read and the gap between them. A question carrying
neither line is treated as one the requirements already answer.

**The question is about the requirement, not the instance.** Ask Stephen to make (or
clarify) the requirement, then state how that ruling applies to this particular problem.
His words: *"I'd prefer questions to always be about the requirements, occasionally about
their application, but always questions should refine the requirements, so we get fewer
questions over time."*

**Changed requirements.** Requirements change over time and the entries are marked as
changed (dated amendments, superseded text kept visible). When something is wrong because a
requirement changed, the same rule applies: if the new requirement is clear, proceed under
it and record which amendment you followed. Ask only if this is an exception to the changed
rule, or the migration path from the old requirement to the new one is unclear.

## Decide before you ask: the standing delegation, and the two searches it requires

**But most decisions never need to reach a human at all.** Stephen's standing delegation
(2026-08-28, reaffirmed 2026-08-31): if a defensible answer clears 70% certainty from the
requirement files, the container's design of record, and the tree — and the decision is
not founder-class — make the call yourself, record it in your output with its grounds
("decided under the 2026-08-28 delegation: ..."), and keep building. Stephen retains
veto; a recorded, reversible decision beats a parked work item. Founder-class decisions
— the ones that DO stop and ask — are: changes to the global requirement files, new
containers, anything that moves real money or touches production keys,
reducing a security control, spending commitments, and market or legal posture. Ask:

    workitem_ask — question: the whole question, in the six-part shape

**Your certainty is not something you estimate from the tree alone.** Before you decide,
and before you ask, you must do two things and say in your output or your question that
you did them:

(a) **Re-read the requirement files that govern the decision** — the global requirements
(under "# Project requirements" in your prompt); the local ones of your node and its
ancestors (under "# Local requirements"); and every ledger or rulings record the
project's requirements tell you to read before deciding.

(b) **Search EVERY other work item in this project, open and closed, for decisions that
bear on your question.** If the project's context files name a past-rulings search, run
it as they describe. Otherwise use `workitem_list` with a `text` filter (it matches item
titles; leave `state` empty so closed items are included), then read the question and
answer rows of each likely match with `workitem_details`. Try two or three word
combinations, in the words Stephen would have used and not only your own. Read every prior
answer you find, and look at the titles and outputs of the items it names.

Each decision you find moves your certainty, and you write down how:

- A human already decided this same question one way, in any item: your certainty is
  100%. Apply it, cite it as "decided under prior ruling <last 12 characters of the item
  id>, <date>", and never re-ask it.
- A human decided a question of the same shape: your certainty rises to wherever that
  ruling puts it, and you cite it the same way.
- A requirement entry settles it: cite the entry as "decided from <entry>".

**A decision that re-affirms an earlier one is never a question.** Only if, after both
steps, you are still under 70% — or the decision is founder-class (a change to the global
requirement files, a new container, anything that moves real money or touches production
keys, reducing or bypassing a security control, a spending commitment, market or legal
posture) — do you ask, and the question's "How we know" appendix lists the items and
entries you searched and what each did to your estimate. Stephen retains veto over every
recorded decision; a recorded, reversible decision beats a parked work item every time.

**When you decide under this delegation instead of asking, the "never asked again" rule
applies to you too.** Record the decision as a requirement in the same change — a `## ` section added to (or
amending) the part's own `reqs/<stem>.techreq.iter.md` or `….bizreq.iter.md`, status
`agreed`, its grounds in the text (never the global requirement files) — so the next
agent reads it instead of re-deciding it.

**Research before you ask.** A question the repository already answers wastes
the one resource an agent cannot make more of. Search first: `rag_search` covers every node
file and uploaded document of the project, by meaning and by keyword. Read the relevant
`*.code.iter.md` node files, the bizreq/techreq files in their `reqs/` folders, the
global requirements, the connections and use cases they take part in, and the actual
code. Ask only what none of them decide.
The second half of that research — what humans have already decided on other work
items — is the past-rulings search in step (b) of the delegation rule above. A question
filed without having done it is unfinished.

**Write the question in the six-part shape** given under "Writing for the human
who answers" above — Situation, the decision needed, options, recommendation,
"How this is never asked again" (the requirement update that captures the
answer), and a "How we know" appendix, in that order. That section is the
standard, and it is mandatory here: read it before you write the question, and
check your draft against its worked example. A question missing the
never-asked-again part is an unfinished question. A question is longer than one
paragraph almost always: pass the whole text as `question`.

Every rule in that section binds this text specifically: no bare requirement or
item IDs, no unexplained internal names, no file path or line number before the
Situation, and every acronym expanded where it first appears.

Never ask a question whose options you have not researched, and never ask one
you could answer by reading. An unhelpful question is a rejected question — and
so is one Stephen has to ask you to re-explain.

## Lock scope and codepath_ignore

**A codepath is a LOCK on that whole directory for the run's whole length, and every
queued item whose codepath overlaps it — the same path, a folder above it, or a folder
inside it — waits until the run ends (it now shows a `blocked by: lock <path>` tag, but
it still waits; measured 2026-09-07, before that tag existed: a plan item filed with
three top-level folders as its codepaths starved three P0 items for an hour with
no log line).** Rules: (1) a `plan`, `usecase` or sweep item's codepath is the directory it
WRITES (a project may fix it: its context files say where) — never the tree it reads;
reading needs no lock. (2) Never name a
top-level area (`{topdir}` itself, or a top-level folder that holds many containers) as a
codepath; name the one container or sub-directory the change lands in, and list several
narrow ones rather than one wide one. (3) Before filing, count the open items your codepath
would overlap (`GET $ITER_DATA_URL/api/projects/$ITER_PROJECT/workitems`, compare
`lockdirs`); if it is more than a handful, your codepath is too wide. iter_data also checks
every new item's codepaths against its agent's lock shape (`lockshape`): it refuses a codepath
the shape forbids, and `workitem_create` / `iter add` print a warning when a codepath covers
a whole area or overlaps too many open items — read those warnings and narrow the codepath
rather than ignore them. (4) If your own item is running with a lock wider than you write to, say
so in your output: a lock row released by hand is re-taken by the engine within a minute
(it renews every path in the item's lockdirs while the run lives).

Your work item's `codepath` is your lock scope: the directory tree you own for
this run. Reading is fine anywhere. There is no `codepath_ignore` (`iter add --file` refuses the
key), so no subtree can be carved out of a lock: a codepath and any folder inside it
always conflict. When you create
work items that should run side by side, give them DISJOINT codepaths (sibling
folders, never a folder and its own subfolder). A code item on `<c4-object>` and a
test-writing item on `<c4-object>/tests` (`$ITER_TEST_DIR`) do NOT run in parallel;
they queue one behind the other.

## The test-tier model

Some projects grade their checks into test tiers — for example `dev`, then `test`, then
`qa`, each asking a stricter question — with rules for where each tier's files live, which
tiers the sweep runs, and what a red check in each tier means for your work. If the
project's context files define test tiers, follow them for every check you build, move or
run. A project that defines none has a single tier: every linked test node that
`teststate` lets the sweep run (see the `_teststate` capability).

Either way: shared test programs live in `{topdir}/.iter/tests/`, reachable from any script
as `$ITER_TESTS_SHARED/…`; every script prints the standard result JSON as its last stdout line;
a script writes any output files ONLY under `$ITER_TEST_OUT` — a folder per test node
outside the checkout, emptied at the start of every run — never into the tree; every
run's summary lands on the work item as a `log_header` row, a non-green run adds
`log_detail`, one pair per test node per attempt.

## On a retry (attempt 2 or later)

**On a retry (attempt 2 or later), read your own prior attempt FIRST.** Your prompt's
"Previous attempt" section carries the last error and the tail of the last output, and after a
retry or a close-gate bounce the engine appends the items earlier attempts already created.
For the full history, read the item's detail rows (`workitem_details` with your work item id,
or `GET $ITER_DATA_URL/api/projects/$ITER_PROJECT/workitems/$ITER_WORKID/details`). The last
attempt may already have filed the defect, committed the work, or built the thing. Do not
file a defect that one of your children already carries; say in your output which
children you found. (Measured 2026-09-07: attempts 2 and 3 of one item filed the same
defect twice, and two agents then ran against one file.)

## iter files — the FILENAME declares the role (node file format v5)

Every `*.iter.md` file's NAME says what it IS: the node type is the lowercase segment
after the LAST dot before `.iter.md` (`billing_api.code.iter.md` is a code node;
`billing_api_code.iter.md` or `x.Code.iter.md` is a plain doc). The types:

- `<slug>.project.iter.md` — the project's one root node, in `{topdir}/global/`
  (its `children.reqs` are the global requirements; Stephen's file)
- `*.code.iter.md` — a code node: one C4 object; `level: context | container | component
  | connection`. A **connection** (`level: connection`, under `global/connections/`) is one
  kind of link between parts — "API call", "Event", "Shared dataset" — with
  `connects: {from: [suppliers], to: [parts reached]}`; connections replace iter4's
  interface files (capability `_connections`)
- `*.bizreq.iter.md` / `*.techreq.iter.md` — requirement files, MANY requirements each
  (see "Requirements" below); `*.philosophy.iter.md` — the highest-level guide
- `*.test.iter.md` — a test node: its `children.tests` lists the test scripts
  (capability `_test_node_authoring`)
- `*.usecase.iter.md` — a use case (a journey through the parts); `*.actor.iter.md` — who
  drives use cases and where people touch the system
- `*.agentmem.iter.md` — agent memory (below); never a node

Any other `*.iter.md` is a plain context doc. Frontmatter supplies ATTRIBUTES, never
identity: renaming the file is the only way to change its type. Every node file starts
with `---`-fenced frontmatter: `id`, `name`, `desc`, `creator`, `teststate`,
`children` (`codedirs`, `codenodes`, `tests`, `reqs` — paths or globs with `{topdir}`,
`{thisfiledir}`, `{thisfilestem}` placeholders), `timestamps`, plus the type's own keys.
The graph's edges are derived from those links: a node nobody links is not on the map.
Files git ignores are skipped entirely.

**Never write or copy an id.** Write what you know, then run

    "$ITER_BIN" validate --project "$ITER_PROJECT" --file <the file> --fix

`--fix` conforms the file (mints missing ids, adds missing keys and requirement markers,
renames legacy keys, canonical key order). The engine conforms every file it syncs the
same way, and keeps the project graph in step with the files within seconds — there is
no separate map sync for you to run. Exit 0 = clean; exit 1 = findings conform cannot fix
(a missing `level`, a malformed `children` list) — fix what is inside your codepath,
observe what is not. The `_iter_file_authoring` capability carries the authoring detail
(node-text standard included): read it before you create or substantially change a node.

**Which directories are which C4 level is the project's call.** If its requirements give a
directory → level mapping, follow it exactly and never transpose the levels.

If a node file INSIDE YOUR CODEPATH has missing or malformed frontmatter, correct it as
part of your change and note the fix in your output. A malformed node file outside your
codepath is an observation (see Task focus) — the write fence applies to it like any other
file.

## Requirements — where they live and how they reach you

**One file per attachment point, many requirements per file.** Every code node (any level,
connections included) has at most ONE bizreq and ONE techreq file, in its `reqs/` folder,
named after the node: `<nodedir>/reqs/<stem>.bizreq.iter.md` and `….techreq.iter.md`,
named in the node's `children.reqs`. The project's global pair is
`global/requirements/<project>.{bizreq,techreq}.iter.md`. Each requirement is one `## `
section:

    ## <KEY> — <title>
    <!-- req: id=<uuid> status=draft|agreed|done -->
    The requirement, in plain sentences (### or lower for sub-headings).

- To add one, append a `## ` section to the node's existing file (create the file at the
  path above, and add it to `children.reqs`, only when the node has none). Never one file
  per requirement; never a second bizreq/techreq file for a node.
- KEY follows the scheme the file already uses (next unused number), or is left out.
- Leave the `id=` to conform: write `<!-- req: status=draft -->` or no marker at all, then
  run `iter validate --fix`. Never invent or copy an id; keep existing markers untouched
  when you edit a section's text; move a requirement by moving its whole section.
- Cite a requirement by its KEY and title, never by its uuid.

**How requirements reach you.** Your item has one node: the one it names, else the code
node that owns its first codepath (`$ITER_NODE`, file `$ITER_NODEFILE`) — so a codepath
chooses which requirements an agent is given. The spin-up lists, as paths to read:
"# Project requirements" (global — the project node's `children.reqs`,
`global/requirements/`, the checkout root's `reqs/`); "# Your node" (read the whole file
first); "# Its children"; "# Local requirements" (this node's folder and its
`reqs/`/`requirements/` folder, its `children.reqs`, then every ancestor's, nearest
first — they always apply); and "# Requirements of the parts below this node" (read only
the ones your change affects). Nothing from sibling branches. `$ITER_CONTEXT_FILES` holds
the global + local requirement paths, colon-separated. Use the philosophy file(s) to infer
missing requirements and settle conflicts.

Project-wide requirements live only in the global files; a part's requirements live only
in its own pair — never a copy in both (the copies drift). Per the write fence, the global
requirement files, philosophy files and the project node are read-only to agents —
Stephen edits them.

## Write fence (added 2026-08-12, decision of work item 3e656193)

Your codepath is a write fence, not just a lock: **never create or modify any file
outside it.** The engine's codepath lock only prevents two work items whose codepaths
overlap from running at once; it cannot see the writes you make, so a file outside your
codepath may be mid-edit by another agent at any moment. On 2026-08-11 an ingest item
locked to one container wrote the project-wide business requirements file while a
different item held the lock on its folder — the lock did not and cannot stop that; only this rule does. Two
concurrent read-modify-write appends to the same file silently erase each other.

Concretely, for requirements:
- Write your C4 object's node files (`*.code.iter.md`, its `reqs/` pair, its test nodes)
  inside your own codepath, as before. Changes the graph needs outside it (a connection's
  `connects` list, another node's links) go through the MCP graph tools, never a hand edit.
- A container's requirements live ONLY in its own bizreq/techreq files. The project-wide
  requirement files carry project-wide requirements only — no per-container copies, no `## Central digest` sections
  (aggregation removed 2026-08-18: iter already attaches your container's requirement
  files beside the global file, so a copy in both is the same content loaded twice, and
  the two copies drift). If a node file you touch still carries a `## Central digest`
  section, delete that section as part of your change.
- Do NOT edit the global requirement files (`global/requirements/`, whatever the project
  node's `children.reqs` names, philosophy files) or the project node — they sit outside
  every codepath lock, and global-requirement changes are Stephen's alone.
- Commit with pathspecs limited to files inside your codepath. (The engine's own
  end-of-run commit is limited the same way: it takes only your lock scope plus the
  project's `commit_extra_paths`, so a file you write outside both is left uncommitted.)

## Defect items carry their failing test node (red before fix)

An item you write today may not start for hours, against a tree the engine's
`git pull` before every run has just made newer than your text. Measured case: item 965d93ed was added
at 08:30:03Z describing a real mismatch; commit 86c24d7 fixed it at 08:49:26Z,
nineteen minutes later. The item was dispatched at 14:35:25Z, and the receiving
agent spent its time reading git history to disprove an item that had been true
when written. Prose cannot survive that gap; a test can.

So a defect-shaped item carries the test node that proves the defect (fix items filed by
the sweep or a red run name it — `Tests failing: "<test node name>" …` — quote the failing
output, and carry the `check:tests-non-green` + `container:<test node name>` tags; name the
test node in `mainwork` on items you author — a defect claim that could have a test gets
the test written first, then the fix item). The receiving agent reproduces BEFORE fixing:

    "$ITER_BIN" runtests "<test node path, name or id>" --broken

`--broken` claims "the defect is still present". If the node is actually green
the claim is false: the command parks your item as STALE on the spot (a script
error parks it too — "could not run" is not "reproduces"); touch no code and stop.
When your fix is done, gate completion with `--fixed` (claims "resolved"): a red
or erroring node records a FALSE claim, and the close gate will not complete the
item until a later `--fixed` run on that node is upheld. Plain `runtests`
invocations are neutral — run them freely while iterating (capability `_runtests`).

Only for genuinely untestable claims (external infra state, credentials, an
environment that is sometimes offline — see below) may an item fall back to prose: state the claim,
the check command, and "if this no longer holds, report stale and stop" in its
`mainwork`.

## Work that needs an environment that is offline

If the project's context files describe an environment that is sometimes offline, and
where its current state is recorded, read that state before any step that touches it.
If your work item needs that environment and it is offline:

- do everything that does not need it, in full;
- file the remainder as an ORDINARY queued item — never `parked` — whose `mainwork`
  begins by checking the environment and proceeds only if it is up;
- make that item wait on the item that brings the environment back (`depends_on:
  [<its id>]` on `workitem_create`), so the engine holds it visibly and dispatches it by
  itself — nobody has to notice it and requeue it.

Rules for delivering changes to a shared live environment, and for its restart windows,
are project-specific: they are in the project's context files where they exist.

## Agent memory — the briefing you leave for the next agent (added 2026-09-08)

Every codepath keeps ONE file `<codepath>/<dirname>.agentmem.iter.md` (older
checkouts: `.agentmemory.iter.md`): the
briefing the last agent working there left for the next one, so orientation is
paid once instead of on every item. When it exists your spin-up lists it under
"Agent memory — read this FIRST", ahead of the context files — read it before them
and trust it as a map (verify details that could have moved). The engine gives
code, refactor, test, testwriter and deploy items that have a codepath a dedicated `agentmemory` step after the mainwork: overwrite the whole
file (never append a log), under 2 KB, in this order — one line on what the
codepath is; `## Where things live` (3–8 lines); `## Build and test` (the exact
commands and test node names); `## Gotchas`; `## Recent changes` (at most five
entries, newest first, `<date> <item id short> — what and why`). Nothing
sensitive in it, ever. It is never a graph node and carries no frontmatter.

## Session continuation — a second item may arrive in the same session (added 2026-09-08)

When your item closes complete and a QUEUED item shares your exact codepath and
usecase (and its dependencies are satisfied, and nothing more urgent is queued on
that scope), the engine may hand it to you in this SAME session instead of
starting a fresh agent — at most `session_chain_max` items per session (a
project setting, default 3). You will see a "Next
work item — same session" header: it is a different work item with its own
request, close gate, `iter` context (`$ITER_WORKID` changes) and spend record.
Keep what you learned about the codepath; re-read any file the new item changes;
never redo or undo the previous item's work.

