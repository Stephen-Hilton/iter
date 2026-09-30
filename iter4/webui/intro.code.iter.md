---
id: f6ceb2e3-68ab-4e50-b95d-d80a22a05c72
name: "Intro slides and wizard"
description: "Shows two slide tracks (a business overview and a technical overview) and ends both with a new-project wizard that creates the project and engine records, mints an engine token and prints the `iter init` command, so that a newcomer can learn what iter is and start a project in one place."
simple_description: "A short slideshow explaining iter, followed by a step-by-step form for setting up a new project."
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

The Intro tab explains iter and gets a new project started. It is the first thing a newcomer sees in the web app.

How it works (`webui/intro.js`, styled by `webui/intro.css`): `index.html` calls `IterIntro.mount(el, ctx)` once and `IterIntro.show(ctx)` whenever the tab is shown again; `ctx` gives it the page's API helper, the current project, admin status and links to open the graph or the queue. The slides come in two tracks, Business and Technical (`setTrack`), with prev/next arrows, keyboard arrows, swipe on phones and a row of progress dots (`go`, `render`). Some slides draw small diagrams (`archDiagram`, `mapSvg`, `ladderBars`, `lockTree`), and code samples get copy buttons (`code`, `copyText`). The reader's place is remembered in browser storage.

The last slide of both tracks is the New project wizard (`wizardHtml`, `validate`). It asks for the project name and description, git repo and checkout folder, the Claude accounts and their switch and stop percentages, the agent-count ladder, a daily cost cap, the engine name and the data server address, explaining each. `createProject` then writes the project record (`PUT /api/projects/{name}`) and creates or updates the engine record so it serves the project (`/api/engines/{engine}`); `mintToken` creates the engine's user and a one-year token (`POST /api/users/{engine}/token`). `resultsHtml` shows the `main.iter.md` and `.iter/config.json` the wizard will lead to (`mainIterMd`, the same text `iter init` writes), the `.env` lines with the token (`envLines`) and the `iter init` command to run (`initCmd`).

It calls only the data server's project, engine and user routes; the files themselves are written later by the iter command line's `iter init` in the new checkout.

Why it matters: without it, starting a project meant hand-writing records and config files.

Example: an admin finishes the wizard for project "shop". The page creates the records, shows a token once, and gives `iter init --project shop …` to paste into a terminal in the new repo.
