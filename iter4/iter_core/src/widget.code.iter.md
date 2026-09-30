---
id: 0de6287f-7b13-4ee9-9db1-aea7e54e0427
name: "Question forms"
description: "Defines the typed form an agent uses to ask a person a question — a title, a summary and fields of type text, number, checkbox, radio or dropdown — and checks that a form is well formed before it is stored."
simple_description: "The format for questions agents ask people, and the check that each question can be answered."
level: component
owner: bespoke
teststate: inherit
children:
  codedirs:   ["{topdir}/iter_core/src/widget.rs"]
  codenodes:  []
  inputs:     []
  outputs:    []
  bizreqs:    []
  techreqs:   []
  tests:      []
---

# Long Description

Question forms are how an agent (or the close gate) asks a person something and gets a structured answer. This module defines the form and checks it.

How it works: `iter_core/src/widget.rs: QuestionWidget` has a `title`, an optional `summary` and `detail`, and a list of `fields`. Each `WidgetField` has a `key`, a `label`, a `type` from `FIELD_TYPES` (`text`, `int`, `checkbox`, `radio`, `combo`), `options` (each a value and a description) and a `value`, which holds the answer: the person's reply overwrites it in place. `validate` returns a list of problems, empty meaning valid: a title is required; there must be at least one field; keys must be present and unique; the type must be known; checkbox, radio and combo fields need options; a checkbox value must be a list (multi-select), a radio or combo value one string, an int value an integer.

What goes in and out: iter_data runs `validate` on the `POST /api/widget/validate` route and on every question row written into an item's history (`iter_data/src/api.rs: prepare_detail`); iter_engine offers the same check locally for a form file, and its close gate (`iter_engine/src/gate.rs: question_widget`) builds its bounce questions in this shape. The web page's Work queue renders the fields and writes the answers back.

Why it matters: a malformed question would reach a person as a form they cannot answer, and the item would sit in state `question` indefinitely. Checking at write time turns that into an immediate error for the agent.

Example: an agent asks "Which schema version should the migration target?" with a radio field `version` offering `v2` and `v3`; a form with a radio field and no options is refused with "field 'version': type 'radio' requires options".
