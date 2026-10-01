# Capability: the Test Loop gate (`iter teststate`)

`teststate:` is a frontmatter flag on a node, use-case, or interface file that says
whether the test sweep (the project's engine-owned Test sweep schedule, called the
Test Loop before iter4) runs that object's testgroups. It also decides whether the
sweep files `test` items for the object's missing or short tests. It exists for
use-case-centric TDD: the user parks broad subtrees out of the sweep, and each new
use-case pulls exactly its own dependencies back in, so the loop works on what is
being built rather than on everything that exists.

Four values, one selector:

- `inherit` — the default. The nearest ancestor with a flag decides.
- `omit` — park this object out of the sweep. Carries down its whole subtree.
- `include` — re-enter this object. Works under an omitted ancestor; **refused**
  under a blocked one.
- `block` — hard park: this object needs outside or vendor setup that is not
  present, so its tests cannot meaningfully run. Agent-proof by design.

**The nearest flag wins**, so including a component works even under an omitted
container. A `block` anywhere above wins over everything below it. An object owned
along several chains (the map is a graph, not a tree) is tested when any one chain
from the project head includes it.

## Editing it — the engine-owned write path

Never hand-edit the `teststate:` key. Use the command, which works from any agent
regardless of lock scope:

    "$ITER_BIN" teststate --project "$ITER_PROJECT" --include "<ref>"
    "$ITER_BIN" teststate --project "$ITER_PROJECT" --list

A `<ref>` is a node key, a name, a use-case name, an interface id, or a
declaring-file path suffix. `--omit`, `--include`, `--block` and `--clear` all
repeat; `--clear` removes the object's own flag so ancestors and the default apply
again. `--list` prints every object with its own flag and its EFFECTIVE state.

## The refusal is the design

If the command REFUSES because a node is `teststate: block`, do NOT try to force it
or work around it — that refusal is the whole point of `block`. Report the blocked
object in your output so the user decides.

Unless your work item explicitly asks for it, only ever `--include`. Parking things
(`--omit` / `--block`) and un-parking them (`--clear`) are the user's calls, not an
agent's.

## Test groups that are run by hand only

Some test groups must never run in the sweep: live-site ("prod") groups that need
the network, real credentials or a deployed system. **Link them anyway**, in the
owner's `children` like every other group, and put `teststate: omit` in the test
group file's OWN frontmatter when you create it. The sweep then skips that group
whatever its owners say, and it still shows on the map (an unlinked group is
reported as unlinked and looks forgotten). This is the one place an agent writes
`teststate:` by hand: in a test group file it is creating. People run such a group
with `"$ITER_BIN" runtests --group "<label>"`.

## When an agent includes something

An object cannot be included before it exists: it enters the sweep the moment its
declaring file exists and is linked. So the rule for a build handoff is that each
built node gets re-entered into the test sweep when its code item completes — links
and sweep coverage must reflect what was BUILT, not what was proposed.
