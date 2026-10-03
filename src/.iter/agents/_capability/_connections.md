# Capability: connections between parts (`level: connection` code nodes)

Read this before you add, link or describe how two parts of the system talk to each
other. iter5 has no interface files: a **connection** is a code node with
`level: connection` that stands for one KIND of connection the project uses — "API
call", "Event", "Stream", "Shared dataset", "Auth token", … — not one per operation.
Connections live under `{topdir}/global/connections/` (`<slug>.code.iter.md`, or
`<slug>/<slug>.code.iter.md` where the project already uses a folder each).

## What a connection file holds

    level: connection
    connects:
      from: ["{topdir}/…/supplier.code.iter.md", …]   # the parts that SUPPLY this connection (servers, publishers, owners of the data)
      to:   ["{topdir}/…/reached.code.iter.md", …]    # the parts it CONNECTS TO (callers, subscribers, readers)
    children:
      reqs: ["{thisfiledir}/reqs/<stem>.techreq.iter.md"]

The graph draws `supplies` edges (supplier → connection) and `connects` edges
(connection → part). The body says, in plain words, what travels over this kind of
connection and the law it follows (transport, security, message conventions). Rules
that every connection of the kind must obey are requirements: `## ` sections in the
connection's own `reqs/` bizreq/techreq files (`_iter_file_authoring`, "Requirement
files").

## Where the details of one operation go

The exact messages of one operation (a request and its reply, an event's fields, a
refusal code) are a requirement of the part that SUPPLIES it: a `## ` section in that
code node's own techreq file, written transport-neutral where the project asks for
that (shapes and rules, not routes or ports), with one success and one refusal example
when an example helps. The supplier is the one place a reader and the test agent look.
Before adding one, search (`rag_search`) for an existing section that already defines
it; extend that instead of writing a near-duplicate.

## Linking a part to a connection

A part that starts supplying or using a kind of connection is added to that connection's
`connects.from` or `connects.to`. The connection file is global and usually outside your
codepath, so do not edit it by hand: use the MCP tool `graph_edge_add` with
`kind: supplies` (from the code node id → the connection id) or `kind: connects` (from
the connection id → the code node id). The server records the edge at once and the
engine writes the connection file when no running item's lock covers it. Link what was
BUILT, not what was proposed. A new KIND of connection is a cross-cutting design
decision: propose it in your output (or a `plan` item) rather than creating one inside a
component's work.

## Testing a connection

A connection may own test nodes (`children.tests`) that check every supplier against the
connection's requirements, so drift turns red instead of accumulating silently
(`_test_node_authoring`).
