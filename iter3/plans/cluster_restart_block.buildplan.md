# Cluster-restart block buildplan — make "blocked by: cluster restart" a real thing

Written 2026-09-09 at Stephen's direction, at his terminal, no work item.
Serves **PDY-TECH-077 as amended 2026-09-09** (deliver one workload; the daily
window is the only rebuild) and **PDY-TECH-086** (a red check in dev is written
down, never a stop). Companion of `devops/script/cluster_restart_window.sh` and
`devops/script/requeue_after_restart.sh`, which are the half that already ships.

**The statement this plan must make true** (Stephen, 2026-09-09): *"if you
require the cluster for your work and it is between 2am and 6am PT, tag yourself
as 'blocked by: cluster restart' and exit without incrementing your attempt; the
iter engine will block your workitem from restarting until the cluster is back up
and healthy, and once it is, you will no longer be blocked; when your workitem
goes from queued to in-progress the 'blocked by: cluster restart' tag will be
stripped."* And: *"write up a spec for making the above statement accurate —
today, there is no 'blocked by: cluster restart'."*

**What is true today, measured in the engine source.** Nothing in the engine
knows the phrase. Worse, the literal phrase is unusable: `BLOCKED_TAG_PREFIX =
"blocked by: "` (`iter_core/src/lib.rs:204`) is an engine-OWNED prefix, and every
tick `reconcile_waits` deletes every tag starting with it that the tick did not
itself derive (`iter_engine/src/engine.rs:812`). A parked item is not queued, so
its derived wait is empty (`engine.rs:785`) and a hand-written `blocked by:
cluster restart` tag is removed within one tick (default 5 s,
`iter_core/src/lib.rs:460`). **The legal spelling is therefore
`blocked-by-cluster-restart`** — outside the engine's prefix, so nothing deletes
it. Tag text itself is unvalidated: `Tag` is `{text, color}`
(`iter_core/src/lib.rs:474-478`), and `iter_data` checks tags only for the
`usecase:` prefix (`iter_data/src/api.rs:757-800`) and for the closed-item
tags-only exception (`api.rs:1038-1043`, `tags_only_change` at `api.rs:1104`).
Spaces and colons are legal; the collision, not the characters, is the problem.

---

## 1. Agent side — the verb the agent runs

**Exists today:** `devops/iter reject --reason <text>` (`iter_engine/src/cli.rs:93`,
handler `cli.rs:939-946`). It appends a `doc` detail row and calls `set_state`
(`cli.rs:913-922`), which writes `state: "parked"` and `lasterror: "rejected:
<reason>"`. The engine sees the state change mid-run and keeps it instead of
completing the item (`iter_engine/src/work.rs:119-129`, `close_keep_state` at
`work.rs:1001-1015`), releasing the item's locks.

**New:** `devops/iter block --cluster-restart [--reason <text>]`, a sibling of
`reject` in the same file. On one call it must:

1. add `{"text":"blocked-by-cluster-restart","color":"#c47a1f"}` to `tags` if absent;
2. set `state: "parked"`;
3. set `lasterror` to `"blocked by: cluster restart — <reason>"` (this is the one
   field the next run reads back: the prompt shows `lasterror` when the previous
   attempt did not complete, `iter_engine/src/prompt.rs:327-328` and `396-397`;
   other detail rows never reach the running agent);
4. **decrement `attempt` by one**, floored at 0.

Step 4 is the whole of "exit without incrementing your attempt". The counter is
incremented at CLAIM, before the agent has run a single turn — `claimed["attempt"]
= json!(item.attempt + 1)` in `engine.rs:846` and again in the session-chaining
claim at `work.rs:193`. By the time the agent decides the cluster is unavailable,
the increment has already happened; the only way to leave the count where it was
is to put it back. `set_state` does not touch `attempt` (`cli.rs:913-922`), so
`block` needs its own versioned PUT rather than reusing it. Nothing else consumes
the count except the failure ladder (`work.rs:1104-1116`, `maxattempts` default 5
at `iter_core/src/lib.rs:318`), which a parked item never reaches.

## 2. Engine side — the dispatch rule

