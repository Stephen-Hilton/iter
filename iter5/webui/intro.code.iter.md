---
id: f6ceb2e3-68ab-4e50-b95d-d80a22a05c72
name: "Intro story and designer wizard"
desc: "Tells iter's story in eight visual slides at three levels of detail (summary, business, technical) chosen with a slider, with a filterable glossary, and hosts the new-project wizard: it creates a designed project (no engine, no repository) seeded with its project node and default requirements, explains the design → engine → Build → accounts route, and walks engine setup, so a newcomer can learn what iter is and start a project in one place."
creator: "iter migrate5"
teststate: inherit
level: component
owner: bespoke
children:
  codedirs:  ["{topdir}/webui/intro.js", "{topdir}/webui/intro.css"]
  codenodes: []
  tests:     []
  reqs:      []
timestamps: {create: "2026-10-02 23:05:35Z", last_modified: "2026-10-02 23:15:00Z", last_tested: ""}
---

# Intro story and designer wizard

## Summary

A short visual story of how iter works, told at the depth you choose, plus a glossary and the step-by-step start of a new project.

## How it works (`webui/intro.js`, `webui/intro.css`)

Mounted by `index.html` as `IterIntro.mount(el, ctx)`; `IterIntro.show` never re-renders, so nothing the reader has open or half-typed is thrown away by the page's refresh.

- **The story** (`STORY`): one set of slides — the whole system on a cover with numbered chapter buttons, the work loop (file → prioritize → pick up → build → check → save), sharing the code, the close gate, people steering, how it plugs into a repository, the two sides (iter_data and the engines) with their shared rulebook, and a last slide to start a project or look around. A 3-stop slider (Summary · Business · Technical) never changes the slide: stop 2 lays business callouts over it, stop 3 adds technical notes (API routes such as `POST /api/projects/{p}/next`, config keys, crate names). ← → move between slides, ↑ ↓ change the detail.
- **Glossary** (`GLOSSARY`, filterable): iter's terms, including iter5's (node file, designed, connection, settings graph, serves / bills edges).
- **Start a new project** (the wizard): step 1 takes the name, an optional git remote, the file-naming rule for name clashes and a daily cost cap; step 2 shows what will be created — a **designed** project (`PUT /api/projects/{name}`, state Stopped) whose project node and default philosophy / bizreq / techreq appear in the Project graph marked designed; step 3 shows the route: design it in the Project graph, have an engine running (an admin can mint the engine's user and token for its env file here, and the page watches for the engine's check-in), **Build** from the Project graph, and connect accounts with a `bills` edge in the Settings graph. For a repository that already exists it points to drawing a `serves` edge instead.

## What goes in and out

Calls `/api/projects`, `/api/engines`, `/api/users/{name}` and `/api/users/{name}/token`; hands off to the Project graph, Settings and queue tabs through `ctx`.

## Example

A sales lead walks the story at Summary; an engineer replays it at Technical and sees the same pictures with routes added, then starts "Ledger" in the wizard and lands in its designed graph.
