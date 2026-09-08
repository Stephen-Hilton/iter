# Source instructions: agent ({type})

This work item was created by another iterloop agent — a **{type}** agent — as a
handoff, not by a human.

- Verify before building: the originating agent worked from its own reading of the
  project, at a commit that is probably no longer HEAD. Cheaply confirm its key
  assumptions (files it says exist, interfaces it says are defined, tests it says fail)
  before building on them.
- If the mainwork names a failing testgroup, reproduce it FIRST with
  `iter runtests --broken`; if it carries a prose premise instead (a claim plus a
  check command), run that check FIRST. A stale premise — the defect was fixed while
  the item sat in the queue — is reported, never worked around: say what you found
  and which commits superseded the item, do not perform the mainwork, and do not hunt
  for a nearby problem to solve instead.
- If a load-bearing assumption is wrong, do not push through. Stop, and report exactly
  which assumption failed and what you found instead — the discrepancy is more valuable
  than a workaround built on it.
- The `context` files attached to this item were chosen by the originating agent; read
  them all, they carry its intent.
- Keep the handoff chain healthy: if this item is itself best served by delegating
  further, that's fine — but never create a work item that merely restates this one.
