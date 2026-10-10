---
id: dd366398-147d-450e-a792-59ef7ae020bc
projectname: "iter4"
projectdescription: "The iter harness, version 4: one ArangoDB-backed data server, local engines that run headless Claude agents, and a map of the program built from its own *.iter.md files."
globalscandirs: ["{topdir}/"]
globalinterfacedir: "{topdir}/interfaces/"
globalusecasedir: "{topdir}/usecases/"
globalcontextfiles: ["{topdir}/reqs/iter4.bizreq.iter.md", "{topdir}/reqs/iter4.techreq.iter.md"]
actorsfile: "{topdir}/map/actors.yaml"
children:
  codenodes: ["{topdir}/map/orchestration/orchestration.code.iter.md", "{topdir}/map/engine/engine.code.iter.md", "{topdir}/map/shared/shared.code.iter.md", "{topdir}/map/delivery/delivery.code.iter.md"]
---
# iter4

iter keeps AI coding agents working on a real software project without letting
them trip over each other. A person, or an agent, files a **work item**: "make
this change, in this part of the code". An **engine** — a program running next
to a copy of the code — picks the most urgent item it can start, locks the
folders the item will touch, starts a headless Claude Code session (or a plain
shell command) to do the work, has a second model check the result against the
request (the **close gate**), commits just those files, and moves on. Many
engines can serve one project; they coordinate through one central **data
server**, which holds every work item, lock, agent definition, schedule, user
and cost record and serves the web page people use to watch and steer.

## The four areas

- **Orchestration and Data** (`map/orchestration/`) — the data server,
  `iter_data`: the only program that touches the database, and the HTTP API
  everyone else calls; and the web page it serves (`webui`): an introduction,
  the work queue, and the project graph.
- **Engines and checkout tools** (`map/engine/`) — `iter_engine`, which runs
  the work and is also the `iter` command line, and `iter_local`, which reads
  and writes the project's files.
- **Shared rules and types** (`map/shared/`) — `iter_core`, the rulebook both
  the server and the engines are built with, so they decide things the same way.
- **Build, ship and prove** (`map/delivery/`) — the all-in-one container,
  `deploy.sh` and the end-to-end test suite.

## What version 4 adds

The data server now stores everything in **ArangoDB Community Edition**
(documents, counters and a graph in one database), normally in the same
container as the server, so a project runs on a laptop in Docker Desktop or on
a small cloud VM with one command. And the server now holds a **map of the
program**: every `*.iter.md` file in the checkout — code nodes, interfaces, use
cases, requirements, test groups — becomes a vertex with a stable id, and every
`children` link between them becomes an edge. With the map, iter can answer
questions the folder tree cannot ("which part owns this file", "what does this
use case touch", "which tests cover this interface"), draw the program in the
web page, let people add parts from that drawing, and run every test group on
a schedule, filing one work item for each group that goes red.

## Why this file exists

This repository is iter4's own checkout, and iter4 maps itself. This file is
the root of the map: its `children` list the four areas, each area file lists
its containers, and each container lists its components. `iter sync` pushes the
whole description to the data server, and the Project graph tab draws it.
