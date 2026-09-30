---
id: 1afa7936-33a3-48dd-a7ed-0167d37f2f56
name: "Close gate verifier"
description: "Builds the evidence, the verifier prompt, the verdict parsing and the human question the close gate uses to decide whether a finished agent run is really done, so that an item closes complete only when its work is shown, not just claimed."
simple_description: "Double-checks an AI worker's claim that a task is finished before the task is marked done."
level: component
owner: bespoke
teststate: inherit
children:
  codedirs:   ["{topdir}/iter_engine/src/gate.rs"]
  codenodes:  []
  inputs:     []
  outputs:    []
  bizreqs:    []
  techreqs:   []
  tests:      []
---

# Long Description

The close gate is the check every agent run passes before the engine may mark its item complete. This part supplies its pieces; the Work runner (`run_gate` and `close` in `iter_engine/src/work.rs`) calls them and writes the outcome.

How it works (`iter_engine/src/gate.rs`): first the Work runner makes deterministic checks, such as a session that was cut off, review rows with no disposition, a required child item or commit that is missing, or an `iter runtests --fixed` claim that was not upheld (`last_fixed_claim`, `open_reviews`). Then `Evidence` gathers what the engine measured: this item's own commits over every attempt (`commits_with_id`), the diffstat limited to its lock scope, other items' commits in that scope listed apart, and the items it created (`children_of`). `verifier_prompt` puts the request, the worker's last message and that evidence in front of a separate verifier session, and `parse_verdict` reads its one JSON answer as `Complete`, `Incomplete`, `Unclear`, `Unparsed` or `Unavailable`.

What happens next: an incomplete verdict bounces the item back to the queue with `feedback_section` added to the next prompt; after too many bounces, or on an unclear verdict, `question_widget` asks a human with one question and a recommendation (`Advice`). If the item declared blockers that are still open (`waiting_on`), it waits behind them instead, with no bounce counted (`waiting_row`). A human who answers "accept" closes it without another run (`accepted_by_human`). The prompt paragraph every worker sees is also defined here.

Why it matters: a worker's "done" is not proof. Without the gate, cut-off or half-finished runs would close as complete and their gaps would be found later, by people.

Example: a worker says it fixed a bug, but its session ended cut off and it made no commit. The gate lists both as open points, bounces the item, and the next attempt's prompt starts with that list.
