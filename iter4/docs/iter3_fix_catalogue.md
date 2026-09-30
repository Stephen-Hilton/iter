# iter3 fix catalogue: what pdy-dev asked the harness to fix, 2026-09-14 to 2026-09-28

Written 2026-09-28. This catalogue lists every fix request that the pdy-dev project made against the iter harness (`~/dev/iter/iter3`: iter_core, iter_data, iter_engine, iter_local and webui/index.html) during these two weeks. Each status was checked against the source at git HEAD `56ed0df`. Every file:line below points at that commit. The only harness commits after 2026-09-12 are four webui changes (d356965, 7388fa6, 3de4da8, 56ed0df), and none of them touches the defects listed here.

Sources read:
- `~/dev/iter/CHANGE_REQUEST_lock_deadlock_prevention_20260925.md` (the CR)
- `~/dev/iter/iter3/plans/close_gate_question_shape.bugfix.md`, `close_write_lost_in_outage.bugfix.md`, `deep_rule_sibling_deadlock.bugfix.md`, `scoped_end_of_run_commit.bugfix.md`
- `~/dev/pdy-dev/Agent_Recommendations.md`: 677 rows are dated 2026-09-14..28. 181 of them mention a harness term. After reading those 181, 24 turned out to target the harness itself. Another 13 target tooling rows, which are prompts stored as data, or are operational, so they appear in the borderline list at the end.
- `git -C ~/dev/pdy-dev log --since=2026-09-14`: this added one request that has no row in the table (F11, from commit badb3f42e).

Rows are cited as `AR <date> <item id>`. In the "Status" line, "open" means nothing in the code does it yet.

Order: open items come first, the most severe first (deadlocks, then lost or stuck work, then wasted runs, then smaller defects). The done item comes last.

---

## F1. Locks held by items that are not running (lease-bound locks)
- Source: CR sections 1–6 and 9 (the incident of 2026-09-25: four P0 items stalled for 5 h 43 min).
- Component: iter_data, iter_engine, iter_core
- Status in iter3: open. No `lease` field exists anywhere (grep finds nothing). `lock_acquire` (iter_data/src/api.rs:1883-1907) checks only that the caller has the writer role. The dispatcher's `lock_holders` closure (iter_engine/src/engine.rs:823-833) never looks at the holder's state. `close` releases only `item.lockdirs` (iter_engine/src/work.rs:1381-1388).
- Problem: Any process that holds the engine token can take or re-take a lock for any workid, whatever state that item is in. The engine releases only the paths in the item's `lockdirs`, so a lock an agent takes on any other path outlives the run. The dispatcher then treats a lock row held by a queued item as live. That turns leaked rows into a wait edge pointing at an item that is itself waiting, and on 2026-09-25 this closed a lock-plus-dependency cycle. An orphaned `renew-locks-*.sh` loop kept 42 such rows alive for 11 hours.
- Requested change:
  - iter_core/src/lib.rs: add `WorkItem.lease` and `LockRow.lease` (around lib.rs:527-610 and lib.rs:709-723). Add `LOCK_LEASE_TTL_SEC=600` and `LOCK_RENEW_EVERY_SEC=60`. Add `Project.lock_lease_enforce`. Add a pure `live_lock_rows()` and move `lock_holders()` out of engine.rs:823-833 into it.
  - iter_data/src/api.rs:1883-1907 (`lock_acquire`): refuse a `kind:"lock"` row with 409 `{"refused":"not running"}` when the holder item has no live lease. Stamp `row.lease` and fill `row.engine` from the holder. Refuse a `reserve` row unless the holder is queued.
  - Add `locks/release_all` and `locks/sweep` routes next to api.rs:171-174.
  - `workitem_put` (api.rs:1373-1422): keep the stored lease when the body omits the key, and delete the old lease's rows when the lease changes.
  - `locks_list` (api.rs:1859-1867): add `holder_state` and `holder_lease_live` to each row.
  - iter_engine/src/engine.rs:1147-1245 (`start_item`): generate a lease in the claim PUT and send it with each acquire. On a lost race, call `release_all` and clear the lease.
  - iter_engine/src/work.rs:173-235 (`claim_chain_candidate`): the same.
  - `close` (work.rs:1316-1379): write `lease:""` on every branch, and replace the loop at work.rs:1381-1388 with `release_all`.
  - `close_keep_state` (work.rs:1225-1235): clear the lease, then call `release_all`.
  - Dispatch (engine.rs:776-796): count only live rows.
  - iter_engine/src/prompt.rs:321 and :390: add one sentence telling agents never to renew locks or leave processes running.
  - Roll out behind `lock_lease_enforce` in dry-run first (CR section 9).
