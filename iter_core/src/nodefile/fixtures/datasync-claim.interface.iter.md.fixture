---
id: d24755d1-1b93-41d1-a9fe-80fb9e0dea26
name: "datasync-claim"
label: "Claims a waiting graph edit"
kind: request-reply
description: "Lets one engine take a waiting graph edit so it alone applies it; the claim lapses after ten minutes if the engine never reports back."
owner: bespoke
teststate: inherit
children:
  bizreqs:    []
  techreqs:   []
  tests:      []
---

# datasync-claim

`POST /api/projects/{p}/datasync/{id}/claim` — lets one engine take a waiting graph edit so it alone applies it; the engine then reports the outcome with `POST …/datasync/{id}/done`.

## Request

```json
{
  "engine": "StephenMBP"
}
```

## Reply, success shape

```json
{
  "id": "2026-09-29T15:02:11Z-ab12cd34",
  "state": "claimed",
  "engine": "StephenMBP",
  "claim_expires": "2026-09-29T15:12:11Z",
  "attempts": 1
}
```

## Reply, failure shape

```json
{
  "refusal": {
    "code": "ALREADY_CLAIMED",
    "detail": {
      "error": "conflict",
      "current": {
        "state": "claimed",
        "engine": "FHServer"
      }
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
      "engine": "StephenMBP"
    },
    "reply": {
      "id": "2026-09-29T15:02:11Z-ab12cd34",
      "state": "claimed",
      "engine": "StephenMBP",
      "claim_expires": "2026-09-29T15:12:11Z",
      "attempts": 1
    }
  },
  {
    "request": {
      "engine": "StephenMBP"
    },
    "reply": {
      "refusal": {
        "code": "ALREADY_CLAIMED",
        "detail": {
          "error": "conflict",
          "current": {
            "state": "claimed",
            "engine": "FHServer"
          }
        }
      }
    }
  }
]
```
