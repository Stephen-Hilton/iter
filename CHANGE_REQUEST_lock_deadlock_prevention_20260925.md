# Change request: locks held by items that are not running, and the wait cycle they closed

Date: 2026-09-25. Project where it was measured: pdy-dev. Engine source read: `~/dev/iter`, commit `56ed0df`.
Author: a read-only investigation agent working for Stephen. Nothing in `~/dev/iter` was changed; this file is the only file written.

All times are UTC, with Pacific Daylight Time (PDT, UTC-7) in brackets where it helps. "Item ed93" means the work item whose id ends in `ed93b98a2679`; ids are cited by their last 12 characters, the way the webui shows them, and in full where a test must use them.

---

## 1. What happened

### 1.1 The items

| Short id | Full id | Priority | State at 17:20Z | What it is waiting on |
|---|---|---|---|---|
| a054fa190fb2 | `e84f5d4f-fb75-417e-afea-a054fa190fb2` | P0 | queued, attempt 0, `run_now: true` | Lock rows held by ed93. Its `blockedby_locks` is `["b5c2bc8f-0ab9-49bf-9f0a-ed93b98a2679"]` and its engine-owned tag is `blocked by: lock {topdir}/core/repos/pdy_core_intake/src`. Its `blockedby` is empty, so its dependencies are satisfied. |
| ed93b98a2679 | `b5c2bc8f-0ab9-49bf-9f0a-ed93b98a2679` | P0 | queued, attempt 1, version 50 | Until 16:40Z: item 63491 by dependency (`blockedby: ["5d68139b-…-63491afb13c0"]`). Now: nothing but the usage cap (tag `blocked by: usage cap (2/2)`). |
| 63491afb13c0 | `5d68139b-c7b7-43ea-a066-63491afb13c0` | P0 | queued, attempt 1 | Its own children: `blockedby` is a054, 146c269bae79, 1294cc1d9ec3, 0837b0f9efd5. Items 146c and 1294 are complete. Item 0837 is queued and is itself blocked by a054, 146c and 1294. |
| 1943e6203935 | `849fe2db-9f13-419c-96a8-1943e6203935` | P0 | queued, attempt 2 | Lock rows held by ed93: `blockedby_locks: [ed93]`, tag `blocked by: lock {topdir}/demos/03_stream_employee_wages/settle_driver.py`. |

Items a054, 146c, 1294 and 0837 all have `createdby` = item 63491, so they are 63491's children.

### 1.2 Timeline of item ed93, from its detail rows (`GET /api/projects/pdy-dev/workitems/b5c2bc8f-…/details`)

- 06:11:32Z: attempt 3 ended ("the three builders are still running"); the close gate bounced it once.
- About 06:26Z: attempt 5 started. It hit the 7200-second session limit at 08:26:24Z (`error` row: "timed out after 7200s").
- 08:27:27Z: Stephen reopened it ("reopened by stephen (was failed)"); the attempt counter went back to 0.
- 09:14:10Z: attempt 1 started (`ts.start`).
- 10:55:45Z: during that run, the agent ran `iter wait --on 5d68139b…` (the `doc` row "waiting on 5d68139b-… (the code agent, attempt 1)"), which added `blockedby: [63491afb13c0]` to item ed93.
- 10:56:01Z: the run ended; commit `17bbcc0b5`.
- 10:57:29Z: the close gate's verifier said incomplete, the engine found an open declared blocker, and it put the item back to `queued` with `lasterror` "close gate: waiting on 1 open blocker(s) [63491afb13c0] — …". No bounce was counted.
- About 16:40Z: Fable removed `blockedby` from ed93 with a versioned PUT, which broke the cycle.

### 1.3 The lock rows (`GET /api/projects/pdy-dev/locks`, read at 17:20:39Z)

- Item ed93 holds **42** rows, all `kind: "lock"`, all `engine: ""`, all with `expires` exactly 3600 seconds after `acquired`. The newest were acquired between 17:12:10Z and 17:12:20Z. Earlier readings the same day showed `acquired` 16:11Z and `expires` 17:11Z on the same paths.
- Only **one** of those 42 paths is in item ed93's own `lockdirs`, which is the single entry `{topdir}/core/repos/pdy_core_intake/src`. The other 41 paths span custody, the Kong gateway, Kafka topics, SDK files, `devops/SECURITY_DELTA.md` and `demos/03_stream_employee_wages/settle_driver.py`.
- Item a054's `lockdirs` overlap five of ed93's rows: `pdy_core_intake/src` (equal), `pdy_core_intake/proto` (it contains the locked `proto/intake.proto`), `pdy_core_intake/deploy` (it contains three locked paths under `deploy/`), and the techreq file (equal).
- Item 1943's `lockdirs` contain `demos/03_stream_employee_wages/settle_driver.py`, which ed93 holds.

### 1.4 What renewed item ed93's locks: an orphaned shell loop the agent started

The engine did not renew them; nothing in the engine renews a lock (section 2.2). On the machine StephenMBP, process **21222** is `bash .iter/temp/renew-locks-b5c2bc8f.sh`. Its parent is PID 1, so it is orphaned. It was started Thu 2026-09-24 23:27:02 PDT, which is 2026-09-25 06:27:02Z, about one minute into attempt 5. Its stdin, stdout and stderr are `/dev/null`, and its process group is 21217, a group whose leader no longer exists. It is not in the process group of any `claude` session the engine started. The script is in pdy-dev's git-ignored `.iter/temp/` directory and reads:

```bash
# Renews this item's file locks every 20 minutes while the item runs.
while true; do
  for p in core/repos/pdy_core_intake/src … devops/SECURITY_DELTA.md $(cat .iter/temp/extra-locks-b5c2bc8f.txt 2>/dev/null); do
    curl -s -o /dev/null -X POST -H "Authorization: Bearer $ITER_ENGINE_TOKEN" -H "Content-Type: application/json" \
      -d "{\"path\":\"{topdir}/$p\",\"workid\":\"$ITER_WORKID\"}" "$ITER_DATA_URL/api/projects/$ITER_PROJECT/locks/acquire"
  done
  date -u +%FT%TZ >> .iter/temp/renew-locks-b5c2bc8f.log
  sleep 1200
done
```

The loop inherited `ITER_WORKID`, `ITER_ENGINE_TOKEN` and `ITER_DATA_URL` from the agent's environment (work.rs:620-633). Its log, `.iter/temp/renew-locks-b5c2bc8f.log`, has a line every 20 minutes from 05:58:57Z to 17:12:20Z. The 05:58:57Z line came from an earlier copy of the loop that no longer runs; process 21222 wrote every line from 06:27:09Z on. The loop kept running through attempt 5's timeout, Stephen's reopen, the 09:14–10:57Z run and everything after it. At 17:20Z it was sleeping (child `sleep 1200`, PID 81194, started 10:12:20 PDT).

