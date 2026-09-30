---
id: 59370797-7ee4-4766-8364-b74aa16960f3
name: "graph-testresult"
label: "Records a test result"
kind: request-reply
description: "Records a test run's verdict — green, red, error or skipped, with its counts — on one test-group vertex of the program map, where it stays across later map syncs."
owner: bespoke
teststate: inherit
children:
  bizreqs:    []
  techreqs:   []
  tests:      []
---

# graph-testresult

`POST /api/projects/{p}/graph/nodes/{id}/testresult` — records a test run's verdict — green, red, error or skipped, with its counts — on one test-group vertex of the program map, where it stays across later map syncs.

## Request

```json
{
  "result": "red",
  "counts": "41/42",
  "groups": [
    {
      "label": "iter_data-unit",
      "result": "red",
      "pass": 41,
      "total": 42,
      "failing": [
        "t1"
      ]
    }
  ],
  "workid": "e84f…"
}
```

## Reply, success shape

```json
{
  "id": "7c3a…",
  "test": {
    "result": "red",
    "counts": "41/42",
    "lastrun": "2026-09-28T19:00:00Z",
    "by": "engine01"
  }
}
```

## Reply, failure shape

```json
{
  "refusal": {
    "code": "BAD_RESULT",
    "detail": {
      "error": "result must be green | red | error | skipped"
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
      "result": "red",
      "counts": "41/42",
      "groups": [
        {
          "label": "iter_data-unit",
          "result": "red",
          "pass": 41,
          "total": 42,
          "failing": [
            "t1"
          ]
        }
      ],
      "workid": "e84f…"
    },
    "reply": {
      "id": "7c3a…",
      "test": {
        "result": "red",
        "counts": "41/42",
        "lastrun": "2026-09-28T19:00:00Z",
        "by": "engine01"
      }
    }
  },
  {
    "request": {
      "result": "red",
      "counts": "41/42",
      "groups": [
        {
          "label": "iter_data-unit",
          "result": "red",
          "pass": 41,
          "total": 42,
          "failing": [
            "t1"
          ]
        }
      ],
      "workid": "e84f…"
    },
    "reply": {
      "refusal": {
        "code": "BAD_RESULT",
        "detail": {
          "error": "result must be green | red | error | skipped"
        }
      }
    }
  }
]
```
