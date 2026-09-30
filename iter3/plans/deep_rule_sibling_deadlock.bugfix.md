# Bugfix spec: the deep dependency rule can deadlock two siblings, and says nothing when it does

Written 2026-09-13 by Stephen's session in pdy-dev, from a measured incident. Send as-is to the
iter agent. File:line references are from a read of `~/dev/iter/iter3` on 2026-09-13; verify each.

Terms: an item's **blockers** are the ids in its `blockedby` list. The **deep rule** (the default,
`blockedby_shallow == false`) says an item also waits on every open item its blockers *created*,
transitively — "the blocker and everything it spawned must be finished". A **follow-up** is an
item created by another item (`createdby`). A **declared link** is an entry in `blockedby`; an
**implied wait** is one the deep rule derives from `createdby`.

## 1. What happened

Item `029f833e-9da8-4ebb-a32e-10920eb3ac88` (P0, demo: three stale assertions in the policy
container's inventory check) sat in `queued` with no wait tag from 2026-09-13 01:30Z until a human
looked at 17:00Z. Its only declared blocker, `ce5f182b8345`, was complete. That blocker had
created one follow-up, `22acf4aebb87` (P0, demo: the ninety-four onboarding-policy checks' redness
proof), still queued, so the deep rule held the item behind it. The follow-up's only declared
blocker, `7aebc409c4ac`, was also complete — and *that* blocker had created `10920eb3ac88`. So
each item was an open descendant of the other's completed blocker, each waited on the other by
the deep rule, and neither could ever start. Both are demo work with a deadline. Nothing on
either record said why it was waiting.

## 2. Why it happens

- `iter_core/src/lib.rs:837-880` `dependency_status`: for a complete blocker it walks
  `children_index` (`lib.rs:816-824`) and returns `Waiting(c)` for the first open descendant `c`,
  unless `waits_on(&c.id, &item.id, by_id)` is true, in which case it skips `c` ("it waits on us:
  we do not wait on it", `lib.rs:864-866`).
- `lib.rs:755-769` `waits_on` follows **declared links only** (`blockedby`), never the deep
  rule's implied waits. So when `c` waits on the item only *implicitly* — because the item is an
  open descendant of `c`'s complete blocker — `waits_on` says false, and both sides return
  `Waiting` on each other. The 2026-09-11 rule at `lib.rs:829-835` (a descendant that waits on the
  item is skipped) covers the declared case and misses the implied one.
- `iter_engine/src/engine.rs:880`: `DepStatus::Waiting(_) | DepStatus::Failed(_) => {}` — the
  engine writes a `blocked by:` reason tag for caps, locks, cluster restart and declared cycles
  (`engine.rs:877-879`), but nothing for an ordinary dependency wait. A deadlocked pair therefore
  shows the same empty face as a healthy wait.

## 3. Goal

Two items can never hold each other through implied waits; when the rule has to choose an order
it chooses deterministically and says so on both records; and every dependency wait carries a
reason tag naming what it waits on, so a human never has to reconstruct the walk by hand.

## 4. Design

### R1 — the deep walk recognises an implied wait in both directions

Add to `iter_core/src/lib.rs` a pure function

```rust
/// true when `from` would wait on `target` by the FULL rule: declared links, or
/// `target` is an open descendant of one of `from`'s complete blockers (deep only).
pub fn waits_on_deep(from: &WorkItem, target: &str, by_id: &HashMap<String,&WorkItem>, children: &HashMap<String,Vec<&WorkItem>>) -> bool
```

It returns true when `waits_on(from.id, target)` is true, or when `!from.blockedby_shallow` and a
depth-first walk over the descendants of each *complete* blocker of `from` (the same walk as
`dependency_status`, same cycle guard) reaches `target` while `target` is not complete.

In `dependency_status` (`lib.rs:864-866`) replace the skip test with: if `c` is open and
`waits_on_deep(c, &item.id, ...)` is true, the pair is **mutual**; apply R2 instead of returning
`Waiting(c)`.

### R2 — a mutual implied wait is broken by a fixed tie-break, and both sides say so

When `item` and `c` each implicitly wait on the other and neither declares the other:

1. The item that goes first is the one with the lower `priority` number; on a tie, the earlier
   `ts.receive`; on a tie, the lexically smaller id. Call it `first`, the other `second`.
2. `dependency_status(first)` skips `second` (as if `second` declared it waits on `first`).
3. `dependency_status(second)` returns a new variant `DepStatus::SiblingFirst(first_id)`.
4. The engine renders it as the tag `blocked by: sibling <first12> goes first (mutual follow-up wait)`
   and treats it exactly like `Waiting` for dispatch. When `first` completes the tag clears on the
   next tick through `reconcile_waits` (`engine.rs:1049`) as any other wait does.
5. The rule is symmetric and pure, so both engines compute the same order without coordination.

A pair where one side *declares* the other stays as today (`lib.rs:829-835`): the declared link
wins.

### R3 — every dependency wait names what it waits on

At `engine.rs:880`, replace the empty arm:

- `DepStatus::Waiting(id)` → tag `blocked by: waiting on <id12>` when `id` is a declared blocker,
  or `blocked by: waiting on <id12> (follow-up of blocker <b12>)` when it is an implied wait — the
  variant carries which; extend `Waiting` to `Waiting { id, via: Option<String> }` or add a
  parallel accessor. The tag goes through the existing `blocked by:` prefix path
  (`engine.rs:919-968` `reconcile_waits`), so it is rewritten every tick and cleared when the
  wait ends, like the cap and lock tags.
- `DepStatus::Failed(id)` → tag `blocked by: blocker <id12> failed`. Today this case is
  invisible too (`engine.rs:880`); the unstick sweep and a human both need to see it.

### R4 — the webui shows the reason where the human looks

No new field: the tags already render on the queue rows. Confirm the `blocked by:` colour
(`BLOCKED_TAG_COLOR`) applies to the two new texts.

## 5. Tests

Each test is shown red under its named mutation before it counts.

| # | Test | Setup | Assertion | Mutation that turns it red |
|---|---|---|---|---|
| T1 | `lib.rs` `#[cfg(test)] waits_on_deep_sees_an_implied_wait` | A complete, A created X (open); B complete, B created Y (open); X.blockedby=[B], Y.blockedby=[A] | `waits_on_deep(X,"Y")` and `waits_on_deep(Y,"X")` both true; `waits_on(X,"Y")` false | make `waits_on_deep` call only `waits_on` |
| T2 | `lib.rs` `mutual_implied_wait_is_broken_by_priority_then_age` | as T1 with X priority 0, Y priority 0, X received earlier | `dependency_status(X)==Satisfied`; `dependency_status(Y)==SiblingFirst(X)` | drop the tie-break (both return Waiting) |
| T3 | `lib.rs` `tie_break_is_symmetric_and_stable` | as T2, evaluate from both sides and with the items list reversed | the same `first` every time | tie-break on iteration order |
| T4 | `lib.rs` `declared_link_still_wins_over_the_tie_break` | as T1 plus Y.blockedby also contains X | `dependency_status(X)==Satisfied`, `dependency_status(Y)==Waiting(X)` regardless of priority | apply the tie-break before the declared-link check |
| T5 | `lib.rs` `shallow_item_is_never_part_of_a_mutual_pair` | as T1 with `X.blockedby_shallow=true` | `dependency_status(X)==Satisfied`; `dependency_status(Y)==Waiting(X)` with `via` naming A | ignore `blockedby_shallow` in `waits_on_deep` |
| T6 | `engine.rs` `#[cfg(test)] dependency_waits_carry_a_reason_tag` (extract the tag-text function from the match at `engine.rs:867-880`) | `Waiting{id, via:None}`, `Waiting{id, via:Some(b)}`, `Failed(id)`, `SiblingFirst(id)` | the four exact tag texts of R2/R3 | restore the empty arm |
| T7 | `e2e.sh`, new block | file A→X, B→Y as in T1 through the CLI; tick | the engine record shows X in-progress or queued with no dependency tag and Y tagged `blocked by: sibling <X12> goes first (mutual follow-up wait)`; after X completes, Y starts | any of the above |
| T8 | replay of the incident | load the 2026-09-13 records of `10920eb3ac88`, `22acf4aebb87`, `ce5f182b8345`, `7aebc409c4ac` (fixture from the API dump in the session scratchpad; copy it into `iter_core/tests/fixtures/`) | `10920eb3ac88` is `Satisfied` and `22acf4aebb87` is `SiblingFirst(10920eb3ac88)` | revert R1 |

## 6. Rollout and acceptance

`iter_core` and `iter_engine` only; no data-service change; tags are the existing `blocked by:`
family. Both engines pick it up on their next restart. Acceptance by hand: on the live pdy-dev
queue, every queued item whose direct blockers are all complete shows either no tag (runnable) or a
`blocked by:` tag naming an id; none shows nothing while waiting. Measured control: before the
change, `10920eb3ac88` showed no tag for 15.5 hours.

## 7. Out of scope

The choice of "deep by default" itself. The unstick sweep's instructions (a separate change adds
"a queued item with all direct blockers complete and no wait tag for more than a few ticks" to its
checklist). The cluster-restart health gate, which has its own design under discussion.
