---
id: 3f0f8e0e-6a51-4d3c-9d0b-2f4a7c1e9b21
name: "webui-page"
label: "Uses the web page"
kind: request-reply
description: "Serves the browser page (Intro, Work queue, Project graph) to a person, who signs in and then reads and steers the work through it."
owner: bespoke
teststate: inherit
children:
  bizreqs:    []
  techreqs:   []
  tests:      []
---

# webui-page

A person opens the page in a browser and signs in; every later click is one of the page's calls to the data server.

## Request

```json
{"method": "GET", "path": "/", "hash": "#tab=queue&p=iter4"}
```

## Reply, success shape

```json
{"status": 200, "content_type": "text/html", "tabs": ["Intro", "Work queue", "Project graph"]}
```

## Reply, failure shape

```json
{"refusal": {"code": "UNAUTHORIZED", "detail": "sign in first"}}
```

## Worked examples

Normative — each pair must hold on every implementation (strict JSON):

```json
[
  {"request": {"method": "GET", "path": "/", "hash": "#tab=graph&p=iter4"}, "reply": {"status": 200, "content_type": "text/html", "tabs": ["Intro", "Work queue", "Project graph"]}},
  {"request": {"method": "GET", "path": "/api/projects", "token": ""}, "reply": {"refusal": {"code": "UNAUTHORIZED", "detail": "sign in first"}}}
]
```
