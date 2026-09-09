# Capability: write an interface contract (`*.interface.iter.md`)

Read this before you create or edit any `*.interface.iter.md`. An interface node is
the LOGICAL data contract between code nodes; a GLOBAL object living under
`{interfaces}` (`globalinterfacedir`, exported as `$ITER_INTERFACE_DIR`, default
`{topdir}/interfaces/`). Code nodes point at it through their `children.inputs` /
`children.outputs` links; the interface file never points back.

## Interfaces are shared contracts — reuse before creating

Before adding a new interface, check the existing ids (every `*.interface.iter.md`
in the tree — the scan aggregates them globally) for a contract that already fits;
extend or reference it instead of creating a near-duplicate. Create a new id only
when no existing contract covers the need. NEW interface files are named
`<id>.interface.iter.md` and land in `$ITER_INTERFACE_DIR` — the scanner finds them
anywhere, but new ones belong there.

## ONE OPERATION PER FILE

An interface file is ONE operation, event, stream or record: `CreateMembership` is
one file, `DescribeMembership` is another. Never bundle a service's operations into
one contract behind an `"op"` / `"verb"` discriminator — `iter validate` flags that
as `multi-op`. Name the file `<service>-<operation>.interface.iter.md` (for example
`pdy-core-authority-create-membership.interface.iter.md`); the code node that serves
the operation links every one of its per-operation files in `children.outputs`.

A shared object — an attestation, an evidence reference, a refusal envelope, a
money amount — is defined ONCE as its own `kind: dataset` interface file and
referenced by id from every contract that carries it (`"attestation": <see
pdy-core-attestation>`). Never restate a shared object's fields in each operation.

## FIXED FORMAT — these sections and ONLY these

Enforced by `iter validate`. Get the current skeleton with

    "$ITER_BIN" validate --file <path> --template

— never write one from memory; an existing interface file's `kind:` steers which
skeleton you get.

- frontmatter: `name:` (the id); `kind:` — the interaction shape,
  `request-reply | event | stream | dataset`, never a transport or a syntax;
  `description:` (quoted prose); `children:` with the per-node defaults
  (`{thisfiledir}/{thisfilestem}/*.bizreq|techreq|testgroup.iter.md`)
- one `# <id> — contract` H1, then a named summary under 300 characters
- the kind's H2 sections: request-reply → `## Request`, `## Reply, success shape`,
  `## Reply, failure shape`; event → `## Event`; stream → `## Stream item`,
  `## Stream end`; dataset → `## Record`. Every field appears once with its type
  and meaning as an inline comment; a closed vocabulary or a limit is a comment
  beside the field it governs
- `## Reply, failure shape` lists EVERY refusal code the operation can return, one
  line each with what it means — the list is the closed vocabulary. No example per
  code
- closing every file: `## Worked examples` — exactly ONE success pair and ONE
  refusal pair in one strict-JSON fence. Per-test cases (each acceptance test's
  refusal, each edge) live in the testgroup scripts, never here; `iter validate`
  flags more than two as `too-many-examples`
- there is NO `## Invariants` section (retired 2026-09-08). Business rules — who may
  do what, what must be true afterwards, founder rulings — belong on bizreq/techreq
  nodes and are referenced by requirement id from there; the contract shows the
  data, not the law. `iter validate` flags the section as `invariants-section`
- optionally, and ONLY as the final section after `## Worked examples`:
  `## Exceptions` — a declared deviation from the internal transport law that
  service-to-service calls ride the mesh with mutual TLS and speak gRPC. State what
  deviates (e.g. a component that must speak an infrastructure wire protocol such
  as Redis's or Kafka's), why gRPC is impractical there, and what still holds (mesh
  transit, mTLS). Most contracts have no such section, and that is the normal case:
  no section, no exception

A one-operation contract with two examples is a few kilobytes. `iter validate`
flags a body over 10 KB as `oversize-body`: look for bundled operations, restated
shared objects, or business rules.

## Message shapes

Message shapes are pseudo-JSON with constraints as inline comments. JSON is the
NOTATION, not a wire format — conventions for what JSON cannot say: binary as
hex/base64 strings, 64-bit ints and money as strings or integer cents, timestamps as
ISO-8601 strings, enums as closed vocabularies named in comments. A function call is
logically request → reply, so a library surface is written as kwargs-object →
return-object messages. Tag a fence (```json) only when the block strictly parses;
pseudo-examples stay untagged.

Models: `e2e/.fixture/interfaces/ledger-command/ledger-command.interface.iter.md`
(request-reply) and `e2e/.fixture/interfaces/entry-recorded/` (event — note the
different fixed sections `kind:` demands).

## The two-clause test — the file is right when

1. a stranger could implement EITHER side from this file alone, and
2. nothing in it would change if a component were rebuilt in another language or
   deployed differently.

An interface is TRANSPORT-NEUTRAL: the same messages may ride an HTTP body, a gRPC
message, a queue record, argv/stdout, or an in-process struct, chained in any order,
without renaming a field or changing a rule. Carrier bindings — routes, ports,
topics, flags, exit codes, build and deploy facts — fail clause 2: record them on
the code node file of the object that serves that binding, never here.

## WHAT, never WHO or HOW

An interface is used by many code nodes; the file must not name providers,
consumers, or callers, nor describe how or where anyone uses it — the structure
graph already carries that through the code nodes' `children.inputs` /
`children.outputs` links. Sentences like "consumed by X" go stale the moment a
second consumer appears, and `iter validate` flags them.

## Copy the quotes

**Copy the quotes** on the prose fields (`name`, `description`, `endpoint`). Prose
routinely contains a colon-plus-space, and while the engine parses that fine
unquoted, strict-YAML tools reading the same file refuse the whole block. Bare
single-token values (`kind:`, `level:`, `owner:`, `teststate:`) stay unquoted.

## Check your work

    "$ITER_BIN" validate --project "$ITER_PROJECT" --file <the file> --fix

Exit 0 = clean; exit 1 = warn/error findings remain — fix them before finishing.
