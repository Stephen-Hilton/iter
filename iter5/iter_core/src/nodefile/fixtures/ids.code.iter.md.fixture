---
id: 10a52e33-761d-4c49-b144-2dc3279a3204
name: "Node-file id stamper"
description: "Finds node files with a missing, duplicated or malformed `id:` and gives each a stable UUID, reusing the id the stored map already knows for that file, so that a part keeps its identity and history across renames and moves."
simple_description: "Gives every map file a permanent ID number so it can be tracked even when it is moved or renamed."
level: component
owner: bespoke
teststate: inherit
children:
  codedirs:   ["{topdir}/iter_local/src/ids.rs"]
  codenodes:  []
  inputs:     []
  outputs:    []
  bizreqs:    []
  techreqs:   []
  tests:      []
---

# Long Description

The Node-file id stamper makes sure every node file (any `*.<nodetype>.iter.md` file, such as a code node, interface or use case) carries a permanent id as the first line of its frontmatter. The data server stores the map by these ids, so a file keeps its place and history when it is renamed or moved.

How it works (`iter_local/src/ids.rs`): `node_files` lists every node file the scan would read. `check` returns an `IdReport` of files with no id, ids shared by several files, and values that are not UUIDs (`is_uuid`). `fix` repairs them. For each file that needs an id it first asks an `IdLookup` what the stored map knows: a vertex at exactly this path, else one with the same nodetype and name. If exactly one match exists and no other file already holds that id, the id is reused; otherwise a fresh UUID is minted. For a duplicated id, the file at the stored vertex's path keeps it and the copies get new ones. `set_id` writes the id into the file (replacing an `id:` line, inserting it as the first key, or opening a frontmatter block). Each repair is reported as an `IdChange` saying "reused" or "minted" and why. `--dry-run` reports without writing.

The Map uploader and test sweep calls it before every map upload (`sync::fix_ids`), supplying a lookup backed by the stored map (`GraphLookup`); with no server, `NoLookup` makes every repair a fresh mint. The iter command line exposes it as `iter ids` and `iter ids --fix`, and the Node-file checker reports the same problems as `missing-id` and `malformed-id`.

Why it matters: without stable ids, a moved file would look like one part deleted and another created, and its test results and links would be lost.

Example: someone copies `billing.code.iter.md` to start a new part, so two files share an id. `fix` keeps the id on the original (the path the map knows) and mints a new one for the copy.
