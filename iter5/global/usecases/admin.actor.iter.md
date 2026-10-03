---
id: d80f37e5-2e18-410d-870d-c55f51301429
name: "Admin"
desc: "A user with the admin role: the only one who can write the settings graph. Creates users and mints engine tokens, adds accounts and providers, and connects everything with edges in the Settings tab — serves (engine → project, topdir), holds (engine → account), bills (account → project, order and switch/stop %), member (user → project), runs (agent → project overrides) — parking an edge on a _deactivated placeholder to switch it off without losing its settings."
creator: "stephen"
teststate: inherit
drives: ["{topdir}/global/usecases/connect_an_engine_to_a_project.usecase.iter.md"]
touches: ["{topdir}/webui/settings.code.iter.md", "{topdir}/webui/webui.code.iter.md", "{topdir}/iter_data/src/settings.code.iter.md"]
children:
  codedirs:  []
  codenodes: []
  tests:     []
  reqs:      []
timestamps: {create: "2026-10-02 23:12:30Z", last_modified: "2026-10-02 23:12:30Z", last_tested: ""}
---
# Admin

Every iter5 setting lives on a node or an edge of the settings graph; writes
to it are admin only (users read the parts that touch their projects). The
admin works in the **Settings** tab (also reachable as "Settings graph" from
the graph picker): configure lightbox on any node or edge, draw edges, drag
an endpoint, copy and paste an edge with its settings, tag edges.

Typical jobs: `POST /api/users/<user>/token` for a new engine's token; an
account node per LLM subscription (its provider through the `of` edge); a
`serves` / `holds` / `bills` triangle per engine and project; `member`
edges for the people on a project; `runs` edges to enable an agent on a
project with per-project overrides. The first admin is created from
`ITER_ADMIN_PASSWORD` when iter_data starts.
