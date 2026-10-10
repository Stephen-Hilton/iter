# Source instructions: agent ({type})

This work item was created by another iter agent — a **{type}** agent — as a
handoff, not by a human. (`exec` means a scheduled shell run filed it, such as the
test sweep; its request says what it found.)

- Verify before building: the originating agent worked from its own reading of the
  project, at a commit that is probably no longer HEAD. Cheaply confirm its key
  assumptions (files it says exist, requirements or connections it says are defined, tests it says fail)
  before building on them.
- If the mainwork ends with a `PREMISE (re-verify before mainwork):` block, run its
  `holds-if` commands FIRST, before anything else, and compare each against the
  `# expected:` comment beside it. Any mismatch means the defect this item describes was
  already fixed while the item sat in the queue: report `PREWORK-FAILED: premise stale`
  with the failing command's actual output and the commits that superseded it, and do
  not perform the mainwork. Then end COMPLETE, not rejected: the outcome the item asked
  for exists, and the commits you cited are the evidence (Stephen, 2026-09-10; see
  `_shared`, "Done, with a recommendation left over"). One exception to "do not perform
  the mainwork": a small piece the mainwork EXPLICITLY asked for that the superseding
  commits did not carry (a sentence in a note, a name in a list) is still yours — do it,
  in scope, no permission needed. A stale premise is reported, never worked around — do
  not hunt for a nearby problem to solve instead. (This bullet is the whole procedure:
  there is no separate `premise-check` prework step any more.)
- If a load-bearing assumption is wrong, do not push through. Stop, and report exactly
  which assumption failed and what you found instead — the discrepancy is more valuable
  than a workaround built on it.
- The `context` files attached to this item were chosen by the originating agent; read
  them all, they carry its intent.
- Keep the handoff chain healthy: if this item is itself best served by delegating
  further, that's fine — but never create a work item that merely restates this one.

