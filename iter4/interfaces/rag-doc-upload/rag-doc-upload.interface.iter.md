---
id: a30c8feb-61a6-43b3-99a6-15189fcc5b52
name: "rag-doc-upload"
label: "Adds a document"
kind: request-reply
description: "Accepts an uploaded document (pdf incl. scans, docx, pptx, html, md, txt) for GraphRAG: an engine then extracts, chunks and embeds it, and the original is written under the project's docs directory."
owner: bespoke
teststate: inherit
children:
  bizreqs:    []
  techreqs:   []
  tests:      []
---

# rag-doc-upload

`POST /api/projects/{p}/rag/docs` — base64 content up to 25 MB; the document waits in state `queued` until an engine ingests it (seconds when one is live); the same file name replaces the earlier document.

## Request

```json
{
  "filename": "Q3 plan (v2).pdf",
  "content_b64": "JVBERi0xLjcK…",
  "store": true
}
```

## Reply, success shape

```json
{
  "id": "f7a1c0…",
  "kind": "file",
  "title": "Q3 plan (v2).pdf",
  "path": "{topdir}/docs/Q3-plan-v2.pdf",
  "state": "queued",
  "chunks": 0,
  "chapters": [
    {
      "idx": 0,
      "title": "Goals",
      "chunks": 5,
      "summary": ""
    }
  ],
  "store": {
    "state": "pending",
    "datasync": "2026-09-29T22:09:28.997077Z-a893c54a"
  }
}
```

## Reply, failure shape

```json
{
  "refusal": {
    "code": "UNSUPPORTED_FORMAT",
    "detail": {
      "error": ".ppt (the old binary Office format) is not supported; save it as .docx/.pptx/.pdf"
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
      "filename": "Q3 plan (v2).pdf",
      "content_b64": "JVBERi0xLjcK…",
      "store": true
    },
    "reply": {
      "id": "f7a1c0…",
      "kind": "file",
      "title": "Q3 plan (v2).pdf",
      "path": "{topdir}/docs/Q3-plan-v2.pdf",
      "state": "summarizing",
      "chunks": 14,
      "chapters": [
        {
          "idx": 0,
          "title": "Goals",
          "chunks": 5,
          "summary": ""
        }
      ],
      "store": {
        "state": "pending",
        "datasync": "2026-09-29T22:09:28.997077Z-a893c54a"
      }
    }
  },
  {
    "request": {
      "filename": "Q3 plan (v2).pdf",
      "content_b64": "JVBERi0xLjcK…",
      "store": true
    },
    "reply": {
      "refusal": {
        "code": "UNSUPPORTED_FORMAT",
        "detail": {
          "error": ".ppt (the old binary Office format) is not supported; save it as .docx/.pptx/.pdf"
        }
      }
    }
  }
]
```
