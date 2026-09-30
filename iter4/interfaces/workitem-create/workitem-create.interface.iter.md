---
id: 7b786985-52f4-44f0-ba51-e62b85a83e33
name: "workitem-create"
label: "Files a work item"
kind: request-reply
description: "Files a new work item in a project, filling in its priority band and inherited use-case tag; when an open item already carries the same check: and container: tags, it records the repeat on that item and returns it instead of creating a second one."
owner: bespoke
teststate: inherit
children:
  bizreqs:    []
  techreqs:   []
  tests:      []
---

# workitem-create

`POST /api/projects/{p}/workitems` — files a new work item in a project, filling in its priority band and inherited use-case tag; when an open item already carries the same check: and container: tags, it records the repeat on that item and returns it instead of creating a second one.

## Request

```json
{
  "name": "Tests non-green: testgroup \"iter_data-unit\" 41/42",
  "agent": "code",
  "state": "queued",
  "lockdirs": [
    "{topdir}/iter_data/"
  ],
  "tags": [
    {
      "text": "check:tests-non-green"
    },
    {
      "text": "container:iter_data-unit"
    }
  ],
  "request": "…"
}
```

## Reply, success shape

```json
{
  "id": "e84f…",
  "version": 1,
  "state": "queued",
  "priority": 40
}
```

## Reply, failure shape

```json
{
  "refusal": {
    "code": "ALREADY_OPEN",
    "detail": {
      "already_open": true,
      "id": "e84f…",
      "repeats": 2
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
      "name": "Tests non-green: testgroup \"iter_data-unit\" 41/42",
      "agent": "code",
      "state": "queued",
      "lockdirs": [
        "{topdir}/iter_data/"
      ],
      "tags": [
        {
          "text": "check:tests-non-green"
        },
        {
          "text": "container:iter_data-unit"
        }
      ],
      "request": "…"
    },
    "reply": {
      "id": "e84f…",
      "version": 1,
      "state": "queued",
      "priority": 40
    }
  },
  {
    "request": {
      "name": "Tests non-green: testgroup \"iter_data-unit\" 41/42",
      "agent": "code",
      "state": "queued",
      "lockdirs": [
        "{topdir}/iter_data/"
      ],
      "tags": [
        {
          "text": "check:tests-non-green"
        },
        {
          "text": "container:iter_data-unit"
        }
      ],
      "request": "…"
    },
    "reply": {
      "refusal": {
        "code": "ALREADY_OPEN",
        "detail": {
          "already_open": true,
          "id": "e84f…",
          "repeats": 2
        }
      }
    }
  }
]
```