- Test that proves it:
  - (i) iter_core `pdy_dev_20260925_lock_dependency_cycle`: with the incident's real ids, `lock_holders(a054)` is empty once ed93 has no lease.
  - (ii) iter_data `lock_for_a_queued_item_is_refused_and_released`: an extra-path row is gone after a PUT with `lease:""`, and an acquire shaped like the stray loop's returns 409 (today it returns 200).
  - (iii) e2e.sh: a fake worker starts a `nohup` renewer and runs `iter wait`. Item A must start within 3 ticks, and the renewer must get 409s.
  - Property tests P1, P2 and P5 (CR 8.3).
- Size: L

## F2. The deep dependency rule deadlocks two sibling follow-ups, and dependency waits carry no reason
- Source: `iter3/plans/deep_rule_sibling_deadlock.bugfix.md` (2026-09-13: item 10920eb3ac88 sat 15.5 h with no tag).
- Component: iter_core, iter_engine
- Status in iter3: open. `dependency_status` (iter_core/src/lib.rs:837-877) skips a descendant only when `waits_on` (lib.rs:755-770) sees a declared link. `waits_on_deep` and `SiblingFirst` do not exist. engine.rs:880 is still `DepStatus::Waiting(_) | DepStatus::Failed(_) => {}`, so no tag is written.
- Problem: Take two items, each an open follow-up of the other's completed blocker. Under the deep rule each waits on the other through an implied `createdby` wait, and `waits_on` follows only declared `blockedby` links. So neither side is skipped and neither can ever start. Nothing on either record says why, because an ordinary dependency wait writes no `blocked by:` tag.
- Requested change:
  - Add `waits_on_deep(from, target, by_id, children)` in lib.rs.
  - In `dependency_status` (lib.rs:863-865), treat a mutual implied wait with a fixed tie-break: lower priority number first, then earlier `ts.receive`, then the smaller id.
  - Add a `DepStatus::SiblingFirst(id)` variant, and extend `Waiting` to carry `via: Option<blocker>`.
  - At engine.rs:880, write the tags `blocked by: waiting on <id12>`, `… (follow-up of blocker <b12>)`, `blocked by: blocker <id12> failed` and `blocked by: sibling <id12> goes first (mutual follow-up wait)`.
  - A declared link still wins over the tie-break.
- Test that proves it: plan tests T1–T8. The key ones are `mutual_implied_wait_is_broken_by_priority_then_age` (X is Satisfied, Y is SiblingFirst(X)), `tie_break_is_symmetric_and_stable`, `dependency_waits_carry_a_reason_tag`, and T8, which replays the four 2026-09-13 records.
- Size: M

## F3. A close write lost in a network outage leaves a permanent "ghost" in-progress item
- Source: `iter3/plans/close_write_lost_in_outage.bugfix.md` (2026-09-22: two ghosts took two cap slots for 7 h).
- Component: iter_engine, webui
- Status in iter3: open. The close loop retries only on a 409 conflict (work.rs:1373) and otherwise logs `close failed` and breaks (work.rs:1375). There is no `pending_close` journal. `prune_running` (engine.rs:411-414) forgets finished threads. No tick sweep repairs this engine's own in-progress records. The heartbeat (engine.rs:~494) carries no `running` count.
- Problem: When the final versioned PUT fails with a transport error (status 0), the engine logs "done -> failed/retry" but the store never changes. The record stays `in-progress` and names the engine, with no process behind it. It counts against the store's in-progress total, which leaves the usage cap wrong, and nothing ever notices it. The detail-row appends and lock releases in the same function swallow their errors the same way.
- Requested change:
  - R1, in work.rs:1316-1379: retry a status-0 or 5xx error with backoff (2 s doubling to 60 s, for up to 10 minutes). Apply the same retry to `put_detail` and the releases. When the budget runs out, journal the intended record to `<topdir>/.iter/pending_close/<id>.json`.
  - R2, at the top of `tick` (engine.rs:416): replay the journal when the version still matches.
  - R3, also in `tick`: repair ghosts. That means any item that is `in-progress`, has `engine == self.name`, is not in `running`, `explaining` or `triaging`, has no journal file, and started longer ago than its `agent_timeout`. Treat it as a failed attempt: add a `doc` row, retry or fail it, and release its locks.
  - R4: add `running: n` to the heartbeat. The webui in-progress chip warns when that sum differs from the store's in-progress count.
- Test that proves it:
  - An Api double fails `put` with status 0 k times; `close` lands the write, or journals it when k exceeds the budget.
  - A tick over a stale in-progress item that names this engine PUTs `queued` and writes a `doc` row. The two controls get nothing: an item started one second ago, and an item owned by another engine.
  - A journal file with a matching version is replayed; one with a stale version is dropped.
  - Break-it test: kill the local iter_data around a run's end.
- Size: M

