---
id: b5703965-cba5-4f2a-a434-adff26013938
name: "Orchestration and Data"
description: "Keeps every record iter holds — work items, locks, users, agents, schedules, spend and the program map — behind one HTTP API, and serves the browser page people use to watch and steer the work, so engines and people never touch the database themselves."
simple_description: "The one place where iter's information is kept, and the page where people see and steer the work."
level: context
owner: bespoke
teststate: inherit
children:
  codedirs:   ["{topdir}/iter_data/", "{topdir}/webui/"]
  codenodes:  ["{topdir}/iter_data/iter_data.code.iter.md", "{topdir}/webui/webui.code.iter.md"]
  inputs:     []
  outputs:    []
  bizreqs:    []
  techreqs:   []
  tests:      []
---

# Long Description

The Orchestration and Data area is where iter's work is coordinated: it keeps everything iter knows, it is the only way in or out of it, and it serves the page people use. Work items, their history rows, locks, agent definitions, projects, engines, users, spend and the architecture map (the picture of the program built from its `*.iter.md` files) all live here. It holds two containers: **iter_data — data server** and **webui — web page**.

How it works: iter_data is an HTTP server. A request arrives at the router in `iter_data/src/api.rs: router`, the caller's token is checked (the `AuthUser` extractor, using `iter_data/src/auth.rs`), and the handler applies the queue rules it shares with the engine (from the Shared model area) before it reads or writes through one storage interface, `iter_data/src/storage.rs: Storage`. ArangoDB implements that interface (`arango.rs`) and is the only store iter4 serves from; iter3's DynamoDB tables are read through it once, by the migration (`ddb.rs`). Every write also bumps a per-project, per-table change counter, so clients re-read only what moved. The web page — plain HTML, CSS and JavaScript under `webui/` with three tabs, Intro, Work queue and Project graph — is compiled into the same binary (`iter_data/src/main.rs: WEBUI_FILES`) and calls the same API the engines use.

What goes in and out: people (developers and operators) use the web page; the page and the engines (Engine area) call the data server to heartbeat, claim work, take locks, write results, push the map, and list, create and edit. The server calls nothing but its database. It has no copy of the code, so a change that must touch files — a graph edit from the web page — waits here as a pending row (`iter_data/src/datasync.rs`) until an engine that has the checkout applies it.

Why it matters: with a single door, rules such as "a closed item cannot be edited" or "only one engine wins a lock" hold no matter who asks, and the page holds no rules of its own — if the page and the server disagree, the server wins, so a stale browser tab cannot break the queue.

Example: a developer opens Project graph, picks the iter_data container, chooses "add a child" and names it "Ledger API"; the page posts a graph edit, the server stores it as pending and tells the next engine heartbeat, and the page redraws once that engine has written, committed and synced the file.
