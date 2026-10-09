---
id: 30bc1c3c-91bd-4c0c-a534-c77a9c83922d
name: "webui kit"
desc: "The small shared toolkit of the web page: HTML escaping and a safe markdown renderer, toasts, one modal dialog shape with styled ask / confirm / choose prompts, the Configure… lightbox (an editable key/value list), context menus, the keyboard-shortcut popover, drag handles on a selected edge's two ends and the copied-edge clipboard — used by the Project graph, the Settings graph and the page."
creator: "agent.code"
teststate: inherit
level: component
owner: bespoke
children:
  codedirs:  ["{topdir}/webui/kit.js", "{topdir}/webui/kit.css"]
  codenodes: []
  tests:     []
  reqs:      []
timestamps: {create: "2026-10-02 23:15:00Z", last_modified: "2026-10-02 23:15:00Z", last_tested: ""}
---

# webui kit

## Summary

The shared building blocks that make both graph views look and behave the same.

## How it works (`webui/kit.js`, `webui/kit.css`)

One global, `IterKit`, with no dependencies; every class is prefixed `kit-` and colours come from the page's `:root` tokens.

- `esc` / `md`: HTML escaping and a small safe markdown subset (headings, lists, fenced code, quotes, inline code / bold / italic / links), escape first.
- `toast`: transient notes, bottom right.
- `modal`, `ask`, `confirm`, `choose`: one dialog shape (title, body, buttons, error line) and styled replacements for `prompt()` / `confirm()` plus a radio picker.
- `configure`: the Configure… lightbox — an editable key/value list built from a node's frontmatter (and body) or an edge's settings, returning only what changed.
- `menu`: a context menu inside a host element; `help`: the keyboard-shortcut popover.
- `endpointHandles`: drag handles on a selected Cytoscape edge's two ends, reporting the node an end is dropped on.
- `springDrag`: in a force layout, a dragged node pulls its neighbours part of the way (half for a neighbour, a quarter two hops out, at most the 40 nearest), and on release fcose re-settles the graph from where things are, with the dropped node pinned.
- `separate`: pushes apart any node boxes a force layout left overlapping.
- `forceLayout`: one fcose run with given nodes held in place (label-box corrected), then `separate`.
- `pins`: P pins a node where it is: not draggable, never pulled along, kept by every layout and settle; remembered per graph and layout in the browser.
- `clip`: the copied edge (⌘C / ⌘V), one per graph scope.

## What goes in and out

Used by the Project graph editor (`graphedit.js`), the Settings graph (`settings.js`) and the page; it calls no API itself.

## Why it matters

Edge dragging, copy / paste and configuring behave identically in the project graph and the settings graph because they are one implementation.