## F4. The engine's own locks expire mid-run and nothing renews them
- Source: CR sections 2.2, 3.4 and option (e).
- Component: iter_engine, iter_data
- Status in iter3: open. The TTL is `3600 + 300` (engine.rs:1191) or `3900` (work.rs:221). The `locks/extend` route exists (api.rs:174, handler api.rs:1934) but nothing calls it. The pdy-dev session limit is 7200 s.
- Problem: A run holds its locks for 65 minutes. After that its rows lapse, dispatch stops counting them, and an overlapping item can start while the first agent is still writing. That gap is also why agents wrote their own renewal loops, which caused F1.
- Requested change:
  - Replace the running tuple (engine.rs:26) with a `Running` struct that carries `cur: Arc<Mutex<(workid, lease)>>`, shared with the worker thread so session chaining updates it.
  - Add `renew_leases()`, called from `tick` (engine.rs:416) before any hold or early return. Every 60 s it POSTs `locks/renew {workid, lease, ttl_sec:600}`, and logs and adds a `doc` row once on a 409.
  - Add a new `lock_renew` handler in api.rs that refuses a lease that does not match. Keep `locks/extend` as an alias of it.
  - Lock TTL becomes 600 s. Depends on F1's lease field.
- Test that proves it: an engine unit test with a fake clock shows a run longer than 600 s still holds its rows. An iter_data test shows `locks/renew` with a stale lease returns 409 and leaves `expires` unchanged.
- Size: M

## F5. No cycle detector over the full wait-for graph; lock edges are invisible to every check and to the webui
- Source: CR options (b) and (c), sections 2.5, 2.6, 5.4-5.5, 6.1.7-8, 6.2.8, 6.2.10, 6.7 and 6.8.
- Component: iter_core, iter_data, iter_engine, webui
- Status in iter3: open.
  - `blockedby_cycle` (lib.rs:777-814) and `refuse_dependency_cycle` (api.rs:1110-1131) follow explicit `blockedby` links only.
  - The engine's `DepStatus::Cycle` check (engine.rs:877-879) sees declared loops only.
  - The webui `lockHolderOf` (webui/index.html:726-730) nests a waiter only under an `in-progress` holder.
  - There is no `/deadlocks` endpoint.
- Problem: Nothing refuses, detects or displays a cycle that runs through a lock edge or through a deep (`createdby`) edge. A loop can be born from state changes alone, when a blocker completes while its descendants are open (CR 5.4), and no write-time check can see that. The operator saw P0 items idle as roots with no parent.
- Requested change:
  - In iter_core: add `WaitKind`/`WaitEdge`, `wait_edges()` (edges leave queued items only), `find_wait_cycles()` (iterative Tarjan, with a deterministic canonical cycle per component) and `dependency_cycle_on_write()` (explicit plus deep edges).
  - `refuse_dependency_cycle` calls the new function and names deep edges in its refusal text.
  - Add `GET /api/projects/{name}/deadlocks`.
  - In the engine, after classification (after engine.rs:890): tag every member `blocked by: deadlock: a → b → c (kinds)`, add a `doc` row once per cycle, and auto-release the locks of a holder that is not running when it sits in a cycle.
  - When a `run_now` item waits on a running holder, its reason names the holder and when that holder's session limit ends it.
  - `iter status` prints the cycles (cli.rs:~1270-1295).
  - Webui: `lockHolderOf` returns any holder and marks one that is not running. Add a red deadlock banner and an amber row for stale locks.
- Test that proves it: `find_wait_cycles` finds the incident's a054 → ed93 (lock) → 63491 → a054 cycle under the old live-row rule. There are unit cases for a self-loop, a two-cycle, the CR 5.4 two-deep example, disjoint cycles and a shuffle-determinism check. Property tests P3/P4 compare against brute-force reachability.
- Size: L

## F6. A session timeout kills only the direct child, leaving background processes alive
- Source: CR sections 2.3 and 6.5.5.
- Component: iter_engine
- Status in iter3: open. `wait_with_timeout`: the stop path signals the whole process group (work.rs:1048: `kill -TERM -- -<pid>`), but the timeout path calls only `child.kill()` (work.rs:1065).
- Problem: When a session hits its time limit, its background builders and loops keep running after the engine has moved on. They keep writing to the shared checkout and can keep hitting the API with the item's credentials.
- Requested change: in work.rs:1063-1066, signal the process group exactly as the stop branch at work.rs:1045-1049 does, then call `child.kill()`. A later option is to strip `ITER_ENGINE_TOKEN` from the agent environment (work.rs:620-633) so detached processes cannot take locks. That part is a design question for Stephen.
- Test that proves it: a script starts `sleep 300 &` in its own group and then outlives a 1 s timeout. The background `sleep` must be gone afterwards; today it survives.
- Size: S

## F7. The verifier cannot see an item's commits from earlier attempts, or with the id in the message body
- Source: AR 2026-09-16 93e706df2466 (third row), AR 2026-09-16 8de31b1b1750, and AR 2026-09-17 24feb8d51181 (agents confused about the first-8 or last-8 id suffix).
- Component: iter_engine
- Status in iter3: open.
  - `git_run_evidence` (work.rs:438-449) reads `git log head_before..head_after`, which covers the current attempt only.
  - `commits_with_id` (gate.rs:65-72) matches only a subject that ends in `(<id8>)`, the first 8 characters of the id.
  - The evidence sentence prints "Commits carrying this item's id: none" without qualification (gate.rs:94-105).
