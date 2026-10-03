---
id: 078e11c8-4c09-495f-9a7c-c4590cae85ce
name: "iter4 to iter5 converter"
desc: "Converts an iter4 checkout into iter5 node files in a copy, never in place: copies the tree (history included), replaces interface files with one connection node per interface kind, turns `main.iter.md` into the project node, testgroup registries into test nodes with `children.tests` scripts and a `last_result`, multi-bullet requirement files into one file per requirement, `actors.yaml` into actor nodes and agent memory into `*.agentmem.iter.md`, rewrites every reference to a renamed file, conforms every node file and checks the result is a fixed point, then prints a report (or only the report, with `--dry-run`)."
creator: "agent.code"
teststate: inherit
level: component
owner: bespoke
children:
  codedirs:  ["{topdir}/iter_local/src/migrate5.rs", "{topdir}/iter_local/src/migrate5_tests.rs", "{topdir}/iter_local/src/testgroups.rs"]
  codenodes: []
  tests:     []
  reqs:      []
timestamps: {create: "2026-10-02 23:10:52Z", last_modified: "2026-10-02 23:10:52Z", last_tested: ""}
---

# iter4 to iter5 converter

## Summary

Turns an old-format project copy into the new node-file format, so existing projects can move to iter5 without hand edits.

## How it works

`iter_local/src/migrate5.rs: run(from, to, dry_run)` is `iter migrate5 --from <iter4 checkout> --to <dir>
[--dry-run] [--json]` (iter5 spec §10). `--from` is only read; `convert` works in memory and writes only
under `--to`. Conversions, in order:

1. **interfaces** — every `*.interface.iter.md` is removed; one connection node per interface kind
   (request-reply → "API call", event → "Event", stream → "Stream", dataset → "Shared dataset") at
   `global/connections/<slug>.code.iter.md`, `connects.from` = the old producers, `connects.to` = the
   old consumers.
2. **main** — `main.iter.md` → `global/<slug>.project.iter.md` (`globalcontextfiles` →
   `children.reqs`, `globalscandirs` → `scandirs`).
3. **testgroups** — `*.tests.iter.md` / `*.testgroup.iter.md` → `*.test.iter.md`; registered scripts
   join `children.tests`, the last run becomes `last_result` and `timestamps.last_tested`. The iter4
   JSONL registry is read by `iter_local/src/testgroups.rs` (`parse`), which iter5 keeps for this only.
4. **requirements** — a bizreq / techreq file with two or more top-level bullets becomes one file per
   bullet in `reqs/` (`requirement_bullets`, `bullet_name`), the original's text kept as a plain doc.
5. **actors** — `actors.yaml` → `global/usecases/<slug>.actor.iter.md`.
6. **agent memory** — `*.agentmemory.iter.md` → `*.agentmem.iter.md`, content untouched.
7. **references** — every children / connects / actors / drives / touches entry naming a renamed file
   is rewritten.
8. **conform** — every node file goes through `nodefile::conform` (creator `iter migrate5`) and is
   checked to be a fixed point.

`Report` counts each conversion and conform finding, lists notes for a human and any file that is not a fixed point; `print` or `--json` shows it.

## What goes in and out

In: an iter4 checkout. Out: a converted copy and a report. No network.

## Why it matters

Running it on a copy means a failed conversion costs nothing; the fixed-point check means the engine's
first sync of the converted checkout does not rewrite every file again.

## Example

`iter migrate5 --from ~/dev/pdy-dev --to /tmp/pdy5 --dry-run` prints how many interface files would
become connection nodes, how many requirement files would split, and every note, writing nothing.
