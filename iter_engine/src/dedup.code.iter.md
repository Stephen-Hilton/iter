---
id: bda753dc-99db-45f5-895f-7b9551a4909b
name: "Duplicate work judge"
desc: "Asks a short AI session whether a newly filed work item repeats one already open, then merges it, notes a possible overlap or lets it through, so that two agents never spend money fixing the same fault twice."
creator: "iter migrate5"
teststate: inherit
level: component
owner: bespoke
children:
  codedirs:  ["{topdir}/iter_engine/src/dedup.rs"]
  codenodes: []
  tests:     []
  reqs:      []
timestamps: {create: "2026-10-02 23:05:35Z", last_modified: "2026-10-02 23:15:33Z", last_tested: ""}
---

# Long Description

## Summary

Checks each new piece of work against the open ones and merges it when it is a repeat.

The Duplicate work judge catches repeats that the data server's own check cannot see. The server (stage 1) refuses a new item only when an open one carries exactly the same `check:` and `container:` tags. This part (stage 2) handles the rest: the same fault under a different rule name, or two agents describing one problem in their own words.

How it works (`iter_engine/src/dedup.rs`): `needs_triage` picks queued items that are new (first attempt, not a schedule copy, not a shell command) and not yet stamped. `plan` narrows the candidates without any model call, using `iter_core::dedup::stage2_candidates` (open items sharing the container, the check or the lock scope; at most 20, newest first). With candidates, `ModelJudge` runs one read-only session through the provider dispatch, billed to the chosen account (the `dedup` agent record's model, default sonnet, 180 seconds; the mock provider answers `{"candidates":[]}`) on the prompt from `judge_prompt` and asks for JSON only. `iter_core::dedup::decide` then acts on the verdicts: `same_fault` merges the item into one survivor through the server's `duplicate_of` route; `unsure` leaves a "may overlap" note on both items; `different` changes nothing. `triage` carries this out on its own thread and stamps the item `dedup_checked`.

The Engine tick loop calls it before an item's first dispatch and holds the item out of dispatch while the judge runs (the item shows "blocked by: dedup triage"). It uses the Data server client for reads and writes and records the judge's spend on the new item.

Why it matters: agents often file the same defect from different angles; without this, two runs would edit the same files for the same cause. A judge that fails, times out or answers in prose never holds work back: the item is stamped and runs as usual. The `Judge` trait lets tests script verdicts so no test calls a model.

Example: an agent files "login page throws on empty email" while "email validation crashes on blank input" is open on the same folder. The judge answers `same_fault`, and the new item is closed into the older one.
