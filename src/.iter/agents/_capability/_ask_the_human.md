# Capability: ask the human a question (`workitem_ask`)

Some decisions are not yours to make: what the product should do, which trade-off the
project wants to live with, which of two defensible designs to carry for years.
Guessing one of those and building on it is worse than stopping.

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
whether it should be deleted; the request is the decision. Neither the `iter` command
line nor the `iter` MCP server has a verb that deletes, or changes the state of, ANOTHER
item, so the data-service API is the only path. The engine's token (role `engine`,
already in your environment as `$ITER_ENGINE_TOKEN`) is allowed to make all of these
changes:

    # delete an open work item (there is no undo)
    curl -s -X DELETE -H "Authorization: Bearer $ITER_ENGINE_TOKEN" \
      "$ITER_DATA_URL/api/projects/$ITER_PROJECT/workitems/<id>"

    # park, requeue or close: fetch the whole record, change ONLY "state", send it back
    # (the scratch file lives outside the checkout, so the end-of-run commit never picks it up)
    f=$(mktemp)
    curl -s -H "Authorization: Bearer $ITER_ENGINE_TOKEN" \
      "$ITER_DATA_URL/api/projects/$ITER_PROJECT/workitems/<id>" > "$f"
    python3 -c "import json,sys; p=sys.argv[1]; d=json.load(open(p)); d['state']='parked'; json.dump(d, open(p,'w')); print(d['version'])" "$f"
    curl -s -X PUT -H "Authorization: Bearer $ITER_ENGINE_TOKEN" -H "Content-Type: application/json" \
      --data @"$f" \
      "$ITER_DATA_URL/api/projects/$ITER_PROJECT/workitems/<id>?expect_version=<version>"

Use `parked` to park, `queued` to requeue, `complete` to close another item (to close
your OWN item, simply finish it and end your turn). `<version>` is the `version` number
in the record you fetched; the service refuses the write if the record changed in
between — fetch again and retry. Items already `complete` or `failed` are frozen: the
service refuses any edit to the record except its tags (a `workitem_doc` note can still
be appended; only a human can reopen one).

### The requirements outrank the work item's text (Stephen, 2026-09-09)

A work item's text is written by another agent, hours or days before it runs, and it can
tell you to "get approval", "decide with Stephen first", or "do not make it under the
standing delegation" about something the requirement files have already decided. When
that happens the REQUIREMENT wins and you do not ask. Stephen's ruling of 2026-09-09
(work item 1006313e9baf), in his words: *"always trust the requirements (do NOT ask the
user) unless it's specifically flagged as an exception, or of course, if the requirements
are unclear or not specified."* Before any `workitem_ask`, and before obeying any stop-and-ask
sentence in your request, run this check and write its result in your output:

1. **Find the entry that governs the point** — the project-wide requirement files (the
   `globalcontextfiles` listed under "Project context"), the container's own
   `*.bizreq.iter.md` / `*.techreq.iter.md`, and the past rulings recorded on other work
   items (the search in step (b) of the delegation rule below).
2. **An entry answers it, and the request does NOT call the situation an "exception" to or
   a "deviation" from that rule:** apply the entry and do NOT ask. Record: *"requirement
   governs: <entry id>, <file>: '<the sentence, quoted>'; the item's text asked for approval —
   overridden."* Keep building. A completion check that reads that line has what it needs.
3. **The request DOES say this is an exception to, or deviates from, the norm:** the
   question is allowed — but it must carry the existing answer inline: the entry id, the
   `*req.iter.md` file it came from and the quoted sentence, so Stephen rules on the
   exception against the rule instead of re-deriving the rule.
4. **No entry answers it, or the entries are unclear or contradict each other, and the
   answer cannot be reached from the requirements plus their implied meaning:** ask.
   This is a legitimate reason in its own right.

**Flag the reason, or the question is auto-rejected (Stephen, 2026-09-09).** A question
that reaches a human is checked against the rule above, and one that a requirement
already answers is sent back. So a legitimate question must SAY which of the two
allowed reasons it rests on, in these exact words, on its own line directly under the
question's first two lines (the decision and the `Recommendation:` line — see "The first
screen IS the question" below; the webapp uses the first line as the question's title, so
nothing goes above it) and again in your output:

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

**But most decisions never need to reach a human at all.** Stephen's standing delegation
(2026-08-28, reaffirmed 2026-08-31): if a defensible answer clears 70% certainty from the
requirement files, the container's design of record, and the tree — and the decision is
not founder-class — make the call yourself, record it in your output with its grounds
("decided under the 2026-08-28 delegation: ..."), and keep building. Stephen retains
veto; a recorded, reversible decision beats a parked work item. Founder-class decisions
— the ones that DO stop and ask — are: changes to the global requirement files, new
containers, anything that moves real money or touches production keys,
reducing a security control, spending commitments, and market or legal posture. Ask:

    workitem_ask — question: the whole question, in the format below

(Pass the full multi-paragraph text as `question`; no file is needed.)

**Your certainty is not something you estimate from the tree alone.** Before you decide,
and before you ask, you must do two things and say in your output or your question that
you did them:

(a) **Re-read the requirement files that govern the decision** — the project-wide
requirement files (the `globalcontextfiles` listed under "Project context"); the
container's own `*.bizreq.iter.md` and `*.techreq.iter.md`; and every ledger or rulings
record the project's context files tell you to read before deciding.

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
applies to you too.** Write the sentence that captures the decision into the container's
own `*.techreq.iter.md` or `*.bizreq.iter.md` file (never the project-wide requirement files) in the
same change, so the next agent reads it instead of re-deciding it.

