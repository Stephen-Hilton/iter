---
id: 00b8ebbb-3a13-49d0-8b80-0fc27f7f2c77
name: "Settings graph"
desc: "Keeps every iter setting on a node or an edge of one graph — engines, projects, agents, tooling, users, accounts, providers and work item states as nodes; serves, bills, holds, of, member, owns, runs, handles, allows, uses and hosts as edges carrying their settings — validates edge endpoints, deactivates instead of deleting by moving an endpoint onto a placeholder, seeds and migrates it at startup, and answers each engine's assignments (which projects it runs, where, with which accounts)."
creator: "agent.code"
teststate: inherit
level: component
owner: bespoke
children:
  codedirs:  ["{topdir}/iter_data/src/settings.rs", "{topdir}/iter_data/src/settings/"]
  codenodes: []
  tests:     []
  reqs:      []
timestamps: {create: "2026-10-02 23:10:16Z", last_modified: "2026-10-02 23:10:16Z", last_tested: ""}
---

# Long Description

## Summary

One picture of who runs what, where, and on whose account — and the place every setting is changed.

The settings graph (`iter_data/src/settings.rs`) replaces iter4's scattered per-record settings. Node records stay in their tables (`project`, `engine`, `agent`, `agent_tooling`, `webui_user`, plus `account`, `provider`, `workitem_type`); node id is `<type>:<name>` (iter_core `settings::node_id`). Edges are rows of the `sys_edge` table `{id, type, from, to, tag, settings, active, created, updated}`.

How it works: `routes` serves `GET /api/settings/graph`, node create / patch / delete / get under `/api/settings/nodes`, and edge create / patch / delete / get / copy under `/api/settings/edges`; writes are admin-only, and non-admins read only the parts touching their projects (`visible_projects`). `check_edge` infers the edge type from the endpoint types and validates endpoints and settings with iter_core. Each type has a virtual, undeletable placeholder `<type>:_deactivated` (and `iter_data:self` is virtual too): deleting a node moves its edges onto the placeholder (`on_node_deleted`), which keeps their settings but makes them inactive. Lifecycle hooks add default edges: `on_project_created` (`allows` to every state, `runs` for every agent, `hosts`, `member` for the creator), `on_agent_created`, `on_tooling_created`, `on_engine_registered`. `bootstrap` seeds providers (claude, mock) and work item states, and `migrate_iter4` once turns iter4-shaped records (engine project maps, project accounts, agent overrides) into edges. `assignments` builds `GET /api/engines/{name}/assignments` from active `serves`, `bills`, `holds` and `of` edges. `settings/tests.rs` holds its unit tests.

What goes in and out: the webui's Settings tab and the MCP `settings_graph` tool read and edit it; engines read their assignments; project access rules and get_next read `serves`, `member`, `owns` and `runs` edges from it.

Why it matters: without it a multi-project engine would not know which projects to run or which credentials to use.

Example: an admin drags the `serves` edge of engine `mbp` from project `pdy` onto `project:_deactivated`; on its next assignments call `mbp` stops running `pdy`, and the edge's `topdir` is kept for later.
