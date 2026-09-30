# Bugfix spec: the close-gate question shows the human no question

Written 2026-09-12 by Stephen's session in pdy-dev, from a measured incident. Send as-is to the
iter agent. File:line references are from a read of `~/dev/iter/iter3` on 2026-09-12; verify each.

Terms: the **close gate** decides after a session whether a work item is done; its LLM half, the
**verifier**, answers `complete`, `incomplete` or `unclear`. When the gate cannot decide it moves
the item to `question` and writes a **widget**, the JSON the webapp renders as the human's inbox
entry, with a title, a summary, a detail block and the answer fields.

## 1. What happened

Item `ea3352ba-27cb-4d9f-9790-152d4dd4fac7` (roll the bundle server on corridor-dev1, priority 0,
demo work) landed in `question` at 17:52Z on 2026-09-12. Stephen opened it and saw no question:
the title was the item's name cut at 120 characters ending in `...[truncated]`; the summary was
the verifier's reason cut at 400 characters, also ending in `...[truncated]`, with the actual
doubt (an unexplained mismatch between "1 work item created" and "I filed no duplicates") falling
just past the cut; "Open obligations" said `(none listed)`; and below that sat the worker's whole
6,000-character report. Nothing said what he was being asked, and nothing recommended an answer.
His words: *"this is worse than not helpful."* The answer, once dug out, was one sentence: the
one child item was auto-filed by the rolling-update script's red-in-dev path, so both statements
were true and the item was done.

## 2. Why it happens

- `gate.rs:185-206` builds the widget as `title = clip(item_name, 120)`, `summary = clip(reason,
  400)`, `detail = "Open obligations:\n" + open_text + "\n\nLast response:\n" + clip(last_response,
  6000)`. `clip` appends `...[truncated]`. There is no field for a question and none for a
  recommendation; the item's name, not the doubt, is the title.
- The verifier prompt (`gate.rs:92-107`) asks only for `{"verdict","open","reason"}`; `reason`
  is "one or two sentences" of free prose that often front-loads what is fine before naming
  what is not, so a 400-character clip removes the doubt.
- `work.rs:1138-1150` sends the item to `question` on `unclear` or when the bounce budget is
  spent, using that same widget in both cases.
- The agent-side rule that every question leads with a one-sentence decision and a
  recommendation (the `_shared` and `_ask_the_human` tooling rows, amended 2026-09-12) does not
  apply here, because this widget is written by the engine, not by an agent.

## 3. Goal

A human who reads only the first screen of a close-gate question sees, in this order: the one
thing they are being asked to decide, as a question; what the engine recommends and why; and
only then the evidence. Nothing the human needs is ever clipped.

## 4. Design

### R1 — the verifier returns a question and a recommendation

Extend the verifier's required output (`gate.rs:92-107`, parsed at `gate.rs:110-137`) to:

```
{"verdict": "complete" | "incomplete" | "unclear",
 "open": ["…"],
 "reason": "one or two sentences",
 "question": "ONE sentence ending in '?', naming the single fact a human must settle — empty when verdict is complete",
 "recommendation": "accept" | "continue",
 "why": "one clause: why that recommendation"}
```

The prompt states the rule the agents already follow: the question is the decision, not a
summary; it names the contradiction or the missing proof in plain words; it is answerable in one
word. A missing or malformed `question` on a non-complete verdict is treated like a parse failure
today (`Unclear`), but see R4. `parse_verdict` gains the three fields with empty-string defaults so
older verifier output still parses.

### R2 — the widget leads with the question

Rewrite `gate.rs:185-206` so the widget is:

- `title`: the verifier's `question` (clip at 200, never the item name). When the gate held for a
  deterministic reason (`work.rs:985-1012`), the title is generated from that reason as a question,
  e.g. `Is this item done although no git commit was produced?`.
