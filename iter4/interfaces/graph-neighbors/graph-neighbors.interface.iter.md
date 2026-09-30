---
id: b7692568-0902-40ba-a28e-5043bfbc247a
name: "graph-neighbors"
label: "Looks up nearby parts"
kind: request-reply
description: "Returns every vertex of the program map within N links of one vertex, following links outward, inward or both ways, each with the kind of link that reached it and how many steps away it is."
owner: bespoke
teststate: inherit
children:
  bizreqs:    []
  techreqs:   []
  tests:      []
---

# graph-neighbors

`GET /api/projects/{p}/graph/nodes/{id}/neighbors?depth=N&direction=out|in|any` — returns every vertex of the program map within N links of one vertex, following links outward, inward or both ways, each with the kind of link that reached it and how many steps away it is.

## Request

```json
{
  "depth": 2,
  "direction": "out"
}
```

## Reply, success shape

```json
{
  "start": "0d7f…",
  "neighbors": [
    {
      "vertex": {
        "id": "6b1e0c2a-…",
        "nodetype": "code"
      },
      "via": "codenodes",
      "depth": 1
    }
  ]
}
```

## Reply, failure shape

```json
{
  "refusal": {
    "code": "NO_VERTEX",
    "detail": {
      "error": "no vertex 1234… in iter4"
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
      "depth": 2,
      "direction": "out"
    },
    "reply": {
      "start": "0d7f…",
      "neighbors": [
        {
          "vertex": {
            "id": "6b1e0c2a-…",
            "nodetype": "code"
          },
          "via": "codenodes",
          "depth": 1
        }
      ]
    }
  },
  {
    "request": {
      "depth": 2,
      "direction": "out"
    },
    "reply": {
      "refusal": {
        "code": "NO_VERTEX",
        "detail": {
          "error": "no vertex 1234… in iter4"
        }
      }
    }
  }
]
```
