# MCP and GraphRAG hardening — requirements and build plan

Requested by Stephen 2026-10-21, after a test run that day against `https://iter.api.paydaay.app` as
`devlocal` (role `user`) through MCP (the Model Context Protocol: the standard way an AI client
discovers and calls a server's tools; iter_data serves it at `POST /mcp`). Agents cannot use
GraphRAG today. GraphRAG is iter's document search: it splits documents into chunks, stores two
vectors per chunk (lists of 384 numbers that place a text by meaning, one for the raw text and one
for its summary), and returns the chunks closest to a question together with their neighbours in
the project map. Every upload fails within about ten seconds, and only 49 of pdy-dev's map node
files are indexed. Agents also cannot list more than the 500 lowest-numbered of pdy-dev's 4,125 work
items. A crafted id can make a tool call a different endpoint from the one it names. Anyone holding
a token can write any name into a work item's "created by" and "requested by". Error bodies show
server internals. `rag_status` reports no engines while two are serving the project. This plan
fixes each of these, documents every tool well enough that an agent can use it without guessing,
and brings the HTTP layer into line with the current MCP specification.

Source of truth for the code: release `v2026.10.21a` of this repo. `git rev-parse HEAD` equals the
tag's commit and the working tree was clean when this plan was written. Line numbers below are from
that commit.

**Evidence.** The test scripts are in
`/private/tmp/claude-501/-Users-stephen-hilton-dev-pdy-dev/1e38fb99-c67a-4930-8238-e92b0ecb4ace/scratchpad/mcp/`
(`h.py` helper, `sec1.py`, `sec2.py`, `read1–5.py`, `w1–w4.py`). Only `read1.out`, `read2.out`,
`read1.json`, `read2.json`, `read3.json` and `read5.json` were saved. Where a result below has no
saved output, the request is quoted from the script and the result comes from the run's own report
of 2026-10-21. Section 7 says which is which.

---

## 1. Requirement

### Every tool documents itself

- **MG-1** Each of the 34 MCP tools (listed in §2.1) has a `description` with six labelled parts,
  in this order:
  - `Use:` what the tool is for and when to choose it over a neighbouring tool;
  - `Arguments:` every argument with its type, whether it is required, and its default (or
    "no default");
  - `Example:` one complete `arguments` object, as JSON, that works against a real project;
  - `Returns:` the exact shape of a successful result: top-level keys, the type of each, and the
    keys of each element of every list;
  - `Errors:` each way the call can fail, the message it returns, and what the caller should do
    about it;
  - `Limits:` caps, latency, what the tool does not see, and when its answer goes stale.

  Each `inputSchema` property also carries its own `description`, plus a JSON Schema `default`
  wherever a default exists. Each tool declares an `outputSchema`, which MCP 2025-06-18 allows: a
  JSON Schema of `structuredContent`, the machine-readable copy of the result that MCP sends next
  to the text copy. Every tool's result is a JSON object, so that `structuredContent` is always
  present. Today `locks_list` returns a bare array, and `handle` sends `structuredContent` only for
  objects (`iter_data/src/mcp.rs:143`), so locks come back as text only.
- **MG-2** The server-level `instructions` returned by `initialize` (`INSTRUCTIONS`,
  `mcp.rs:154-161`) tell a calling agent:
  - it does **not** upload node files: the iter engine indexes `*.iter.md` from the checkout by
    itself;
  - how soon an uploaded document becomes searchable and how to tell that it is (MG-5);
  - that `workitem_list` pages with a cursor (MG-8);
  - that `created by` / `requested by` are set by the server from the token (MG-12).

  Today the instructions say nothing on any of the four.

### GraphRAG ingestion works, for uploads and for node files

- **MG-3** No GraphRAG write stores a vector attribute whose value is `null`. A chunk with no
  summary yet has **no** `vec_sum` attribute at all. The same rule covers every vector-valued field
  (`vec_raw` and `vec_sum` on `rag_chunk`, `vec_sum` on `rag_doc`) and every writer of them: the
  four callers of `store_prepared` (uploads, node files, repo files, the built-in guide), the
  one-chunk rollup, resummarize, and summary/rollup reporting.
  Today: `store_prepared` writes `"vec_sum": null` on every new chunk (`pipeline.rs:146`). The
  comment two lines above it says the sparse vector index "refuses a null one". Once the vector
  indexes exist, which happens when `rag_chunk` holds 1,024 chunks (`search.rs:35, 481`; pdy-dev
  alone holds 2,671), every insert of a chunk without a summary is refused. The refusal measured on
  2026-10-21 was `arango 400 (errorNum 17): AQL: array expected for vector attribute`. The upload
  failed three times on engine `bakugo` (`MAX_ATTEMPTS = 3`, `pipeline.rs:22`) and stayed `failed`.
  Node files take the same path, through `nodes_put` → `store_prepared` (`pipeline.rs:317`). So
  every node file that changed after the indexes were built is refused and listed under `failed`,
  and its hash is never recorded. The next sync sends it again and is refused again.
- **MG-4** Documents and chunks already damaged are repaired once:
  - every `rag_chunk` with `vec_sum: null` loses the attribute;
  - every `rag_doc` in state `failed` whose stored error contains `array expected for vector
    attribute`, and whose bytes are still in `rag_blob`, goes back to `queued` with
    `ingest_attempts: 0`.
- **MG-5** An agent can tell when a document is searchable, and the tools state the real latency.
  - `rag_doc` and `rag_docs` return `searchable: true|false`. It is true when the document has at
    least one stored chunk and its state is `summarizing`, `rollup` or `ready`. Keyword and raw-text
    search work from that moment; summary search works once `ready`.
  - `rag_add_document` returns `{id, state, searchable, live_engines, message}`. It takes
    `wait_sec` (default 0, max 60): when set, the call returns as soon as the document is
    searchable, or when the wait runs out, with whichever state it reached.
  - When no live engine serves the project, `rag_add_document` still stores the upload. It sets
    `live_engines: 0`, and its message says the document stays `queued` until an engine serving the
    project checks in.

  Today the description says the document "is searchable at once" (`mcp.rs:289`) and the user
  guide says the same ("so a document is searchable immediately", `docs/iter_guide.md:388`). Both
  are false: nothing is searchable until an engine has claimed the upload, extracted, chunked and
  embedded it, and stored the chunks. Expected latency for a text document under 1 MB, with a live
  engine, is one engine tick (default 5 s) plus extraction and embedding, about 5–30 s. Summaries
  follow minutes later from the Summary agent. On 2026-10-21 the engine claimed the upload and used
  up its three attempts within about 10 s.
- **MG-6** `rag_add_document` refuses a filename ending in `.iter.md`, with the message: "node
  files are indexed by the iter engine from the checkout; change the node with graph_node_update
  (or edit the file) instead". Indexing a node file as an upload would create a second, stale copy
  under a different document id.
- **MG-7** After the MG-3 fix ships, every project's node files are re-indexed, pdy-dev first. A
  count check passes for each project:
  - the number of `rag_doc` rows with `kind == "node"` and state `summarizing`, `rollup` or `ready`
    equals the number of distinct `*.iter.md` paths in the project's map, after the project's
    `node_types` setting is applied;
  - `iter rag sync --dry-run` reports `0 changed/new`.

  The map, not the repo, is the reference. In pdy-dev, `git ls-files '*.iter.md'` lists 1,118
  files. 421 of them are `agentmem` files and 107 are `interface` files, neither of which is a map
  node type. `graph_stats` reports 626 map files.

### Work item listing

- **MG-8** `workitem_list` can return **every** work item of a project, page by page.
  - It returns `{total, items, next_cursor, sort, unknown_fields}`. `total` is the number of items
    matching the filter. `next_cursor` is an opaque string, or `null` on the last page. `limit` is
    1–500, default 50.
  - Passing `cursor` returns the next page of the same query. A cursor is bound to its filter and
    sort, so a cursor sent with a different query is refused with a 400.
  - Paging uses a cursor (the sort key and id of the last item returned), not an offset, so an item
    created between two calls neither shifts a page nor appears twice.

  Today the list is sorted on `created` (`mcp.rs:450`), a field no work-item record has: the
  record's times are `ts.receive`, `ts.start` and `ts.complete`. Every key compares equal, the
  stable sort keeps the storage order (ascending id), and `take(limit)` with a maximum of 500 is the
  only window. There is no offset or cursor. With 4,125 items in pdy-dev, the 3,625 items whose ids
  sort above the 500th can never be listed. `read1.out` shows `total 4125` and items `0019236c…`,
  `00563bb0…`, `007d17d2…`, each with `"created": null`.
- **MG-9** `filter` selects on **any** field of the stored record, nested fields included, named
  with a dotted path (`ts.receive`, `tags.text`, `sched.kind`).
  - Each clause is `{"field": …, "op": …, "value": …}`. Clauses in the list are ANDed;
    `{"any": [clauses]}` ORs them.
  - Operators: `eq`, `ne`, `in`, `nin`, `gt`, `gte`, `lt`, `lte`, `contains` (a case-insensitive
    substring of a string, or an element of a list), `prefix`, `exists`, `empty`.
  - A path that crosses a list (`tags.text`) yields every element. A positive clause matches when
    any element matches; `ne` and `nin` match only when none does.
  - **A missing field, or one holding `null`, reads as FALSE/empty.** It equals `false`, `""`,
    `[]` and `null`. It is `empty: true` and `exists: false`. It matches `ne` against any non-empty
    value. It never matches `gt`, `gte`, `lt`, `lte`, `contains` or `prefix`. It matches `in` only
    when the list holds `false`, `""` or `null`.
  - A field that no record in the project has is still allowed: it reads as missing. It is named
    in `unknown_fields`, so a typo is visible instead of silently matching every item.
  - The old shortcuts stay: `state`, `tag` and `text` (a substring of the name).
- **MG-10** `sort` orders on any field, nested included: `[{"field": "ts.receive", "dir":
  "desc"}, …]`. The default is `ts.receive` descending (newest first), then `id` ascending as the
  last key, so the order is total.
  - A missing field sorts as the lowest value, so it comes last in a descending sort.
  - Values of different types order null/missing < boolean < number < string < list < object.
    Strings compare by byte, which orders ISO-8601 times correctly.
  - A list field sorts by its first element.
  - `fields` (a list of dotted paths, or `["*"]` for the whole record) adds fields to each returned
    item. The default item shape is `id, name, state, agent, priority, tags, blockedby, received`:
    `received` is `ts.receive` and replaces the always-null `created`.

### A tool call reaches the endpoint it names

- **MG-11** Every request path that MCP builds from caller input is assembled by one helper that
  percent-encodes each segment. Ids are validated before any call:
  - empty, control characters or more than 200 characters → refused;
  - a work-item id or suffix (`workid`, `id`, `on`, `depends_on`) is resolved against the project's
    real items and only the resolved full id is used;
  - a graph node, requirement, document or test-node id is non-empty and encoded.

  Today `workitem_ask`, `workitem_reject`, `workitem_wait`, `workitem_block` and `workitem_doc`
  (when `workid` is used) paste the raw `workid` into the path (`mcp.rs:470-471, 481-485, 498-520,
  530, 754-764`), while every other id goes through `enc()` (`mcp.rs:303`). Measured: `workitem_doc`
  with `workid: "<id>/explain?"` built `POST /api/projects/pdy-dev/workitems/<id>/explain?/details`.
  The router read that as path `…/workitems/<id>/explain` with query `/details` and ran
  `workitem_explain` (`api.rs:2089`). That handler stamped `explain_requested`, which tells a
  serving engine to start a paid explain session. The tool returned `ok: true` and no doc row was
  written. Separately, `graph_node` with an empty `id` called `…/graph/nodes/` and came back as
  `404 /api/projects/pdy-dev/graph/nodes/: ""` (`read2.out`) when it should have been refused as
  "id is required".

### Who filed and changed an item comes from the token

- **MG-12** Anyone with access to a project keeps seeing **all** of it (`authz.rs:63-87`, unchanged).
  What changes is that the server, not the caller, records who acted:
  - **`requestedby`** is always set by `workitem_create` from the authenticated principal, which is
    one of:
    - `user:<name>`: a person's token;
    - `agent:<agent>@<engine>`: an engine session whose `X-Iter-Workid` names a work item of this
      project that is `in-progress` and claimed by that same engine (`engine` field equal to the
      session's engine);
    - `engine:<engine>`: an engine session without such a work item.

    The caller's value is ignored and reported in `warnings`.
  - **`createdby`** keeps its second job as the parent link: children inherit priority and
    use-case tags from it (`api.rs:1204-1222`), and the close gate and deep dependencies count
    children by it (`iter_core/src/lib.rs:605, 1122-1127`). It is either a **verified parent id**
    or, when there is none, the same principal as `requestedby`:
    - an agent principal's parent is its own verified work item;
    - a person may name a parent that exists in this project and is open;
    - an engine principal has no parent.

    Any other value the caller sends (a name, an unverified id) is replaced.
  - **`PUT …/workitems/{id}`** keeps the stored `createdby` and `requestedby`, whatever the body
    says.
  - **Detail rows**: `by` is already the token subject (`prepare_detail`, `api.rs:1994`). An agent
    principal's rows also get `by_workid`.

  Measured: a REST `POST /api/projects/pdy-dev/workitems` sent with a user token and a body carrying
  `requestedby: "agent:code"` and `createdby: "iter_engine:bakugo"` stored both exactly as sent. MCP
  `workitem_create` with `workid: "engine-fake-id"` (w1.py) stored `createdby: "engine-fake-id"`,
  and with `workid: <A>` it made the new item a child of A for any caller (`mcp.rs:667, 702-706,
  720`). The dedup "seen again by …" note copies `requestedby` (`api.rs:1689-1695`), so it repeated
  whatever the caller wrote.
- **MG-13** Text that names an actor names the real one. Today `workitem_reject` writes "rejected by
  the code agent: …" (`mcp.rs:483`; the same sentence is at `iter_engine/src/cli.rs:1267`) using
  the item's `agent` field. On 2026-10-21 that row's `by` was the user `devlocal`. Rejecting becomes
  a server route, `POST …/workitems/{id}/reject {reason}`, which writes "rejected by
  `<principal>`: `<reason>`" and parks the item with `lasterror`. MCP and the `iter` CLI both call
  it. The `wait` and `block` doc rows (`mcp.rs:518-520, 760-762`) name the principal the same way.
- **MG-14** The web interface stops sending `requestedby` / `createdby` (`webui/index.html:1388,
  1640`), and shows the server's values (`index.html:2014`).

### Engine liveness

- **MG-15** `rag_status.engines` lists every engine with an active `serves` edge to the project:
  `{name, display, account, hold, last_seen, age_sec, live}`. `live` is true when `last_seen` is
  within 3 × `ticksec` + 5 s. This is the rule `project_status` (`api.rs:745-790`) and
  `live_engines_for` (`api.rs:2116-2140`) already apply, so all three share one function.
  `live_engines` (`rag/mod.rs:279-290`) instead looks for a `projects` map on the engine record
  (`e.pointer("/projects/<p>")`). Engine records no longer have one: which engine serves which
  project is now a `serves` edge in the settings graph. Measured: `rag_status` returned
  `engines: []` while `bakugo` and `midoriya` both had active `serves` edges to pdy-dev and had
  checked in 5 s earlier.

### HTTP layer

- **MG-16** No error response shows internal detail. Every 5xx, and every 401 that comes from token
  verification, answers `{"error": "<generic message>", "error_id": "e-<12 hex>"}`. The full detail
  is logged once on the server, on a line carrying the same `error_id`, the method and the path.
  4xx messages that describe the caller's own input stay as they are, because agents act on them.
  MCP tool errors no longer repeat the internal route (today `"404 /api/projects/pdy-dev/graph/nodes/:
  …"`, `mcp.rs:346`). A document's stored `error` keeps a generic text and the `error_id`. Measured
  leaks:
  - the JWT library's error names (`InvalidSignature`, `InvalidAlgorithm`) and its algorithm lists,
    from `auth::verify_token` passed straight through (`auth.rs:48-52`, `api.rs:153, 159`);
  - Arango's AQL error text (`arango 400 (errorNum 17): AQL: …`, from `rag::backend`,
    `rag/mod.rs:103-105`, and `StorageError::Backend`, `api.rs:78`), shown both in responses and in
    `rag_doc.error`;
  - the server's model directory `/data/iter/…` (`embed::status`,
    `iter_core/src/rag/embed.rs:179-185`, returned by `rag_status.model`). This is shown to admins
    only from now on.
