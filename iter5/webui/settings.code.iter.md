---
id: d12996da-9a77-4fa0-a5c4-708b45515dcb
name: "Settings graph"
desc: "The Settings tab: draws iter5's settings graph — iter_data, engines, projects, accounts, providers, agents, agent tooling, users and work-item states, each with a _deactivated placeholder — and the edges that carry the settings joining them (serves with a topdir, bills with switch/stop percents, holds, member, runs…); admins create nodes, configure nodes and edges, draw, tag, copy and drag edges, and drop an edge end on a placeholder to switch it off."
creator: "agent.code"
teststate: inherit
level: component
owner: bespoke
children:
  codedirs:  ["{topdir}/webui/settings.js"]
  codenodes: []
  tests:     []
  reqs:      []
timestamps: {create: "2026-10-02 23:15:00Z", last_modified: "2026-10-02 23:15:00Z", last_tested: ""}
---

# Settings graph

## Summary

Where an admin wires the system together: which engine runs which project from which folder, which account pays for it, who may see it, which agents run on it.

## How it works (`webui/settings.js`)

`IterSettings.mount` draws `GET /api/settings/graph` with the same Cytoscape engine as the Project graph. Nodes are the nine settings types (iter_data, iter_engine, project, workitem_type, agent, agent_tools, user, account, provider), each with its non-deletable `<type>:_deactivated` placeholder; edges show their `tag` as a label (an untagged edge shows its type on hover, when selected or zoomed in). The edge table in the script mirrors `iter_core::settings::EDGE_TYPES`, so the edge type is inferred from the two ends when one is drawn.

Admins can create a node (`POST /api/settings/nodes`), configure a node or an edge in the Configure… lightbox (`PATCH …/nodes/{id}`, `PATCH …/edges/{id}`: settings, tag), draw an edge, drag an edge end onto another node — onto a placeholder to make the edge inactive while keeping its settings — copy an edge and paste it onto another node (`POST …/edges/{id}/copy`, settings and tag come along) and delete nodes or edges (placeholders and `iter_data:self` refuse). Everyone else reads the parts that touch their projects. `IterSettings.focus(id)` centres a node; the Project graph's picker and the wizard link here.

## What goes in and out

Reads and writes only `/api/settings/*`. The engines see the result through `GET /api/engines/{name}/assignments`.

## Why it matters

In the Force layout a dragged node pulls its neighbours along and the graph settles around where it is dropped (`IterKit.springDrag`, weak springs); P pins the selected node in place in any layout. Every edge is a straight line. An engine node does not show its usage report (`accounts`, `usage`, `next`): that is status the engine rewrites each heartbeat, and the account settings live on the bills edges. was a set of lists on several records; here it is one picture, and stopping a project on one engine is a drag, not a config edit.

## Example

An admin draws account `main` → project `pdy`; the edge is typed `bills`, the lightbox sets switch 80 and stop 95, and the next assignments reply lets the engine serving pdy dispatch on `main`.