- Problem: A multi-attempt item whose real work landed in attempt 1 shows the verifier only attempt 2's bookkeeping commits. A commit that carries the full id in its body, or the webui's last-12 short form, is not counted. The verifier reads "none" as proof that nothing was done and bounces finished work. This was measured twice, and it cost a full re-run each time.
- Requested change:
  - In work.rs:447, build the commit list from `git log --grep=<full id>` (which searches the whole message) and also accept a `(<id8>)` subject suffix. Scope the log to the item's lifetime (since `ts.receive`), not to this attempt.
  - In gate.rs:94-105, label the list with its real scope ("commits carrying this item's id, all attempts") and never print a bare "none" when commits carrying the id exist.
  - Optionally accept the last-12 form too, so the id convention matches what the webui shows.
- Test that proves it: in gate.rs, a git fixture holds attempt-1 and attempt-2 commits, one carrying the id only in its body. The evidence lists both, and the sentence does not say "none". The test fails today on the body-only commit.
- Size: M

## F8. A committed path outside the lock scope is reported to the verifier as "left uncommitted"
- Source: AR 2026-09-14 48be824326cd (SECURITY_DELTA.md ledger rows, affecting 17 children), AR 2026-09-26 69be61bdae7a (router techreq R219), AR 2026-09-18 9a2a848386bc (a guard script under the `tests/` directory), and AR 2026-09-16 93e706df2466 (a techreq one level above the lock scope).
- Component: iter_engine
- Status in iter3: open.
  - The diffstat is limited to `commit_scope` = lockdirs + `commit_extra_paths` (work.rs:355-384, work.rs:1122-1126).
  - `scope_sentence` (gate.rs:100-103) tells the verifier that "path(s) outside that scope were left uncommitted and are not this item's".
  - The verifier prompt (gate.rs:137-138) calls a file outside the scope "NOT evidence".
  - Commit b00f3e0 introduced the scoping and fixed the opposite problem (siblings' files swept in). It left this gap.
- Problem: Plans routinely tell an item to append to a file outside its codepath under a one-file lock: a ledger row, the container's techreq, or a guard script in the testwriter's directory. The item commits that file correctly, with its own id, and the verifier is told the file is not its work. It then bounces the item for "not writing" it. Every bounce is a wasted attempt that can change nothing.
- Requested change: in `git_run_evidence` (work.rs:438-449), add a second list: files changed by this item's own commits (from F7's id match) that fall outside the scope. Render it in gate.rs `describe()` as "committed by this item outside its lock scope: <path> (<hash>)". Reword `scope_sentence` so that "uncommitted" refers only to dirty, uncommitted paths. Adjust the prompt line at gate.rs:137 so a path committed under this item's id counts as evidence.
- Test that proves it: in a work.rs git fixture, the item's lockdirs are `a/`, and the item commits `a/x.rs` and `LEDGER.md` with `(id8)`. The evidence names `LEDGER.md` as committed by this item, and the words "left uncommitted" do not appear for it. The test fails today.
- Size: M

## F9. The verifier's diffstat still includes sibling items' commits inside the same lock scope
- Source: AR 2026-09-23 5f4e19bef7c3 (item b7c353758fdd bounced over sibling commit ba2d6a2ae).
- Component: iter_engine
- Status in iter3: open. The diffstat is `git diff --stat head_before head_after -- <scope>` (work.rs:444). That spans every commit in the range, including other items' commits whose paths overlap the scope, for example a lock scope that nests inside another item's lock scope.
- Problem: When a sibling commits inside this item's folders during the run, the diffstat shows the sibling's file. A worker that truthfully says it kept that file out of its commit then looks as if it is lying.
- Requested change: in work.rs:444, build the diffstat from this item's own commits only. That means `git show --stat` per hash from F7's list, or `git diff --stat` over their union. List the other in-scope commits separately, labelled "commits by other items in this scope (not this item's work)".
- Test that proves it: in a fixture, two commits touch `a/`, one ending `(id8)` and one ending `(other8)`. The diffstat names only the first file, and the second commit appears under the "other items" label.
- Size: S

## F10. The close gate judges the `--fixed` claim as it stood when the run started
- Source: AR 2026-09-22 d8e7659fd3a4 (item 169419d1: the claim at 10:40 read 154/154 upheld, but the gate at 10:43 used the 09:02 failure). Also pdy-dev commits 7fc70541e and 0ab8f2c11, which added agent-memory workarounds for it.
- Component: iter_engine
- Status in iter3: open. `details` is fetched once before the run (work.rs:124) and moved into `GateCtx.details` (work.rs:154-161). `gate::last_fixed_claim(&ctx.details)` (work.rs:1162) therefore never sees a claim row the run itself wrote.
- Problem: An agent that fixes the defect and makes a passing `iter runtests --fixed` claim during its own run is still bounced, because the gate compares against the claim from before the run. The bounce text quotes a count that is no longer true. This costs a whole extra attempt on every item of this kind.
- Requested change: in `run_gate` (work.rs:1117), re-fetch details (`fetch_details`) before the deterministic checks and use the fresh list for `last_fixed_claim` and `open_reviews` (work.rs:1140, 1162). Put the claim's `ts` in the hold reason.
- Test that proves it: in work.rs, an Api double returns details without the claim at dispatch and with a green upheld claim at gate time. The gate passes, where today it holds.
- Size: S