Its request sends no `engine` and no `ttl_sec`, so the server fills in `engine: ""` and a 3600-second lifetime (api.rs:1869-1881). A re-acquire by the same `workid` always succeeds and rewrites `acquired` (sqlite.rs:185-205; ddb.rs:277, whose condition includes `OR workid = :wid`). That is why the rows show `engine ""`, `acquired` about 20 minutes ago and `expires` one hour after `acquired`.

The same leak is visible on a second item, without a renewer. Item `42d8c7a2-…-7a8109b1e56b` (P50) is queued after a close-gate bounce. It still holds `{topdir}/core/repos/pdy_core_conformance` with `engine ""`, acquired 16:53:15Z and expiring 17:53:15Z. That path is not in its `lockdirs`, so the engine did not release it at close, and it blocks any queued item overlapping that container until 17:53Z.

### 1.5 The cycle

- Item a054 waited on item ed93 through a **lock edge**: the dispatcher counts ed93's rows as blocking (engine.rs:820-834), even though ed93 was not running.
- Item ed93 waited on item 63491 through a **dependency edge**: its `blockedby` held 63491, which was open.
- Item 63491 waited on item a054 through a **dependency edge**: its `blockedby` held its own child a054, which was open.

None of the three could start. The cycle existed from 10:57:29Z, when ed93 closed back to queued, until about 16:40Z: about 5 h 43 min, with four P0 items stalled. During that time the engine correctly started P50 items whose locks were free: `e920cf25-…-2a4ea59e2588` at 16:33:40Z and `…-a30daaf98565` at 17:05:01Z, which is why the cap now reads 2/2.

**Breaking the dependency edge did not unblock a054 or 1943, and nothing in the engine ever will.** Process 21222 re-acquires ed93's 42 rows every 20 minutes with no end condition. Item ed93 will run when a cap slot frees and will release only its one `lockdirs` path, and the loop will take that path back within 20 minutes. Stopping process 21222 is an operational step for Stephen (section 11, question 1), and it is needed before the code change lands.

---

## 2. How the engine works today, with each claim checked

### 2.1 Acquiring locks

- **The engine at dispatch.** `Engine::start_item` (engine.rs:1147-1245) first claims the item with a versioned PUT to `in-progress` (engine.rs:1157-1188). Then, for each `lockdirs` entry, it POSTs `locks/acquire` with `kind: "lock"`, `engine: <engine name>` and `ttl_sec: 3900`; the lifetime is `let ttl = 3600 + 300` at engine.rs:1191, and the loop is engine.rs:1190-1217. If any acquire fails, it releases the ones it took, PUTs the item back to `queued`, and defers the item by 5 seconds (engine.rs:1200-1215).
- **The engine at session continuation.** `claim_chain_candidate` (work.rs:173-235) claims a neighbouring item the same way, again with `ttl_sec: 3900` (work.rs:221).
- **The engine's reservations.** When the cap has a free slot, dispatch picks the best queued item that is blocked by a lock, the first `scope_blocked` item in priority order (engine.rs:980). It POSTs one `kind: "reserve"` row per `lockdirs` entry with `engine: self.name` and `ttl_sec: 600` (engine.rs:981-988). That is the "reserve" row with engine "StephenMBP". A reserve row gates only items of equal or worse priority whose paths overlap (engine.rs:1009-1017). No reservation is written while the cap is full, because dispatch returns at engine.rs:969-976 before it reaches engine.rs:980.
- **A running agent.** The agent's environment carries the engine's own token (work.rs:632), so an agent can POST `locks/acquire` itself. That is the "widen the write fence" practice recorded in pdy-dev's memory. The server checks nothing about the holder: `lock_acquire` (api.rs:1883-1907) requires only the writer role (api.rs:81-86) and passes the request to `acquire_lock`. It does not check that the `workid` exists, that its item is in progress, or that the caller is the engine running it.
- **The storage rule.** One row exists per (project, path). An acquire succeeds when no row exists, the row has expired, or the row's `workid` equals the caller's (sqlite.rs:165-210; ddb.rs:258-290, condition at ddb.rs:277). Lock rows and reserve rows share that key, so they share one row per path.

### 2.2 Renewing locks

- **Nothing in the engine renews a lock.** The server has `POST locks/extend` (api.rs:1934-1955), but no code in `iter_engine`, `iter_core`, `iter_local`, the webui or `e2e.sh` calls it. A search of the whole `~/dev/iter` tree for `locks/extend` finds only the route (api.rs:174).
- **A consequence nobody had noticed: the engine's own locks expire mid-run.** The engine takes its rows for 3900 seconds (65 min), while the session limit in pdy-dev is 7200 seconds. After 65 minutes a running item's rows lapse, `locks_list` stops returning them (api.rs:1859-1867), dispatch stops counting them (engine.rs:785-789), and another item overlapping the same paths can start while the first agent is still writing. The loop's own comment, "Renews this item's file locks every 20 minutes while the item runs", suggests this is why the agent wrote it. That is an inference from the comment; no agent said so. Item 2a4ea59e2588's engine row, acquired 16:33:40Z, expires 17:38:40Z whatever its run is doing.
- **What renewed ed93's locks at 16:11Z and 17:12Z** was process 21222 (section 1.4), not the engine and not a heartbeat.

### 2.3 Releasing locks

- **At the end of a run** `close` (work.rs:1239-1405) writes the final state, then releases each path in `item.lockdirs` (work.rs:1381-1388). `item` here is the snapshot claimed at dispatch (engine.rs:1233-1240 passes `claimed_item` into `work::execute`). Release errors are ignored (`let _ =`).
- **When the agent moved the item itself** (`iter ask`, `iter reject`, `iter block`), `close_keep_state` (work.rs:1225-1235) releases the same `item.lockdirs`.
- **When the close gate re-queues behind a declared blocker**, the path is `GateHold::Waiting` (work.rs:1285-1300 decides it; work.rs:1340-1345 writes `state: "queued"`). It releases through the same loop at work.rs:1381-1388. **That claim is confirmed: the Waiting path does release, but only the item's `lockdirs`.**
- **A row the agent took with `locks/acquire` on any other path is never released** by the engine. It lives until its own expiry, 3600 seconds by the server default. Item ed93 left 41 such rows at 10:57Z, and the loop kept them and the 42nd alive. Item 7a8109b1e56b left one (section 1.4).
- **A human state change does not release anything either.** `workitem_state` (api.rs:1748-…), `workitem_reopen` (api.rs:1699-…) and `workitem_put` (api.rs:1373-1422) never touch lock rows.
- **The timeout path kills less than the stop path.** `wait_with_timeout` starts the session in its own process group (work.rs:1031). On a user stop it signals the whole group (work.rs:1045-1050). On timeout it kills only the direct child (work.rs:1063-1066). Killing the group would still not have reached process 21222, which is in a different group (21217).

### 2.4 How the dispatcher chooses

`Engine::dispatch` (engine.rs:680-1040), in order:

