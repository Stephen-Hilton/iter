---
id: 367a9d16-4266-4c23-9b3f-fd6550a6ae2c
name: "rag-work-claim"
label: "Takes summary work"
kind: request-reply
description: "Lets one engine take the next batch of chunks (or one document rollup) for the Summary agent; the claim lapses after ten minutes if the engine never reports back with POST …/rag/work/done."
owner: bespoke
teststate: inherit
children:
  bizreqs:    []
  techreqs:   []
  tests:      []
---

# rag-work-claim

`POST /api/projects/{p}/rag/work/claim` — at most 8 chunks of one document (12,000 characters), else one rollup, else `{"job": null}`.

## Request

```json
{
  "engine": "StephenMBP"
}
```

## Reply, success shape

```json
{
  "job": "chunks",
  "job_id": "j5b1e0c2a9d7f3e41",
  "doc": {
    "id": "f489ec3c…",
    "title": "iter4 spec.md",
    "kind": "file",
    "path": "{topdir}/docs/iter4-spec.md"
  },
  "chunks": [
    {
      "id": "f489ec3c…_00000",
      "idx": 0,
      "chapter": 0,
      "heading": "iter4 — requirements and build plan",
      "text": "Written 2026-09-28 from …"
    }
  ]
}
```

## Reply, failure shape

```json
{
  "refusal": {
    "code": "NOT_WRITER",
    "detail": {
      "error": "forbidden"
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
      "job": "chunks",
      "job_id": "j5b1e0c2a9d7f3e41",
      "doc": {
        "id": "f489ec3c…",
        "title": "iter4 spec.md",
        "kind": "file",
        "path": "{topdir}/docs/iter4-spec.md"
      },
      "chunks": [
        {
          "id": "f489ec3c…_00000",
          "idx": 0,
          "chapter": 0,
          "heading": "iter4 — requirements and build plan",
          "text": "Written 2026-09-28 from …"
        }
      ]
    }
  },
  {
    "request": {
      "engine": "StephenMBP"
    },
    "reply": {
      "refusal": {
        "code": "NOT_WRITER",
        "detail": {
          "error": "forbidden"
        }
      }
    }
  }
]
```
