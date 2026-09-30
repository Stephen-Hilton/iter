---
id: 241c96a5-2100-4349-b9c4-d99308c45ec5
name: "rag-nodes-sync"
label: "Re-indexes changed node files"
kind: request-reply
description: "Sends the node files that changed since the last sync, already chunked and embedded by the engine, plus every current node path so the documents of deleted files are dropped; iter_data stores them, keeping the summaries of unchanged chunks."
owner: bespoke
teststate: inherit
children:
  bizreqs:    []
  techreqs:   []
  tests:      []
---

# rag-nodes-sync

`PUT /api/projects/{p}/rag/nodes` — sent by `iter rag sync` in batches of 25; each node carries `chapters` and `chunks` (text + 384-number `vec_raw`) and the model `model` stamp; the `keep` list rides on the last call.

## Request

```json
{
  "nodes": [
    {
      "path": "{topdir}/iter_engine/src/rag.code.iter.md",
      "node_id": "4b7e…",
      "nodetype": "code",
      "name": "GraphRAG engine worker",
      "description": "Keeps the GraphRAG index in step …",
      "text": "---\nid: 4b7e…\n---\n\n# Long Description\n…",
      "hash": "9c1d…"
    }
  ],
  "keep": [
    "{topdir}/iter_engine/src/rag.code.iter.md",
    "{topdir}/main.iter.md"
  ]
}
```

## Reply, success shape

```json
{
  "added": 1,
  "changed": 0,
  "unchanged": 0,
  "removed": 0,
  "failed": []
}
```

## Reply, failure shape

```json
{
  "refusal": {
    "code": "NOT_A_NODE_FILE",
    "detail": {
      "added": 0,
      "changed": 0,
      "unchanged": 0,
      "removed": 0,
      "failed": [
        {
          "path": "{topdir}/docs/notes.md",
          "error": "not a *.iter.md node file"
        }
      ]
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
      "nodes": [
        {
          "path": "{topdir}/iter_engine/src/rag.code.iter.md",
          "node_id": "4b7e…",
          "nodetype": "code",
          "name": "GraphRAG engine worker",
          "description": "Keeps the GraphRAG index in step …",
          "text": "---\nid: 4b7e…\n---\n\n# Long Description\n…",
          "hash": "9c1d…"
        }
      ],
      "keep": [
        "{topdir}/iter_engine/src/rag.code.iter.md",
        "{topdir}/main.iter.md"
      ]
    },
    "reply": {
      "added": 1,
      "changed": 0,
      "unchanged": 0,
      "removed": 0,
      "failed": []
    }
  },
  {
    "request": {
      "nodes": [
        {
          "path": "{topdir}/iter_engine/src/rag.code.iter.md",
          "node_id": "4b7e…",
          "nodetype": "code",
          "name": "GraphRAG engine worker",
          "description": "Keeps the GraphRAG index in step …",
          "text": "---\nid: 4b7e…\n---\n\n# Long Description\n…",
          "hash": "9c1d…"
        }
      ],
      "keep": [
        "{topdir}/iter_engine/src/rag.code.iter.md",
        "{topdir}/main.iter.md"
      ]
    },
    "reply": {
      "refusal": {
        "code": "NOT_A_NODE_FILE",
        "detail": {
          "added": 0,
          "changed": 0,
          "unchanged": 0,
          "removed": 0,
          "failed": [
            {
              "path": "{topdir}/docs/notes.md",
              "error": "not a *.iter.md node file"
            }
          ]
        }
      }
    }
  }
]
```