1. It reads the live lock rows and drops expired ones (engine.rs:776-796).
2. `deps_satisfied` means `dependency_status(...) == Satisfied` (engine.rs:816-819; lib.rs:835-877). The rule is deep: a blocker counts only when it and everything it created are complete (lib.rs:826-877).
3. `lock_holders(item)` lists every live row with `kind == "lock"` whose `workid` is not the item's own and whose path overlaps one of the item's `lockdirs` (engine.rs:823-833). The comments at engine.rs:820-822 and lib.rs:560-567 say "the running items", but **the code never checks the holder's state.** A row held by a queued, parked or failed item blocks exactly as one held by a running item. This is the defect that let a054 and 1943 wait on ed93.
4. Each queued item gets a wait reason (engine.rs:836-890). A lock wait writes `blockedby_locks` and the tag `blocked by: lock <path>` (reconcile_waits, engine.rs:1049-1097). A `DepStatus::Cycle` becomes `blocked by: dependency cycle with <id12>` (engine.rs:874-878). That cycle check follows `blockedby` links only (`waits_on`, lib.rs:752-770), so it never sees a lock edge, and this cycle was never named.
5. **`run_now`** (engine.rs:925-945) starts a flagged item even when the cap is full, but only when `deps_satisfied(i) && !scope_blocked(i)` (engine.rs:932). **The claim "run_now did not override the lock" is confirmed as the designed behaviour**: the comment at engine.rs:925-926 says it starts "as soon as its dependencies are complete and no lock overlaps".
6. The remaining queued items are those whose dependencies are satisfied and which are not held for triage, backoff, the cluster restart or a deferral. They are sorted by `(priority, ts.receive)` (engine.rs:952-967).
7. If the cap is full, every queued item gets the reason "usage cap" and dispatch returns (engine.rs:969-976).
8. Reservation happens here (engine.rs:978-997; see 2.1).
9. The pick loop (engine.rs:999-1039) skips any scope-blocked item (engine.rs:1004-1006), skips items gated by a reservation, applies the per-agent cap, and starts the rest in priority order. **The claim "the engine ran two P50 items whose locks were free" is confirmed as correct behaviour.** Every P0 was either scope-blocked (a054, 1943) or not dependency-satisfied (ed93, 63491, 0837), so the loop moved on to P50 items.

### 2.5 Write-time cycle refusal

`refuse_dependency_cycle` (api.rs:1104-1131) runs on create (api.rs:1190) and on every PUT that changes `blockedby` (api.rs:1415-1418). It calls `blockedby_cycle` (lib.rs:777-820), which follows explicit `blockedby` links only. At 10:55:45Z, ed93 → 63491 → {a054, 146c, 1294, 0837} → (a054 has no `blockedby`) did not reach ed93, so the `iter wait` write was accepted correctly by its own rule. **The lock edge a054 → ed93 that closed the loop is invisible to every cycle check in the code.**

### 2.6 The webui hid the edge

`lockHolderOf` (webui/index.html:726-730) nests a queued item under a lock holder only when the holder is `in-progress`. Item ed93 was queued, so a054 and 1943 were shown as roots with no parent, and the operator saw P0 items sitting idle for no visible reason.

### 2.7 What the code does not explain

- The code does not show when `run_now` was set on a054; no detail row records it.
- The code does not show whether the 05:58:57Z copy of the loop was stopped by the agent or killed with attempt 3's session.
- Why the agent chose to renew locks itself is inferred only from the script's comment (2.2).

---

## 3. Root cause

1. **A lock is not tied to a run.** The server grants and re-grants a lock to any `workid`, whatever state its item is in (api.rs:1883-1907; sqlite.rs:185-205; ddb.rs:277). The engine releases only the paths in `lockdirs` (work.rs:1381-1388, 1232-1234), so rows an agent took itself survive the run, and a process the agent leaves behind can keep them alive forever.
2. **The dispatcher treats a row held by a non-running item as a live lock** (engine.rs:823-833, despite the comments at engine.rs:820-822 and lib.rs:560-567). That turns the leaked rows into a wait edge pointing at an item that is itself waiting.
3. **No cycle check looks at lock edges** (lib.rs:752-820, api.rs:1104-1131), and the webui hides lock edges to non-running holders (index.html:726-730). The cycle was neither refused, nor detected, nor shown.
4. **A contributing cause:** the engine's own rows last 65 minutes and are never renewed (engine.rs:1191, work.rs:221, no caller of api.rs:1934), so a long run loses its locks. That gives agents a reason to renew locks themselves.

---

## 4. The options, evaluated

**(a) An item that is not running holds no locks. Recommended; this is the core of the fix.**
Lock edges are the only edges that can point at an item that is itself waiting on something else. If only running items can hold locks, a lock edge always ends at an item with no outgoing wait edges, so no cycle can contain a lock edge (section 5.3). Engine-side release alone is not enough: process 21222 re-acquires within 20 minutes of any release. The server must refuse to grant or extend a lock for an item that is not running. So (a) is implemented as a **lease**: a random id the engine writes on the item when it claims it and clears when the run ends. The server grants and extends lock rows only for an item that carries a lease, and stamps each row with that lease.

**(b) Wait-for-graph cycle detection. Recommended, as a backstop and for visibility.**
Once (a) is in place, lock edges cannot form a cycle, but dependency-only cycles still can. Deep edges, which run from an item to the open descendants of a complete blocker (lib.rs:853-873), appear without any `blockedby` write, so a write-time check cannot see all of them. Section 5.4 gives an example. The detector runs every tick and on the server's `GET /deadlocks`. It auto-resolves only a cycle that contains a lock edge to a non-running holder, which can happen only before enforcement is switched on or through a bug: it releases every lock of every non-running holder in the cycle. Dependency-only cycles are tagged and reported, not edited (question 3).

**(c) Refuse at write time any write that would close a cycle. Recommended, extended to deep edges.**
For locks, (c) reduces to (a): the only item allowed to acquire is running, and a running item has no outgoing wait edges, so its acquisition cannot close a cycle. For `blockedby`, the existing refusal (api.rs:1104-1131) is extended to walk deep edges as well as explicit ones, using the same function the detector uses.

**(d) Priority rules and preemption. Not recommended beyond what (a) gives for free.**
"run_now preempts non-running lock holders" becomes automatic, because non-running items hold nothing. Preempting a *running* lower-priority holder would kill a live agent's work partway through a commit, and a lock wait on a running holder is already bounded by that holder's session limit. What is recommended is visibility: a `run_now` or P0 item waiting on a running lower-priority holder gets a wait reason naming the holder, when it started and when its session limit ends it. Whether P0 should ever preempt is question 4.

**(e) Short leases renewed only by a live engine. Recommended.**
Lock rows get a 600-second lifetime. The engine renews them every 60 seconds, only for the runs its own threads are executing. An abandoned row (a crashed engine, a race, a stray process) is gone within 10 minutes, and a long run no longer loses its locks at 65 minutes. The server refuses a renewal whose lease does not match the item's current lease, so a stray process cannot extend a row after the run ends.

**Lock ordering** (acquiring paths in a global order) solves a different problem: two holders each waiting for the other's lock while both are acquiring. The engine acquires all of an item's paths at once at claim time and backs off on any conflict (engine.rs:1200-1215), so that deadlock cannot happen today, and ordering adds nothing.

