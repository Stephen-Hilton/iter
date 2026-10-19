# Capability: reject invalid work (`workitem_reject`)

Failing an item means "I couldn't do the work" — the engine retries it. When the
problem is the WORK ITSELF (out of scope for the project, goal unclear, premise
no longer true, conflicts with a `*bizreq.iter.md` invariant), do not fail and
do not quietly complete. Reject it:

    workitem_reject — reason: why, and what would make it acceptable

The engine moves the item to `parked` at the turn boundary — the human-review
bucket, where the user edits and requeues (or deletes) it. No retries are
burned; nothing gets buried in the completed archive. Your reason and your
output are what the re-evaluating human sees: name the blocking fact and the
smallest change that would make the item valid, then end your work.

## Rejecting is not asking — and neither is confirming

A rejection says the WORK is invalid. It is never a way to hand a decision back that the
work item already made. Before you reject, or before you turn to `workitem_ask`, apply the rule
below: if the item told you to do something, you are already authorized to do it.

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
  listed in the ask capability (the `capability` tool, name `_ask_the_human`) — reducing a security
  control, a new container, real money, production keys, global requirement files —
  still stop and ask.)
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

## What a rejectable item looks like

- **Out of scope** for this project (a Netflix codebase asked to "order food").
- **Unclear in its goal** ("hit button" — no actor, no outcome).
- **Overly technical for its type** — e.g. a use-case item saying "run database
  query ABC", which is implementation, not a user journey.
- **In violation of a business invariant** in the global or local
  `*bizreq.iter.md` ("user logs in with a 1-character password").
- **Premise no longer true** — the thing the item asks you to change is already
  gone, or was decided the other way since the item was filed. **This is NOT "the work
  is already done."** If the outcome the item asks for already exists in the tree (a
  sibling or an earlier attempt landed it), verify it against the acceptance criteria,
  cite the commits, and end COMPLETE; and if a small piece the item explicitly asked
  for is still missing, do that piece — it is your mainwork. Stephen, 2026-09-10: a
  finished item is never left parked, rejected or in question to carry a leftover
  recommendation; the recommendation goes into your output (or into the project's
  recommendations file, where its context files name one) and the item closes (full rule in `_shared`, "Done, with a
  recommendation left over").

Do NOT mark rejected work complete, and do NOT grind out work you believe is
invalid. A rejection with a specific reason is a useful result; a completed item
that did the wrong thing is not.