`dispatch` classifies every queued item once and gives each one a reason it is
idle (`engine.rs:631-654`); a project-wide `hold` short-circuits the tick
(`engine.rs:522`, `engine.rs:655-658`). The block is a **per-item** rule, not a
hold: add one branch to that classifier, ahead of the lock check —

```
} else if has_tag(i, CLUSTER_RESTART_TAG) && !cluster_healthy {
    w.reason = Some("cluster restart".into());   // renders "blocked by: cluster restart"
```

and one filter on the pick list at `engine.rs:685-694`, so a tagged item is never
selected while `cluster_healthy` is false. The reason string is what makes
Stephen's exact phrase appear in the web app: `reconcile_waits` writes
`format!("{BLOCKED_TAG_PREFIX}{reason}")` (`engine.rs:786`), which is literally
`blocked by: cluster restart`. The durable tag and the displayed tag are two
different strings on purpose — one the agent owns, one the engine owns.

**"Back up and healthy", concretely.** Evaluated once per tick per project:

- Find the newest work item whose `source_schedule` (`iter_core/src/lib.rs:540`)
  equals the restart template `9da12551-bb18-4adf-8fae-a4ca138fb317` — the daily
  02:00 `America/Los_Angeles` template that runs
  `devops/script/cluster_restart_window.sh` (verified live 2026-09-09: state
  `scheduled`, agent `exec`, `sched.kind` `daily`).
- Healthy = that clone's `state == "complete"` **and** its recorded env-status
  exit code is 0.
- If no clone's `ts.complete` is within the last 24 h, healthy = the wall clock in
  `America/Los_Angeles` is NOT between 02:00 and 06:00. `chrono_tz` is already a
  dependency (`iter_core/src/sched.rs:10-11`), so this is `Tz` arithmetic the
  crate already does.

**Where the exit code must live.** It has no machine-readable home today. The
window keeps `STATUS_EXIT` and prints it (`cluster_restart_window.sh:153-175`) but
exits 0 regardless, deliberately, so a red status does not make a nightly item
read as `failed` (its EXIT CODES block, lines 64-75). So "clone closed complete"
alone does not mean healthy, and the engine must read a field. **Proposed:** the
window POSTs one detail row on its own clone, `key: "clusterhealth"`, `valuetype:
"json"`, value `{"cluster":"corridor-dev1","bringup_exit":N,"status_exit":M,"at":"<ISO>"}`,
and the engine reads that row. This needs a second, one-line engine change: exec
items get no environment at all — `run_shell` spawns bare `bash -c`
(`work.rs:825-834`) while `ITER_WORKID`, `ITER_DATA_URL` and `ITER_ENGINE_TOKEN`
are assembled only in the agent branch (`work.rs:476-492`) — so `run_shell` must
be given the same three variables before the window script can address itself.

**When the window is red.** Tagged items stay blocked. That is the deliberate
choice: the alternative is dispatching agents into a cluster the 04:5x status
check called broken. What unblocks them is the P0 emergency item the window
spawns on a red status (a sibling is building that now) **closing complete** — at
which point the next window's `clusterhealth` row reads 0, or, if a full day
passes with no window at all, the 24-hour fallback above releases them outside the
02:00–06:00 band. There is no path where a red night parks work for ever.

## 3. Strip on transition, and its one home

The strip already exists for the engine-owned prefix and it has **two** homes, not
one: the dispatch claim at `engine.rs:845` and the session-chaining claim at
`work.rs:190`, both filtering `t.text.starts_with(BLOCKED_TAG_PREFIX)` out of
`tags` in the queued → in-progress PUT. The new durable tag must be stripped in
the same write. Do it by extracting one function — `iter_core::claim_tags(&[Tag])
-> Vec<Tag>`, dropping both `BLOCKED_TAG_PREFIX` tags and `CLUSTER_RESTART_TAG` —
and calling it from both sites, so the rule has one authored home (PDY-TECH-046).
An item can only reach in-progress through a claim, so those two call sites are
the complete set.

## 4. The interim — what an agent does today, and what it does not get

Until the above ships:

