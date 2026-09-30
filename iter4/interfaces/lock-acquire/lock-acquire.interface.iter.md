---
id: fb5fac13-6101-4790-ab18-830a6e61b3df
name: "lock-acquire"
label: "Locks a folder"
kind: request-reply
description: "Gives a running work item a lock on one folder path when the path is free, expired or already held by that item, and refuses when another live run holds it or the asking item is not running."
owner: bespoke
teststate: inherit
children:
  bizreqs:    []
  techreqs:   []
  tests:      []
---

# lock-acquire

`POST /api/projects/{p}/locks/acquire` — gives a running work item a lock on one folder path when the path is free, expired or already held by that item, and refuses when another live run holds it or the asking item is not running.

## Request

```json
{
  "path": "{topdir}/iter_data/",
  "workid": "e84f…"
}
```

## Reply, success shape

```json
{
  "path": "{topdir}/iter_data/",
  "workid": "e84f…",
  "expires": "2026-09-28T20:05:00Z"
}
```

## Reply, failure shape

```json
{
  "refusal": {
    "code": "LOCK_HELD",
    "detail": {
      "error": "conflict",
      "current": {
        "path": "{topdir}/iter_data/",
        "workid": "5d68…"
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
      "path": "{topdir}/iter_data/",
      "workid": "e84f…"
    },
    "reply": {
      "path": "{topdir}/iter_data/",
      "workid": "e84f…",
      "expires": "2026-09-28T20:05:00Z"
    }
  },
  {
    "request": {
      "path": "{topdir}/iter_data/",
      "workid": "e84f…"
    },
    "reply": {
      "refusal": {
        "code": "LOCK_HELD",
        "detail": {
          "error": "conflict",
          "current": {
            "path": "{topdir}/iter_data/",
            "workid": "5d68…"
          }
        }
      }
    }
  }
]
```
