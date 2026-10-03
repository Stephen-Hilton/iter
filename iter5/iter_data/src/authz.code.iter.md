---
id: 498436e5-1d3c-4539-b723-2945cf8d49d8
name: "Project access rules"
desc: "Decides, in one router-wide middleware, whether the caller may touch the project named in the request path: admins reach every project, engine tokens only projects one of their engines serves (an active serves edge), other users only projects they are members of (a viewer member may only read). Every /api/projects/{p} route and every MCP tool inherits it without the handler knowing."
creator: "agent.code"
teststate: inherit
level: component
owner: bespoke
children:
  codedirs:  ["{topdir}/iter_data/src/authz.rs"]
  codenodes: []
  tests:     []
  reqs:      []
timestamps: {create: "2026-10-02 23:10:16Z", last_modified: "2026-10-02 23:10:16Z", last_tested: ""}
---

# Long Description

## Summary

Keeps each person and each engine inside the projects they belong to.

Project access rules (`iter_data/src/authz.rs`) apply iter5's authorization in one place, keyed on the project segment of the request path, so work items, locks, the project graph, file sync, GraphRAG, datasync and anything added later are covered by the same check.

How it works: `project_authz` is an axum middleware layered over the whole API router. `project_of_path` reads the project from `/api/projects/{p}/…` or `/api/prepostwork/{p}/…` (percent-decoded); other paths pass straight through. `check_project` then applies the rules from the settings graph: `admin` passes; role `engine` passes only when one of the token subject's engines (engine record `user` = sub, or an active `owns` edge) has an active `serves` edge to the project; any other user needs an active `member` edge to it, and a member with role `viewer` may only use GET/HEAD. Creating a project that does not exist yet is left to the handler. A token that does not verify is passed on untouched, so the `AuthUser` extractor answers 401 as usual; a refusal here is 403 with the reason.

What goes in and out: it reads edges through the Settings graph (`settings::load_edges`, `engines_of_user`, `member_role`) and the engine records through the Storage interface. The MCP gateway replays tool calls through the same router, so MCP gets identical rules.

Why it matters: one engine per machine now serves several projects, and the server must stop it — or a user — from reading or changing a project it was never given.

Example: engine `mbp` serves only `pdy`; its `GET /api/projects/iter5/workitems` gets 403, while `GET /api/projects/pdy/workitems` passes.