## F11. A human's "accept" on a close-gate widget waits for a free usage-cap slot
- Source: pdy-dev commit badb3f42e (2026-09-20, `temp/unstick/unstick_20260920T2228Z.md`). The acceptance written at 22:33Z sat 40+ minutes behind `blocked by: usage cap (3/2)`.
- Component: iter_engine
- Status in iter3: open. The human-accept shortcut runs inside `execute_one` (work.rs:126-133), which is reached only through dispatch. Dispatch returns when `running_now >= cap` (engine.rs:969-975), and it does the same during an account hold.
- Problem: Closing an item a human has already accepted uses no model time. But it is queued behind the usage cap like a real run, so accepted work stays open for hours while every account is at its stop percentage. Priority-0 demo items were held this way.
- Requested change: in `dispatch`, before the cap check at engine.rs:969, close every queued item where `gate::accepted_by_human(details)` holds (on its own short thread, or inline) without consuming a slot. The same should apply while the engine is holding for usage. Cache the details lookup so it is fetched only for items whose latest detail row is an answered close-gate widget.
- Test that proves it: an engine test with the cap full (2/2) and one queued item carrying an answered accept widget. After one tick the item is `complete` and carries the "closed complete by a human" row.
- Size: S

## F12. The close-gate question shows the human no question
- Source: `iter3/plans/close_gate_question_shape.bugfix.md` (2026-09-12, item ea3352ba; Stephen: "worse than not helpful").
- Component: iter_engine, webui
- Status in iter3: open.
  - `question_widget` (gate.rs:226-247) still sets `title = "Close gate held '<clip(name,120)>' …"`, `summary = clip(reason,400)` and a detail of open obligations plus a 6,000-character report.
  - The verifier's JSON shape (gate.rs:139-140) has no `question` or `recommendation`.
  - A verifier parse failure becomes `Unclear` with no retry (gate.rs:151-163); work.rs:1193-1207 retries only when the session itself fails.
- Problem: The widget's title is the item's name. The verifier's actual doubt is clipped out of the summary, and there is no recommendation. The human has to dig through the worker's whole report to find what is being asked. A malformed verifier output goes straight to the human queue.
- Requested change:
  - R1: the verifier returns `question`, `recommendation` (accept or continue) and `why`, parsed with defaults (gate.rs:139-186).
  - R2: the widget title is the question. The summary reads `Recommendation: … — why · bounce n of m · item: …`. The detail holds the question, reason, open list and evidence unclipped, then the report clipped to 1,500 characters. The radio button defaults to the recommendation.
  - R3: the webui expands `detail` for close-gate widgets.
  - R4: retry once on a parse failure before asking the human.
  - R5: store `answered_question` when the human answers.