1. Set the tag with a tags-only PUT: GET the item, append
   `{"text":"blocked-by-cluster-restart"}` to `tags`, PUT with
   `?expect_version=<version>`. Legal on open and closed items alike
   (`api.rs:1038-1043`).
2. `devops/iter reject --reason "blocked by: cluster restart — <what you need>"`.
   This parks the item and puts that sentence in `lasterror`, which the next run
   reads back.
3. The nightly window re-queues it. `requeue_after_restart.sh` takes a repeatable `--tag TEXT`
   (`requeue_after_restart.sh:146-159`; its default is `awaiting-cluster-restart`),
   so the window passes it a second time for `blocked-by-cluster-restart`; it moves `parked` and `paused` items to
   `queued`, removes the tag, and appends a `doc` row naming the window's time and
   the env-status exit code.

**Gap one: the attempt counter is one higher than it should be.** `reject` does
not decrement, and the claim already incremented (`engine.rs:846`). Every night a
work item waits, it burns one of its five attempts (`maxattempts`,
`iter_core/src/lib.rs:318`). An item that parks four nights running has one
attempt left before the failure ladder writes it off. Until §1 step 4 ships, a
human must reset `attempt` by hand on any item that parks more than twice.

**Gap two: the tag is stripped at re-queue time, not at in-progress time.** The
requeue script removes it during the PUT that sets `queued`, which is minutes to
hours before the engine actually dispatches. In that gap the item shows no reason
it is waiting, which is the exact silence `blocked_by` visibility was built to
end. The engine version strips it inside the claim instead (§3) and the tag stays
truthful right up to the moment the agent starts.

## 5. Tests the engine change ships with

Each rule broken on purpose and watched go red (PDY-TECH-020), as `#[test]`s in
the crate that owns the rule:

1. `claim_tags` drops `blocked-by-cluster-restart` and every `blocked by: ` tag
   and keeps everything else — red control: return the input unchanged.
2. A tagged queued item is absent from the pick list when `cluster_healthy` is
   false and present when it is true — red control: drop the new filter from
   `engine.rs:685-694`.
3. `cluster_healthy` is false when the newest clone is complete but its
   `clusterhealth.status_exit` is 3 — red control: test only `state == "complete"`.
4. `cluster_healthy` with no clone in 24 h is false at 03:00 and true at 06:01
   `America/Los_Angeles` — red control: compare in UTC, which is 8 hours out.
5. `block --cluster-restart` leaves `attempt` at its pre-claim value — red
   control: reuse `set_state`, which does not touch it.

**Acceptance measured on THIS project.** `iter_engine` spawns the agent as bare
`Command::new("claude")` off `PATH` (`work.rs:539`), so the fake agent is a script
named `claude` placed first on `PATH`; `--ticks N` runs N ticks and drains
(`iter_engine/src/main.rs:33-35`). In a throwaway project under
`$TMPDIR`: the fake agent calls `iter block --cluster-restart` at a faked 02:30
Pacific; one `--ticks 1` at a faked 03:00 dispatches nothing and the item still
carries the tag; one `--ticks 1` at a faked 06:01 dispatches it and the read-back
item has `state: in-progress`, no `blocked-by-cluster-restart` tag, and the same
`attempt` number it had before it blocked.

## 6. For Stephen

**Engine work, in your repo `~/dev/iter` (agents here are read-only there):**
§1 the `block` verb, §2 the dispatch filter and the `cluster_healthy` evaluation,
§3 the extracted `claim_tags`, the one-line `run_shell` environment change, and
§5's tests. Roughly one focused session.

**pdy-dev work, here:** the `clusterhealth` detail-row POST inside
`cluster_restart_window.sh`, the second `--tag` pass in the window's re-queue
call, and this file.

**One decision needed.** The 24-hour fallback treats 02:00–06:00 Pacific as
unavailable *by the clock* even when no rebuild is running — a cluster that was
fine all night is off limits to tagged items for four hours. The alternative is to
block only on a genuine in-flight window and let the clock rule go. Your sentence
says the clock, so the clock is what is specified; say the word if you want the
narrower rule instead.
