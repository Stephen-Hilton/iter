# Capability: block on the cluster restart (`workitem_block`)

**pdy-dev only: in any other project, reject the item with `workitem_reject` and say why.**
This capability describes pdy-dev's shared dev cluster and its (rare) rebuilds;
no other project has either, so never call `workitem_block` there. If an item in another
project was written expecting a cluster restart, reject it with that reason.

**First read `eks_active` on the context node "AWS EKS / K8S System".** When it is `false` the
cluster is switched off on purpose: never block and never tag — reject with "needs the EKS cluster
(eks_active: false): …" instead (your agent definition's EKS section). Everything below assumes `true`.

pdy-dev's dev cluster is rebuilt rarely: only on a night when a work item is waiting for
a rebuild. Since 2026-09-13 one thing does it, an external loop on Stephen's Mac
(`devops/script/drain_rebuild_resume.sh`; the engine's own 02:00 schedule was deleted that
day). At 01:00 Pacific it counts open items carrying a cluster-restart tag; with none,
nothing happens that night; with one or more, it sets the project to Draining, waits
until nothing is in progress, rebuilds (about 2 h 40 min), and sets the project back to
Running once a live status check is green. While the project is Draining no new work
starts, so most work never meets a rebuild. Work that needs the cluster while a rebuild
is under way cannot succeed, and it must not fail either:
failing burns one of the item's attempts (the project's `failure.maxattempts`, default
five), and rejecting says the WORK is invalid, which it is not. Block instead:

    workitem_block — reason: what you need the cluster for

(Fallback without the MCP server: `"$ITER_BIN" block --cluster-restart --reason "…"`.)

**Block only when the cluster is really down.** Most nights nothing is rebuilt, so check
first: namespaces younger than about 3 hours still gaining pods (`kubectl get ns`) mean a
bring-up is running (pdy-dev's CLAUDE.md section 8 has the one process check that works).
On a night with no rebuild, do the work now; if the cluster is unavailable for some other
reason, that is a question (`workitem_ask`), not a block. An item still carrying the tag
at 01:00 Pacific counts as waiting for a rebuild and can cause one.

Otherwise, end your turn. In one write the engine-side verb:

1. tags the item `blocked-by-cluster-restart` (the durable tag; the engine shows it
   as `blocked by: cluster restart` while the hold lasts);
2. parks the item — no close gate runs, its locks are released;
3. records your reason as the item's last error, which your next run reads back in
   its "Previous attempt" section;
4. puts the attempt counter back where the claim found it — you exit WITHOUT
   incrementing your attempt.

The engine holds the item until the cluster is judged back up. It reads that from the
newest restart-window work item's `clusterhealth` row; since the rebuild moved out of the
engine on 2026-09-13 no such row is recorded, so in practice the rule is the clock: the
item is held while the Pacific wall clock is between 02:00 and 06:00. The moment the clock leaves that window the engine requeues the item (a `doc` row
names the release), and the tag is stripped when the item goes from queued to
in-progress. You do nothing to unblock yourself.

## When to block, and when not to

- **Block** when the work needs the live cluster (a deploy, a smoke test, a query
  against it) and the cluster is really down: a bring-up is running (check as above)
  or the engine has just told you the cluster is unavailable. Say in `reason` exactly what you needed, so the
  re-run starts where you stopped.
- **Do not block** for work that does not touch the cluster — write the code, the
  tests, the plan; only the step that needs the cluster waits.
- **Do not fail** a run because the cluster was down: that is what the attempt
  counter is for, and it is not yours to spend on the calendar.
- **Do not reject**: the item is valid; it is the hour that is wrong.

A human sees the parked item in the review bucket with the tag and your reason;
removing the tag by hand is how a human keeps it parked on purpose.

## The sibling tag this is NOT

`blocked-until-cluster-restart` (renamed 2026-09-09 from `awaiting-cluster-restart`) is a different contract: it
parks an item UNTIL the next rebuild has run, and it is what asks for a rebuild. It is only for a change that
genuinely needs the cluster rebuilt (a bring-up-order change, a cluster-level resource, a from-empty proof), and
the item must state what the restart must include, why it is needed, and why no rolling update carries the change
(both justifications mandatory, PDY-TECH-089). Testing and verification never qualify; a schema or data change never
qualifies. It is owned by pdy-dev's rebuild scripts (`devops/script/requeue_after_restart.sh` re-queues the items),
not by the engine, and is set by a tags-only PUT plus `workitem_reject`, never by `workitem_block`. The full rule
is in pdy-dev's `reqs/pdy_agent_rules.md` and CLAUDE.md section 8. `blocked-by-cluster-restart` (this verb)
holds an item only WHILE the cluster is unavailable and does nothing once it is healthy.