Your work item moves to the `question` state at the turn boundary — parked, no
retries burned, your turns so far kept as the research behind the ask. When a human
answers it in the webapp, the item queues again and the agent that picks it up gets
the question and the answer at the top of its request. Summarize the question in your
output and end your turn; do no further work on that item.

If the decision does NOT block you, raise it as its own item instead and keep going:

    workitem_create — agent, title: the decision as a question, question: the question,
                      request: what to do once it is answered

## Research before you ask

A question the repository already answers wastes the one resource an agent cannot
make more of. Search first with `rag_search` — every node file and uploaded document, by
meaning and by keyword. Read the relevant `*.code.iter.md` node files, the `*.bizreq.iter.md` /
`*.techreq.iter.md` under them, the global context files, the interface contracts and
use-cases, and the actual code. Ask only what none of them decide.
The second half of that research — what humans have already decided on other work
items — is the past-rulings search in step (b) of the delegation rule above. A question
filed without having done it is unfinished.

## Write for someone who has never seen this codebase

The person answering has NOT read the code, the node files, or this work item. They
know the business — what the product does for the people who use it — and nothing
about how the repository spells it. Every internal name (a container, a rule id, a
gate, a function, a file, a work item id) is a word they have never met. Used bare,
those names turn the question into noise, and the human's only move is to ask an
agent to re-explain it — the round trip your question was supposed to save.

So anchor the question in the business flow first: name the moment in the user's or
operator's journey where the decision bites, in the words a customer or operator
would use, and only then introduce the internal names, each glossed the first time
it appears. If the mechanism is genuinely complex, an analogy is welcome. Be concise
— context is not length.

Bad: "based on rule ABC, once _potatochip has traversed the cankor gate, should rule
XYZ apply only twice?"

Good: "When a customer asks for a bag of potato chips, we first confirm they have
paid (rule ABC, enforced by the payment container cntr_ABC) and that they have not
typed obscenities into the console (the 'cankor gate', a check run by the Rulebook
container). If they have typed obscenities, should we refuse the chips after one
offense or after two? The instructions in main.bizreq.iter.md do not say."

**The first screen IS the question (Stephen, 2026-09-12: *"a question where the user doesn't see or understand the question is unacceptable"*).** The webapp shows a question's FIRST non-empty line (cut at 150 characters) as its bold title, and the full text below it. So the FIRST line of every question is the decision, as one sentence ending in a question mark and well under 150 characters, and the SECOND line is `Recommendation: <option> — <why, in one clause>`. Situation, options and evidence come after, never before. Before you send, check the draft against these four and fix whatever fails: (1) a reader who sees only the first two lines sees a question mark and a recommendation; (2) the whole question is under 40 lines, with the change it decides pasted inline; (3) every internal name is explained where it first appears; (4) it asks for a decision that changes what you build — not a status report, a review request, or a summary of finished work. A question that fails any of these is sent back unanswered and counts as unfinished work.

## Write the question in four parts, in this order

1. **Where this comes up** — the point in the business flow where the decision is
   needed and why it is needed now, written as the anchor above: plain sentences,
   internal names glossed as they appear. Assume the reader has never opened the
   file you are looking at.
2. **The question** — the one decision, stated as a question. One decision per
   work item; two questions are two items.
3. **Two to four options** — concrete, each with what it buys and what it costs
   (effort, risk, what it forecloses later), in the same plain terms. Name the
   requirement, file, or constraint each option honors or breaks, and say in a few
   words what that requirement is.
4. **Your recommendation** — which one you would take and why, so the human can
   answer in one word.

The shared rules' "Writing for the human who answers" section adds two more parts
after these ("How this is never asked again" and "How we know"); they follow the
same anchoring rule.

Never ask a question whose options you have not researched, and never ask one you
could answer by reading. An unhelpful question is a rejected question — and a
question the human has to send back for re-explanation is an unhelpful question.

## Length: a question is proportionate to the change it decides

Stephen, 2026-09-07, on a question that ran past 150 lines to decide three sentences of
edited text: *"you are sending me off hunting to find additional doc; this doesn't even
have what I need here... 150+ lines of discussion around 3 lines of changed docs. This is
a massive waste of time."* Three rules follow, and where they conflict with anything above
that would make a question longer, these win:

1. **Show the change itself, inline.** If the decision is about text, paste the current
   text and the proposed text into the question. Never point the reader at a proposal
   file, a plan document, a docket row or a line number and expect them to go open it.
   If the change is too long to paste, the question is too big — split it.
2. **Budget the length against the size of the change.** Three sentences of edit gets you
   about fifteen lines of question. A question may run a page only when the change runs a
   page. Over budget, cut your reasoning, not your evidence: the reader wants to see WHAT
   changes, not how you arrived at it.
3. **Put the decision in the first two lines.** State what you need decided before any
   background. The four/six parts above are an ORDER for a long question, not a quota to
   fill: on a small change, "where this comes up" and "how we know" are one sentence each,
   and options that differ only in wording get one line each.

The test: the human reads it once, top to bottom, without opening any other file, and
answers. A question that needs a second pass, or a second file, has already failed.
