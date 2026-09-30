---
id: d7e17757-778a-469c-8d26-667e00e99336
name: "graph-sync"
label: "Uploads the map"
kind: request-reply
description: "Replaces a project's stored program map with the snapshot an engine built from its checkout, keeping each test group's last result, and answers with added, changed and removed counts — or unchanged when the snapshot's content hash matches the stored one."
owner: bespoke
teststate: inherit
children:
  bizreqs:    []
  techreqs:   []
  tests:      []
---

# graph-sync

`PUT /api/projects/{p}/graph` — replaces a project's stored program map with the snapshot an engine built from its checkout, keeping each test group's last result, and answers with added, changed and removed counts — or unchanged when the snapshot's content hash matches the stored one.

## Request

```json
{
  "hash": "9f2c…",
  "vertices": [
    {
      "id": "6b1e0c2a-…",
      "nodetype": "code",
      "name": "iter_data — the data server",
      "path": "{topdir}/iter_data/iter_data.code.iter.md",
      "level": "container",
      "codedirs": [
        "{topdir}/iter_data/"
      ],
      "vhash": "a41…"
    }
  ],
  "edges": [
    {
      "from": "0d7f…",
      "kind": "codenodes",
      "to": "6b1e0c2a-…"
    }
  ]
}
```

## Reply, success shape

```json
{
  "unchanged": false,
  "hash": "9f2c…",
  "counts": {
    "added": 1,
    "changed": 0,
    "unchanged": 38,
    "removed": 0,
    "edges": 44,
    "edges_dropped": 0
  }
}
```

## Reply, failure shape

```json
{
  "refusal": {
    "code": "DUPLICATE_ID",
    "detail": {
      "error": "duplicate vertex id 6b1e0c2a-… (run `iter ids --fix`)"
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
      "hash": "9f2c…",
      "vertices": [
        {
          "id": "6b1e0c2a-…",
          "nodetype": "code",
          "name": "iter_data — the data server",
          "path": "{topdir}/iter_data/iter_data.code.iter.md",
          "level": "container",
          "codedirs": [
            "{topdir}/iter_data/"
          ],
          "vhash": "a41…"
        }
      ],
      "edges": [
        {
          "from": "0d7f…",
          "kind": "codenodes",
          "to": "6b1e0c2a-…"
        }
      ]
    },
    "reply": {
      "unchanged": false,
      "hash": "9f2c…",
      "counts": {
        "added": 1,
        "changed": 0,
        "unchanged": 38,
        "removed": 0,
        "edges": 44,
        "edges_dropped": 0
      }
    }
  },
  {
    "request": {
      "hash": "9f2c…",
      "vertices": [
        {
          "id": "6b1e0c2a-…",
          "nodetype": "code",
          "name": "iter_data — the data server",
          "path": "{topdir}/iter_data/iter_data.code.iter.md",
          "level": "container",
          "codedirs": [
            "{topdir}/iter_data/"
          ],
          "vhash": "a41…"
        }
      ],
      "edges": [
        {
          "from": "0d7f…",
          "kind": "codenodes",
          "to": "6b1e0c2a-…"
        }
      ]
    },
    "reply": {
      "refusal": {
        "code": "DUPLICATE_ID",
        "detail": {
          "error": "duplicate vertex id 6b1e0c2a-… (run `iter ids --fix`)"
        }
      }
    }
  }
]
```
