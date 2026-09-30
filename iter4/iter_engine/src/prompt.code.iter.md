---
id: 1b8bf52c-5b10-480d-9f6a-5ce364843bfa
name: "Agent prompt builder"
description: "Assembles the text an agent session starts with and each turn after it (the agent's role, shared rules, project head files, context files, memory notes and the work item itself), so that every agent starts with the same orientation and knows exactly what it is asked to do."
simple_description: "Writes the briefing an AI worker reads before it starts a task."
level: component
owner: bespoke
teststate: inherit
children:
  codedirs:   ["{topdir}/iter_engine/src/prompt.rs"]
  codenodes:  []
  inputs:     []
  outputs:    []
  bizreqs:    []
  techreqs:   []
  tests:      []
---

# Long Description

The Agent prompt builder writes what an AI agent is told. It lists files for the agent to read rather than pasting their contents in.

How it works (`iter_engine/src/prompt.rs`): `spinup` builds the first message from a `SpinupInput`: the agent's own instructions (its body from iter_data), the shared rules and capability index (`Tooling::from_rows`, from the agent tooling rows), the project head read from `main.iter.md` (`read_head`, `Head`), source instructions that depend on who asked (`source_instructions`: a user, another agent or an error), the work-item block, the previous attempt, then the files to read. Those files are the codepath's node files and every ancestor's (`marker_chain`), the item's context patterns (`resolve_context`), and the one agent memory file per codepath, `<name>.agentmemory.iter.md`, which is the last agent's briefing for the next (`agentmemory_section`). The order is fixed so the part shared by every item of an agent type is byte-identical and the model's prompt cache can reuse it. Later turns come from `mainwork_prompt` (the request, preceded by any answer a human gave to the item's question, `answered_question`), `agentmemory_prompt` (refresh the memory file) and `selfcheck_prompt`. `chain_spinup` sends only the per-item part when an item continues an existing session. `explain_prompt` builds the read-only ELI5 ("explain like I'm five") session.

The Work runner calls it for every agent run and sends the turns into one Claude session. It reads through the Data server client (agent records, tooling rows) and the checkout on disk.

Why it matters: an agent's work is only as good as its briefing. A missing context file or a lost human answer leads straight to wrong work.

Example: a `code` item on `iter_local/src/` starts with the code agent's instructions, the shared rules, `main.iter.md`, that folder's node files, its memory file and the request; after the main turn it is asked to update the memory file, then to check its own work.
