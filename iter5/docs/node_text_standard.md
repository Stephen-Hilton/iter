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

## Interfaces: label (2–5 plain words)
An interface's `name:` is its id (`workitem-create`); its `label:` is what the map writes on the connection, for a reader who never saw the code: "Files a work item", "Reads the map to draw". `validate` warns `missing-interface-label` when it is absent; the viewer falls back to the id with its dashes read as spaces.

## Use cases: the parts they need
A use case lists, in `children.codenodes`, the parts its journey needs, at the most specific level that is true. Their owners are never listed: when the map is stored, `usecase_map()` walks each listed part up its ownership chain and tags every node on the way with the use case's id. The Project graph pulls a use case's picture by that tag (an array index, no traversal) and draws it as a hierarchy: the use case → its top-level parts → ownership lines down to the parts it needs.
