---
id: 1b8bf52c-5b10-480d-9f6a-5ce364843bfa
name: "Agent prompt builder"
desc: "Assembles the text an agent session starts with and each turn after it — the agent's role, shared rules and capability index, the project's global requirements with the philosophy sentence, source instructions, the work item, the previous attempt, and the item's node context from `load_context` (its node file, every child as id / name / desc / path, local requirements and the codepath's agent memory) — so that every agent starts oriented on the right part of the graph and is told which files to read rather than handed their contents."
creator: "iter migrate5"
teststate: inherit
level: component
owner: bespoke
children:
  codedirs:  ["{topdir}/iter_engine/src/prompt.rs"]
  codenodes: []
  tests:     []
  reqs:      []
timestamps: {create: "2026-10-02 23:05:35Z", last_modified: "2026-10-02 23:15:33Z", last_tested: ""}
---

# Agent prompt builder

## Summary

Writes the briefing an AI worker reads before it starts a task: who it is, the rules, the requirements, the task and which files to read.

## How it works

`iter_engine/src/prompt.rs` never inlines file content; it names files for the agent to read.

- **Node context** (iter5 spec §8): `load_context` reads the project's nodes from iter_data
  (`nodes_from_graph`, `GET /api/projects/{p}/graph`), falling back to the checkout's node files
  (`nodes_from_files`). `build_context` picks the item's node (`item.node`, else the node owning its
  first lockdir, else the project node), lists every child edge's other end as `[id, name, desc,
  path]` (codenodes, tests, reqs, uses, supplies, connects, drives, touches), the global requirements
  (project node `children.reqs`, philosophy included) and the local ones (this node's and its
  ancestors'), and the codepath's `*.agentmem.iter.md` file (`agentmemory_files`).
- **Spin-up** (`spinup`): the agent body, the shared rules and capability index (`Tooling::from_rows`),
  the shared rules' placeholders filled by `shared_text` (`{critreview_max_rounds}`, and from the engine's
  settings node, set each tick by `set_engine_env`: `{engine_name}`, `{engine_os}`,
  `{engine_os_instructions}`; constant per engine, so the cached prefix holds),
  `global_context_section` with `PHILOSOPHY_SENTENCE`, the close-gate paragraph, then the per-item part:
  `source_instructions` (who asked), the work-item block, the previous attempt's error and output tail,
  the agent memory section, `node_context_section` and the item's own context patterns
  (`resolve_context`). The order is fixed so the part shared by every item of an agent type is
  byte-identical and the provider's prompt cache can reuse it.
- **Turns**: `mainwork_prompt` (the request, preceded by a human's answer, `answered_question`),
  `agentmemory_prompt` (rewrite the codepath's memory file, under 2 KB), `selfcheck_prompt`;
  `chain_spinup` sends only the per-item part when a chained item continues a session; `explain_prompt`
  builds the read-only ELI5 session.

## What goes in and out

In: agent records and tooling rows, the project graph (or node files), the work item and its details.
Out: prompt text plus the list of context files and warnings, handed to the Work runner, which sends
the turns through the provider dispatch in one session.

## Why it matters

An agent's work is only as good as its briefing. Without its node and requirements it guesses; with
whole files pasted in, the prompt cache breaks and every item costs more.

## Example

A `code` item with `node:` set to the Work runner's id starts with the code agent's instructions, the
shared rules, this project's global requirement files, then "Your node: iter_engine/src/runner.code.iter.md",
its child list, the `src.agentmem.iter.md` memory file and the request.
