---
id: 154e052e-26ce-40d2-bf40-87a7846ca1eb
name: "mcp-call"
label: "Calls an iter tool"
kind: request-reply
description: "Lets an MCP client call one iter tool (work items, the map, GraphRAG) in a single stateless JSON-RPC request authenticated with the caller's own bearer token."
owner: bespoke
teststate: inherit
children:
  bizreqs:    []
  techreqs:   []
  tests:      []
---

# mcp-call

`POST /mcp` — JSON-RPC 2.0 `tools/call` (also `initialize`, `tools/list`, `ping`); headers `Authorization: Bearer <token>`, optional `X-Iter-Project`, `X-Iter-Workid`.

## Request

```json
{
  "jsonrpc": "2.0",
  "id": 7,
  "method": "tools/call",
  "params": {
    "name": "rag_search",
    "arguments": {
      "query": "where are locks released?",
      "k": 3
    }
  }
}
```

## Reply, success shape

```json
{
  "jsonrpc": "2.0",
  "id": 7,
  "result": {
    "isError": false,
    "content": [
      {
        "type": "text",
        "text": "{\"query\": \"where are locks released?\", \"results\": […]}"
      }
    ],
    "structuredContent": {
      "query": "where are locks released?",
      "results": []
    }
  }
}
```

## Reply, failure shape

```json
{
  "refusal": {
    "code": "UNKNOWN_TOOL",
    "detail": {
      "jsonrpc": "2.0",
      "id": 7,
      "error": {
        "code": -32602,
        "message": "unknown tool \"rag_serch\""
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
      "jsonrpc": "2.0",
      "id": 7,
      "method": "tools/call",
      "params": {
        "name": "rag_search",
        "arguments": {
          "query": "where are locks released?",
          "k": 3
        }
      }
    },
    "reply": {
      "jsonrpc": "2.0",
      "id": 7,
      "result": {
        "isError": false,
        "content": [
          {
            "type": "text",
            "text": "{\"query\": \"where are locks released?\", \"results\": […]}"
          }
        ],
        "structuredContent": {
          "query": "where are locks released?",
          "results": []
        }
      }
    }
  },
  {
    "request": {
      "jsonrpc": "2.0",
      "id": 7,
      "method": "tools/call",
      "params": {
        "name": "rag_search",
        "arguments": {
          "query": "where are locks released?",
          "k": 3
        }
      }
    },
    "reply": {
      "refusal": {
        "code": "UNKNOWN_TOOL",
        "detail": {
          "jsonrpc": "2.0",
          "id": 7,
          "error": {
            "code": -32602,
            "message": "unknown tool \"rag_serch\""
          }
        }
      }
    }
  }
]
```