- Test that proves it: plan tests T1–T7 (`widget_title_is_the_question_not_the_item_name`, `widget_never_clips_question_reason_or_open`, `radio_default_is_the_recommendation`, `verifier_parse_failure_retries_once_before_asking`, and an e2e check of the widget's title and summary).
- Size: M

## F13. `iter runtests` writes back the whole registry file and erases concurrent groups' results
- Source: AR 2026-09-14 3f81255f98fd, AR 2026-09-18 1a1a52d748bd and AR 2026-09-20 697abf8f122c (all merged; measured three times, including a group left with no result at all).
- Component: iter_local
- Status in iter3: open. `run_group` reads the file at the start (iter_local/src/runtests.rs:108) and writes back `testgroups::update(&content, &groups)` built from that snapshot (runtests.rs:210-219).
- Problem: Two groups that share one registry file and finish close together overwrite each other's `lastrun`, `result` and `counts`, so the later writer erases the earlier result. A group with no recorded result looks like one that never ran, and readers and schedulers act on the stale stamp.
- Requested change: in runtests.rs:210-219, re-read the file immediately before writing and splice only this group's JSONL line, under an advisory file lock (for example `flock` on `<file>.lock`). Then write to a temp file and rename it.
- Test that proves it: two threads call `run_group` on two groups in one registry, with their writes forced to interleave (a barrier after the read). Both groups keep their fresh stamps. Today one is lost.
- Size: S

## F14. `iter runtests` grades a stale copy of a group inside `.claude/worktrees/`
- Source: AR 2026-09-26 edeb2846019f (item a86ce75b: the `--fixed` claim recorded 40/42 from a worktree copy, while the real group passed 42/42).
- Component: iter_local
- Status in iter3: open.
  - `collect_files` skips only `.git`, `target`, `node_modules` and `.iter` (iter_local/src/testgroups.rs:135). validate.rs:824 and placeholders.rs:174 have the same list.
  - `locate_group` warns about duplicates and takes the first match in sort order (runtests.rs:80-91), and `.claude/…` sorts first.
- Problem: While any Claude Code worktree exists, every shared label runs the worktree's older copy and writes its stamp into the worktree's registry. The completion gate records a false red.
- Requested change: add `.claude` to the skip list in testgroups.rs:135, validate.rs:824 and placeholders.rs:174 (ideally one shared const). When `locate_group` finds duplicates (runtests.rs:83-90), prefer the copy that is not inside a hidden directory, or refuse when labels are ambiguous.
- Test that proves it: in a fixture, the same label sits in `.claude/worktrees/x/a.testgroup.iter.md` and `core/a.testgroup.iter.md`. `locate_group` returns the `core/` copy.
- Size: S

## F15. `iter add` stores items with no real request, and silently ignores keys it does not read
- Source: AR 2026-09-16 e614949ec46f (`--file` produced an item with no request or lockdirs), and AR 2026-09-21 6debe77a33f0 (an item stored with the request `PLACEHOLDER - replaced immediately by the filing agent`).
- Component: iter_engine (cli.rs), iter_data
- Status in iter3: partial. `add` does read `title/name`, `type/agent`, `mainwork/request` and `codepath/codepaths/lockdirs` from `--file` (iter_engine/src/cli.rs:850-866). But it silently ignores any other key, any non-string value and any misspelling. Neither the CLI nor `workitem_create` in api.rs refuses an empty or placeholder request.
- Problem: An agent filing with a slightly different key or value shape gets `added <id> state=queued`, which reads as success, while its item has no instructions and no lock scope. An item whose request was never written gets dispatched and wastes a session, and it also gives the dedup judge nothing to compare.
- Requested change:
  - In cli.rs:845-866, refuse a `--file` that contains any key outside the documented set, or a non-string value for a string field, and name the key.
  - Refuse an empty request for agent types other than `exec`.
  - In iter_data's create handler (api.rs, near :1190), refuse a request that is empty or starts with `PLACEHOLDER`, so every client is covered.
- Test that proves it: a CLI unit test shows `--file` with `{"title":"t","description":"x"}` dies naming `description`. An iter_data test shows a create with `request:"PLACEHOLDER - …"` returns 400.
- Size: S

## F16. Dedup missed two items for one defect that had near-identical titles
- Source: AR 2026-09-21 6debe77a33f0 (second half: 6debe77a33f0 and 333cb0dc were both stamped `dedup_checked`, and both titles ended "the group pdyadmin-keys-dev is red").
- Component: iter_engine (dedup.rs), iter_core (dedup.rs)
- Status in iter3: partial. The stage-2 judge prompt does include names (iter_engine/src/dedup.rs:126-129). But stage 2 only runs when candidates share a `check:`/`container:` key or scope (dedup.rs:162), so two items without a shared key never reach the judge.
- Problem: Two items filed for the same failing group, where one has no request text, are never compared. A whole agent session re-proves work already committed by the other item.
- Requested change: in iter_core/src/dedup.rs, add a candidate rule for open items whose normalised titles share a testgroup label or a long common suffix, even without a shared key. Treat an empty or placeholder request as "title-only", and send those pairs to the judge.
- Test that proves it: a core unit test shows two open items with different containers and no `check:` tag, but titles ending "the group pdyadmin-keys-dev is red", are returned as candidates.
- Size: S

## F17. Reserve rows and lock rows share one row per path
- Source: CR section 10 (adjacent defect 1).
- Component: iter_data, iter_engine
- Status in iter3: open. Both kinds go through `acquire_lock("lock", project, path, …)` (api.rs:1904; sqlite.rs:165, ddb.rs:258). Dispatch re-reserves every tick (engine.rs:980-988).
- Problem: A reservation on path P makes any other item's lock on exactly P fail until the 600 s reservation lapses. That includes an item of strictly better priority that the reservation gate means to let through (engine.rs:1009-1013). Its claim fails at engine.rs:1200 and is deferred 5 s, again and again.
- Requested change: store reserve rows under the key `reserve:<path>`, keeping `path` in the body. Dispatch reads them by `kind` (engine.rs:776-796, 1009-1017).
- Test that proves it: in iter_data, a reserve on `{topdir}/a` by R followed by a lock on `{topdir}/a` by B succeeds. In the engine, a strictly-better item starts on the same tick as the reservation.
- Size: S

## F18. On a night with no rebuild, the cluster-restart block holds items until 06:00 by the clock alone
- Source: AR 2026-09-16 caf9f85a148a (help text says the cluster rebuilds nightly; since 2026-09-13 it rebuilds only on demand), and AR 2026-09-28 d848544d7a41 (8 P0 demo items waiting on one parked gateway item on a no-rebuild night).
- Component: iter_core (cluster.rs); also a tooling row
- Status in iter3: open.
  - `evaluate` (iter_core/src/cluster.rs:149-184) falls back to the clock whenever the restart clone posted no health row, so inside 02:00–06:00 PT it always reports unhealthy (cluster.rs:179-180).
  - The engine's requeue once the cluster is healthy exists (engine.rs:1119-1140), so items do come back at 06:00.
  - The `_block_cluster_restart` capability text is a tooling row, not code.
- Problem: pdy-dev's drain_rebuild_resume script now skips the rebuild when no item needs one. An agent that blocks at 02:30 on such a night is parked until 06:00 waiting for a rebuild that is not coming, which on a P0 item costs about three hours.
- Requested change: in `evaluate`, accept an explicit "no rebuild tonight" health row (for example `{"skipped": true}`) from a recent completed clone as healthy. The restart template writes it when it skips. `iter block --cluster-restart` (cli.rs:97-104) should refuse, or warn, when that row is present. Update the capability text so it no longer promises a nightly rebuild.
- Test that proves it: a cluster.rs unit test shows a clone completed at 01:05 PT with a `skipped` row reads as healthy at 02:30 PT. Today it reads the clock and reports unhealthy.
- Size: S

## F19. The note written when a green run replaces a red one hides where the red output went
- Source: AR 2026-09-20 cdfcac7ba199.
- Component: iter_engine (cli.rs)
- Status in iter3: open. `report_run` writes "(green on the latest run of testgroup … ; the earlier failing output was replaced)" with no path (iter_engine/src/cli.rs:443-446). The runner keeps every run's log in `<test_dir>/runs/<ts>-<id>.log` (runtests.rs:96-99).
- Problem: A reader is told the failing evidence is gone when a full copy sits on disk. An item was filed on the false premise that no evidence survived.
- Requested change: carry the replaced run's log path(s) (from the red run's `log_detail` row, or the latest failing file in `runs/`) into the note at cli.rs:446: "…replaced; its full output is at <path>".
- Test that proves it: a cli.rs unit test on the note builder shows that, given a red-then-green pair, the note contains `runs/` and the red log's filename.
- Size: S