**Recommended combination: (a) as a lease + (e) + (c) extended to deep edges + (b) as detector and dashboard.**

---

## 5. Design

### 5.1 Terms

- **Lease:** a UUID v4 string the engine generates when it claims an item, written to a new field `WorkItem.lease` in the same versioned claim PUT that sets `state: "in-progress"`. It stays on the item for the whole run, including the seconds or minutes after an agent has moved its own item to `question` or `parked` mid-turn. Pdy-dev's memory records that agents keep committing after such a move. The engine clears the lease (`""`) in the final PUT of every exit from the run.
- **Live lease:** `item.lease` is non-empty.
- **Live lock row:** a row with `kind == "lock"` that has not expired, whose holder item exists and has a live lease, and whose `row.lease == holder.lease`.
- **Wait-for graph:** nodes are open items (queued, question, parked, in-progress). Edges go out of **queued** items only, because only queued items are waiting to be dispatched:
  - `Blocker`: X → B for every entry B in `X.blockedby` whose item is open.
  - `Deep`: X → C for every open descendant C (by `createdby`, transitively) of a *complete* blocker of X, unless C waits on X (the same exception as lib.rs:864-866), and only when X is not `blockedby_shallow`.
  - `Lock`: X → H for every live lock row held by H ≠ X whose path overlaps one of X's `lockdirs`.
- **Deadlock:** a strongly connected component of the wait-for graph with more than one node, or a node with an edge to itself.

### 5.2 Invariants the change guarantees

- **I1. A lock row is created or extended only for an item with a live lease, and it carries that lease.** The server enforces this in `lock_acquire` and `lock_renew`.
- **I2. Every exit from a run removes every lock row the run held, including rows the agent took outside `lockdirs`.** The engine clears the lease and calls `locks/release_all`. The server also deletes the rows carrying the old lease whenever a PUT changes an item's lease. Any row left by a race is removed by the sweep within 60 seconds, and in any case expires within 600 seconds, because nothing can renew it (I1).
- **I3. A queued item holds zero lock rows once the sweep has run.** This follows from I1 and I2: a queued item has no live lease, except for the one tick between the claim PUT and the start of the thread, during which the state is already `in-progress`.
- **I4. Every lock edge points at a running item.** Dispatch counts only live rows (5.1), and a live lease exists only between claim and close.
- **I5. No cycle in the wait-for graph contains a lock edge.** A lock edge X → H ends at a running item H (I4). A running item is not queued, so it has no outgoing edges. A cycle through H would need an edge out of H. Therefore none exists.
- **I6. Every lock wait is bounded.** It lasts at most the holder's session limit (`agent_timeout`, work.rs:451-463) plus the close, or the 600-second lease lifetime if the holder's engine dies.
- **I7. The explicit-plus-deep dependency graph gains no cycle through an accepted `blockedby` write** (extended (c)). Any cycle that arises without such a write is reported by the detector within one tick (5 seconds by default), tagged on every member, and served by `GET /deadlocks`.

What the change does **not** guarantee: it does not stop a P0 item waiting behind a running P50 item for one run of that item, and it does not auto-edit a dependency-only cycle.

### 5.3 Why I5 holds, as a check the implementer must keep true

The proof depends on exactly two facts, and each must stay true in code:

1. Only queued items have outgoing edges (`wait_edges` in 5.5 must filter on `state == "queued"`).
2. A live lease exists only while a run is live. Every code path that ends a run must clear the lease. The paths are `close` (work.rs:1239), `close_keep_state` (work.rs:1225), the claim rollback in `start_item` (engine.rs:1200-1215), the claim rollback in `claim_chain_candidate` (work.rs:222-231), and the human-accept shortcut (work.rs:127-134, which goes through `close`).

The property tests in 8.3 assert I5 directly, so a future edit that breaks either fact goes red.

### 5.4 A dependency-only cycle write-time refusal cannot see today

Suppose item X is `blockedby` P, and P is complete. P's child C is open, and C is `blockedby` Q, which is complete. Q's child D is open, and D is `blockedby` X. Then `waits_on(C, X)` is false, because C → Q stops at Q, which has no blockers. So X waits on C (Deep), C waits on D (Deep) and D waits on X (Blocker). No explicit `blockedby` link closes this loop; it closes when P and Q complete. The extended check in (c) catches it when the last link is written *after* P and Q are complete. If P or Q completes last, the loop is born from a state change, and only the detector sees it.

### 5.5 Deterministic resolution rule

On every tick, after classification:

- For each deadlock component, pick the canonical cycle: start from the member with the smallest id, take the shortest path back to it (breadth-first, visiting edges in id order), and report it as the list of edges.
- **If the cycle contains a lock edge whose holder has no live lease** (possible only with enforcement off, or through a bug), release every lock row of every such holder: `POST locks/release_all {workid}`. Append a `doc` detail row to the holder and to the waiter: "deadlock resolved by engine <name> at <ts>: released <n> lock rows of <holder12>, which was not running; cycle <a054 → ed93 → 6349 → a054>". Log one line.
- **Otherwise** (a dependency-only cycle), change nothing. Set the wait reason of every member to `deadlock: <id12> → <id12> → … (<kinds>)`, which reconcile_waits turns into the engine-owned tag `blocked by: deadlock: …`. Append one `doc` row per member the first time the engine sees this cycle. The engine remembers seen cycles in `HashSet<Vec<String>>` keyed by sorted member ids, so each cycle is logged once per engine process.

---

## 6. Exact code changes

Line numbers refer to commit `56ed0df`.

### 6.1 `iter3/iter_core/src/lib.rs`

1. **`WorkItem`** (field list around lib.rs:540-610): add
   ```rust
   /// engine-owned (2026-09-25): the live run's lease — set by the claim,
   /// cleared by every exit from the run; empty while the item is not running.
   /// Lock rows are granted and renewed only against it (iter_data lock_acquire).
   #[serde(default)]
   pub lease: String,
   ```
   Correct the `blockedby_locks` comment (lib.rs:560-567) so that it describes the new truth: holders with a live lease.
