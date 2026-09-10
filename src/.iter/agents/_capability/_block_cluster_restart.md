# Capability: block on the cluster restart (`iter block --cluster-restart`)

The dev cluster is rebuilt every night between 02:00 and 06:00 Pacific (the daily
"cluster restart window" schedule). Work that needs the cluster in that window
cannot succeed, and it must not fail either: failing burns one of the item's five
attempts, and rejecting says the WORK is invalid, which it is not. Block instead:

    "$ITER_BIN" block --cluster-restart --reason "<what you need the cluster for>"

Then end your turn. In one write the engine-side verb:

1. tags the item `blocked-by-cluster-restart` (the durable tag; the engine shows it
   as `blocked by: cluster restart` while the hold lasts);
2. parks the item — no close gate runs, its locks are released;
3. records your reason as the item's last error, which your next run reads back in
   its "Previous attempt" section;
4. puts the attempt counter back where the claim found it — you exit WITHOUT
   incrementing your attempt.

The engine holds the item until the cluster is back up and healthy: the newest
restart-window run must have closed complete with a green `clusterhealth` row, or,
when no window ran in the last 24 hours, the Pacific wall clock must be outside
02:00–06:00. The moment that is true the engine requeues the item (a `doc` row
names the release), and the tag is stripped when the item goes from queued to
in-progress. You do nothing to unblock yourself.

## When to block, and when not to

- **Block** when the work needs the live cluster (a deploy, a smoke test, a query
  against it) and the clock says the window is open or the engine has just told
  you the cluster is unavailable. Say in `--reason` exactly what you needed, so the
  re-run starts where you stopped.
- **Do not block** for work that does not touch the cluster — write the code, the
  tests, the plan; only the step that needs the cluster waits.
- **Do not fail** a run because the cluster was down: that is what the attempt
  counter is for, and it is not yours to spend on the calendar.
- **Do not reject**: the item is valid; it is the hour that is wrong.

A human sees the parked item in the review bucket with the tag and your reason;
removing the tag by hand is how a human keeps it parked on purpose.