## F20. A testgroup entry cannot be run and counted without deciding the group's colour
- Source: AR 2026-09-14 4150d638ee6a.
- Component: iter_local
- Status in iter3: open. `TestEntry` (iter_local/src/testgroups.rs:13) has no `gates` field, and every run counts toward the verdict (runtests.rs:200-206).
- Problem: A repository-wide inventory check can only gate the group or not exist. pdy-dev keeps a `--defer-to-owners` launcher flag and counting rules to work around this.
- Requested change: add `#[serde(default = "true")] gates: bool` to `TestEntry`. In runtests.rs:200-208, record the result of a `gates:false` run in the log and counts display but exclude it from `outcome`. Show it as "(non-gating)" in `log_header`. The validator (validate.rs) accepts the key.
- Test that proves it: in a runtests.rs fixture group, a red entry marked `gates:false` next to a green one gives an overall Green, and the red entry still appears in the log header.
- Size: M

---

## F21. Scoped end-of-run commit (sibling agents' files swept into this item's commit)
- Source: `iter3/plans/scoped_end_of_run_commit.bugfix.md` (2026-09-12; items aca5162724a8 and a180b52e47b8).
- Component: iter_engine
- Status in iter3: done (commit b00f3e0: `commit_scope`/`commit_scoped` at work.rs:355-432, evidence scoped at work.rs:1117-1126, and `commit_scope`/`outside_scope_dirty` in the gate evidence at gate.rs:52-58).
- Problem: The engine's end-of-run `git add -A` committed other agents' unfinished files under this item's id, and the verifier bounced finished work over them.
- Requested change: shipped. The commit and evidence are limited to lockdirs plus `project.commit_extra_paths`, and the leftover count is logged. Its side effects are F8 and F9 above.
- Test that proves it: `commit_scope_is_relative_and_drops_outside_entries` (work.rs:1528) and `evidence_describes_the_shared_checkout_and_scope` (gate.rs:419).
- Size: M

---

## Borderline: requests aimed at tooling rows or operations, not at harness code

These rows target the agent prompt and tooling rows served by iter_data (`_shared`, the agent definitions and capability documents). Stephen edits those rows through the webui, not in iter3 source, so none of them needs a code change. They are listed so whoever maintains the tooling rows can act on them.