2. **`LockRow`** (lib.rs:707-723): add `#[serde(default)] pub lease: String`.
3. New constants: `pub const LOCK_LEASE_TTL_SEC: i64 = 600;` and `pub const LOCK_RENEW_EVERY_SEC: u64 = 60;`.
4. **`Project`** (lib.rs:345-…): add `#[serde(default)] pub lock_lease_enforce: bool`, the rollout switch (section 9).
5. New `pub fn live_lock_rows<'a>(rows: &'a [LockRow], by_id: &HashMap<String, &WorkItem>, now_iso: &str, enforce: bool) -> Vec<&'a LockRow>`. It keeps a row when all of these hold: `kind == "lock"`; `expires` is empty or `>= now_iso`; the holder exists; and either (`holder.lease` is non-empty and `row.lease == holder.lease`) or (`!enforce` and `row.lease` is empty and `holder.state == "in-progress"`). The second branch keeps rows written by engines from before the change while enforcement is off.
6. New `pub fn lock_holders(item: &WorkItem, live: &[&LockRow]) -> Vec<(String, String)>`. It is moved verbatim from the closure at engine.rs:823-833, minus the `kind` check, which `live_lock_rows` already made.
7. New wait-for graph types and functions:
   ```rust
   #[derive(Debug, Clone, PartialEq, Eq, Serialize)]
   #[serde(tag = "kind", rename_all = "lowercase")]
   pub enum WaitKind { Blocker, Deep, Lock { path: String } }
   #[derive(Debug, Clone, PartialEq, Eq, Serialize)]
   pub struct WaitEdge { pub from: String, pub to: String, #[serde(flatten)] pub kind: WaitKind }
   pub fn wait_edges(items: &[WorkItem], live: &[&LockRow]) -> Vec<WaitEdge>;
   pub fn find_wait_cycles(items: &[WorkItem], edges: &[WaitEdge]) -> Vec<Vec<WaitEdge>>;
   ```
   `wait_edges` enumerates **all** edges out of every queued item, following 5.1. It mirrors `dependency_status` (lib.rs:835-877) but collects every open blocker and every open deep descendant instead of returning at the first. `find_wait_cycles` runs Tarjan's strongly-connected-components algorithm, iteratively rather than recursively, because pdy-dev has about 2800 items. It returns one canonical cycle per component (5.5), sorted by smallest member id, so the output is deterministic.
8. New `pub fn dependency_cycle_on_write(item_id: &str, blockers: &[String], items: &[WorkItem]) -> Option<Vec<String>>`. It builds the dependency-only edges (Blocker and Deep) with the item's `blockedby` replaced by `blockers`, and returns the path of a cycle through `item_id` if one exists. It treats the item as queued for this purpose, because the item will be queued at the moment the link matters. Keep `blockedby_cycle` for its callers and tests, and have `refuse_dependency_cycle` call the new function.
9. Update `claim_tags` if needed so the claim also strips a `blocked by: deadlock…` tag. It already strips every `BLOCKED_TAG_PREFIX` tag, so no change is expected; confirm with a test.

### 6.2 `iter3/iter_data/src/api.rs`

1. **Routes** (api.rs:171-174): add
   - `.route("/api/projects/{name}/locks/renew", post(lock_renew))`
   - `.route("/api/projects/{name}/locks/release_all", post(lock_release_all))`
   - `.route("/api/projects/{name}/locks/sweep", post(lock_sweep))`
   - `.route("/api/projects/{name}/deadlocks", get(deadlocks_get))`
   
   Point `locks/extend` at `lock_renew`, keeping the old body shape and ignoring `path`, so no old caller gets a 404.
2. **`LockAcquireReq`** (api.rs:1869-1879): add `#[serde(default)] lease: String`. Change `default_ttl` (api.rs:1881) to return `LOCK_LEASE_TTL_SEC` when the project enforces, and 3600 otherwise. The project is read in the handler, so `default_ttl` becomes a plain `0` sentinel resolved inside the handler.
3. **`lock_acquire`** (api.rs:1883-1907): before writing,
   - load the project, and load the holder with `st.store.get("workitem", &name, &req.workid)`. A missing holder is a `404` "no such work item <workid>".
   - `kind == "lock"`: if `holder.lease` is empty, then with enforcement on return `409` with body `{"refused": "not running", "workid": …, "state": holder.state}` and message "refused: work item <id12> is not running (state <s>); locks are held only by a running item, and the engine releases them when the run ends". With enforcement off, allow the write only when `holder.state == "in-progress"`; otherwise allow it but add `"warnings": ["would be refused under lock_lease_enforce: …"]` and log one line (`eprintln!`). If `req.lease` is non-empty and differs from `holder.lease`, return `409` "stale lease". Stamp `row.lease = holder.lease`. With enforcement on, cap `ttl_sec` at `LOCK_LEASE_TTL_SEC`.
   - `kind == "reserve"`: require `holder.state == "queued"`, else `409` "a reservation is held only by a queued item".
   - Fill `row.engine` from `holder.engine` when the request left it empty. Rows then name the engine whose run holds them, not "".
