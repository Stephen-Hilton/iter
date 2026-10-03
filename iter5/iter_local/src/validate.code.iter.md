---
id: 573cb889-e15d-448c-90ce-bbcb1b12239b
name: "Node-file validator"
desc: "Conforms every node file under the topdir (or one file) in memory with `iter_core::nodefile::conform`, reports each file whose text would change, conform's findings (including those conform cannot fix, such as a missing code level) and ids shared by two files, and with `--fix` rewrites the files conformed; it also holds the node-text rules (an action-first desc, a body of at least 60 words) the test sweep files `ingest` items from, so that every node file the engine syncs is already in canonical form."
creator: "iter migrate5"
teststate: inherit
level: component
owner: bespoke
children:
  codedirs:  ["{topdir}/iter_local/src/validate.rs"]
  codenodes: []
  tests:     []
  reqs:      []
timestamps: {create: "2026-10-02 23:05:35Z", last_modified: "2026-10-02 23:15:33Z", last_tested: ""}
---

# Node-file validator

## Summary

Checks that every description file is in the standard shape and, on request, puts it there.

## How it works

`iter_local/src/validate.rs: run(topdir, single, fix)` is `iter validate [--file F] [--fix]`, a thin
wrapper over `iter_core::nodefile::conform`:

- every node file under the topdir (found by the Node-file walker; agent memory excluded), or one
  `--file`, is conformed in memory (timestamp `now_ts()`, no creator);
- a file whose conformed text differs is "not conformed" (`FileReport.changed`); with `--fix` it is
  rewritten (`fixed`);
- conform's findings are reported per file; `PERSISTENT` lists the ones conform cannot clear itself
  (`level-missing`, `teststate-invalid`, `connects-invalid`, `children-invalid`,
  `timestamps-invalid`, `frontmatter-unterminated`);
- ids shared by two or more files are reported (`duplicate_ids`; the server would give the second file
  a new id);
- `Report::exit_code` is 0 when clean, 1 when something is left to do.

`node_text_findings` and `description_is_action` are the node-text rules: a `desc` that starts with an
action rather than a label ("The …", "Parser: a, b, c"), and a body of at least 60 words. The Test sweep
files `ingest` items for code nodes that break them.

## What goes in and out

In: node files on disk. Out: a `Report` (printed by the CLI) and, with `--fix`, rewritten files.

## Why it matters

The engine and iter_data both store conformed text; a file that is already conformed syncs without a
rewrite commit, and duplicate ids are caught before they reach the graph.

## Example

`iter validate` after hand-adding a code node with no `level:` reports it as not conformed with a
`level-missing` finding; `iter validate --fix` adds `level: component` and the finding stays listed until
a person picks the right level.
