---
id: d9ca9817-f330-4097-9495-753a37b069c6
name: "graph-edits"
label: "Sends a map edit"
kind: request-reply
description: "Accepts one change to the program map from the web page — a new node, a connection, a global requirement or use case, or new text for a node — as a pending edit an engine applies in its checkout; run_tests is queued as a test work item instead."
owner: bespoke
teststate: inherit
children:
  bizreqs:    []
  techreqs:   []
  tests:      []
---

# graph-edits

`POST /api/projects/{p}/graph/edits` — accepts one change to the program map from the web page — a new node, a connection, a global requirement or use case, or new text for a node — as a pending edit an engine applies in its checkout; run_tests is queued as a test work item instead.

## Request

```json
{
  "op": "new_node",
  "kind": "container",
  "name": "Ledger API",
  "description": "Answers balance reads for the ledger.",
  "parent": "{topdir}/data/data.code.iter.md"
}
```

## Reply, success shape

```json
{
  "id": "2026-09-29T15:02:11Z-ab12cd34",
  "state": "pending",
  "summary": "+ container Ledger API",
  "lockdirs": [
    "{topdir}/data/data.code.iter.md",
    "{topdir}/data/ledger_api"
  ]
}
```

## Reply, failure shape

```json
{
  "refusal": {
    "code": "UNKNOWN_OP",
    "detail": {
      "error": "unknown op \"frobnicate\""
    }
  }
}
```

## Worked examples

Normative — each pair must hold on every implementation (strict JSON):

```json
[
  {
    "request": {
      "op": "new_node",
      "kind": "container",
      "name": "Ledger API",
      "description": "Answers balance reads for the ledger.",
      "parent": "{topdir}/data/data.code.iter.md"
    },
    "reply": {
      "id": "2026-09-29T15:02:11Z-ab12cd34",
      "state": "pending",
      "summary": "+ container Ledger API",
      "lockdirs": [
        "{topdir}/data/data.code.iter.md",
        "{topdir}/data/ledger_api"
      ]
    }
  },
  {
    "request": {
      "op": "new_node",
      "kind": "container",
      "name": "Ledger API",
      "description": "Answers balance reads for the ledger.",
      "parent": "{topdir}/data/data.code.iter.md"
    },
    "reply": {
      "refusal": {
        "code": "UNKNOWN_OP",
        "detail": {
          "error": "unknown op \"frobnicate\""
        }
      }
    }
  }
]
```
