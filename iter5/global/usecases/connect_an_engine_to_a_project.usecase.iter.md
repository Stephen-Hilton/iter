---
id: 43f5efdd-7136-4f8d-8db3-4d4ef198e977
name: "Connect an engine and an account to a project in the settings graph"
desc: "An operator sets up the one engine of a machine with tools/iter_engine_setup.sh (env file with the engine token and account tokens); the engine registers itself and appears in the Settings graph. An admin then draws edges there — serves (engine → project, with topdir), holds (engine → account), bills (account → project, with order and switch/stop %) — and the engine's next assignments read starts running the project with that account. Dragging an edge end onto a _deactivated placeholder stops it and keeps its settings."
creator: "stephen"
teststate: inherit
actors: ["{topdir}/global/usecases/operator.actor.iter.md", "{topdir}/global/usecases/admin.actor.iter.md"]
flowmap:
  summary: "The setup script writes ~/.iter5/.env and starts iter_engine with only a data URL, an env file and a name. The engine self-registers (PUT /api/engines/{name}), which adds a hosts edge and an owns edge from the token's user. In the Settings tab the admin draws serves, holds and bills edges; iter_data validates each edge's endpoint types and stores it in sys_edge. Every tick the engine reads GET /api/engines/{name}/assignments — the projects with an active serves edge (topdir, read_only) and, per project, the accounts that both bill it and are held by the engine, with their provider — and from then on syncs files and asks for work for that project."
  sequence: ["{topdir}/global/usecases/operator.actor.iter.md", "{topdir}/tools/tools.code.iter.md", "{topdir}/iter_engine/src/loop.code.iter.md", "{topdir}/global/usecases/admin.actor.iter.md", "{topdir}/webui/settings.code.iter.md", "{topdir}/iter_data/src/settings.code.iter.md", "{topdir}/iter_engine/src/assign.code.iter.md"]
  process_flow:
    - step: 1
      from: "{topdir}/global/usecases/operator.actor.iter.md"
      to: "{topdir}/tools/tools.code.iter.md"
      what: "On the machine, the operator pipes the setup script from the server with the data URL, an engine name and an engine token minted by an admin; it checks git, curl and claude, finds or builds iter_engine, checks the token, writes ~/.iter5/.env (mode 600) with ITER_ENGINE_TOKEN and the account tokens, and starts the engine."
      plain: "The machine gets its engine."
      evidence: "curl <data_url>/iter_engine_setup.sh | bash -s -- --data-url … --engine … --token … --start"
    - step: 2
      from: "{topdir}/iter_engine/src/loop.code.iter.md"
      to: "{topdir}/iter_data/src/settings.code.iter.md"
      what: "The engine finds no engine record under its name and registers itself; the server adds the iter_engine node to the settings graph with a hosts edge from iter_data and an owns edge from the token's user."
      plain: "The engine shows up in the Settings graph."
      evidence: "iter_engine/src/engine.rs (self-register, PUT /api/engines/{name}) → iter_data/src/settings.rs: on_engine_registered"
    - step: 3
      from: "{topdir}/global/usecases/admin.actor.iter.md"
      to: "{topdir}/webui/settings.code.iter.md"
      what: "In the Settings tab the admin draws a serves edge from the engine to the project and sets its topdir (the checkout folder), a holds edge from the engine to an account, and a bills edge from that account to the project with order, switch and stop percentages; edges can be tagged, copied and pasted."
      plain: "The admin connects engine, account and project."
      evidence: "webui/settings.js: POST /api/settings/edges, PATCH /api/settings/edges/{id}, POST …/{id}/copy"
    - step: 4
      from: "{topdir}/webui/settings.code.iter.md"
      to: "{topdir}/iter_data/src/settings.code.iter.md"
      what: "The server infers or checks the edge type from the endpoint types (a placeholder of the right type counts), refuses non-admin writes, and stores the edge in sys_edge."
      plain: "The server checks and keeps the connection."
      evidence: "iter_data/src/settings.rs: edge_create, check_edge"
    - step: 5
      from: "{topdir}/iter_engine/src/assign.code.iter.md"
      to: "{topdir}/iter_data/src/settings.code.iter.md"
      what: "On its next tick the engine reads its assignments: each project with an active serves edge (topdir, read_only, state, edge tag) and the billing accounts it also holds (provider, token_envar, order, switch, stop, model)."
      plain: "The engine learns what to run and how to pay."
      evidence: "GET /api/engines/{name}/assignments → iter_data/src/settings.rs: assignments; iter_engine/src/assign.rs"
    - step: 6
      from: "{topdir}/iter_engine/src/loop.code.iter.md"
      to: "{topdir}/iter_engine/src/mapsync.code.iter.md"
      what: "The engine starts serving the project: file sync of its topdir every tick, schedules, and get_next with the chosen account. Dragging the serves edge's engine end onto iter_engine:_deactivated later makes the edge inactive and the engine stops the project, keeping topdir for when it is dragged back."
      plain: "The project runs on that engine — until the edge is parked."
      evidence: "iter_engine/src/engine.rs: tick, run_filesync, dispatch"
  data_flow:
    - step: 1
      from: "{topdir}/webui/settings.code.iter.md"
      to: "{topdir}/iter_data/src/settings.code.iter.md"
      data: "A sys_edge {id, type, from, to, tag, settings, active} — e.g. serves {topdir, read_only}, bills {order, switch, stop, model}."
      stored: true
      plain: "The connection and its settings rest on the edge."
    - step: 2
      from: "{topdir}/iter_data/src/settings.code.iter.md"
      to: "{topdir}/iter_engine/src/assign.code.iter.md"
      data: "Assignments {engine, projects: [{project, topdir, read_only, state, accounts: [...], edge_tag}], accounts: [...]}."
      plain: "The engine's whole job description in one reply."
children:
  codedirs:  []
  codenodes: ["{topdir}/tools/tools.code.iter.md", "{topdir}/webui/settings.code.iter.md", "{topdir}/iter_data/src/settings.code.iter.md", "{topdir}/iter_engine/src/assign.code.iter.md", "{topdir}/iter_engine/src/loop.code.iter.md"]
  tests:     []
  reqs:      []
timestamps: {create: "2026-10-02 23:12:30Z", last_modified: "2026-10-02 23:12:30Z", last_tested: ""}
---
# Connect an engine and an account to a project in the settings graph

iter5 runs one engine per machine and keeps every setting on a node or an
edge of the settings graph. Connecting work to a machine is therefore two
small jobs.

**On the machine** (operator): run the setup script served by iter_data. It
writes the engine's env file — the iter_data token plus the LLM account
tokens the machine has — and starts the engine. The engine knows nothing
else; it registers itself and appears in the Settings tab.

**In the Settings tab** (admin): draw `serves` from the engine to the project
(the edge holds the checkout folder), `holds` from the engine to each account
whose token it has, and `bills` from each account to the project (order, and
the usage percentages at which to switch to the next account or stop). The
engine reads its assignments every tick, so the project starts within
seconds. An account that bills the project but is not held by this engine is
left out of its assignments.

To take a project off a machine for an afternoon, drag the `serves` edge's
engine end onto the `_deactivated` placeholder: the edge and its settings
stay, the engine stops the project, and dragging it back resumes it.
