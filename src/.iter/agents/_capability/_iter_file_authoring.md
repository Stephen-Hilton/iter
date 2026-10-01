# Capability: write or restructure an `*.iter.md` node file

Read this before you create, move, or substantially change any `*.iter.md` file. The
naming law (the dot rule, the seven nodetypes, children links, placeholders) is in
the shared instructions and always applies; this file is the AUTHORING detail — what
the frontmatter and body of each kind must contain. The seven nodetypes are the word
after the last dot before `.iter.md`: `main`, `code`, `bizreq`, `techreq`,
`interface`, `tests`, `usecase` (`testgroup` is the older spelling of `tests` and
still reads). Interface files have their own, longer rules: see
`_interface_contracts` (`"$ITER_BIN" capability _interface_contracts`).

**Never write any `*.iter.md` skeleton from memory.**

    "$ITER_BIN" validate --file <path> --template

prints the current authoritative template for the nodetype named by the filename
(the file need not exist yet). The template is the single deterministic source, so
format changes reach every agent at once. Start from it every time.

## Code node (`*.code.iter.md`)

    ---
    id: <uuid>              # stable node id — see below
    name: "Human-Readable Name"
    level: component        # context | container | component
    description: "One sentence, action first: what it does, so that why"
    simple_description: "One plain sentence for a business reader"
    owner: bespoke          # bespoke | oss | 3rdparty
    teststate: inherit      # omit | include | block | inherit
    children:
      codedirs:   ["{thisfiledir}/"]          # the actual source code
      codenodes:  []                          # child *.code.iter.md files (paths/globs)
      inputs:     []                          # interface FILES consumed
      outputs:    []                          # interface FILES produced
      bizreqs:    ["{thisfiledir}/*.bizreq.iter.md"]
      techreqs:   ["{thisfiledir}/*.techreq.iter.md"]
      tests:      ["{thisfiledir}/tests/*.tests.iter.md"]
    ---

The template may still print the older spelling `testgroups:` with
`*.testgroup.iter.md`; both spellings read, and a project that already uses one
keeps it. `children.documents` (optional) lists uploaded documents that describe the
node; the web page usually writes it.

**`id`** is a UUID that lets the architecture map follow the node across renames and
moves. For a NEW file, run `"$ITER_BIN" ids --fix` after writing it (it mints the
missing ids); a moved or renamed file keeps its `id:` line unchanged. Never copy an
id from another file — `iter ids` reports duplicates.

**The code node file defines the C4 object — every file belonging to it is linked
here, never inferred from directory positions.** `context`-level nodes attach to the
project head automatically; containers and components attach where a parent's
`codenodes` lists them, and unlinked ones orphan. A `tests` link matching nothing does
NOT keep a node out of testing: the test sweep files a `test` item for every included
leaf code node with no registered test. Keeping a node out of the sweep is a
`teststate` decision (see `_teststate`), which is the user's call.

One code node per component directory, usually alongside that component's
requirement files — a `<component>.code.iter.md` works well.

After writing nodes, check what stranded and link it. `iter markers` prints the scan
as JSON; its `orphans` list names every node file nobody links, with the reason:

    "$ITER_BIN" markers --project "$ITER_PROJECT"

## Requirement files (`*.bizreq.iter.md` / `*.techreq.iter.md`)

Same frontmatter law — `name`, `description`, and a `children:` mapping. When the
body itself holds the requirements rather than pointing at other files, write
`children.reqpaths: []` explicitly.

Component-scoped requirements go beside their component, linked by that node's
`children.bizreqs` / `children.techreqs` globs. PROJECT-WIDE requirements go where a
`globalcontextfiles` pattern in `$ITER_MAINFILE` will load them — that is the one
spot that configures always-loaded context for every agent.

## The `# Long Description` section (required in every code node body)

Every code node's BODY must contain a `# Long Description` section: a plain-language
description of the object for a reader who has never seen the code — describe, don't
state; gloss every internal term the first time it appears; define every acronym on
first use ("three letter acronym (TLA)"); link related project parts by their node
file path. Its length and order are set by the node-text standard below. Write it for
real when you create or substantially change a node — never leave `TBD`.

## Copy the quotes

**Copy the quotes** on the prose fields (`name`, `description`,
`simple_description`, `label`, `endpoint`). Prose routinely contains a
colon-plus-space, and while the engine parses that fine unquoted, strict-YAML tools
reading the same file refuse the whole block. Bare single-token values (`id:`,
`level:`, `kind:`, `owner:`, `teststate:`) stay unquoted.

## Check your work

After touching any iter file:

    "$ITER_BIN" validate --project "$ITER_PROJECT" --file <the file> --fix

`--fix` applies the safe corrections (fence normalization, quoting `name`,
`description` and `endpoint` values that contain ": "); everything else is reported
for you to correct by hand — a missing `id` is fixed by `iter ids --fix`, not here.
Exit 0 = clean (info-level notes are fine); exit 1 = warn/error findings remain — fix
them before finishing. The validator skips git-ignored files.

# Node-text standard (iter4, 2026-09-29)

Every `*.iter.md` node is read by people who have never seen the code, in the Project graph and in agent prompts. Its text must say what the part **does** and why anyone should care. `iter validate` checks the mechanical parts of this standard. The test sweep turns each failing node into one `ingest` work item.

## name
What a person would call the part, in 2–6 words: never an internal code word. "iter command line", not "iter verbs"; "Work runner", not "Run and close". A container may keep its crate or package name, but it leads with what it is: "iter_data — data server".

## description (one sentence, action first)
- Say **"<It> does <what>, so that <why>"**. For example: *"Reads the command a person or agent typed (`iter sync`, `iter add` …), checks its arguments and hands the work to the part that does it."*
- A pure lookup (a table, an enum, a list of constants) says so outright. For example: *"Lists the work-item states and agent types every other part uses; it runs nothing itself."*
- Never a bare noun phrase or a label followed by a list. `validate` warns `description-not-action` when the sentence starts with *The / A / An / This*, or is a label followed by a colon and a comma list.

## simple_description (one sentence, business reader)
No crate names and no jargon: *"Where every request from the engines and the web page arrives and gets answered."* `validate` warns `missing-simple-description` on code nodes.

## Long Description (body, under `# Long Description`)
Write 3–6 short paragraphs, roughly 150–350 words, in this order:
1. what it does, in action terms;
2. how it works: walk the steps and name the key functions and types with their files (`iter_engine/src/cli.rs: Verb::Sync`);
3. what it takes in and hands out: which parts call it and which it calls, by their map names;
4. why it matters, or what would break without it;
5. one concrete example.

Gloss every internal term the first time it appears. `validate` warns `thin-long-description` when the section is under 60 words or is mostly the description again.

## Where a node sits
A part is always drawn inside its owner: component → container → context. Any level may own any other (a context may own a context). Link it from its owner's `children.codenodes`: a file nobody links is not on the map.