- **MG-17** The `Authorization` scheme is matched without regard to case (`Bearer`, `bearer`,
  `BEARER`), as RFC 7235 §2.1 requires, with one or more spaces or tabs before the token. One
  function parses the header for the `AuthUser` extractor (`api.rs:124`), for `user_password`
  (`api.rs:383`) and for the project authorization layer (`authz.rs:97`). Today all three use
  `strip_prefix("Bearer ")`, so `bearer <token>` gets a 401. The shared parser is a requirement in
  its own right: when the authorization layer cannot verify a token it passes the request on
  unchecked and lets the handler refuse it (`authz.rs:101-104`, module comment lines 17-19). If the
  extractor learned the lowercase form and the layer did not, a lowercase `bearer` from a
  non-member would pass the layer as "unverified", then be accepted by the handler, and the project
  check would never run.
- **MG-18** Origin checking and CORS (Cross-Origin Resource Sharing: the response headers that let
  a web page served from one site call a server on another; the browser enforces them, other
  clients ignore them). Stephen's ruling (2026-10-21): restricting origins is acceptable only if it
  is an easy setting inside iter_data, and it must never get in the way of agents reaching MCP;
  any iter_data instance can be renamed at any time, so nothing may depend on a fixed public name.
  - **What MCP requires, and why the default stays open.** MCP does not require
    `Access-Control-Allow-Origin: *`. CORS matters only to browser-based MCP clients (pages on
    another site calling `/mcp`, e.g. the MCP Inspector). Agents started by iter engines, Claude
    Code and other non-browser clients send no `Origin` and ignore CORS. The Streamable HTTP
    "Security Warning" (2025-06-18, 2025-11-25) says servers "**MUST** validate the `Origin`
    header ... to prevent DNS rebinding attacks" (a hostile page makes its own hostname resolve to
    the server's address so the browser treats it as the same site). That attack works against
    servers that trust something the browser attaches by itself (cookies, or a local network
    address). iter_data trusts neither: every request must carry a bearer token in the
    `Authorization` header, which a browser never adds on its own, so a hostile page that reaches
    the server still has no token to send. The default therefore stays open, as today. This does
    not meet the letter of the MCP "MUST"; it is a deliberate choice, recorded here.
  - **The optional restriction: one admin setting, no flags, no restart.** The server-wide record
    (`webui` table, row `server`, where the server on/off switch already lives, `switches.rs`) gains
    `allowed_origins: [string]`, edited on the webui Settings page ("Allowed web origins") or with
    `GET`/`PUT /api/server/origins` (admin only). iter_data re-reads it within 15 s of a change.
  - **The rule.**
    1. A request with no `Origin` is always allowed (every non-browser agent).
    2. A request whose `Origin` equals the origin the request was addressed to (scheme + `Host`,
       i.e. the webui calling its own server) is always allowed. Using `Host` here is safe only
       because no credential rides along automatically (above); this is what lets a renamed
       instance keep working with no setup.
    3. `allowed_origins` empty (the default) → every other `Origin` is allowed too.
    4. `allowed_origins` non-empty → any other `Origin` not in the list gets 403 (on `/mcp` a
       JSON-RPC error with no `id`; elsewhere `{"error":"origin not allowed"}`). The literal `null`
       origin counts as "other".
  - **CORS headers.** Sent to every allowed origin: `Access-Control-Allow-Origin: <that origin>`
    (the origin echoed, not `*`), `Vary: Origin`; methods `GET, POST, PUT, PATCH, DELETE, OPTIONS`;
    headers `authorization, content-type, accept, mcp-protocol-version, mcp-session-id,
    last-event-id, x-iter-project, x-iter-workid`; exposed `mcp-session-id, mcp-protocol-version`;
    `Max-Age: 600`; no `Allow-Credentials`, because iter uses bearer tokens, not cookies.
  - **Today.** `main.rs:266-274` sets `allow_origin(Any)`, `allow_methods(Any)`,
    `allow_headers(Any)` on every route, and no code reads `Origin`. With an empty list the new
    behaviour is the same for every caller.
