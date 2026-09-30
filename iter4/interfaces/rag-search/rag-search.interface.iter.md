---
id: 29b70014-2fc9-4215-ae42-ca01c7c8ebe5
name: "rag-search"
label: "Searches the project's documents"
kind: request-reply
description: "Finds the chunks of a project's documents and node files closest in meaning to a plain-language question and returns each full chunk with its summaries and, for node files, its neighbours in the map."
owner: bespoke
teststate: inherit
children:
  bizreqs:    []
  techreqs:   []
  tests:      []
---

# rag-search

`POST /api/projects/{p}/rag/search` — embeds the question with all-MiniLM-L6-v2 and scores every chunk by the better of its raw-text and summary vectors.

## Request

```json
{
  "query": "how does the engine claim a graph edit?",
  "k": 2,
  "kinds": [
    "node"
  ],
  "nodetypes": [
    "code"
  ]
}
```

## Reply, success shape

```json
{
  "query": "how does the engine claim a graph edit?",
  "method": "exact",
  "results": [
    {
      "id": "n3f0c9…_00001",
      "doc": "n3f0c9…",
      "heading": "Long Description",
      "text": "The graph edit inbox is where a change made on the Project graph waits for an engine. …",
      "summary": "Graph edits wait as pending datasync rows; engines learn of them from the heartbeat and claim one each.",
      "score": 0.58,
      "matched": "summary",
      "document": {
        "title": "Graph edit inbox",
        "kind": "node",
        "nodetype": "code",
        "path": "{topdir}/iter_data/src/datasync.code.iter.md"
      },
      "graph": {
        "neighbours": [
          {
            "dir": "out",
            "kind": "outputs",
            "name": "datasync-claim",
            "nodetype": "interface"
          }
        ]
      }
    }
  ]
}
```

## Reply, failure shape

```json
{
  "refusal": {
    "code": "EMBEDDING_UNAVAILABLE",
    "detail": {
      "error": "embedding model all-MiniLM-L6-v2 not found (looked in […]); run iter4/tools/fetch_model.sh or pass --embed-model"
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
      "query": "how does the engine claim a graph edit?",
      "k": 2,
      "kinds": [
        "node"
      ],
      "nodetypes": [
        "code"
      ]
    },
    "reply": {
      "query": "how does the engine claim a graph edit?",
      "method": "exact",
      "results": [
        {
          "id": "n3f0c9…_00001",
          "doc": "n3f0c9…",
          "heading": "Long Description",
          "text": "The graph edit inbox is where a change made on the Project graph waits for an engine. …",
          "summary": "Graph edits wait as pending datasync rows; engines learn of them from the heartbeat and claim one each.",
          "score": 0.58,
          "matched": "summary",
          "document": {
            "title": "Graph edit inbox",
            "kind": "node",
            "nodetype": "code",
            "path": "{topdir}/iter_data/src/datasync.code.iter.md"
          },
          "graph": {
            "neighbours": [
              {
                "dir": "out",
                "kind": "outputs",
                "name": "datasync-claim",
                "nodetype": "interface"
              }
            ]
          }
        }
      ]
    }
  },
  {
    "request": {
      "query": "how does the engine claim a graph edit?",
      "k": 2,
      "kinds": [
        "node"
      ],
      "nodetypes": [
        "code"
      ]
    },
    "reply": {
      "refusal": {
        "code": "EMBEDDING_UNAVAILABLE",
        "detail": {
          "error": "embedding model all-MiniLM-L6-v2 not found (looked in […]); run iter4/tools/fetch_model.sh or pass --embed-model"
        }
      }
    }
  }
]
```