| AR row | Target | Request | Harness angle |
|---|---|---|---|
| 2026-09-18 8971aa609d4d, 2026-09-20 24ea055d-a29b, 2026-09-18 08dae0ac732f | `_shared` row | Drop "## Invariants" from the interface-format list; the section was retired 2026-09-08 and `iter validate` rejects it | Stale prompt text contradicts the validator. Three separate reports. |
| 2026-09-18 7311a69bd0c0 | `usecase` agent definition | Replace `iter testloop --include` (not a V3 verb) with `iter teststate --include` | The prompt names a CLI verb that does not exist. The CLI could also alias `testloop` to `teststate`. |
| 2026-09-20 d2023cc888fc | `usecase` agent definition, test sweep | The definition says the sweep turns empty testlists into testwriter items; no V3 code does that (grep finds none) | Either build it or fix the prompt; the prompt fix is probably right, since testsweep was retired. |
| 2026-09-18 e0b80b0ce642 | `_shared` write fence | Say whether the run stamp `iter runtests` writes into a registry outside the fence is committed or left alone | This goes away if F13 moves stamps out of committed files. |
| 2026-09-16 93e706df2466 (row 2) | `_shared` write fence | Allow a sub-codepath agent to write its container's techreq under a one-file lock | Interacts with F1 (agents taking their own locks) and F8. |
| 2026-09-14 aab7ef3ae104 | `_shared` commit rules | Never `git commit --amend`, `rebase` or `reset --hard` on the shared checkout | The engine could enforce this through a pre-commit hook or by checking HEAD's author id. |
| 2026-09-20 1e0e6c5c7e71 | `_shared` "defect items carry their failing testgroup" | A `--fixed` claim is owed only when every non-green entry is inside the fence | Could become a `--fixed --only <script>` CLI mode. |
| 2026-09-20 45c02a74e4af | `_shared` "decide before you ask" | Agreeing past rulings count as the 70% delegation | Prompt only. |
| 2026-09-18 fcf70895d488 | `code`/`deploy`/`plan` definitions | Update the rebuild-detection sentence to match CLAUDE.md section 8 | Prompt only. |
| 2026-09-18 9c0d9c9e15dd | `_shared` agent-memory rule | Raise the 2 KB agentmemory limit to 4 KB, or add a check for it | `iter validate` could warn on oversized `*.agentmemory.iter.md` files. |
| 2026-09-17 24feb8d51181 | pdy-dev agentmemory files | Correct "last-8-hex" to the first-8 id suffix the gate matches | Merged into F7 as evidence: the gate's id8 (first 8 characters) and the webui's last-12 disagree. |
| 2026-09-28 d848544d7a41 | operations | Check that parked item 83d4e9f4001c was requeued after the window | Engine requeue exists (engine.rs:1119); merged into F18. |
| 2026-09-18 0f694fe38584, 2026-09-21 017abf0d5c83 | specific pdy-dev items | Close one duplicate, and add a lockdir to another | Data fixes, not harness. F16 and F15 cover the classes. |

---

## Summary

| Id | Title | Component | Status | Size |
|---|---|---|---|---|
| F1 | Locks held by non-running items (lease-bound locks) | iter_data, iter_engine, iter_core | open | L |
| F2 | Deep-rule sibling deadlock; silent dependency waits | iter_core, iter_engine | open | M |
| F3 | Lost close write leaves ghost in-progress item | iter_engine, webui | open | M |
| F4 | Engine locks expire mid-run; no renewal | iter_engine, iter_data | open | M |
| F5 | Wait-for-graph cycle detector, deep-edge write refusal, visibility | iter_core, iter_data, iter_engine, webui | open | L |
| F6 | Timeout kills only the direct child | iter_engine | open | S |
| F7 | Verifier blind to earlier-attempt and body-id commits | iter_engine | open | M |
| F8 | Committed paths outside lock scope reported as uncommitted | iter_engine | open | M |
| F9 | Verifier diffstat includes sibling commits in scope | iter_engine | open | S |
| F10 | Close gate reads the pre-run `--fixed` claim | iter_engine | open | S |
| F11 | Human close-gate accept blocked by usage cap | iter_engine | open | S |
| F12 | Close-gate question shows no question | iter_engine, webui | open | M |
| F13 | runtests registry whole-file rewrite race | iter_local | open | S |
| F14 | runtests grades `.claude/worktrees` copy | iter_local | open | S |
| F15 | `iter add` accepts empty/placeholder request, ignores unknown keys | iter_engine, iter_data | partial | S |
| F16 | Dedup misses same-title items without a shared key | iter_engine, iter_core | partial | S |
| F17 | Reserve and lock rows share one row per path | iter_data, iter_engine | open | S |
| F18 | Cluster-restart clock fallback on no-rebuild nights | iter_core | open | S |
| F19 | Green-replaces-red note omits the log path | iter_engine | open | S |
| F20 | `gates: false` testgroup entries | iter_local | open | M |
| F21 | Scoped end-of-run commit | iter_engine | done (b00f3e0) | M |

Totals: 21 fixes. 18 are open, 2 partial and 1 done, and there are 13 borderline tooling or operations rows. F1, F4 and F5 share the lease design in the CR and should ship together, in the CR's section 9 rollout order. F7, F8 and F9 all change `git_run_evidence` and `Evidence` and fit one change.
