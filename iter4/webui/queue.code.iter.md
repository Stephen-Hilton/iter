---
id: 78ce641c-b5ab-403e-b761-2ae333880816
name: "Work queue page"
description: "Shows every work item of the selected project as tiles nested under what they wait on, opens an item's full history, lets a person answer an agent's question, change state or priority, and edit project, agent, engine and user settings, so that people can watch and steer the agents' work from a browser."
simple_description: "The main screen for following the AI workers' tasks, answering their questions and changing settings."
level: component
owner: bespoke
teststate: inherit
children:
  codedirs:   ["{topdir}/webui/index.html"]
  codenodes:  []
  inputs:     ["{topdir}/interfaces/workitem-create/workitem-create.interface.iter.md", "{topdir}/interfaces/graph-neighbors/graph-neighbors.interface.iter.md"]
  outputs:    []
  bizreqs:    []
  techreqs:   []
  tests:      []
---

# Long Description

The Work queue page is the main screen of the web app. It shows what the agents are doing and lets people act on it.

How it works (`webui/index.html`): `boot` handles sign-in; `load` fetches projects, engines, agents and the signed-in user and draws the side panel: projects, the data server, each engine with its state and account usage, and an Accounts list (`renderAccounts`). `loadItems` fetches the project's work items and any deadlocks and draws the tiles (`renderItems`, `tileHtml`). Items are nested under the item they wait on or the running item that holds their lock (`runGroup`, `depStatus`, `lockHolderOf`), sorted by run order (`comparator`), with progress chips per use case and tags such as "blocked by: …". The page refreshes every 10 seconds unless a dialog is open.

Clicking a tile opens the lightbox (`openDetail`): request, every detail row (responses, verifier rows, spend, docs), and question widgets rendered as forms (`widgetForm`) so a person can answer an agent or the close gate. The tile menu (`menuItems`, `doAction`) changes state, priority, tags, clones or reopens an item, or asks for an ELI5 ("explain like I'm five") summary. `openForm` and `createItem` file new items. Gear buttons open settings dialogs for the project, agents, tooling, engines, users and the data server (`settingsBox`). The Architecture map side panel shows map counts (`renderMapStats`) and a browse dialog (`openMap`) that lists a node's links in and out through the neighbours route.

It talks only to the data server's API (`api`); the Intro tab and the Project graph viewer are separate files this page loads and switches between (`setTab`).

Why it matters: it is how a human answers questions, approves work and spots stuck items; without it the queue runs blind.

Example: an agent asks "Keep the old endpoint?". The item's tile shows state question; the developer opens it, picks an answer in the widget and saves, and the item queues again.
