---
id: 33be6c5a-f64a-4698-83aa-5ecb607e381e
name: "graph-view"
label: "Reads the map to draw"
kind: request-reply
description: "Returns the program map reshaped for the Project graph viewer: contexts, containers and components with their parents, ownership links, interface connections between parts, actors, and each use case's numbered steps."
owner: bespoke
teststate: inherit
children:
  bizreqs:    []
  techreqs:   []
  tests:      []
---

# graph-view

`GET /api/projects/{p}/graph/view` — returns the program map reshaped for the Project graph viewer: contexts, containers and components with their parents, ownership links, interface connections between parts, actors, and each use case's numbered steps.

## Request

```json
{}
```

## Reply, success shape

```json
{
  "summary": {
    "counts": {
      "nodes": 60
    }
  },
  "nodes": [
    {
      "id": "iter_data",
      "level": "container",
      "parent": "map/data"
    }
  ],
  "edges": [
    {
      "type": "contains",
      "source": "map/data",
      "target": "iter_data"
    }
  ],
  "usecases": []
}
```

## Reply, failure shape

```json
{
  "refusal": {
    "code": "NO_PROJECT",
    "detail": {
      "error": "no project x"
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
    "reply": {
      "summary": {
        "counts": {
          "nodes": 60
        }
      },
      "nodes": [
        {
          "id": "iter_data",
          "level": "container",
          "parent": "map/data"
        }
      ],
      "edges": [
        {
          "type": "contains",
          "source": "map/data",
          "target": "iter_data"
        }
      ],
      "usecases": []
    }
  },
  {
    "request": {},
    "reply": {
      "refusal": {
        "code": "NO_PROJECT",
        "detail": {
          "error": "no project x"
        }
      }
    }
  }
]
```
