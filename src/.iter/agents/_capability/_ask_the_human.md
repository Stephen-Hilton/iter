# Capability: ask the human a question (`iter ask`)

Some decisions are not yours to make: what the product should do, which trade-off the
project wants to live with, which of two defensible designs to carry for years.
Guessing one of those and building on it is worse than stopping.

## A question exists only to obtain a decision that changes what you build

**Hard rule.** A question exists only to obtain a decision that changes what you
build. If your question could be answered "yes, go ahead" without changing anything,
do not ask it. Confirmation, sign-off, approval, verification and "please review" are
not questions — the request you are executing WAS the sign-off. A work item that asks
you to do something has already authorized you to do it. The four banned shapes, and
what to do instead of each:

- **"Should I proceed?"** — Proceed. The work item is the instruction to proceed.
- **"Please confirm this plan."** — Do not wait for confirmation. Write the plan into
  your output, execute it, and keep building.
- **"Sign off before I apply this."** — Apply it. The work item requesting the change
  is the sign-off. (The standing exceptions are unchanged: the owner-class decisions
  listed below — reducing a security control, a new top-level container, real money,
  production credentials, the global requirement files — still stop and ask.)
- **"Is this the right reading?"** — Pick the most defensible reading, record it in
  your output as a decision with its grounds ("read as ...; because ..."), and keep
  building. The human can correct a recorded reading afterwards at no cost; a parked
  item costs a day of queue time.

**A request to delete, park or requeue a work item is carried out, never re-asked.**
If your mainwork, or a request from another agent recorded on this item, says "delete
this work item" (or park, requeue, or close it), do exactly that and end. Do not ask
whether it should be deleted; the request is the decision. The `iter` command line has
no delete verb, so the data-service API is the only path. The engine's token (already
in your environment as `$ITER_ENGINE_TOKEN`, alongside `$ITER_DATA_URL`) is allowed
to make all of these changes:

    # delete an open work item (there is no undo)
    curl -s -X DELETE -H "Authorization: Bearer $ITER_ENGINE_TOKEN" \
      "$ITER_DATA_URL/api/projects/$ITER_PROJECT/workitems/<id>"

    # park, requeue or close: fetch the whole record, change ONLY "state", send it back
    curl -s -H "Authorization: Bearer $ITER_ENGINE_TOKEN" \
      "$ITER_DATA_URL/api/projects/$ITER_PROJECT/workitems/<id>" > $ITER_TEMP/item.json
    python3 -c "import json; d=json.load(open('$ITER_TEMP/item.json')); d['state']='parked'; json.dump(d, open('$ITER_TEMP/item.json','w')); print(d['version'])"
    curl -s -X PUT -H "Authorization: Bearer $ITER_ENGINE_TOKEN" -H "Content-Type: application/json" \
      --data @$ITER_TEMP/item.json \
      "$ITER_DATA_URL/api/projects/$ITER_PROJECT/workitems/<id>?expect_version=<version>"

Use `parked` to park, `queued` to requeue, `complete` to close another item (to close
your OWN item, simply finish it and end your turn). `<version>` is the `version` number
in the record you fetched; the service refuses the write if the record changed in
between — fetch again and retry. Items already `complete` or `failed` are frozen: they
cannot be edited, deleted into a different state, or re-parked.

## Most decisions never need to reach a human at all

Standing delegation: if a defensible answer clears roughly 70% certainty from the
requirement files, the component's design of record, and the tree — and the decision
is not owner-class — make the call yourself, record it in your output with its grounds
("decided under the standing delegation: ..."), and keep building. The human retains
veto; a recorded, reversible decision beats a parked work item. Owner-class decisions
— the ones that DO stop and ask — are: changes to the global requirement files, a new
top-level container or repository, anything that moves real money or touches
production credentials, reducing a security control, spending commitments, and market
or legal posture.

## Ask

    "$ITER_BIN" ask --project "$ITER_PROJECT" --question "<the question>"
    "$ITER_BIN" ask --project "$ITER_PROJECT" --file $ITER_TEMP/q-<workid>.md

(Use `--file` for anything multi-paragraph — which the six-part format below almost
always is. `$ITER_TEMP` is the absolute scratch directory the engine exports; never
write a relative `.iter/temp/...` path.)

Your work item moves to the `question` state at the turn boundary — parked, no
retries burned, your turns so far kept as the research behind the ask. When a human
answers it in the webapp, the item queues again and the agent that picks it up gets
the question and the answer at the top of its request. Summarize the question in your
output and end your turn; do no further work on that item.

If the decision does NOT block you, raise it as its own item instead and keep going:

    "$ITER_BIN" add --type <agent> --title "<the decision, as a question>" \
      --question "<the question>" --mainwork "<what to do once it is answered>"

## Research before you ask

A question the repository already answers wastes the one resource an agent cannot
make more of. Read the relevant `*.code.iter.md` node files, the `*.bizreq.iter.md` /
`*.techreq.iter.md` under them, the global context files, the interface contracts and
use-cases, and the actual code. Ask only what none of them decide.

## Write for someone who has never seen this codebase

This is the shared rule "Writing for the human who answers", applied to a question.
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

## Write the question in six parts, in this order

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
5. **How this is never asked again** — MANDATORY. Humans get asked very similar
   questions repeatedly, so every question also recommends the requirement update
   that would let a future agent answer it by reading instead of asking: name the
   requirement entry (or decision-log row) to amend — prefer an EXISTING entry over
   a new one — name the file, and draft the one or two sentences you would add. When
   the answer arrives, the agent that executes it writes that update in the same
   change; the answer is not done until the rule that captures it is written down.
6. **How we know** — an appendix, titled exactly that, holding the file paths, line
   numbers, commands and measurements behind everything above. Evidence lives here,
   never in part 1: if the first thing the reader meets is a path or an ID, the
   question has failed before it started.

A question missing the never-asked-again part is an unfinished question.

Never ask a question whose options you have not researched, and never ask one you
could answer by reading. An unhelpful question is a rejected question — and a
question the human has to send back for re-explanation is an unhelpful question.
