# Bugfix spec: a close write that fails leaves the item in-progress forever, and the engine never looks back

Written 2026-09-22 by Stephen's session in pdy-dev, from a measured incident. Send as-is to the
iter agent. File:line references are from a read of `~/dev/iter/iter3` at commit `3de4da8` on
2026-09-22; verify each.

Terms: an item's **record** is its row in iter_data (state, attempt, engine, ts, version). The
engine's **running table** is `self.running` in `engine.rs`, one entry per agent thread this
process has alive. The **close** is `work.rs::close`, the function that runs when an agent
session ends and writes the outcome to the record. A **ghost** is a record that says
`in-progress` and names this engine while no thread and no process exists for it.

## 1. What happened

On 2026-09-22 the StephenMBP engine (pid 1708, up since 2026-09-21 21:11 Pacific) started
`12f5f3b3…0a28eeafa9f6` at 03:38 Pacific and `1b3a962a…e2371a8be34c` at 03:51. Four log lines
after the second start, every API call began failing with `HTTP 0: error sending request for
url (…)` — the Mac lost its network — and kept failing for 44 consecutive log lines, about
three to four minutes at the 5 s tick. Both agent sessions ended inside that window (a
`claude -p` that cannot reach Anthropic exits non-zero). For each, the log then reads, in order:

```
[engine] could not append 'error' detail to <id>: HTTP 0: error sending request …/details
[engine] <id8> '<name>': attempt 1 failed, retry after 300s
[engine] close failed for <id>: HTTP 0: error sending request …?expect_version=N
[engine] done <id8> '<name>' -> failed/retry
```

The engine believed it had requeued both with a 300 s backoff. The store never received the
write. Seven hours later the webui header read **4 in-progress** while every queued item was
tagged `blocked by: usage cap (2/2)`: the store counted four records; the running table held
two threads. The two ghosts had `state: in-progress`, `attempt: 1`, `engine: StephenMBP`, no
`retry_after`, no closing detail row, and no process anywhere. They would have stayed that way
indefinitely. They were put back to `queued` by hand through the API at 10:52 Pacific (a `doc`
row on each says so).

The items' locks did not need help: iter_data lock rows carry `expires` (about 65 minutes after
`acquired`, seen on a live row), so the releases that also failed during the outage were made
good by expiry. Only the record has no such self-repair.

## 2. Why it happens

- `client.rs:47-85`: every transport failure is mapped to `ApiError { status: 0, … }` and
  returned once. There is no retry on a transport error anywhere in the client.
- `work.rs:1320-1378` (the versioned close loop): the loop runs at most twice, and the second
  pass exists only for a `409` version conflict (`Err(e) if e.status == 409 && attempt == 0 =>
  continue`). Any other error, including status 0, hits `eprintln!("close failed …"); break;`.
  Nothing records that the close is still owed.
- `work.rs:1239-1246`: the detail-row appends (`put_detail`) log and swallow their errors the
  same way, so the `error` row that would have explained the failure is also lost.
- `work.rs:1381-1387`: the lock releases are `let _ = api.post(…)`. Same outage, same loss;
  saved only by the lock TTL.
- `engine.rs:411-414` `prune_running`: `self.running.retain(|(_, _, h)| !h.is_finished())`.
  Once the thread returns, the engine forgets the item completely. The usage-cap arithmetic at
  `engine.rs:944-1001` counts `self.running.len()`, so the cap reads 2/2 while the store reads 4.
- The only place the engine ever examines an `in-progress` record it owns is the stop sweep at
  `engine.rs:500-506` (`i.stop_requested && i.state == "in-progress" && i.engine == self.name`).
  There is no sweep for "in-progress, mine, and not in my running table". The items cache is
  refreshed every tick (`engine.rs:571-579`), so the data to notice a ghost is already in hand.

So a single failed PUT at the end of a run converts a finished item into a permanent
in-progress record, and the engine has no code path that could ever notice.

## 3. Goal

An outcome the engine has decided is never lost: the close write is retried until it lands or
the engine gives up loudly; and independently of that, an engine that sees a record it owns in
`in-progress` with no thread behind it repairs the record on the next tick, so a ghost cannot
outlive one tick after the network returns.

## 4. Design

### R1 — the close is durable: retried with backoff, then journaled

In `work.rs::close`, replace the two-pass loop with: attempt the versioned PUT; on `409`
re-read and retry once as now; on any transport error (`status == 0`) or `5xx`, sleep and
retry with backoff (2 s, 4 s, 8 s, … capped at 60 s) for up to 10 minutes. Detail-row appends
and lock releases in the same function get the same treatment, in order, so the record's
`error`/`response` row exists before the state flips.