- **MG-19** JSON-RPC batching (several messages in one JSON array) and the protocol-version header
  follow the specification version the client is using.
  - Batching was removed in 2025-06-18 (its changelog: "Remove support for JSON-RPC batching") and
    is still absent in 2025-11-25 ("The body of the POST request **MUST** be a single JSON-RPC
    request, notification, or response"). The client names its version in the
    `MCP-Protocol-Version` header. The specification says a server with no header and no other way
    to know "**SHOULD** assume protocol version `2025-03-26`", and must answer 400 to "an invalid or
    unsupported `MCP-Protocol-Version`".
  - So `/mcp`:
    1. answers 400 when the header names a version not in `PROTOCOL_VERSIONS`;
    2. refuses an array body with 400 and JSON-RPC error `-32600` ("batching is not supported in
       protocol version X") when the header is `2025-06-18` or later;
    3. still accepts an array when the header is absent, `2025-03-26` or `2024-11-05`.
  - Today `handle` accepts arrays from everyone (`mcp.rs:78-86`) and never reads the header.
- **MG-20** HSTS (HTTP Strict Transport Security: a response header telling a browser "for the next
  N seconds, reach this name only over HTTPS, and never let the user click past a certificate
  warning").
  - iter_data adds `Strict-Transport-Security: max-age=31536000` (one year) to every response it
    serves over HTTPS. No `includeSubDomains`, no `preload`: other names under the same domain may
    not all be HTTPS.
  - One place sends it, iter_data. A proxy in front (Caddy on the AWS host) passes upstream
    response headers through unchanged, so the header reaches browsers on the public name with no
    proxy setting; nothing outside the repo needs changing.
  - Today nothing sends it.
- **MG-21** `docs/iter_spec.md` §4.4 and §6.5, `docs/iter_guide.md` ("How does GraphRAG index a
  document?", "How is the index kept up to date?", "MCP for agents") and
  `iter_data/src/mcp.code.iter.md` describe the behaviour above. The guide's tool list
  (`iter_guide.md:414-416`) lists 24 of the 34 tools and gains the 10 it is missing: the four graph edit tools
  (`graph_node_create`, `graph_node_update`, `graph_edge_add`, `graph_edge_remove`), the four
  `req_*` tools, `testlogs` and `settings_graph`.

---

## 2. Design

### 2.1 Tool documentation (MG-1, MG-2)

The 34 tools, from `tools()` in `mcp.rs:179-293`:
- **work queue:** `status`, `workitem_list`, `workitem_get`, `workitem_details`, `workitem_create`,
  `workitem_ask`, `workitem_reject`, `workitem_wait`, `workitem_doc`, `workitem_block`,
  `capability`, `locks_list`;
- **map:** `graph_stats`, `graph_lookup`, `graph_node`, `graph_neighbors`, `graph_owner`,
  `graph_usecase`, `graph_node_create`, `graph_node_update`, `graph_edge_add`, `graph_edge_remove`;
- **requirements:** `req_create`, `req_update`, `req_move`, `req_delete`;
- **other:** `testlogs`, `settings_graph`;
- **GraphRAG:** `rag_search`, `rag_status`, `rag_docs`, `rag_doc`, `rag_link_document`,
  `rag_add_document`.

- Replace the positional helper `tool(name, title, desc, props, required, read_only)`
  (`mcp.rs:171-177`) with a struct `ToolDoc { name, title, use_, arguments, example: Value,
  returns, errors, limits, props, required, output: Value, read_only }`. `ToolDoc::render()` builds
  the description from the six parts, in order, with the labels of MG-1. The labels are part of
  the contract: the guard reads them.
- Put each tool's `ToolDoc` in a new file `iter_data/src/mcp_docs.rs`, one function per tool. This
  is a sub-file of the existing MCP component and keeps `mcp.rs` readable. `tools()` becomes
  `docs().iter().map(ToolDoc::render)`.
- `outputSchema` per tool (`output`): object schemas with `required` keys. Array results are
  wrapped so that every result is an object, which means `structuredContent` is always sent
  (`mcp.rs:143-145`):

  | tool | today returns | becomes |
  |---|---|---|
  | `locks_list` | `[...]` | `{"locks": [...]}` |
  | `workitem_details` | `{"id", "details"}` (already wrapped) | unchanged |
  | `testlogs` | `{"logs": [...]}` (already wrapped) | unchanged |

  `call_tool` checks every result is an object before returning it (debug assertion, plus the
  guard in §3).
- Argument defaults move into the schema (`"default": 50` on `workitem_list.limit`, `"default": 8`
  on `rag_search.k`, and so on). Each tool's `Arguments:` text is generated from the schema, so the
  two cannot drift apart.
- `Errors:` lists the error families each tool can return, after MG-16:
  - `400 <the API's validation message>`;
  - `403 forbidden: '<you>' is not a member of project '<p>'`;
  - `404 not found`;
  - `409 conflict` (with `current` for versioned writes);
  - `500 internal error (error_id e-…)`;
  - the tool's own refusals, e.g. `no calling work item: pass workid or send X-Iter-Workid`.
- `INSTRUCTIONS` (`mcp.rs:154-161`) is rewritten to carry the four MG-2 statements. The
  node-file sentence reads: "Do not upload *.iter.md node files: the iter engine indexes them from
  the checkout after each map sync. A node edited with graph_node_update reaches search after the
  engine writes the file and its next sync (about a minute)."

Example of the rendered description, `workitem_list`:

```
Use: page through a project's work items with any filter and sort; use workitem_get for one item's full record.
Arguments: project (string, default X-Iter-Project header); filter (list of {field, op, value} or {any: [...]}, default none);
  sort (list of {field, dir}, default [{"field":"ts.receive","dir":"desc"}]); limit (integer 1–500, default 50);
  cursor (string from next_cursor, default none); fields (list of dotted paths or ["*"], default none);
  state / tag / text (shortcuts, default none).
Example: {"project": "pdy-dev", "filter": [{"field": "state", "op": "in", "value": ["queued", "question"]},
  {"field": "ts.receive", "op": "gte", "value": "2026-10-01"}], "sort": [{"field": "priority", "dir": "asc"}], "limit": 100}
Returns: {total: integer, items: [{id, name, state, agent, priority, tags: [string], blockedby: [string], received: string, …fields}],
  next_cursor: string|null, sort: [{field, dir}], unknown_fields: [string]}
Errors: 400 "cursor belongs to another query" — drop the cursor and start again; 400 "unknown op X"; 403 not a member of the project.
Limits: a missing field reads as false/empty (see filter rules); 500 items per page; total is counted at call time.
```

### 2.2 GraphRAG writes never store a null vector (MG-3, MG-4)

`iter_data/src/rag/pipeline.rs`:
- **`store_prepared` (lines 109-195).** Build each chunk row (lines 136-149) without `vec_sum`.
  Insert `"vec_sum"` (and `"sum_model"`) only when `kept` holds a summary vector that passes
  `vec_ok` (line 72). `vec_raw` is already checked by `Prepared::check` (line 85). Today the old
  summaries are read with `RETURN {…, vec_sum: c.vec_sum, …}` (line 114), which yields `null` for a
  chunk that never had one even though it is marked `done`. So `kept` must also be tested with
  `vec_ok`; on failure the chunk is marked `pending` and gets no attribute.
- **The document row (line 165).** Remove `"vec_sum": null` from the `rag_doc` row. No vector index
  exists on `rag_doc` today, so this is not failing yet. It is the "similar null-vector field" and
  would fail the day one is added.
- **Line 178** (`INSERT r INTO rag_chunk OPTIONS {keepNull: false}`). Keep it, but stop relying on
  it. ArangoDB applies `keepNull` to updates, replacements and `overwriteMode: "update"` inserts,
  not to a plain `INSERT`; that is why the comment at line 144 did not protect the insert. This
  reading of the ArangoDB documentation is confirmed by the first red test in §3, not assumed.
- **`maybe_single_chunk_rollup` (lines 201-215).** `vec_sum: c.vec_sum` becomes
  `vec_sum: IS_ARRAY(c.vec_sum) ? c.vec_sum : null`, with `OPTIONS {keepNull: false}` on that
  `UPDATE`, so a missing chunk vector removes the document attribute instead of writing null.
- **Summary and rollup reporting (lines 697-698, 781, 801).** These already write a vector from
  `vector_or_embed`. Add `vec_ok` on the value before the update; a failure marks the chunk or doc
  failed with a generic error (MG-16).

`iter_data/src/rag/mod.rs`:
- **Resummarize (lines 606, 616).** Line 606 already uses `keepNull: false`. Add the same option to
  the `rag_doc` `UPDATE` at line 616, which today writes `vec_sum: null`.
- **Repair, run once per start in `ensure_schema` (`rag/mod.rs:155`, the cleanup at lines 176-180).**
  - Keep the existing chunk cleanup, and extend it to `vec_raw`.
  - Add: `FOR d IN rag_doc FILTER HAS(d,'vec_sum') AND d.vec_sum == null UPDATE d WITH {vec_sum:
    null} IN rag_doc OPTIONS {keepNull: false}`.
  - Add the re-queue of MG-4: `FOR d IN rag_doc FILTER d.state == 'failed' AND CONTAINS(d.error,
    'array expected for vector attribute') AND DOCUMENT(CONCAT('rag_blob/', d._key)) != null UPDATE
    d WITH {state: 'queued', ingest_attempts: 0, error: '', updated: @now} IN rag_doc`. Use the
    blob key the upload path uses (`doc_upload`, `rag/mod.rs:379`).
  - Each statement logs how many rows it changed, and is idempotent.
- **The guide.** It is stored through the same `store_prepared` (`rag/guide.rs:66`), so it gets the
  fix with no extra code. From the code, not measured: on a server whose vector indexes exist, a
  changed guide would fail to re-index at the next start, the same way.

### 2.3 Searchable signal, latency and node files (MG-5, MG-6)

- `rag/mod.rs` `docs_list` and `doc_get` (line 532): add `searchable` (rule in MG-5) to each
  document row. This is a computed field, never stored.
- `rag/mod.rs` `doc_upload` (line 379):
  - refuse `filename` ending `.iter.md` (MG-6), 400;
  - return `live_engines` from the shared function of §2.6;
  - when it is 0, add `message: "no live engine serves this project: the document stays queued
    until one checks in"`.
- `mcp.rs` `rag_add_document` (lines 618-632): after the upload, when `wait_sec > 0`, poll `GET
  …/rag/docs/{id}` once a second until `searchable` is true or a state is `failed`, or until
  `min(wait_sec, 60)` seconds pass. Return `{id, state, searchable, live_engines, waited_sec,
  message}`. The `message` names the next step: "searchable now", "still <state>: poll rag_doc", or
  "failed: <generic error> (error_id …)".
- Descriptions of `rag_add_document`, `rag_doc`, `rag_docs`, `rag_search` and `rag_status` carry
  the latency numbers of MG-5. Node files reach the index only after the engine's next file-sync
  round that changed something, plus at most `RAG_EVERY` = 60 s (`iter_engine/src/filesync.rs:64,
  766-790`).

### 2.4 Re-indexing every project (MG-7)

The engine already re-indexes node files after any file-sync round that changed something
(`maybe_rag`, `filesync.rs:766-790`). Every failed node file had no hash recorded, so it is sent
again on each of those runs. Once the server fix is live, the next run indexes them without any
intervention. Rollout still runs the sync by hand (§5 step 4), so the result is known that day
rather than inferred. The count check is a script, `test/iter_data/rag_node_count.sh <project>`:
- read `GET /api/projects/<p>/graph`, collect the distinct `path` values ending `.iter.md` (after
  the project's `node_types` setting from `GET …/rag/settings`): that is M;
- read `GET …/rag/docs?kind=node` and count the rows whose state is `summarizing`, `rollup` or
  `ready`: that is N;
- print `ITER_RESULT` with M, N and the repo's `git ls-files '*.iter.md' | wc -l` for context, and
  exit 0 only when N == M.

Distinct paths matter: requirement section nodes (`req`, 8,068 of them in pdy-dev) can share their
file's path.

### 2.5 Work item listing (MG-8, MG-9, MG-10)

- **New module `iter_data/src/wiquery.rs`.** It holds pure functions only, with no I/O:
  - `parse(body) -> Result<Query, String>`: filter, sort, limit, cursor, fields, plus the shortcuts
    `state`, `tag`, `text` turned into clauses;
  - `get_path(&Value, "a.b.c") -> Vec<&Value>`: maps over lists, and returns empty for missing or
    `null`;
  - `matches(&Value, &Clause) -> bool`: the MG-9 rules, including missing = FALSE/empty;
  - `cmp(&Value, &Value, &[SortKey]) -> Ordering`: the MG-10 type order, with `id` as the final key;
  - `encode_cursor` / `decode_cursor`: base64url JSON `{v: 1, q: <sha256 of the canonical
    filter+sort>, k: [last sort values], id}`. A different `q` → `Err("cursor belongs to another
    query")`.
- **New route `POST /api/projects/{name}/workitems/query`** in `api.rs`, next to `workitems_list`
  (line 1166). It loads `st.store.query("workitem", &name)`, filters, counts `total`, sorts, skips
  to the first row after the cursor's `(k, id)`, takes `limit`, and projects the fields. It returns
  `{total, items, next_cursor, sort, unknown_fields}`. `unknown_fields` lists the filter and sort
  paths that no row of the project has.
- **`GET …/workitems` is unchanged** (the engine and the webui read the whole array).
- **`mcp.rs` `workitem_list` (lines 440-454)** becomes one call to the new route. `brief`
  (lines 637-641) drops `created` and adds `received: ts.receive`. `status` (lines 428-439) uses
  the same brief.
- **Cost.** 4,125 items are already loaded whole by `resolve` on every id lookup, so one query
  holds no more in memory than today. Moving the filter into AQL is out of scope (§6).

### 2.6 One path builder, validated ids (MG-11)

- **`mcp.rs`.** Replace `enc` (lines 303-313) with:
  - `fn seg(s: &str) -> Result<String, String>`: refuses empty, any control character, or more
    than 200 characters; otherwise percent-encodes everything except `A-Z a-z 0-9 - _ . ~`, and
    also encodes a segment that is exactly `.` or `..`;
  - `fn api_path(parts: &[&str], query: &[(&str, &str)]) -> Result<String, String>`: joins
    `/api/…` from `seg`'d parts and builds the query string with the same encoding.

  Every `format!("{pp}/…")` in `project_tool`, `create` and `block` becomes `api_path(...)`, and
  `pp` itself is removed.
- **Work-item ids.** `workid_of` (lines 382-385) returns the id **resolved** through `m.resolve`,
  so `ask`, `reject`, `wait`, `doc` and `block` only ever use a full id that exists in the project.
  `workitem_doc` already resolves `id` (line 529); it now resolves `workid` too.
- **Required ids** are checked before any call: `graph_node.id`, `graph_neighbors.id`,
  `rag_doc.id`, `req_*.id`, `rag_link_document.doc/node` and `graph_usecase.ucid`. An empty value
  answers `"<arg> is required"`.

### 2.7 Provenance from the token (MG-12, MG-13, MG-14)

- **`api.rs`, new `pub(crate) async fn principal(st, user: &AuthUser, project, workid_hdr: &str) ->
  Principal`**, where `Principal { requestedby: String, parent: Option<String>, by_workid:
  Option<String> }`:
  - role `admin`, `user` or `viewer` → `user:<sub>`;
  - role `engine` → engine name from `settings::machine_engine(sub)` (`settings.rs:280`). If
    `workid_hdr` names an item of `project` with `state == "in-progress"` and `engine == <that
    name>`: `agent:<item.agent>@<engine>`, with `parent = by_workid = Some(id)`. Otherwise
    `engine:<engine>`.
- **`workitem_create` (lines 1611-1687)** calls `principal` first, before
  `test_sweep_on_create` and dedup, so the "seen again" note (`repeat_source`, lines 1689-1695)
  records the real actor.
  - It sets `body.requestedby`.
  - It sets `body.createdby`: `parent` when there is one; else, for a person, the body's
    `createdby` if it is the id of an open item of this project; else the principal.
  - When a caller-supplied value was replaced, it pushes `"requestedby/createdby are set by the
    server; the sent value X was ignored"` into `warnings`.
- **`place_new_item` (line 1224)**: `human = requestedby.starts_with("user:")`. The current
  `!starts_with("agent")` would put `engine:` items into the human priority band.
- **`workitem_put` (lines 1856-1920)**: when `current` exists, copy its `createdby` and
  `requestedby` over the body's before `put_versioned`.
- **`prepare_detail` (lines 1991-2010)**: takes the `Principal`, and adds `by_workid` when present.
  `details_append` and `detail_put` pass it through.
- **The `X-Iter-Workid` header reaches the API.**
  - `Mcp::call` (`mcp.rs:329`) adds `x-iter-workid: <caller.workid>` to every replayed request.
  - The `iter` CLI client (`iter_engine/src/cli.rs:315`, the `Api` it builds) adds it when
    `ITER_WORKID` is set.
  - `work.rs:1180` already puts it in each agent's MCP config.
  - Handlers read it with a small extractor `WorkidHeader`.
- **New route `POST /api/projects/{name}/workitems/{id}/reject {reason}`**. It:
  1. requires a writer;
  2. refuses a closed item;
  3. appends doc row "rejected by `<principal.requestedby>`: `<reason>`";
  4. sets `state: parked` and `lasterror: "rejected: <first 400 chars>"` in one versioned write.

  `mcp.rs` `workitem_reject` (lines 475-487) and `cli.rs:1267` call it.
- **`workitem_wait` and `block` doc rows** (`mcp.rs:518-520, 760-762`) take their actor from a new
  `GET /api/whoami` (with the workid header), which returns the `Principal`, instead of from
  `item.agent`. `workitem_ask` writes no actor text and is unchanged apart from §2.6.
- **The CLI's own `add` body** (`cli.rs:1068-1074`), `sweep.rs:429` and
  `iter_core/src/lib.rs:735` may keep sending the fields; the server ignores them. No engine change
  is required for correctness, only for the header.
- **`webui/index.html`**: remove `requestedby:me||"",createdby:me||""` from `cloneBody` (line
  1388) and `createItem`'s caller (line 1640). Line 2014 already displays the stored values.

### 2.8 `rag_status` engines (MG-15)

- Move `live_engines_for` (`api.rs:2116-2140`) to `settings.rs` as `pub async fn
  serving_engines(store, project) -> Vec<EngineLiveness>`, where `EngineLiveness { name, display,
  account, hold, state, last_seen, age_sec, live }`. It returns every engine with an active
  `serves_edge` (`settings.rs:309`), and computes `live` by the 3 × `ticksec` + 5 s rule.
- The three callers use it:
  - `project_status` (`api.rs:758-787`, unchanged output);
  - `workitem_explain` (live and `state == "Running"`, as today);
  - `rag::status` (`rag/mod.rs:323`, replacing `live_engines`, lines 279-290, which is deleted).

### 2.9 Generic errors with an id (MG-16)

- **New `iter_data/src/errid.rs`**: `pub fn record(detail: &str, method: &str, path: &str) ->
  String`. It mints `e-` + 12 hex characters from a random UUID, prints `[iter_data] error <id>
  <method> <path>: <detail>` to stderr, and in `cfg(test)` also pushes onto a process-wide ring
  buffer that tests read.
- **`api.rs` `ApiError`** (lines 52-80) gains `Internal(String)`. `From<StorageError>`
  (`Backend`, line 78) and `rag::backend` / `rag::unavailable` (`rag/mod.rs:103-108`) produce it.
  `IntoResponse` cannot see the request, so a middleware `errid::layer` placed outermost in the
  router reads `Internal` from a response extension, records it with the method and path, and
  writes the body `{"error": "internal error", "error_id": id}`.
- **Token failures** (`api.rs:153, 159`): `auth::verify_token`'s message goes to `errid::record`.
  The response is `401 {"error": "invalid or expired token", "error_id": id}`. "missing bearer
  token", "token revoked" and "password change required" stay as they are: they tell the caller
  what to do and reveal nothing about the server.
- **`mcp.rs` `Mcp::call` (lines 342-347)**: the error becomes `"<status>: <error>"`, plus `"
  (error_id <id>)"` when the body has one. The path is no longer included.
- **`pipeline.rs` `ingest_failed` (line 648) and `nodes_put` / `files_put` `failed` entries**
  (lines 317-323, 406-407) store and return `"indexing failed (error_id e-…)"`; the detail goes to
  the log. Engines report their own extraction errors in `req.error`. Those are the caller's own
  text, so they are kept, apart from the two permanent-failure phrases the code already matches.
- **`rag::status`**: `model` is `embed::status()` without `dir` and `error` for non-admins.

### 2.10 One bearer parser (MG-17)

`api.rs`: `pub(crate) fn bearer(headers: &HeaderMap) -> &str`. It splits the value at the first run
of spaces or tabs, compares the scheme with `eq_ignore_ascii_case("bearer")`, and returns the
trimmed rest, or `""`. Its callers:
- `AuthUser::from_request_parts` (line 124);
- `user_password` (line 383);
- `authz::project_authz` (`authz.rs:93-98`).

`mcp.rs:67` forwards the raw header to the extractor and needs no change.

### 2.11 Origin allowlist, CORS, protocol version, batching, HSTS (MG-18, MG-19, MG-20)

- **New `iter_data/src/weblayers.rs`** with `pub fn apply(app: Router, cfg: Arc<WebCfg>) -> Router`,
  called from `main.rs` in place of the `CorsLayer` block (lines 266-274). The router tests in
  `settings/tests.rs` build the app with it too, so the layers are tested as deployed. It holds:
  - **`origin_guard`**: applies the MG-18 rule in order (no `Origin`; same origin as the request's
    scheme + `Host`; empty list; list membership) and answers 403 only in case 4. `OPTIONS`
    preflights pass through the same check.
  - **The CORS layer**: `tower_http::cors::CorsLayer` with `AllowOrigin::predicate` calling the
    same rule, the method, header and expose lists of MG-18, and `max_age(600)`.
  - **HSTS**: when `cfg.tls_on`, `SetResponseHeaderLayer::if_not_present(STRICT_TRANSPORT_SECURITY,
    "max-age=31536000")`.
- **`WebCfg { origins: ArcSwap<Vec<HeaderValue>>, tls_on: bool }`**: `origins` is loaded from
  the `webui/server` record at start and refreshed every 15 s, and immediately after
  `PUT /api/server/origins`.
- **`GET`/`PUT /api/server/origins`** (in `keys.rs` beside `/api/server/tls`, `require_admin`):
  the body is `{"allowed_origins": ["https://example.org", ...]}`; each entry must parse as
  `scheme://host[:port]` with no path, or the PUT answers 400 naming the bad entry.
- **webui Settings page**: an "Allowed web origins" box (one per line) with the sentence "Leave
  empty to allow every site. Agents that are not browsers are never affected.", saving through the
  route above.
- **`mcp.rs` `handle` (lines 65-92)**:
  - read `mcp-protocol-version`; a value not in `PROTOCOL_VERSIONS` (`mcp.rs:32`) → 400 with a
    JSON-RPC error that has no `id`;
  - for `Value::Array`, when that version is `>= "2025-06-18"` (string order is date order) → 400
    with `-32600` "batching is not supported in protocol version <v>"; otherwise keep the loop.

### 2.12 Docs (MG-21)

- `docs/iter_spec.md` §4.4: provenance (MG-12), and the shared bearer and authz parser (MG-17).
- `docs/iter_spec.md` §6.5: the list and query route, origin and batching rules, tool doc
  contract.
- `docs/iter_guide.md:388`: correct "searchable immediately".
- `docs/iter_guide.md:402-404`: node files are indexed by engines, latency, the count check.
- `docs/iter_guide.md:406-428`: the full tool list, `wait_sec`, paging, the `.mcp.json` example
  with `X-Iter-Workid`.
- `iter_data/src/mcp.code.iter.md`: new files `mcp_docs.rs`, and the dispatch through `api_path`.
- `docs/iter_guide.md` (Settings): the "Allowed web origins" box, what empty means, and that
  non-browser agents are never affected. No installer change: there is no new flag.

---

## 3. Tests

Each guard is shown to fail before it is trusted. For each test, the builder breaks the named thing,
runs the test, sees red, restores, and sees green. The red output goes into the commit message that
lands the test, as one line per guard: `MG-n red: <test> — <first failing assert>`. Where the "break"
is the bug as it stands at `v2026.10.21a`, the red run is simply the test run against the unfixed
code, before the fix commit.

Router tests use the existing harness in `iter_data/src/settings/tests.rs` (the `T` struct: a
throwaway ArangoDB database, minted user tokens and engine sessions, the real router plus MCP). They
are extended to build the app through `weblayers::apply`. RAG tests need an ArangoDB started with
`--vector-index`. When it is missing, the test **fails** with "harness: vector index unavailable"
rather than skipping, so a missing capability cannot read as a pass.

| req | test (file) | asserts | break it by |
|---|---|---|---|
| MG-1 | `every_tool_is_documented` (`mcp.rs` tests) | for each of the 34 tools: the six labels in order, each part non-empty; every schema property has a `description`; each optional property has a `default` or its text says "no default"; `Example:` parses as a JSON object whose keys are schema properties and include every `required` one; `outputSchema` is an object schema; the tool count is 34 and the names equal the list in §2.1 | deleting `Errors:` from one tool (red names the tool); adding a 35th tool without docs; removing the `default` from `workitem_list.limit` |
| MG-1 | `examples_run_and_match_output_schema` (`settings/tests.rs`) | each read-only tool, called with its own `Example` against a seeded project, returns `isError: false` and `structuredContent` that validates against its `outputSchema` (a 40-line checker for `type`, `required`, `properties`, `items`); write tools are run on a scratch project in dependency order | changing `locks_list` back to returning a bare array; renaming `total` to `count` in `workitem_list`'s result |
| MG-2 | `instructions_state_the_rules` (`mcp.rs` tests) | `INSTRUCTIONS` contains the node-file sentence, "searchable", "next_cursor" and "set by the server" | deleting the node-file sentence |
| MG-3 | `chunk_without_summary_stores_no_null_vector` (`rag` tests) | after training a vector index on 64 seeded chunks (nLists 8), `store_prepared` of a new 3-chunk document succeeds; no stored chunk or doc has `HAS(x,'vec_sum') AND x.vec_sum == null` | running it on `v2026.10.21a`: expect `arango 400 (errorNum 17)`; then restoring `"vec_sum": Value::Null` at line 146 |
| MG-3 | `every_vector_writer_omits_null` (`rag` tests) | the same "no null vector" query after each writer: `nodes_put`, `files_put`, `doc_chunks_put`, guide ingest, single-chunk rollup, resummarize | restoring `"vec_sum": null` on the doc row (line 165), or removing `keepNull: false` from the resummarize doc update |
| MG-3 | `kept_summary_with_missing_vector_is_pending` | a chunk marked `done` with no `vec_sum`, re-stored with the same text, comes back `pending` with no attribute | removing the `vec_ok` check on `kept` |
| MG-4 | `repair_requeues_vector_failures` | a seeded `failed` doc with the errorNum-17 error and a blob becomes `queued` with 0 attempts; one with another error, or with no blob, is untouched; a second run changes nothing | dropping the blob condition (the no-blob doc flips: red) |
| MG-5 | `upload_reports_searchable_and_engines` | `rag_doc.searchable` is false while `queued` or `ingesting` and true after `doc_chunks_put`; `rag_add_document` with no serving engine returns `live_engines: 0` and the "stays queued" message; `wait_sec: 2` returns within 3 s with `searchable: false` | hard-coding `searchable: true` |
| MG-5 | `wait_returns_when_searchable` | a test task plays the engine (claims and PUTs chunks after 1 s); `wait_sec: 10` returns in under 3 s with `searchable: true` | polling only at the end of the wait |
| MG-6 | `node_files_are_not_uploads` | `rag_add_document` with `filename: "x.code.iter.md"` → `isError: true`, message names `graph_node_update`; `notes.md` succeeds | removing the suffix check |
| MG-7 | `test/iter_data/rag_node_count.sh` | N == M on a test project after `iter rag sync`; the control case deletes one node doc and expects exit 1 naming the missing path | the control case is the break: it must go red |
| MG-8 | `list_pages_through_everything` (`settings/tests.rs`) | with 1,203 seeded items, paging at `limit: 500` returns 500 + 500 + 203 items, all ids distinct, union equals the set; `total` is 1,203 on every page; an item created between pages 1 and 2 neither repeats nor shifts page 2; a page-2 cursor sent with another filter → 400 "cursor belongs to another query" | sorting on `created` again (pages repeat the lowest ids: the distinct-count assert fails); dropping `q` from the cursor |
| MG-9 | `wiquery` unit tests (`wiquery.rs`) | a table of record × clause → bool covering every operator: nested `ts.receive gte`; list `tags.text eq`; `ne` over a list; a missing field against `eq false`, `eq ""`, `eq []`, `empty`, `exists`, `gt`, `contains`, `in [false]`, `in ["x"]`; `any` ORs; an unknown field is named in `unknown_fields` | treating missing as `null` that fails `eq false`; making `ne` match when any element differs |
| MG-10 | `sort_default_and_any_field` | with no `sort`, items are by `ts.receive` descending then `id`; missing values come last in `desc`; mixed types follow the type order; `sort: [{"field":"priority","dir":"asc"}]` holds across pages; `fields: ["ts.start","requestedby"]` adds exactly those | swapping the `id` tie-break for nothing (pages overlap on equal times) |
| MG-11 | `hostile_ids_never_change_the_route` (`settings/tests.rs`) | for `workitem_ask/reject/wait/block/doc` (as `workid` and as `id`), `graph_node`, `graph_neighbors`, `graph_usecase`, `req_update/move/delete`, `rag_doc`, `rag_link_document`, with ids `<real>/explain?`, `<real>?x=1`, `<real>#frag`, `../locks/release_all`, `..`, `%2F`, `""`, `" "`, `a\nb`, 201 × `a`: either `isError: true` with a validation message, or (for a resolvable suffix) the effect on the real item only. Afterwards no item has `explain_requested` set and no lock row changed. A wrapper router records each replayed request's matched route template, and every one equals the tool's own template | running it on `v2026.10.21a` (doc with `<id>/explain?` sets `explain_requested`: red); then replacing one `api_path` call with `format!` |
| MG-11 | `api_path_encodes_every_segment` (`mcp.rs` tests) | `seg` encodes `/ ? # % space` and `..`, refuses empty, control and over-long | allowing `/` through |
| MG-12 | `server_sets_provenance` (`settings/tests.rs`) | a REST create by user `alice` with `requestedby: "agent:code"`, `createdby: "iter_engine:bakugo"` stores `requestedby: "user:alice"`, `createdby: "user:alice"` and returns a warning; MCP create by `alice` with `workid: "engine-fake-id"` stores the same; an engine-session create with `X-Iter-Workid` of an item in progress on that engine stores `agent:code@e1` and the parent link; the same header from a different engine, or for a queued item, stores `engine:e2` and no parent; a person naming an open item as `createdby` keeps it as parent, naming a closed or foreign id does not; a PUT changing `requestedby` keeps the stored one; the dedup "seen again by" row names the principal | running it on `v2026.10.21a` (values stored as sent: red); then removing the PUT copy-over |
| MG-12 | `principal_band` | an `engine:` item with no priority lands in the maintenance band, a `user:` item in the human band | restoring `!starts_with("agent")` |
| MG-13 | `reject_names_the_real_actor` | `alice` rejecting by MCP writes "rejected by user:alice: …" with `by: alice`; an agent session writes "rejected by agent:code@e1: …" with `by_workid`; the item is parked with `lasterror` | restoring the `item.agent` wording |
| MG-14 | Playwright `create_item_provenance` (`test/e2e/playwright`) | creating and cloning an item in the webui sends no `requestedby` or `createdby` (request intercepted) and the detail line shows `user:<login>` | re-adding the fields to `cloneBody` |
| MG-15 | `rag_status_lists_serving_engines` (`settings/tests.rs`) | engine `e1` with an active serves edge and a heartbeat now → `live: true`; `e2` serving with `last_seen` 10 min ago → listed, `live: false`; `e3` not serving → absent | running it on `v2026.10.21a` (`engines: []`: red) |
| MG-16 | `internal_errors_are_generic` | a test `Storage` whose `query` fails with `Backend("arango 400 (errorNum 17): AQL: secret /data/iter/x")` → response body has no `arango`, `AQL`, `errorNum` or `/data/`, has `error_id`, and the ring buffer holds a line with that id and the full detail; through MCP the tool error has no `/api/` path; tokens signed with alg `none`, a wrong key, or truncated → `{"error":"invalid or expired token","error_id":…}` with no `Invalid`, `HS256` or `Algorithm`; `rag_status.model` has no `dir` for a `user` and has it for an admin | passing `e` through at `api.rs:159` |
| MG-17 | `bearer_scheme_any_case_and_authz_still_runs` | `Bearer`, `bearer`, `BEARER`, `Bearer<TAB>`, `Bearer  ` (two spaces) + valid token → 200 on a member route; `Basic`, no scheme → 401; **`bearer` + valid token of a non-member → 403** (the authz layer ran); the same 403 for `BEARER` | changing only the extractor to the new parser and leaving `authz.rs:97` on `strip_prefix` (the non-member case returns 200: red) |
| MG-18 | `origin_guard_and_cors` | list empty: no `Origin` → 200, `Origin: https://evil.example` → 200 with `access-control-allow-origin: https://evil.example`; same-origin (`Origin` = scheme + `Host`) → 200; after `PUT /api/server/origins` with one entry: that entry → 200 with CORS headers and `vary: origin`, evil → 403 on `/mcp` (JSON-RPC body without `id`) and on `/api/projects/p/workitems`, `Origin: null` → 403, no `Origin` → 200, same-origin → 200; a preflight from the listed origin lists `mcp-protocol-version` and `x-iter-workid`; `PUT` by a `user` → 403; a malformed entry → 400 | making the list non-empty by default (the empty-list cases go red); skipping the same-origin step (the webui case goes red) |
| MG-19 | `protocol_version_and_batching` | no header + array → array answer; `2025-03-26` + array → array answer; `2025-06-18` + array → 400 `-32600`; `2025-11-25` (unsupported until added) → 400; `1999-01-01` → 400; `2025-06-18` single message → 200 | removing the version check |
| MG-20 | `hsts_on_tls_only` | with `tls_on: true` every response carries `strict-transport-security: max-age=31536000`; with `tls_on: false` none does | dropping the layer; and the live check of §5 step 8 through the public name |

The existing `every_tool_has_a_schema_and_unique_name` and `lockdirs_and_encoding` (`mcp.rs:774-790`)
stay. `lockdirs_and_encoding` moves to `seg`.

---

## 4. Build order (one agent, about three days)

1. **GraphRAG** (MG-3, MG-4, MG-15, the MG-16 parts in `rag/`): red tests first on the unfixed
   code, then `pipeline.rs`, `rag/mod.rs`, `settings::serving_engines`. This ships alone, first,
   because it unblocks agents.
2. **Paths** (MG-11): `seg`, `api_path`, resolved work ids, the hostile-id test.
3. **Listing** (MG-8 to MG-10): `wiquery.rs`, the query route, `workitem_list`, `brief`.
4. **Provenance** (MG-12 to MG-14): `principal`, the create/put/detail changes, the reject route,
   `whoami`, the workid header in `Mcp::call` and the CLI client, the webui.
5. **HTTP layer** (MG-16 to MG-20): `errid.rs`, `bearer`, `weblayers.rs`, the protocol version and
   batching in `handle`, the origins setting (record field, route, Settings box).
6. **Tool docs** (MG-1, MG-2, MG-5's wording, MG-6): `mcp_docs.rs` for all 34 tools, written last
   so they describe the final behaviour, then both guards.
7. **Docs** (MG-21).

Steps 2–5 are independent of each other and can be parallel agents, given separate files: step 4
and step 5 both touch `api.rs`, in different functions.

---

## 5. Rollout and migration

1. **Release.** Build and release per this repo's `CLAUDE.md`: `cargo build --release -p
   iter_engine -p iter_data`, `installers/build.sh`, `installers/build.sh --release`. iter_core is
   unchanged by this plan, but the engine changes (CLI reject route, workid header), so **both**
   binaries are redeployed. The GraphRAG part (build step 1) may go out as its own release first.
2. **No origin or proxy setup is needed.** With `allowed_origins` empty the server behaves as
   today for every caller, and the webui is always allowed as its own origin, whatever name it is
   served under.
3. **Start iter_data.** `rag::ensure_schema` runs the MG-4 repair once and logs the counts:
   chunks and docs with `vec_sum` removed, and failed uploads re-queued. On 2026-10-21 the
   re-queued uploads include the `mcp-test-20261021` note (`f3aa54129150e49d1b7261f2fc71ef63d`).
   The pdy-dev upload `f05da5730e92243589504811191bc4cfb` was deleted by the test (w4.py) and is
   not re-queued.
4. **Re-index pdy-dev first.** From a pdy-dev checkout on an engine machine:

   ```
   iter_engine cli rag sync --project pdy-dev --dry-run   # expect "N changed/new sent" with N ≈ 577
   iter_engine cli rag sync --project pdy-dev             # expect 0 failed
   bash test/iter_data/rag_node_count.sh pdy-dev          # expect N == M (M ≈ 626)
   ```

   577 is 626 map files minus the 49 already indexed, if `node_types` is empty. The script's own
   M is authoritative. The Summary agent then summarises about 30,000 new chunks over the following
   hours, at Haiku rates (Haiku is the small, cheap Claude model the Summary agent runs on by
   default). `rag_status.waiting` shows the backlog shrinking.
5. **Every other project.** Repeat step 4 for each project in `GET /api/projects`, one at a time,
   so the summary backlog of one does not starve another. Then create the change-sweep schedule
   where none exists (pdy-dev's `schedule` is `null`): `POST /api/projects/<p>/rag/schedule
   {"every_min": 60}`. This is a backstop; engines also re-index after each map sync.
6. **Agents.** No change is needed to agents' MCP configuration; `work.rs:1180` already sends
   `X-Iter-Workid`. MCP clients see the new tool descriptions on their next `tools/list`.
   `locks_list` now returns `{"locks": [...]}`; a script that read the bare array must read
   `.locks`. No such script exists in this repo (searched `iter_engine/src` and `test/`).
7. **Proxy.** Nothing to change. Caddy on the AWS host passes iter_data's `Strict-Transport-Security`
   header through; step 8 confirms it on the public name.
8. **Live checks**, each against `https://iter.api.paydaay.app` as `devlocal`, with control cases:
   - an upload with `wait_sec: 30` → `searchable: true`, and a keyword search finds its phrase;
     then delete it;
   - `workitem_list` paged to the end → the union of ids has 4,125+ distinct items and equals
     `GET …/workitems`;
   - `workitem_doc` with `workid: "<id>/explain?"` → `isError: true`, and that item's
     `explain_requested` is unchanged;
   - a create as `devlocal` with `requestedby: "agent:code"` → stored `user:devlocal`;
   - `rag_status.engines` lists `bakugo` and `midoriya` with `live: true`;
   - `Authorization: bearer …` → 200;
   - list empty: `Origin: https://evil.example` → 200; then, as an admin, set one allowed origin
     and repeat: evil → 403, the listed origin → 200, no `Origin` → 200, the webui still works;
     then clear the list;
   - `curl -sI` shows `strict-transport-security`.

---

## 6. Out of scope

- **Moving work-item filtering into ArangoDB (AQL)** for very large projects. In-memory filtering
  of 4,125 rows is what every id lookup already does.
- **Adding `2025-11-25` to `PROTOCOL_VERSIONS`.** It needs its lifecycle and tool changes checked
  first. Until then such a client is told 400, and per the specification it falls back to a version
  it shares.
- **SSE (Server-Sent Events) streams and MCP sessions.** `/mcp` stays stateless: `GET` and `DELETE`
  keep answering 405 (`mcp.rs:49-54`).
- **OAuth for MCP**: the protected-resource metadata of MCP 2025-06-18. Bearer tokens stay.
- **Restricting what a project member may read.** Members keep seeing every item (Stephen,
  2026-10-21).
- **Rewriting the provenance of existing work items.** Stored `requestedby` / `createdby` values
  written before this change are left as they are; only new writes are stamped.
- **Indexing `agentmem` and `interface` `*.iter.md` files.** They are not map nodes.

---

## 7. Notes for Stephen: where the code differed from the brief, and judgement calls

**Where the code or the saved evidence differs from the brief.**
- **The CORS layer is in `iter_data/src/main.rs:266-274`, not in `api.rs`.** It wraps the API, the
  MCP route and the webui fallback alike.
- **Node files are not indexed "only by hand".** The engine already re-indexes after any map sync
  that changed something (`maybe_rag`, `iter_engine/src/filesync.rs:766-790`, at most every 60 s).
  The 49-of-626 gap comes from the null-vector refusal, not from a missing schedule.
- **The 1,118 `*.iter.md` files are not all node files.** 421 are `agentmem` and 107 are
  `interface`; the map lists 626. MG-7 counts against the map.
- **`store_prepared` has four callers, not two**: uploads, node files, repo files, and the built-in
  guide at startup.
- **The `keepNull: false` on the chunk insert** (`pipeline.rs:178`) is why the code looks protected
  but is not. Per the ArangoDB documentation `keepNull` does not apply to a plain `INSERT`. The
  MG-3 red test confirms it.
- **Only `read1/2/3/5` outputs were saved.** The REST provenance request, the `"<id>/explain?"`
  call, the lowercase-bearer result, the CORS and HSTS headers, the JWT and Arango error texts, and
  the `rag_status` engine result are cited from the run's report. Their requests are in `sec1.py`,
  `sec2.py` and `w1–w4.py`; the REST provenance call is in none of the saved scripts. The §5 step 8
  checks re-measure each one.
- **Lowercase `bearer` is a security problem, not just a rejected request** (MG-17). The authz
  layer passes unverified requests on to the handler, so fixing the parser in one place only would
  have let non-members into projects.

**Judgement calls.**
- **`createdby` keeps its parent-link meaning.** The brief says set it from the token, but the
  close gate, deep dependencies and priority inheritance read it as the parent id. It becomes
  either a verified parent id or the principal. A person may still name an open item of the project
  as parent; an agent's parent is only its own verified item.
- **Agent identity is `agent:<agent>@<engine>`.** It is granted only when the workid header names
  an item in progress on that same engine; otherwise the principal is `engine:<engine>`.
- **Caller-supplied provenance is ignored with a warning, not refused.** The CLI, `sweep.rs` and
  `iter_core` still send the fields, and refusing would break every engine until all were
  redeployed.
- **Cursor only, no offset.** A cursor is stable while items are added; an offset is not. `total`
  is returned on every page.
- **Batches are refused only for clients that say `MCP-Protocol-Version: 2025-06-18` or later.** A
  client that sends no header is treated as 2025-03-26, as the specification says, and keeps
  batching.
- **Origins stay open by default; restricting is one optional admin setting.** Stephen ruled
  (2026-10-21) that a restriction must be an easy setting inside iter_data and must never get in
  the way of agents, and that the public name can change at any time. So: no flags, nothing tied to
  a name, the webui always allowed as its own origin, and an empty list meaning "allow all". This
  departs from the letter of MCP's "MUST validate Origin", on purpose: that rule defends servers
  that trust cookies or a local address, and iter_data trusts only an explicit bearer token.
- **HSTS is sent by iter_data alone, one year, no `includeSubDomains`.** The proxy passes it
  through, so there is no proxy step.
- **`rag_add_document` refuses `*.iter.md` uploads** rather than only documenting the rule.
- **`locks_list` changes its result to `{"locks": [...]}`** so every tool has `structuredContent`
  and an `outputSchema`.
