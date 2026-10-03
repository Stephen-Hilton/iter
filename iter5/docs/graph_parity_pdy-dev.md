# Project graph parity: iter4 vs pdy-dev's usecase_map (2026-09-29)

**What was compared:**
- **Reference:** `~/dev/pdy-dev/demos/usecase_map/`, graph built by `build_usecase_map.py` at pdy-dev commit `c560feb1e`.
- **iter4:** the Project graph tab, http://127.0.0.1:8300/#tab=graph&p=pdy-dev.

**How iter4 got the data:** pdy-dev was mapped read-only with `iter sync --read-only --actors demos/repomapper/actors.yaml` (pdy-dev's `git status` was byte-identical before and after). The iter4 view is `GET /api/projects/pdy-dev/graph/view`.

## Settings used

The reference hard-codes some pdy-dev picture rules in its builder. iter4 holds the same rules as data, on the project record (`graph`):

```json
{"hide": ["demos", "sdk/repos/conformance", "sdk/repos/pdy_sdk_core_fake", "sdk/repos/pdy_test_auditor",
          "sdk/repos/pdy_test_employer", "sdk/repos/pdy_test_investor", "sdk/repos/pdy_test_paymentrails"],
 "parents": {"webapp": "devops"}}
```

These match the reference's `SKIP_NODE_FILES`, `SKIP_NODE_DIRS` and `PARENT_OVERRIDES`. Without them, iter4 shows 7 more nodes: the `demos` context and 6 test-tooling containers. It also draws the Build Status Console on its own rather than inside DevOps.

## Counts

| | reference | iter4 |
|---|---|---|
| project / actors / contexts / containers / components / use cases | 1 / 8 / 4 / 47 / 53 / 9 | 1 / 8 / 4 / 47 / 53 / 9 |
| contains edges | 104 | 104 |
| flow edges (process / data) | 889 (481 / 408) | 889 (481 / 408) |
| use-case touches | 149 | 149 |
| use cases with a flowmap | 9 | 9 |
| interface edges | 1558 (780 library) | 1544 (665 library) |

Flow and touch edges match per use case, not only in total.

## Playwright comparison

Both apps were driven through the same URL-hash states, and each state compared the set of visible node ids, the visible edge count and the sorted edge labels (the step numbers):
- every use case × every layout (cluster, tiered, rings, sequence, flow) × every flow mode (process, data, both);
- the whole repository × layouts × edge toggles.

**Shipped configuration (the reference's demo mode, edge toggles locked off): 138 of 138 states are identical.** Same nodes, same edges, same step labels. Neither page logged an error.

**Edge toggles on** (compared against an unlocked copy of the reference, which is otherwise the same app):
- The node sets are identical in all 153 states.
- 42 states are fully identical.
- The rest differ only in interface edges: a use-case view shows 1–5 fewer, and the whole repository differs as below.

## Why interface edges differ

The reference reads its interface edges from `demos/repomapper/out/graph.json`, the output of pdy-dev's separate audit tool (repomapper). iter4 derives them from the node files alone: a code node that lists interface I under `inputs` uses the node that lists I under `outputs`.

| | count |
|---|---|
| edges in both | 1428 |
| only in the reference | 130 |
| only in iter4 | 116 |

**Only in the reference (130):**
- 41 come from `demos/repomapper/flows.yaml`, a hand-written file of pdy-dev flow facts that no node file states;
- 74 come from repomapper's audit rules, which link edges the declarations do not name;
- 15 are actor edges that repomapper resolves to a different endpoint.

**Only in iter4 (116):**
- 101 come from `inputs`/`outputs` declarations that repomapper drops or re-attributes;
- 15 are the actor edges that match the reference's 15 with a different endpoint.

**Library versus network:**
- repomapper marks 108 more edges as build-time libraries than iter4 does, for example the `pdy-infra-base-container-image-*` interfaces.
- iter4 has no transport field to read on those interface files, so it applies a rule: `transport: build` in the interface frontmatter, or else a `-lib-` in the name. The same pairs exist on both sides; only the library label differs.
- Adding `transport: build` to those interface files would make the labels agree.

**Statuses:**
- The reference shows 8 interface edges as not live (2 broken, 6 not-deployed), from flows.yaml.
- pdy-dev's interface files carry no `status:`, so iter4 shows them all as live. A `status:` key in an interface's frontmatter is read when present.
