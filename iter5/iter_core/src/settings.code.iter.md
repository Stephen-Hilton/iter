---
id: 5aed5c09-b2d6-4a14-9519-ac04d475dbe9
name: "Settings-graph types"
desc: "Defines the settings graph shared by the server and the engine: the nine node types (iter_data, iter_engine, project, workitem_type, agent, agent_tools, user, account, provider) with their <type>:_deactivated placeholders, the edge-type table (serves, bills, holds, of, member, owns, runs, handles, allows, uses, hosts), endpoint and settings validation, and the Assignments reply that tells an engine which projects it serves with which accounts."
creator: "agent.code"
teststate: inherit
level: component
owner: bespoke
children:
  codedirs:  ["{topdir}/iter_core/src/settings.rs"]
  codenodes: []
  tests:     []
  reqs:      []
timestamps: {create: "2026-10-02 23:15:00Z", last_modified: "2026-10-02 23:15:00Z", last_tested: ""}
---

# Settings-graph types (`iter_core::settings`)

## Summary

The vocabulary of iter5's settings: every setting is a node or an edge, and this module says which nodes and edges exist and what they may carry.

## How it works

- **Nodes**: `NODE_TYPES` lists the nine types; a node id is `<type>:<id>` (`node_id`, `parse_node_id`), where `<id>` is the record's stable id (its storage key, fixed at creation); its `name` is a display name that can be renamed. `record_id` / `record_name` read the two from a record (a record older than ids: its `name` was the key), `check_name` validates a name and `mint_id` makes a new id from a first name. `NODE_TABLES` lists the tables that hold node records. Every type has a non-deletable placeholder `<type>:_deactivated` (`placeholder_id`, `is_placeholder`); `iter_data:self` is the one server node. `is_protected` covers both. `table_of` maps a type to its storage table (an engine lives in `engine`, an agent_tools node in `agent_tooling`, a user in `webui_user`…). `PROVIDERS` (claude, mock) are seeded at startup.
- **Edges**: `EDGE_TYPES` is the table of edge type → (from type, to type): `serves` engine→project (topdir, read_only), `bills` account→project (order, switch, stop, model), `holds` engine→account (token_envar), `of` account→provider, `member` user→project (role), `owns` user→engine, `runs` agent→project (per-project agent override), `handles` agent→workitem_type, `allows` project→workitem_type (per-state policy), `uses` agent_tools→agent, `hosts` iter_data→project / engine. Each (from, to) pair is unique, so `edge_type_for` infers the type from the endpoints. `validate_endpoints` accepts a placeholder of the right type, and `validate_settings` checks the known keys (percents 0–100, strings, booleans, the role) while keeping unknown ones.
- **`SysEdge`**: one stored edge (`id`, `type`, `from`, `to`, `tag`, `settings`, `active`, timestamps). `is_active` is false when the edge is switched off or either end sits on a placeholder — moving an end onto `_deactivated` keeps the edge and its settings but stops it.
- **`Assignments`** (`GET /api/engines/{name}/assignments`): per served project the topdir, read-only flag, state, the billing accounts this engine also holds (`BilledAccount`, by order, with switch / stop / model), the serves edge's tag and the enabled agents' overrides; plus every account the engine holds. `BilledAccount::to_account` turns one into the work-item model's `Account` for `pick_account`.

## What goes in and out

iter_data's settings store and API (`iter_data/src/settings.rs`: seeding, the iter4 → iter5 migration, `/api/settings/*`, assignments) and authz (`authz.rs`: a project-scoped engine token is accepted only with an active serves edge) build on it. iter_engine reads `Assignments` (`iter_engine/src/assign.rs`). The webui's Settings tab mirrors `EDGE_TYPES` in `settings.js`.

## Why it matters

iter4 kept who-runs-what in lists on project and engine records that drifted apart. One edge table, validated the same way on every write, makes "engine E runs project P from folder F, billed to account A" a single fact.

## Example

An admin drags the `serves` edge Engine01 → pdy onto `iter_engine:_deactivated`. `is_active` is now false, the engine's next assignments reply leaves pdy out, and the engine stops running it — the edge's `topdir` is still there when the end is dragged back.
