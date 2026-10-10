---
id: 1c5e4548-c62f-49e8-bfe0-d41212491585
name: "Node-file library"
desc: "Reads, repairs and writes iter5 node files (*.iter.md, format v5): parse a file into a NodeDoc (tolerating iter4 legacy keys), conform it to the canonical text idempotently, render a NodeDoc back, derive its graph edges, resolve children globs against the project's node paths, plan the path of a designer-created node, and add or remove one edge in the owning file — the one implementation the server's node side and the engine's file side share."
creator: "agent.code"
teststate: inherit
level: component
owner: bespoke
children:
  codedirs:  ["{thisfiledir}/"]
  codenodes: []
  tests:     []
  reqs:      []
timestamps: {create: "2026-10-02 23:15:00Z", last_modified: "2026-10-02 23:15:00Z", last_tested: ""}
---

# Node-file library (`iter_core::nodefile`)

## Summary

The one piece of code that knows what a node file says and how it must be written, used by both the server and the engine.

In iter5 a project lives twice: as `*.iter.md` files in the repository and as nodes in iter_data's project graph. This library is the bridge. It works on strings only — the engine hands it file text, the server hands it node paths — and never touches the filesystem.

## How it works

- **Types** (`mod.rs`): `NodeType` (project, code, test, bizreq, techreq, philosophy, usecase, actor, agentmem) read from the filename's last segment before `.iter.md` (`type_of`); `is_synced` is false for agentmem, which never reaches the graph. `NodeDoc` holds the common frontmatter (id, name, desc, creator, teststate, level, `children` with codedirs / codenodes / tests / reqs, `timestamps`), every type-specific key in `front` (connects, flowmap, actors, drives, touches, status, scandirs, file_naming, last_result…), the body and the `{topdir}/…` path.
- **Parse** (`parse.rs`, `yaml.rs`): splits the `---` fences, reads YAML tolerantly, and renames iter4 keys on the way in (`description→desc`, `bizreqs/techreqs/reqpaths→reqs`, `testpaths/testgroups→tests`, `simple_description` / `long_description` folded into the body; interface keys dropped).
- **Conform** (`conform.rs`): `conform(path, text, now, creator)` returns the canonical text plus findings — adds a missing frontmatter or id, fills missing keys, sets a missing code `level` to `component`, bumps `timestamps.last_modified` only when the content changed. `conform(conform(x)) == conform(x)`.
- **Render** (`render.rs`): `NodeDoc` → text in the fixed key order (id, name, desc, creator, teststate, type keys alphabetically, children, timestamps), body verbatim. The server renders every node edit through it, so the file the engine writes is exactly what `conform` would produce.
- **Paths and edges** (`paths.rs`, `edges.rs`): `slug`, placeholder expansion (`{topdir}`, `{thisfiledir}`, `{thisfilestem}`…), `resolve` (a children glob → matching node paths), `plan_path` (the designer's folder rules with sequence or uuid12 collision naming), `edges_of` (codenodes, tests, reqs, supplies, connects, drives, touches, uses), and `add_child` / `remove_child`, which write or remove one edge in whichever file owns it (a connection node owns its supplies / connects edges) and refuse to remove an edge only a glob matches.

## What goes in and out

The engine's filescan → conform → sync service (`iter_engine/src/filesync.rs`) conforms each changed file and writes it back before posting it. iter_data (`nodes.rs`, `filesync.rs`, `graph.rs`) parses synced files, derives edges, plans paths for nodes created in the graph and renders every server-side edit for the engine to write. iter_local uses it for `iter validate`, the test runner and `iter migrate5`.

## Why it matters

If the two sides formatted or interpreted a file differently, every sync would rewrite the file and the graph would flap. One library makes the file ⇄ node round trip a fixed point.

## Example

A person adds a container under "Data" in the Project graph. iter_data calls `plan_path` (→ `{topdir}/src/data/ledger_api/ledger_api.code.iter.md`), `add_child` on Data's file (a new `children.codenodes` entry) and `render` on both; the engine writes the two files, and its next scan finds `conform` changes nothing.

Tests: `nodefile/tests.rs` (round trips, conform idempotence over every `*.iter.md` in the repo and the iter4 files in `fixtures/`, legacy migration, glob resolution, path collisions, edge derivation), run by the iter_core unit-test node.
