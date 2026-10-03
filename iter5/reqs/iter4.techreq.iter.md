---
id: 0cfd3caf-cb31-4512-ada2-915b3bdfde4f
name: "iter4 technical requirements"
description: "Engineering rules every iter4 part follows."
children:
  reqpaths: ["{topdir}/docs/iter4_spec.md"]
---

# iter4 — technical requirements

- **T1.** iter_data is the only component that talks to the database; engines and the web page use its HTTP API.
- **T2.** Every storage backend passes the same contract (`iter_data/src/contract_tests.rs`): versioned writes with exactly one winner, atomic lock acquire, atomic seq bumps.
- **T3.** Every node file's frontmatter starts with `id: <uuid>`.
- **T4.** A migration reads its source and never writes it.
- **T5.** Timestamps are UTC ISO-8601 in storage; the web page shows the viewer's timezone.
- **T6.** The Project graph reads one endpoint, `GET /api/projects/{p}/graph/view`, whose shape is pdy-dev's usecase_map `graph.json`; a code node's id is its folder (or `folder/stem` when a folder holds several). Picture rules a project needs (hidden folders, parent overrides) live on the project record (`graph.hide`, `graph.parents`), never in code.
- **T7.** Interface connections are derived, never stored twice: a code node listing interface I in `inputs` uses the node listing I in `outputs`. `transport: build` (or a `-lib-` name) marks a library link; `status:` on the interface file drives the not-built styling.
- **T8.** A read-only mapping (`iter sync --read-only`) writes nothing into the checkout: a file without an `id:` gets a UUIDv5 of project + path, flagged `id_derived`.
- **T9.** Graph edits are work items: `POST …/graph/edits` queues an `exec` item running `iter graph-apply` with lockdirs from `graph_edit::lock_scope`; `run_tests` queues a `test` shell item. Every generated file passes `iter validate`.
- **T10.** `WorkItem::is_shell()` — `exec`, or `test`/`testwriter` with an `exec_shell` — decides shell versus agent everywhere (dispatch caps, gate, dedup, lock shape); a shell run gets `ITER_SHELL=1`.
- **T11.** The nodetype is `tests` (`*.tests.iter.md`, children key `tests`); `testgroup` stays readable. Lease-bound locks are always enforced (no switch).
- **T12.** The webui is embedded in the iter_data binary, Cytoscape vendored (MIT, licences kept), so the container needs no network; the viewer is verified against the reference with Playwright (docs/graph_parity_pdy-dev.md).