- `summary`: `Recommendation: <accept|continue> — <why>` followed by ` · bounce <n> of <max> · item: <clip(item_name, 90)>`.
- `detail`, in this order and with NO clipping of the first three parts: (1) `Question:` the full
  question sentence; (2) `Why the gate could not decide:` the full `reason`; (3) `Open obligations:`
  the full `open` list, or `none — the verifier's doubt is the one above`; (4) `Evidence the gate
  saw:` the commit hash, the diffstat limited to the item's lock scope (see
  `scoped_end_of_run_commit.bugfix.md`), the children count and their ids; (5) `Last response
  (first 1,500 characters; the full text is the last response row):` the clipped report.
- `fields`: unchanged (`action` radio continue|accept, `guidance` text), except that the radio's
  default `value` is the verifier's `recommendation` instead of always `continue`.

### R3 — `clip` never hides the question

`clip` keeps its `...[truncated]` marker for the report excerpt only. Title and summary are built
from fields the verifier is told to keep short, and the webapp's question panel
(`webui/index.html`, the block that renders `title`/`summary`/`detail` of a `question` row) shows
`detail` expanded by default for close-gate widgets rather than behind a disclosure.

### R4 — a verifier parse failure is not a human question

When the verifier's JSON cannot be parsed (measured 2026-09-11 on item `ef9bcf5c7864`: `verifier
json did not parse: key must be a string at line 1 column 2`), retry the verifier once with the
instruction "answer with exactly one JSON object and nothing else"; only if the retry also fails
does the gate ask the human, and then the title is `The verifier could not judge this item (its
output did not parse twice). Accept on the worker's evidence?` with recommendation `accept` when
`evidence.result_subtype == "success"` and there is a commit, else `continue`.

### R5 — the human's answer records what was asked

The answered widget already keeps the fields. Add `"answered_question": <the title>` to the
widget on answer (webapp, the PUT in the answer flow) so the record shows what the human was
asked even if the title format changes later.

## 5. Tests

Each test is shown red under its named mutation before it counts.

| # | Test | Assertion | Mutation that turns it red |
|---|---|---|---|
| T1 | `gate.rs` `#[cfg(test)] parse_verdict_reads_question_and_recommendation` | a verifier JSON with the six fields parses into `Incomplete{question, recommendation:"continue", ...}`; one without the new fields still parses with empty strings | drop the defaults |
| T2 | `gate.rs` `widget_title_is_the_question_not_the_item_name` | for a held gate, `widget["title"]` equals the question sentence and ends with `?`; the item name appears only in `summary` | restore `clip(item_name,120)` |
| T3 | `gate.rs` `widget_never_clips_question_reason_or_open` | with a 2,000-character reason and 12 open items, `detail` contains all of them verbatim and no `...[truncated]` before the `Last response` heading | apply `clip` to `reason` |
| T4 | `gate.rs` `radio_default_is_the_recommendation` | `fields[action].value == "accept"` when the verifier recommends accept | hard-code `continue` |
| T5 | `gate.rs` `deterministic_hold_makes_a_question` | a `requires_commit` hold yields a title ending in `?` naming the missing commit | leave the title as the reason text |
| T6 | `work.rs` `verifier_parse_failure_retries_once_before_asking` | with a fake verifier that returns garbage then valid JSON, no widget is written and the verdict is the second answer; garbage twice yields the R4 widget | remove the retry |
| T7 | `e2e.sh`, extend the close-gate block | the fake verifier answers `unclear` with a question; the engine record's last question row has `title` ending in `?` and `summary` starting with `Recommendation:` | any of the above |

## 6. Rollout and acceptance

Engine-only change plus one webapp rendering tweak; no data-service change; the widget schema
(`iter_core/src/widget.rs`) accepts the added keys as-is or gains them as optional strings. Both
engines pick it up on their next restart. Acceptance by hand: force a gate hold on a throwaway item
and confirm that the inbox entry's first screen reads as a question with a recommendation, that
nothing before "Last response" is truncated, and that answering leaves `answered_question` on the
row.

## 7. Out of scope

The quality of agent-asked questions (`iter ask`), which is governed by the `_shared` and
`_ask_the_human` tooling rows and was tightened on 2026-09-12 with a first-screen rule; and the
sweep-commit evidence problem, which has its own spec.
