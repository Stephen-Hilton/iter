---
id: 19c98604-319e-4e9e-aa7b-dd5eeb145b3c
name: "versions-poll"
label: "Checks what changed"
kind: request-reply
description: "Returns one change counter per table of a project, so a client — each engine, every tick — re-reads only the tables whose counter moved since it last looked."
owner: bespoke
teststate: inherit
children:
  bizreqs:    []
  techreqs:   []
  tests:      []
---

# versions-poll

`GET /api/projects/{p}/versions` — returns one change counter per table of a project, so a client — each engine, every tick — re-reads only the tables whose counter moved since it last looked.

## Request

```json
{}
```

## Reply, success shape

```json
[
  {
    "projectname": "iter4",
    "table": "workitem",
    "seq": 48213,
    "updated": "2026-09-29T06:00:00Z"
  }
]
```

## Reply, failure shape

```json
{
  "refusal": {
    "code": "NO_AUTH",
    "detail": {
      "error": "missing bearer token"
    }
  }
}
```

## Worked examples

Normative — each pair must hold on every implementation (strict JSON):

```json
[
  {
    "request": {},
    "reply": [
      {
        "projectname": "iter4",
        "table": "workitem",
        "seq": 48213,
        "updated": "2026-09-29T06:00:00Z"
      }
    ]
  },
  {
    "request": {},
    "reply": {
      "refusal": {
        "code": "NO_AUTH",
        "detail": {
          "error": "missing bearer token"
        }
      }
    }
  }
]
```
