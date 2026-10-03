---
id: 1636948d-0dc2-4397-bb9d-f6861fa947a0
name: "Design a project in the graph and build it"
desc: "A developer creates a project with no engine and no repo, designs its parts in the Project graph, then presses Build and picks an engine and a checkout folder; the engine creates the git repo, writes every designed node file in one first commit, reports build/done, and the server optionally queues a plan item to build the code from the design."
creator: "iter migrate5"
teststate: inherit
actors: ["{topdir}/global/usecases/developer.actor.iter.md"]
flowmap:
  summary: "The wizard creates a designed project; the server seeds its project node and default requirements. Every node the developer adds in the Project graph is stored as designed, with a file path from the designer folder rules. Build records the request and creates the serves edge; on its next heartbeat the engine sees build_waiting, runs git init, writes .gitignore and every pending file, commits once, acks the files and posts build/done, which flips the nodes to synced and can queue the plan item."
  sequence: ["{topdir}/global/usecases/developer.actor.iter.md", "{topdir}/webui/intro.code.iter.md", "{topdir}/iter_data/src/api.code.iter.md", "{topdir}/iter_data/src/settings.code.iter.md", "{topdir}/webui/grapheditor.code.iter.md", "{topdir}/iter_data/src/graph.code.iter.md", "{topdir}/iter_data/src/nodes.code.iter.md", "{topdir}/iter_data/src/filesync.code.iter.md", "{topdir}/iter_engine/src/loop.code.iter.md", "{topdir}/iter_engine/src/mapsync.code.iter.md"]
  process_flow:
    - step: 1
      from: "{topdir}/global/usecases/developer.actor.iter.md"
      to: "{topdir}/webui/intro.code.iter.md"
      what: "The developer opens Intro → \"Design a new project\" and gives the wizard a name and description."
      plain: "The developer starts a new project on paper."
      evidence: "webui/intro.js: wizardHtml"
    - step: 2
      from: "{topdir}/webui/intro.code.iter.md"
      to: "{topdir}/iter_data/src/api.code.iter.md"
      what: "The wizard writes the project record; the server creates it with its settings-graph defaults (allows edges to every work-item state, a member edge for a non-admin creator) and, because no engine serves it yet, seeds the project node and one philosophy, bizreq and techreq as designed nodes."
      plain: "The server creates the empty design."
      evidence: "PUT /api/projects/{p} → iter_data/src/settings.rs: on_project_created; iter_data/src/nodes.rs: ensure_project_node"
    - step: 3
      from: "{topdir}/global/usecases/developer.actor.iter.md"
      to: "{topdir}/webui/grapheditor.code.iter.md"
      what: "In the Project graph (\"Designed — not built\" banner) the developer adds contexts, containers, connections, use cases and requirements; each becomes a node whose file path comes from the designer folder rules (nodefile::plan_path), stored with file_state designed."
      plain: "The developer draws the parts."
      evidence: "POST /api/projects/{p}/graph/nodes → iter_data/src/graph.rs"
    - step: 4
      from: "{topdir}/webui/grapheditor.code.iter.md"
      to: "{topdir}/iter_data/src/filesync.code.iter.md"
      what: "The developer presses Build and picks an engine, a topdir and optionally \"queue plan agent\". The server activates the serves edge (engine → project, settings.topdir), records project.build = requested and flips every designed node to pending_write."
      plain: "The developer asks an engine to build it."
      evidence: "POST /api/projects/{p}/build → iter_data/src/filesync.rs: build, activate_serves"
    - step: 5
      from: "{topdir}/iter_engine/src/loop.code.iter.md"
      to: "{topdir}/iter_engine/src/mapsync.code.iter.md"
      what: "The engine's assignments now include the project; its heartbeat reply names it in build_waiting, so the tick hands it to the designer build."
      plain: "The engine notices the build request."
      evidence: "iter_engine/src/engine.rs: run_filesync → filesync::build"
    - step: 6
      from: "{topdir}/iter_engine/src/mapsync.code.iter.md"
      to: "{topdir}/iter_data/src/filesync.code.iter.md"
      what: "The engine creates the folder, runs git init (adding origin when the project has a gitrepo), writes .gitignore with .iter/, fetches files/pending, writes every file, commits them as \"iter: build from design\", acks them and posts build/done with the commit."
      plain: "The engine creates the repo and the files."
      evidence: "iter_engine/src/filesync.rs: build, apply_pending; POST …/files/ack, POST …/build/done"
    - step: 7
      from: "{topdir}/iter_data/src/filesync.code.iter.md"
      to: "{topdir}/iter_data/src/api.code.iter.md"
      what: "build/done marks the build complete and, if queue_plan was asked and the build had no error, files a plan work item at priority 5 on the project node: \"Build the project from its design\"."
      plain: "A planning task is queued to write the code."
      evidence: "iter_data/src/filesync.rs: build_done"
  data_flow:
    - step: 1
      from: "{topdir}/webui/grapheditor.code.iter.md"
      to: "{topdir}/iter_data/src/nodes.code.iter.md"
      data: "Each designed node (nodetype, level, name, desc, body, children, planned path) with file_state designed; it rests in the project graph's node and link collections."
      stored: true
      plain: "The design rests in the graph."
    - step: 2
      from: "{topdir}/iter_data/src/filesync.code.iter.md"
      to: "{topdir}/iter_engine/src/mapsync.code.iter.md"
      data: "The pending file list [{id, op: write, path, text, node_version}], each text rendered by iter_core::nodefile exactly as conform would write it."
      plain: "The files travel as their final text."
    - step: 3
      from: "{topdir}/iter_engine/src/mapsync.code.iter.md"
      to: "{topdir}/iter_data/src/filesync.code.iter.md"
      data: "Acks [{id, node_version, path, hash, commit}] and build/done {engine, commit}; the nodes become synced."
      stored: true
      plain: "The server learns the repo now exists."
children:
  codedirs:  []
  codenodes: ["{topdir}/webui/intro.code.iter.md", "{topdir}/webui/grapheditor.code.iter.md", "{topdir}/iter_data/src/api.code.iter.md", "{topdir}/iter_data/src/settings.code.iter.md", "{topdir}/iter_data/src/graph.code.iter.md", "{topdir}/iter_data/src/nodes.code.iter.md", "{topdir}/iter_data/src/filesync.code.iter.md", "{topdir}/iter_engine/src/loop.code.iter.md", "{topdir}/iter_engine/src/mapsync.code.iter.md"]
  tests:     []
  reqs:      []
timestamps: {create: "2026-10-02 23:05:35Z", last_modified: "2026-10-02 23:12:30Z", last_tested: ""}
---
# Design a project in the graph and build it

A developer wants to start a new project from a drawing rather than from a
repo. The Intro tab's wizard creates the project with no engine and no repo;
the server seeds a project node and default philosophy, bizreq and techreq.

In the Project graph the developer lays the project out — contexts,
containers, components, connections, actors and use cases. Every node is
stored at once with the path its file will have (designer folder rules:
`global/…` for the project, use cases, actors and global requirements,
`src/<slug>/…` for top-level contexts, a subfolder of the parent for each
child, `global/connections/` for connections) and the `designed` state.

When the design is ready, **Build** picks the engine and the folder. The
server connects the engine to the project (a `serves` edge with that topdir)
and marks every node waiting to be written. The engine's next heartbeat says
`build_waiting`: it creates the folder and the git repo, writes every file,
makes one commit (`iter: build from design`), acknowledges each file and
reports `build/done`. From then on the project is an ordinary served project:
edits flow both ways, and the optional plan item (priority 5) asks the plan
agent to build the code the design describes.