4. **New `lock_renew`**, body `{workid, lease, ttl_sec}`: load the holder; if `holder.lease` is empty or differs from `lease`, return `409` "lease <8> is not the live lease of <id12>". Otherwise set `expires = now + min(ttl_sec, LOCK_LEASE_TTL_SEC)` on every row where `workid == W` and `lease == L`, and return `{"renewed": n}`. Rows are found with `st.store.query("lock", &name)` and filtered, because the storage layer has no secondary index. The row count per project is small: 51 in pdy-dev at 17:20Z.
5. **New `lock_release_all`**, body `{workid, lease?}`: delete every row (`kind` lock or reserve) with that `workid`, and with that `lease` too when one is given. Return `{"released": [paths]}`. The engine may call it with a stale workid, so it must succeed on zero rows.
6. **New `lock_sweep`**, body `{dry_run: bool}`: delete every lock row that `live_lock_rows` would not keep (with the project's `enforce` value), and every reserve row whose holder is missing or not queued. Return the removed rows. Require the `engine` or `admin` role.
7. **`locks_list`** (api.rs:1859-1867): add to each returned row `holder_state` and `holder_lease_live` (true when the row counts as live under 6.1 item 5). The webui and operators can then see a row held by a non-running item.
8. **New `deadlocks_get`**: read items and lock rows, compute `live_lock_rows`, `wait_edges` and `find_wait_cycles`, and return
   ```json
   {"computed": "<ts>", "enforce": true,
    "cycles": [{"members": ["<id>", …],
                "edges": [{"from": "<id>", "to": "<id>", "kind": "lock", "path": "{topdir}/…"}, {"from": …, "kind": "blocker"}],
                "auto_resolvable": true}],
    "stale_lock_rows": [{"path": …, "workid": …, "holder_state": …}]}
   ```
   `auto_resolvable` is true when the cycle contains a lock edge to a holder with no live lease. `stale_lock_rows` lists rows that are not expired and not live.
9. **`workitem_put`** (api.rs:1373-1422):
   - **Keep the stored lease when the body omits the `lease` key** (`body.get("lease").is_none()` → copy it from `current`). Otherwise an older client, such as an agent's GET-modify-PUT script or an old webui bundle that drops unknown fields, would clear a live lease by accident. Only an explicit `"lease": ""` or a new value changes it.
   - After `put_versioned` succeeds, if the stored lease was non-empty and the new lease differs, delete every lock row with `workid == id && lease == old_lease`. This is the server-side half of I2, so no engine code path can forget it.
10. **`refuse_dependency_cycle`** (api.rs:1110-1131): call `iter_core::dependency_cycle_on_write` in place of `blockedby_cycle`, and keep the refusal text format ("refused: dependency cycle — <id12> would wait on itself (<path>)"), adding the edge kind when an edge is Deep ("… -> <id12> (deep: created by <id12>) -> …").
11. **`workitem_state`** (api.rs:1748-…) and **`workitem_reopen`** (api.rs:1699-…): make no lease change. A human moving an item does not end a live run. The engine's close clears the lease, and if the engine is dead the rows expire within 600 seconds.

### 6.3 `iter3/iter_data/src/sqlite.rs`, `ddb.rs`, `storage.rs`

No schema change: `lease` lives in the JSON body. `acquire_lock` stays as it is. The same-workid overwrite is still correct, because a new lease for the same workid must be allowed to replace a stale row from a previous run.

### 6.4 `iter3/iter_engine/src/engine.rs`

1. **`running`** (engine.rs:26): replace the tuple `(String, String, JoinHandle<()>)` with
   ```rust
   struct Running { agent: String, project: String, cur: Arc<Mutex<(String /*workid*/, String /*lease*/)>>, handle: JoinHandle<()> }
   ```
   `cur` is shared with the worker thread, because `work::execute` chains to other items in the same thread (work.rs:88-110), and renewal must follow the item that is running now. Update `prune_running` (engine.rs:411-414), the `in_flight` closure (engine.rs:803-807) and the per-agent cap count (engine.rs:1020-1021) to read `cur`.
2. **New `fn renew_leases(&mut self)`**, called from `tick` (engine.rs:416) **before** any project hold or early return. A project on hold or at its budget still has running items whose locks must stay alive. Every `LOCK_RENEW_EVERY_SEC`, for each `Running`, POST `locks/renew {workid, lease, ttl_sec: LOCK_LEASE_TTL_SEC}`. On `409`, log "[engine] <id8>: lease lost (<reason>) — its locks are gone; the run continues unfenced" once per run, and append a `doc` row saying so. Question 5 asks whether to stop the run instead.
3. **`start_item`** (engine.rs:1147-1245):
   - generate `let lease = uuid::Uuid::new_v4().to_string();` and set `claimed["lease"] = json!(lease)`.
   - in the acquire body (engine.rs:1196-1197), send `"lease": lease` and `"ttl_sec": iter_core::LOCK_LEASE_TTL_SEC`. Delete `let ttl = 3600 + 300`.
   - on a lost race (engine.rs:1200-1215), replace the per-path release loop with one `POST locks/release_all {workid, lease}`, and set `back["lease"] = json!("")` together with `back["state"] = json!("queued")`.
   - pass `cur` into `work::execute`.
4. **`dispatch`**:
   - engine.rs:776-796: deserialize the rows into `Vec<LockRow>` and compute `let live = iter_core::live_lock_rows(&rows, &by_id, &now_iso, project.lock_lease_enforce);`. Reserve rows are still read from `rows`, filtered on `kind == "reserve"` and not expired.
   - engine.rs:823-833: replace the closure body with `iter_core::lock_holders(item, &live)`.
   - **Sweep:** once per `LOCK_RENEW_EVERY_SEC` per project, POST `locks/sweep {dry_run: !project.lock_lease_enforce}`. With enforcement off, log each row it would remove, as the dry-run signal for rollout.
   - **Detect:** after the classification loop (after engine.rs:890), compute `let edges = iter_core::wait_edges(&items, &live); let cycles = iter_core::find_wait_cycles(&items, &edges);` and apply 5.5. The detector's reason replaces `dependency cycle with …` (engine.rs:874-878) for every member of a detected cycle; keep the `DepStatus::Cycle` branch as the fallback for the one-tick gap.
   - **run_now visibility:** when a `run_now` item is scope-blocked, set its reason to `run now waits on running <id8> (started <hh:mm>Z, session limit ends it by <hh:mm>Z)`. The holder's `ts.start` plus `agent_timeout` gives the second time.
5. **`reconcile_waits`** (engine.rs:1049-1097): no change. It already writes whatever reason dispatch sets.
6. New field `announced_cycles: HashSet<Vec<String>>` on `Engine`, for the log-once rule in 5.5.

### 6.5 `iter3/iter_engine/src/work.rs`

1. **`execute`** (work.rs:88-110): take `cur: Arc<Mutex<(String, String)>>`. After a successful `claim_chain_candidate`, write the new `(workid, lease)` into it.
2. **`claim_chain_candidate`** (work.rs:173-235): generate a lease and set `claimed["lease"]`. In the acquire at work.rs:219-222, send the lease and `LOCK_LEASE_TTL_SEC` instead of 3900. On rollback, call `release_all` and PUT back with the lease cleared. Return the lease with the item (`Option<(WorkItem, String)>`).
3. **`close`** (work.rs:1239-1405): in the versioned final PUT (work.rs:1316-1379), set `updated["lease"] = json!("")` for **every** branch: stopped, Bounce, Waiting, complete, failed and retry. Replace the lockdirs release loop at work.rs:1381-1388 with `POST locks/release_all {workid: item.id, lease: <the run's lease>}`, retried up to three times with 1, 2 and 4 seconds of backoff, logging the final failure. Rows it cannot release still die: the server deletes them when the PUT clears the lease (6.2 item 9), the sweep catches the rest, and nothing renews them.
4. **`close_keep_state`** (work.rs:1225-1235): the agent already changed the state. GET the item, set `lease: ""`, and PUT with `expect_version`, retrying once on `409`. Then call `release_all`.
5. **`wait_with_timeout`** (work.rs:1063-1066): before `child.kill()` on timeout, signal the process group exactly as the stop branch does (work.rs:1045-1049). This does not reach a process that left the group, as process 21222 had, but it ends the ordinary background builders a timed-out session leaves behind.
6. The human-accept path (work.rs:127-134) goes through `close` and needs nothing else.

### 6.6 `iter3/iter_engine/src/prompt.rs`

In both work-item blocks (prompt.rs:321 and prompt.rs:390), after the "Codepath (your working directory and lock scope)" line, add:

> Locks: the engine keeps every lock this run holds alive while the run lasts, and releases all of them when it ends, including any you took yourself with `locks/acquire`. Do not renew locks, and do not leave any process running after your turn ends; a lock request made after this run has ended is refused.

### 6.7 `iter3/iter_engine/src/cli.rs`

- `iter status` (around cli.rs:1270-1295): after the item list, GET `/deadlocks` and print each cycle as one line: `deadlock: a054fa190fb2 -(lock {topdir}/…/src)-> ed93b98a2679 -(blocker)-> 63491afb13c0 -(blocker)-> a054fa190fb2`.
- `iter wait` (cli.rs:1109-1130): no change. It inherits the extended refusal from the server.

### 6.8 `iter3/webui/index.html`

- `lockHolderOf` (index.html:726-730): return the first holder in `blockedby_locks` whatever its state. When the holder is not `in-progress`, render the link at index.html:1413 as `<id8> (lock, holder not running)` in the deadlock colour, so a leaked lock is visible at a glance.
- A **deadlock banner** above the item list: poll `GET /deadlocks` together with the items, and show one red row per cycle with its members as links and the edge kinds. Show a second, amber row when `stale_lock_rows` is non-empty, reading "n lock rows are held by items that are not running".
- The locks panel shows `holder_state` and `holder_lease_live` per row.

### 6.9 `iter3/e2e.sh`

The lock section (e2e.sh:168-178) acquires for `workid: "w-test"`, which is not an item. Under enforcement that request is a `404`. Change it to create an item, claim it with a PUT carrying `state: "in-progress"` and a lease, then acquire, conflict, re-acquire and release as today. Then add the refusal case: PUT the item back to queued with `"lease": ""` and assert that an acquire for it returns `409` with `refused: "not running"`.

---

## 7. API and dashboard summary

- `POST locks/acquire`: refuses a lock for an item with no live lease (`409 {"refused": "not running"}`), refuses a reservation for an item that is not queued, and stamps `lease` and `engine` on the row.
- `POST locks/renew {workid, lease, ttl_sec}`: new. It is the only way a lock row's life is extended, and only the lease's own engine knows the lease.
- `POST locks/release_all {workid, lease?}`: new.
- `POST locks/sweep {dry_run}`: new. It removes rows held by items that are not running.
- `POST locks/extend`: an alias of `renew`.
- `GET locks`: rows gain `holder_state` and `holder_lease_live`.
- `GET deadlocks`: new; the shape is in 6.2 item 8.
- `PUT workitems/{id}`: an omitted `lease` key is kept, and a changed lease deletes the old lease's rows.
- **Tags**, all engine-owned under the existing `blocked by: ` prefix, so the claim strips them (lib.rs:227-228, claim_tags):
  - `blocked by: deadlock: <id12> → <id12> → <id12> (lock, blocker, blocker)`
  - `blocked by: run now waits on running <id8> (started hh:mmZ, session limit ends it by hh:mmZ)`
- **Detail rows** (`doc`) on each member the first time a cycle is seen, and on each auto-resolution.
- **Engine log lines:** `[engine] <project>: deadlock <cycle>`, `[engine] <project>: released <n> lock rows of <id8> (not running) to break <cycle>`, `[engine] <id8>: lease lost …`, and one sweep line per removed row.

---

## 8. Tests

Each test named here must first be seen failing against today's code, and the failing output kept in the commit message or the plan. This is the "a guard ships with evidence of its own failure" rule pdy-dev works under.

### 8.1 A reproduction of this exact cycle, at three levels

**(i) Pure, `iter_core` unit test `pdy_dev_20260925_lock_dependency_cycle`.** The fixture uses the real full ids from section 1.1 and the states, `createdby`, `blockedby` and `lockdirs` read at 10:57:29Z: ed93 is queued with `blockedby [63491]` and `lease ""`; 63491 is `blockedby` its four children; 146c and 1294 are complete; a054 is `run_now: true`. It also has five of ed93's lock rows, including `{topdir}/core/repos/pdy_core_intake/src` and `{topdir}/demos/03_stream_employee_wages/settle_driver.py`, stamped with `lease` "L5", the lease of a finished run.
- *Today's behaviour, captured first:* extract `lock_holders` into `iter_core` in a refactor commit that changes no behaviour, with a `live_lock_rows` that reproduces today's filter (kind and expiry only). Then assert `lock_holders(a054)` is non-empty and `lock_holders(1943)` names ed93. Committed as a "documents the defect" assertion, this is green on the refactor. The real test asserts the opposite and is red on the refactor.
- *After the change:* `live_lock_rows(…, enforce=true)` drops all five rows, because ed93's lease is empty. `lock_holders(a054)` and `lock_holders(1943)` are empty. `find_wait_cycles` returns no cycle. With `enforce=false` the rows are still dropped, because ed93 is queued and the legacy rule keeps a lease-less row only when its holder is `in-progress`.
- *Detector:* with the old live-row rule, `find_wait_cycles` returns exactly one cycle, a054 → ed93 (lock) → 63491 (blocker) → a054 (blocker). This proves the detector sees the incident's cycle. It is also the 5.5 auto-resolvable case, and the test asserts `auto_resolvable == true`.

**(ii) Server, `iter_data` `#[tokio::test] lock_for_a_queued_item_is_refused_and_released`,** next to the tests at api.rs:1974-2200, using `mem()` (api.rs:2005-2007).
1. Create item H with `lockdirs [{topdir}/a/src]`. PUT it `in-progress` with `lease "L1"`. Acquire `{topdir}/a/src` and `{topdir}/b/extra.rs` (the second is outside `lockdirs`, standing in for an agent's widened fence). Both succeed, and both rows carry `lease "L1"`.
2. PUT H to `queued` with `"lease": ""`, as the close gate's Waiting branch will. Assert both rows are gone. Today the extra row survives: red.
3. Acquire `{topdir}/b/extra.rs` for H with no lease and no engine, byte for byte what process 21222 sends. Assert `409` with `refused: "not running"`. Today this is `200`: red.
4. `locks/renew {workid: H, lease: "L1"}` returns `409`.
5. PUT H without a `lease` key while it holds lease "L2", and assert the lease is kept.

**(iii) End-to-end, a new section in `e2e.sh`,** following the fake-`claude` pattern at e2e.sh:556-700.
- Create P (the parent), A (a child of P, `lockdirs [{topdir}/intake/src, {topdir}/intake/proto]`, `run_now: true`), and E (`lockdirs [{topdir}/intake/src]`). Give P `blockedby [A]`.
- The fake worker for E does three things. It POSTs `locks/acquire` for `{topdir}/intake/proto/intake.proto` with only `path` and `workid`. It starts `nohup bash -c 'while :; do curl … locks/acquire {"path":"{topdir}/intake/proto/intake.proto","workid":"'$ITER_WORKID'"}; sleep 2; done' >/dev/null 2>&1 &`, recording its PID in the parent shell for the cleanup trap (not inside `$( )`). It runs `iter wait --on <P>` and exits 0. The fake verifier answers incomplete.
- Run the engine with `--max-ticks 30` (a 1-second tick).
- **Today:** E closes to queued waiting on P; A stays queued with `blockedby_locks [E]` for every remaining tick; the renewer's row is live at the end. Assert it and see it red against the expected outcome below.
- **After:** within 3 ticks of E's close, A goes `in-progress`. The renewer's POSTs return 409 (count them in a log). `GET /deadlocks` shows zero cycles at every tick. At the end, `GET /locks` holds no row for E.
- Kill the renewer in the trap.

### 8.2 Unit tests for each new function and branch

- `wait_edges`: one case each for Blocker, Deep (including the "it waits on us" exception and `blockedby_shallow`), and Lock (including overlap in both directions of `paths_overlap`). No edge ever leaves a non-queued item.
- `find_wait_cycles`: a self-loop, a two-cycle, the three-cycle from 8.1, the two-deep example from 5.4, two disjoint cycles in one graph, and a determinism check (shuffle the input order, get byte-identical output).
- `dependency_cycle_on_write`: refuses 5.4's last link when it is written after P and Q complete, and accepts it when it closes nothing.
- `lock_acquire`: the `reserve` branch refuses a non-queued holder, the ttl cap applies, and `engine` is filled from the holder.
- `close`: every branch writes `lease: ""`. Test this by extracting the "updated" builder at work.rs:1316-1362 into a pure `fn close_body(…) -> Value` and asserting on it for each branch.
- `wait_with_timeout`: a script that spawns `sleep 300 &` in its own process group and then sleeps past a 1-second timeout. Assert the background `sleep` is gone after the timeout. It is alive today: red.

### 8.3 Property tests on random graphs

Add `proptest = "1"` to `[dev-dependencies]` of `iter_core`. If Stephen prefers no new dependency (question 6), use a seeded xorshift generator in the test module and 2000 seeds per property, printing the seed on failure.

The generator draws 2 to 40 items with random states (weighted towards queued), a random `createdby` forest, random `blockedby` sets (including cycles), `lockdirs` drawn from a pool of 8 paths with nesting (`a`, `a/b`, `a/b/c.rs`, `d`, …), random leases (present or empty), and 0 to 30 lock rows on random pool paths with random holders and leases.

- **P1 (I5):** with `enforce=true`, no cycle returned by `find_wait_cycles(wait_edges(items, live_lock_rows(…)))` contains a `Lock` edge.
- **P2 (I4):** every `Lock` edge's `to` has a non-empty lease equal to the row's lease.
- **P3 (the detector is correct):** `find_wait_cycles` returns a non-empty list if and only if a Floyd–Warshall reachability matrix over the same edges has some `reach[i][i]`. Every returned cycle is a closed walk whose every consecutive pair is an edge in the input.
- **P4 (the write check is correct):** for a random item and a random new blocker set, `dependency_cycle_on_write` returns `Some` if and only if brute-force reachability over dependency-only edges, with the item forced to queued, finds a cycle through the item.
- **P5 (a state-machine simulation of the server rules):** a model of the lock table and item records applies random events: claim with a new lease, agent acquire, stray acquire (no lease, arbitrary item), renew with a random lease, close to any of complete, failed, queued-bounce, queued-waiting or question, a human state change, and the passage of time past the ttl. After every event, assert I1 (every row's lease equals its holder's live lease, or the row is expired, or it is a row the sweep will remove this tick). After every simulated tick (sweep plus detector), assert I3 and I5.
- **P6 (liveness):** in a P5 run with no dependency cycles in the generated `blockedby` and every run finishing within k events, every queued item is eventually claimed within a bound computed from the item count and k.

---

## 9. Rollout and rollback

Order matters, because a new server must accept old engines and an old server must not meet a new engine.

1. **Deploy `iter_data`** (`iter3/deploy_lambda.sh`) with `lock_lease_enforce` false for every project. Acquire keeps granting as today, but returns a `warnings` entry and logs a line for every grant it would refuse. `GET /deadlocks` and the enriched `GET /locks` work immediately. Nothing changes for the running engine.
2. **Clean pdy-dev by hand first** (question 1). Stop process 21222 on StephenMBP, check `GET /deadlocks` and `GET /locks`, and run `POST locks/sweep {"dry_run": true}` to see what it would remove. On today's data that is ed93's 42 rows and 7a8109b1e56b's one row.
3. **Drain and upgrade the engine.** Set the project to Draining and wait until nothing is in progress. That is the loop pdy-dev already uses (`devops/script/drain_rebuild_resume.sh`), and it avoids orphaning in-progress runs, which an engine restart does (pdy-dev memory: orphaned in-progress after an engine restart). Build and install the new `iter_engine`, then set the project back to Running. From now on, claims carry leases, renewals run, closes clear leases and call `release_all`, and the sweep runs in dry-run mode.
4. **Watch for one day.** The dry-run sweep log should list only rows from items that are not running. An engine lease-lost line means a PUT cleared a live lease, and must be investigated before step 5.
5. **Enforce:** set `lock_lease_enforce: true` on pdy-dev with the webui project settings; the engine reads it on its next project reload. Run `POST locks/sweep {"dry_run": false}` once.
6. Then make enforcement the default for new projects (question 2).

**Rollback.** Setting `lock_lease_enforce` to false restores today's grant rule at once, with no deploy. Reverting the engine binary is safe with the new server: old engines send no lease, and with enforcement off the server grants their rows as today. Reverting the server binary under a new engine is also safe, but loses renewals: the old server ignores `lease`, has no `renew` route, and gives `404` on `release_all` and `sweep`. So revert the engine first, then the server.

**Migration of existing held locks.** The `lease` field defaults to `""` on every existing item and row, so nothing needs rewriting. With enforcement off, legacy rows count as live only when their holder is `in-progress` (6.1 item 5). Rows of queued, parked, question, complete or failed items stop blocking dispatch as soon as the new engine runs, even before enforcement. The one-shot sweep in step 5 deletes them.

---

## 10. Adjacent defects found while reading (not required for this change)

- **Reserve rows and lock rows share one row per path** (2.1). A reservation by item R on path P therefore makes any other item's lock acquire on exactly P fail with a conflict until R's 600-second reservation lapses. R re-reserves every tick while the cap has a free slot (engine.rs:980-988). That includes a strictly-better-priority item the reservation gate means to let through (engine.rs:1009-1013): its claim fails at engine.rs:1200 and is deferred 5 seconds, again and again. Suggested fix: store reserve rows under the key `reserve:<path>`, keep the `path` field unchanged in the body, and read them in dispatch by `kind`. Nothing else in dispatch changes.
- **`claim_chain_candidate` does not check locks held by other items** before claiming (work.rs:196-205 compares only lockdirs). It relies on the acquire failing. That is correct, but it writes a claim and then a rollback PUT on every lost race.

---

## 11. Open questions for Stephen

1. **Process 21222 on StephenMBP is still re-acquiring item ed93's 42 locks every 20 minutes**, and it will keep a054 and 1943 blocked after ed93 runs again. Stopping it (`kill 21222`; its `sleep` child exits with it) and then releasing ed93's rows other than `core/repos/pdy_core_intake/src` is a change to shared state, so it is yours or Fable's to authorise; this investigation changed nothing. Should that be done now, before the code change?
2. After pdy-dev has run enforced for a day, should `lock_lease_enforce` become the default for every project, and the switch then be removed?
3. **Dependency-only cycles:** should the engine only report them (the proposal), or also cut one edge by a fixed rule? The rule could be: remove the most recently added `blockedby` entry among the cycle's members. That needs a new per-edge timestamp, for example `blockedby_added: {id: ts}`, because entries carry none today.
4. **Priority:** should a P0 or `run_now` item ever preempt a running lower-priority holder, by stopping it with the existing stop path and parking it? The proposal says no, and shows the wait with the holder's start time and session-limit time instead.
5. **A lost lease on a live run** (a renewal gets 409) means the agent is still writing without a fence. Should the engine only log it and add a `doc` row (the proposal), or stop the run?
6. May `iter_core` take `proptest` as a dev-dependency, or should the property tests use a hand-written seeded generator?
7. The lease lifetime of 600 seconds with a 60-second renewal is a proposal. If an engine dies, its runs' locks free after at most 10 minutes instead of 65 today, while a surviving orphaned agent process may still be writing. Is 10 minutes the right trade, or should it be longer?
