---
id: f6ceb2e3-68ab-4e50-b95d-d80a22a05c72
name: "Intro slides and wizard"
description: "Tells iter's story in eight visual slides — the work loop, sharing the code, the close gate, people steering, how it plugs into a repository, and what it is built from — at three levels of detail chosen with a slider (summary, business, technical), with a glossary page and a new-project wizard page that creates the project and engine records and prints the setup commands, so that a newcomer can learn what iter is and start a project in one place."
simple_description: "A short visual story of how iter works, told at the depth you choose, plus a glossary and a step-by-step form for setting up a new project."
level: component
owner: bespoke
teststate: inherit
children:
  codedirs:   ["{topdir}/webui/intro.js", "{topdir}/webui/intro.css"]
  codenodes:  []
  inputs:     []
  outputs:    []
  bizreqs:    []
  techreqs:   []
  tests:      []
---

# Long Description

The Intro tab explains iter and gets a new project started; it is the first thing a newcomer sees (`webui/intro.js`, `webui/intro.css`, mounted by `index.html` as `IterIntro.mount`).

It is one story of eight slides (`STORY`): a cover drawing the whole system (your repository with iter_engine inside it, the Test, Code, Deploy and Plan agents it starts, iter_data reached over the API and MCP, and you steering from the web page) with the six chapters pinned on it as numbered buttons, the work loop drawn as a circle (file → prioritize → pick up → build → check → save, and back), sharing the code (reserve, wait, go in order), the close gate, people steering (run/stop, the question inbox, budgets and accounts), "it plugs into your repo, not your project" with where each part runs, and what iter is built from, grouped by context above its crates. A 3-stop detail slider (`levelBar`) never changes the slide: stop 1 is the summary, stop 2 adds green business callouts, stop 3 adds purple technical notes (`biz`, `tech`, `grow`). The arrow keys move between slides and ↑ ↓ change the detail.

Two pages sit beside the story behind header buttons: the Glossary (`GLOSSARY`, filterable) and "Start a new project", the wizard that validates the project, accounts and agent ladder, creates the project and engine records, mints the engine's token and prints `main.iter.md`, the `.iter/config.json` and the `iter init` command.

The page's 10-second refresh calls `IterIntro.show`, which never re-renders, so nothing the reader has open or half-typed is thrown away. Example: a sales lead walks the story at Summary; an engineer replays it at Technical and sees the same pictures with API routes and config keys added.
