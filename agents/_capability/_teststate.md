# Capability: the test sweep gate (`iter teststate`)

`teststate:` is a frontmatter key on every node file that says whether the test sweep
(the project's engine-owned "Test sweep" schedule) runs the test nodes under that
node, and whether it files `test` items for that node's missing or short tests. It
exists for use-case-centric TDD: the user parks broad subtrees out of the sweep, and
each new use case pulls exactly its own parts back in, so the sweep works on what is
being built rather than on everything that exists.

Four values:

- `inherit` — the default. The nearest node above with a flag decides.
- `omit` — park this node out of the sweep. Carries down its whole subtree.
- `include` — re-enter this node, even under an omitted one.
- `block` — hard park: the node needs outside or vendor setup that is not present, so
  its tests cannot meaningfully run. Everything below a `block` stays out, whatever it
  says; only a human lifts a block.

The sweep walks chains that start at the project node, every use case and every actor,
down `codenodes` and `uses` edges; the nearest flag on each chain wins. A test node runs
when its own teststate is not `omit`/`block` and at least one chain reaching its owner
includes it. A node no chain reaches is not tested.

## Editing it — the engine-owned write path

Never hand-edit `teststate:` (one exception below). Use the command, which works from any
agent regardless of lock scope; it takes node file paths:

    "$ITER_BIN" teststate --project "$ITER_PROJECT" --include "<node file path>"
    "$ITER_BIN" teststate --project "$ITER_PROJECT" --list

`--omit`, `--include`, `--block` and `--clear` (back to `inherit`) repeat; `--list`
prints every node file with its own flag. Unless your work item explicitly asks for it,
only ever `--include`. Parking (`--omit` / `--block`) and un-parking (`--clear`) are the
user's calls.

## A block is the design

The command does not check what lies above the node. Before you include one, look up its
owners (`graph_owner` / `graph_neighbors`, or the `codenodes` links upward): if any of them
is `block`, do NOT include it or work around it — report the blocked node in your output
so the user decides.

## Test nodes that are run by hand only

Some test nodes must never run in the sweep: live-site ("prod") checks that need the
network, real credentials or a deployed system. **Link them anyway** from the tested
node's `children.tests`, and put `teststate: omit` in the test node's OWN frontmatter
when you create it — the one place an agent writes `teststate:` by hand. The sweep then
skips it whatever its owners say, and it still shows on the map. People run it with
`"$ITER_BIN" runtests <test node>`.

## When an agent includes something

A node cannot be included before it exists. The rule for a build handoff: each built
node is re-entered into the sweep when its code item completes — links and sweep
coverage reflect what was BUILT, not what was proposed.
