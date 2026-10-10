# Capability: write or restructure an `*.iter.md` node file

Read this before you create, move, or substantially change any `*.iter.md` file. The
naming law (the filename declares the node type, children links, placeholders) is in
the shared instructions and always applies; this file is the AUTHORING detail — what
the frontmatter and body of each kind must contain. Requirements have their own
section format (below, and `_shared`, "Requirements"); test nodes have their own
capability (`_test_node_authoring`).

**Never write node ids, and never copy one.** Write the frontmatter you know, then run

    "$ITER_BIN" validate --project "$ITER_PROJECT" --file <path> --fix

`--fix` conforms the file: it mints a missing `id`, adds every missing common key
(`desc`, `creator`, `teststate`, the four `children` lists, `timestamps`), renames
legacy keys, adds missing requirement markers and ids, and writes the keys in their
canonical order. The engine runs the same conform on every file it syncs, so a file
you leave unconformed is rewritten and committed by the engine a few seconds later.
To see a full example of a kind, read an existing sibling file of that kind (there is
no `--template` any more).

## Common frontmatter (every node file)

    ---
    id: <uuid>                 # minted by `iter validate --fix`; a moved or renamed file keeps it
    name: "Human-Readable Name"
    desc: "About 100 words, action first: what it does, so that why — enough for an agent to decide whether to read the whole file."
    creator: "agent.<your agent name>"
    teststate: inherit         # inherit | include | omit | block (capability `_teststate`)
    children:
      codedirs:  ["{thisfiledir}/**"]   # the code this node reads, modifies and locks
      codenodes: []                     # child *.code.iter.md files
      tests:     []                     # *.test.iter.md files that test this node
      reqs:      []                     # its bizreq/techreq/philosophy files (or docs)
    timestamps: {create: "…", last_modified: "…", last_tested: ""}
    ---

Entries in `children` are paths or globs; placeholders `{topdir}`, `{thisfiledir}`,
`{thisfilestem}`, `{thisfilename}`, `{projectname}`; a relative path with no
placeholder is relative to the file's folder. Only `*.iter.md` matches become graph
edges. Leave `timestamps` alone: conform keeps them (`last_tested` is written by the
test runner).

## Code node (`<name>.code.iter.md`)

Type keys: `level: context | container | component | connection` (required — conform
cannot choose it for you), `owner: bespoke | oss | 3rdparty` (optional).
`children.reqs` names the node's own requirement pair:
`["{thisfiledir}/reqs/<stem>.bizreq.iter.md", "{thisfiledir}/reqs/<stem>.techreq.iter.md"]`.
`children.tests` names its test nodes. `children.codenodes` names the code nodes it
owns: a code node nobody links is not on the map.

**Which directories are which C4 level is the project's call.** If its requirements
give a directory → level mapping, follow it exactly and never transpose the levels.
One code node per component directory; designer-created ones sit at
`<parent dir>/<slug>/<slug>.code.iter.md`.

A connection (`level: connection`) is a code node too: see `_connections`.

## Requirement files (`<stem>.bizreq.iter.md` / `<stem>.techreq.iter.md`)

ONE bizreq and ONE techreq file per code node, in the node's `reqs/` folder, named after
the node file's stem; the project's global pair is `global/requirements/<project>.{bizreq,techreq}.iter.md`.
Frontmatter as for every node. Body: an optional preamble, then one `## ` section per
requirement:

    ## <KEY> — <title>
    <!-- req: status=draft -->
    The requirement in plain sentences (### or lower for sub-headings).

- Add a requirement by appending a section to the existing file — never a new file
  per requirement, and never a second bizreq or techreq file for one node. If the
  node has no file of that type yet, create it at the path above and add it to the
  node's `children.reqs`.
- KEY follows the scheme the file already uses (the next unused number); leave it
  out (`## <title>`) when the file has no scheme.
- The marker line's `id=` is minted by conform (`iter validate --fix`). Write the
  marker without an id, or leave the line out entirely; never invent or copy an id.
  Keep existing markers exactly as they are when you edit a section's text.
- `status`: leave it (conform sets `draft`) unless your item says otherwise; a decision
  you record under the standing delegation is `agreed`, with its grounds in the text.
- Move a requirement by moving its whole section, marker included (the id travels with it).
- The global pair and any philosophy file are Stephen's: read-only to agents.

## Use case and actor

`<slug>.usecase.iter.md`: `children.codenodes` (the parts the journey uses — edge kind
`uses`), `actors: [actor file paths]`, `flowmap:` (see the `usecase` agent). Edit another
agent's use case only through `iter usecase` (capability `_usecase_links`).
`<slug>.actor.iter.md`: `drives: [use case paths]`, `touches: [code node paths]`.

## Project node and philosophy

The project node is `global/<slug>.project.iter.md` (keys `scandirs`, `file_naming`,
`gitrepo`; `children.codenodes` = the root context nodes; `children.reqs` = the global
requirements). It and philosophy files are Stephen's: never edit them.

## Agent memory

`<codepath>/<dirname>.agentmem.iter.md` is written only by the engine's agentmemory step;
it is never a graph node and carries no frontmatter.

## Node-text standard

Every node is read by people who have never seen the code, in the Project graph and in
agent prompts. Its text must say what the part **does** and why anyone should care. The
test sweep checks code nodes against the rules below and files one `ingest` item per
failing node.

- **name** — what a person would call the part, in 2–6 words, never an internal code
  word: "Work runner", not "Run and close". A container may keep its crate or package
  name if it leads with what it is: "iter_data — data server".
- **desc** — about 100 words, action first: **"<It> does <what>, so that <why>"**. A pure
  lookup (a table, an enum, a list of constants) says so outright. Never a bare noun
  phrase, never "The …/A …/This …" as the opening, never a label followed by a colon and a
  comma list.
- **body** — 3–6 short paragraphs, at least 60 words, in this order: what it does; how it
  works (walk the steps, name the key functions and types with their files,
  `iter_engine/src/cli.rs: Verb::Sync`); what it takes in and hands on (which parts call
  it and which it calls, by their map names); why it matters; one concrete example.
  Gloss every internal term the first time it appears; define every acronym on first use;
  link related parts by their node file path. Never leave `TBD`.

## Copy the quotes

Quote the prose values (`name`, `desc`): prose routinely contains a colon-plus-space,
which strict YAML readers refuse. Bare single tokens (`id`, `level`, `owner`,
`teststate`, `status`) stay unquoted. Conform renders the canonical quoting for you.

## Check your work

After touching any iter file, run `iter validate --file <the file> --fix`. Exit 0 =
clean; exit 1 = findings remain. Conform fixes what it can; the ones it cannot
(`level-missing`, `teststate-invalid`, `connects-invalid`, `children-invalid`, …) are
yours to correct by hand. `iter markers` prints every node file of the checkout,
parsed, as JSON, when you need to see what links what.
