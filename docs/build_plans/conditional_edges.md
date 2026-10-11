# Conditional edges — requirements and build plan

Requested by Stephen 2026-10-11 while drawing Rendfall's two replication hosts: a peer-host that
replicates to clients **when a group of 3+ has at least 3 eligible peers**, and a server-host that
replicates **otherwise, or as fallback, or as auditor**. Both edges are true, neither is always true,
and the graph has no way to say so. Today the condition lives in a requirement (RF-GS-B-001) and in
two node descriptions, where a reader of the map never sees it.

## 1. Requirement

- **CE-1** A `calls` / `pulls` / `pushes` edge may carry a **condition**: a short phrase (≤ 60 chars)
  saying when the edge applies, e.g. `≥3 eligible peers`, `fallback only`, `audit sample`,
  `single player`. No condition means "always".
- **CE-2** The condition lives in the **source node's file**, next to the protocol, so a hand edit, the
  engine and the server all read the same thing:
  ```yaml
  protocols:
    pushes:
      "{topdir}/src/.../ue_game_replication_tick_for_client.code.iter.md": "UDP (UE replication, AES-GCM)"
  conditions:
    pushes:
      "{topdir}/src/.../ue_game_replication_tick_for_client.code.iter.md": "≥3 eligible peers"
  ```
  Same shape and same path-matching rules as `protocols` (§2.3 of `iter5_spec.md`).
- **CE-3** The map draws a conditional edge **dashed** with the condition in its label:
  `pushes (UDP) · if ≥3 eligible peers`. Hover, selection and zoom rules are the protocol's.
- **CE-4** It is editable everywhere a protocol is: the edge's right-click menu (`Condition…`), the
  configure lightbox (a `condition` text field with a hint list), `POST graph/edges` (optional
  `condition`), a new `POST graph/edges/condition {from, to, kind, condition|null}`, and MCP
  `edge_condition`. `graph` lists it as `condition` on the edge.
- **CE-5** Removing the edge removes its condition (as `remove_child` already clears the protocol);
  `conform` drops a `conditions` entry whose edge no longer exists and drops empty maps. Idempotent.
- **CE-6** A filter chip **"hide conditional"** on the Project graph hides dashed edges, so a reader can
  see the always-true backbone alone.
- **CE-7** Out of scope: conditions on ownership (`codenodes`), `same_as`, actor or usecase edges;
  machine-readable condition expressions. A condition is prose for a human.

## 2. Design

Mirror the protocol feature line for line; it landed on 2026-10-11 and every touch point is known.

| layer | protocol today | condition |
|---|---|---|
| `iter_core/src/nodefile/edges.rs` | `PROTOCOLS`, `KNOWN_PROTOCOLS`, `protocol_of`, `set_protocol`; `remove_child` clears it | add `CONDITIONS = "conditions"`, `KNOWN_CONDITIONS` (`always`, `fallback only`, `audit sample`, `single player`, `≥N eligible peers`, `feature flag`), `condition_of`, `set_condition`; `remove_child` clears it; `mod.rs` re-exports |
| `iter_core` conform | — | in the code-node pass: for each `conditions.<kind>.<path>` with no matching edge → drop (finding `condition-orphan`); length > 60 → truncate (finding `condition-long`) |
| `iter_data/src/nodes.rs` | `edge_protocol`, `set_edge_protocol`; `graph` edge gets `protocol` | `edge_condition`, `set_edge_condition` (same WrongKind / length checks, 60 chars); `graph` edge gets `condition` |
| `iter_data/src/api.rs` | `POST graph/edges/protocol`; `POST graph/edges` reads `protocol` | `POST graph/edges/condition`; `POST graph/edges` reads `condition`; change text `condition X → Y: …` |
| `iter_data/src/mcp.rs` | `edge_protocol` | `edge_condition` |
| `webui/graph.js` | `KNOWN_PROTOCOLS`; `edgeVisual` label `kind (protocol)`; detail pane "Protocol:" line | `KNOWN_CONDITIONS`; `edgeVisual`: if `m.link.condition` → `line = 'dashed'`, label `+ ' · if ' + condition`, class `conditional`; detail pane "Condition:" line; chip `hide conditional` (selector `edge.conditional`) |
| `webui/graphedit.js` | `editProtocol`, `pickProtocol`, configure field `protocol`, `addEdge(…, protocol)` | `editCondition`, `pickCondition`, configure field `condition`, `addEdge` passes `condition`; the paste/copy of an edge carries it |
| `docs/iter5_spec.md` | §2.3 code keys, §3 routes table, §4 MCP | add `conditions` beside `protocols` in each place |
| `docs/node_text_standard.md` | — | one line: a condition is 2–5 plain words saying *when*, not *why* |

Edge identity stays `(from, kind, to)`: a condition never creates a second edge. Two different
conditions on the same pair are one edge with one condition; model "A under X, B under Y" as two
targets, which is what Rendfall does (peer-host → client, server-host → client).

## 3. Build order (one agent, ~half a day)

1. `iter_core`: constants, `condition_of` / `set_condition`, `remove_child` clearing, conform pass.
   Unit tests beside the protocol tests: set/get/clear, path placeholder matching, orphan drop,
   idempotent conform, 60-char cap.
2. `iter_data`: graph edge field, `set_edge_condition`, route, MCP tool. API tests: set, clear, wrong
   kind rejected, long text rejected, `graph` shows it, change text written.
3. `webui`: `edgeVisual` + chip + menu + lightbox + add-edge dialog. e2e: create a `pushes` edge with a
   condition, assert the dashed class and the label, toggle the chip, clear it via the lightbox.
4. Docs: spec §2.3 / §3 / §4, node text standard line, this plan marked done.
5. Rendfall follow-up (separate repo, engine writes it): set `≥3 eligible peers` on the peer-host →
   client push and `otherwise · fallback · audit` on the server-host → client push, then delete the
   "no conditional edge type yet" sentence from RF-GS-B-001.

## 4. Acceptance

- A hand-written `conditions:` block round-trips through `conform` unchanged and shows on the map
  after the next sync, dashed and labelled.
- Setting a condition in the webui writes exactly that block to the source node's file and commits it
  as a graph edit; clearing it removes the block.
- `graph/conflicts` stays empty through the round trip; `iter validate` reports no new findings on
  pdy-dev or Rendfall.
