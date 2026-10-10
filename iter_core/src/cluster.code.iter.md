---
id: 4254206c-03cc-48f9-9491-65aa88e81f48
name: "Cluster-restart hold"
desc: "Decides whether work that needs the shared test cluster must wait because the cluster is being rebuilt, from the nightly restart window's own result or, failing that, the clock, and says how to tag, park and later release such items."
creator: "iter migrate5"
teststate: inherit
level: component
owner: bespoke
children:
  codedirs:  ["{topdir}/iter_core/src/cluster.rs"]
  codenodes: []
  tests:     []
  reqs:      []
timestamps: {create: "2026-10-02 23:05:35Z", last_modified: "2026-10-02 23:05:35Z", last_tested: ""}
---

# Long Description

## Summary

Keeps cluster-dependent jobs waiting while the shared test environment is restarting.

The cluster-restart hold stops work that needs a shared cluster from starting while that cluster is being rebuilt, and releases it once the cluster is healthy again.

How it works: an agent that needs the cluster during the restart window runs `iter block --cluster-restart`. That verb calls `iter_core/src/cluster.rs: apply_block`, which adds the durable tag `blocked-by-cluster-restart`, parks the item, writes a `lasterror` the next run will read, and puts the attempt counter back so the aborted run does not count. Each engine tick, `evaluate` decides whether the cluster is healthy. It looks first at the newest run of the project's nightly restart schedule (`newest_clone`): a run that completed in the last 24 hours with a `clusterhealth` row of exit codes 0/0, or one that recorded "skipped tonight", means healthy; a red, failed or still-open run means not. With no recent evidence it falls back to the clock (`in_window`, default a 02:00–06:00 window in the configured time zone, wrapping midnight correctly). `blocks(item, healthy)` then says whether a tagged item must wait; the engine shows it as the tag "blocked by: cluster restart". When the item is claimed, `claim_tags` strips both tags.

What goes in and out: settings come from the project record's `cluster_restart` object. iter_engine's tick calls `evaluate` and `blocks` (`iter_engine/src/engine.rs: cluster_health`), and the `iter block` verb calls `apply_block`.

Why it matters: without it, agents start cluster tests mid-rebuild, fail, and burn attempts and model time on a fault that is not theirs.

Example: at 03:10 Pacific an item tagged for the cluster stays parked; at 05:40 the restart run closes with exits 0/0, the next tick finds the cluster healthy, and the item is queued again.
