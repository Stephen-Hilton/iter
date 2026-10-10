---
id: 41a80490-61d9-416c-bf2d-a492cf8a9a5b
name: "Repeat detection rules"
desc: "Decides when a new work item is a repeat of one already open — instantly when both carry the same check: and container: tags, or by a model's judgement over a narrowed list of similar open items — and what happens to the survivor, so one fault never becomes a dozen items."
creator: "iter migrate5"
teststate: inherit
level: component
owner: bespoke
children:
  codedirs:  ["{topdir}/iter_core/src/dedup.rs"]
  codenodes: []
  tests:     []
  reqs:      []
timestamps: {create: "2026-10-02 23:05:35Z", last_modified: "2026-10-02 23:15:00Z", last_tested: ""}
---

# Long Description

## Summary

The rules that stop the same problem from being filed over and over.

Repeat detection keeps one problem as one work item. It holds the rules both the data server and the engine apply; it runs no model itself.

How it works: an item's repeat key is its pair of tags `check:<rule>` and `container:<name>` (`iter_core/src/dedup.rs: key_of`, `key_of_row`). **Stage one** happens at create time in the data server: if an open item already has the same key, no new row is written; the open one is returned with `already_open`. **Stage two** happens in the engine before dispatch: `stage2_candidates` narrows the open items to at most 20 that share a tag, overlap a lock folder or have a matching long title ending (`titles_match`); a model (the engine's `dedup` judge) returns a verdict per candidate — same fault, different, or unsure — parsed by `parse_judgements`; and `decide` merges only on a confident same-fault verdict, never into an in-progress or closed item, keeping the oldest open item. Either way `apply_repeat` updates the survivor: `repeats` goes up by one, its priority number is halved (lower runs sooner, never below 1), and after `repeated_threshold` repeats (project setting, default 3) it gets the `repeated` tag. The closed duplicate gets a `dup of: <last 12 of id>` tag. Items received before `DEDUP_EPOCH` are never touched.

What goes in and out: the HTTP API's `workitem_create` and `duplicate_of` routes use the key and survivor rules; the engine's dedup judge (`iter_engine/src/dedup.rs`, its model call going through the engine's provider dispatch like every other) uses candidates, parsing and `decide`; server-side get_next (`iter_data/src/next.rs`) holds a new item back until the judge has looked at it (`DEDUP_EPOCH`, `dedup_checked`). A red test result filed by iter_data carries the key `check:tests-failing` + `container:<last 12 of the test node id>`, so a repeat failure merges.

Why it matters: on 2026-09-10 one unchanged fault filed thirteen priority-1 items, nine word-for-word twins. With these rules the survivor's rising priority is the signal instead of clutter.

Example: the test sweep files "Tests non-green: iter_data-unit" twice; the second create returns the first item, now with `repeats: 1` and its priority halved from 40 to 20.