If the write has still not landed after the budget, write the intended final record to a
journal file `<topdir>/.iter/pending_close/<workid>.json` (the full `updated` value plus the
`expect_version` it was built against) and log `[engine] close journaled for <id>`. The thread
then returns. Nothing is silently dropped: either the store has it or the disk has it.

### R2 — the tick replays the journal

At the top of `tick` (`engine.rs:416`), before dispatch: for each file in
`.iter/pending_close/`, re-read the record; if its `version` still equals the journaled
`expect_version`, PUT the journaled value and delete the file; if the version moved, log
`[engine] journaled close for <id> superseded (version N -> M)`, delete the file, and let R3
decide. A replay that fails with status 0 leaves the file for the next tick.

### R3 — the tick repairs ghosts it owns

Also at the top of `tick`, after the items cache refresh: for every item with
`state == "in-progress" && engine == self.name` whose id is in neither `self.running` nor
`self.explaining` nor `self.triaging` and that has no journal file:

1. If `ts.start` is younger than the agent timeout for that item (`work.rs:453
   agent_timeout`), skip — a thread may have just started and the cache may lag the claim by a
   tick (the same reason `triaged` exists at `engine.rs:33-35`).
2. Otherwise treat it exactly as a failed attempt: append a `doc` row
   `"engine <name> found this record in-progress with no session behind it (last known: <ts.start>); treated as a failed attempt"`,
   then the same branch as `work.rs:1350-1362` — `failed` if `attempt >= maxattempts`, else
   `queued` with `retry_after = now + retry_delay_sec(attempt)` — via the versioned PUT, and
   release its locks. Log `[engine] <id8> '<name>': ghost in-progress repaired -> <state>`.

The rule is deliberately "this engine's own records only": another engine's in-progress
record may be a live session on another machine, and only its owner can know.

### R4 — the webui says when the two numbers disagree

`webui/index.html` already has both numbers: the in-progress count from the item list and, per
engine, nothing yet about its running table. Add `running: <n>` to the engine heartbeat
(`e.running`, the length of `self.running`), and in the counts bar render the in-progress chip's
title as `N in-progress · engines report M running` and give the chip a warning colour when
`N != sum(M)` across live engines. That is the one-glance signal Stephen used today by hand.

## 5. Tests

- `work.rs`: a test double `Api` that fails `put` with status 0 for the first k calls and then
  succeeds. `close` returns after the write lands, the record has the expected state, and no
  journal file exists. With k larger than the budget, a journal file exists with the intended
  record and the thread has returned within budget + one backoff step.
- `engine.rs`: a tick over a cache holding an in-progress item that names this engine, older
  than its timeout, absent from `running` — after the tick the double received a PUT with
  `state: queued` (or `failed` at maxattempts), a `doc` detail row, and a lock release per
  lockdir. A control with `ts.start` one second old receives nothing. A control naming another
  engine receives nothing.
- `engine.rs`: a journal file whose `expect_version` matches → replayed and deleted; one whose
  version is stale → deleted without a PUT, and R3 then handles the record on the same tick.
- Break-it evidence (PDY-TECH-020 shape): run the engine against a local iter_data, start an
  item whose agent is `exec: sleep 5; exit 1`, and pull the network (kill iter_data) for the
  five seconds around its end; watch the journal appear, restore iter_data, and watch the
  next tick log the replay and the record read `queued`. Record the log lines in the plan's
  acceptance section.

## 6. Rollout

Engine only, plus the webui heartbeat field. No schema change: `retry_after`, `lasterror`,
`attempt`, `ts` and detail rows all exist. Deploy the engine binary to StephenMBP first; the
FHServer engine (last seen 2026-09-06) is offline and picks it up whenever it returns.

## 7. Acceptance, by hand

After deploy, on the live project: the in-progress count in the header equals the sum of
`running` over live engines at every refresh over an hour of normal work; an `in-progress`
record that names StephenMBP and has no `claude` child of pid 1708's successor is repaired
within one tick; the engine log shows no `close failed` line that is not followed, within the
budget, by either the same id's `done … ->` line or `close journaled`.

## 8. Out of scope

- Retrying dispatch-side writes (the claim at `engine.rs:1155-1160`). A lost claim leaves the
  item `queued`, which is the safe side; it re-dispatches next tick.
- Repairing another engine's ghosts. That needs an ownership lease and is its own spec.
- The Mac's network dropping. Nothing here prevents the outage; it stops the outage from
  leaving permanent damage.
