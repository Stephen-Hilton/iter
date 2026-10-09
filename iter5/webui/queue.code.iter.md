---
id: 78ce641c-b5ab-403e-b761-2ae333880816
name: "Work queue page"
desc: "Shows every work item of the selected project as tiles nested under what they wait on, opens an item's full history, lets a person answer an agent's question, change state or priority, ask for an ELI5 explanation, and run or stop the project; the side panel shows engines with their account usage, deadlocks and the project graph's counts and pending file writes, so people can watch and steer the agents' work from a browser."
creator: "iter migrate5"
teststate: inherit
level: component
owner: bespoke
children:
  codedirs:  ["{topdir}/webui/index.html"]
  codenodes: []
  tests:     []
  reqs:      []
timestamps: {create: "2026-10-02 23:05:35Z", last_modified: "2026-10-02 23:15:00Z", last_tested: ""}
---

# Work queue page

## Summary

The main screen for following the AI workers' tasks, answering their questions and running or stopping a project.

## How it works (`webui/index.html`)

`boot` handles sign-in; `load` fetches projects, engines, agents and the signed-in user and draws the side panel: each engine with its state, held accounts and usage (`renderAccounts`; "Suspended, no usage left" when an engine is holding), the deadlock rows (`renderDeadlocks`: a red row per wait loop, an amber row for locks held by items not running) and the project graph's counts by node type with how many nodes wait for an engine to write their file and how many are still only designed (`renderMapStats`, from `GET …/graph/stats`; "Open" jumps to the Project graph). `loadItems` fetches the project's work items and draws the tiles (`renderItems`, `tileHtml`), nested under the item they wait on or the running item that holds their lock, sorted by run order (`comparator`), with tags such as "blocked by: …" and "⟲ cycle". The side panel is shared by every tab and can be hidden per tab (‹, brought back with ›). Its Accounts section re-reads every account's 5h / 7d usage on opening, every 15 minutes and on ↻ (`POST /api/engines/{e}/probe`; the engine answers on its next tick); agents and tooling are edited in the settings graph. The side panel holds the Active | Stopped switches — the iter_data server (the master switch), each project (its commanded state, Draining while running work finishes), each engine and each account — and says why an Active project still starts nothing (server Stopped, no engine serves it, its engines Stopped); the header line names what keeps the selected project from starting work. The page refreshes every 10 seconds unless a dialog is open.

Clicking a tile opens the detail lightbox (`openDetail`): the request, every detail row (responses, verifier rows, spend, docs), and question widgets rendered as forms (`widgetForm`) so a person can answer an agent or the close gate. The tile menu (`menuItems`, `doAction`) changes state, priority, tags, clones, reopens, pauses a schedule (the engine-owned test sweep can be paused, never deleted) or asks for an ELI5 explanation. `openForm` / `createItem` file new items. Gear buttons open record forms for the project, agents, tooling, engines and users (`settingsBox`); which engine serves a project and which accounts bill it are edited in the Settings graph instead.

## What goes in and out

It talks only to the data server's API (`api`). The other tabs are separate files this page loads and switches between (`setTab`).

## Why it matters

It is how a human answers questions, approves work and spots stuck items; without it the queue runs blind.

## Example

An agent asks "Keep the old endpoint?". The item's tile shows state question; the developer opens it, picks an answer in the widget and saves, and the item queues again.
