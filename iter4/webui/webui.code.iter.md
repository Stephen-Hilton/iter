---
id: 7b63d1a2-84f0-405e-9dbf-691237a04a6a
name: "webui — web page"
description: "Shows people the work queue, the program map and an introduction in one browser page, and sends their requests — new items, answers, edits to the map — to the data server, which makes every decision."
simple_description: "The page in the browser where people see and steer the work."
level: container
owner: bespoke
teststate: omit
children:
  codedirs:   ["{thisfiledir}/"]
  codenodes:  ["{topdir}/webui/queue.code.iter.md", "{topdir}/webui/projectgraph.code.iter.md", "{topdir}/webui/grapheditor.code.iter.md", "{topdir}/webui/intro.code.iter.md", "{topdir}/webui/rag.code.iter.md"]
  inputs:     []
  outputs:    ["{topdir}/interfaces/webui-page/webui-page.interface.iter.md"]
  bizreqs:    []
  techreqs:   []
  tests:      []
---

# Long Description

webui is the single web page people use. It is static HTML, CSS and JavaScript with no server code of its own; iter_data serves it.

How it works: `webui/index.html` holds the login form, the shared header with the project picker, and the three tabs: **Intro** (`intro.js`, `intro.css` — business and technical slides and a new-project wizard), **Work queue** (inside `index.html` — item tiles nested by dependency, the item detail view, question forms, project and engine settings) and **Project graph** (`graph.js`, `graph.css`, drawing with the Cytoscape libraries vendored under `webui/vendor/`, and `graphedit.js` for the add-and-connect forms). At build time `iter_data/src/main.rs` compiles every one of these files into the server binary, so a container needs no web folder (`./deploy.sh local` serves them from disk instead, for fast editing). The page logs in with `POST /auth/login`, keeps the token, and reloads the selected project's items every ten seconds.

What goes in and out: it calls only iter_data's HTTP API — work items and their details, engines, projects, `/graph/view` for the map, `/graph/edits` for changes. Nothing calls it.

Why it matters: it is how a human files work, answers an agent's question, accepts or reopens a result, and sees which engine is running what. It holds no rules: a stale tab can ask for something, but the server still refuses what the rules refuse.

Example: opening `http://127.0.0.1:8300/#tab=graph&p=iter4` logs in, selects project iter4 and draws its program map with contexts as enclosing areas and containers inside them.
