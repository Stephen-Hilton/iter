---
id: 573cb889-e15d-448c-90ce-bbcb1b12239b
name: "Node-file checker"
description: "Checks every `*.iter.md` file against the file rules (name, required frontmatter keys, `children` keys, interface format, ids and the node-text standard), reports each finding and applies the safe mechanical fixes on request, so that the map and the agents read well-formed files."
simple_description: "Proof-reads the project's description files and points out anything missing or malformed."
level: component
owner: bespoke
teststate: inherit
children:
  codedirs:   ["{topdir}/iter_local/src/validate.rs"]
  codenodes:  []
  inputs:     []
  outputs:    []
  bizreqs:    []
  techreqs:   []
  tests:      []
---

# Long Description

The Node-file checker is `iter validate`. It reads node files and says, file by file, what breaks the rules.

How it works (`iter_local/src/validate.rs`): `run` validates one file or every `*.iter.md` under the scan folders and returns a `Report` of `Finding`s, each with a code, a message and a `Severity` (error, warn or info). `validate_file` knows each file's role from its name (the dot rule, `markers::role_of`) and checks what that role needs. For example: frontmatter present and terminated; required keys (`missing-key`, `bad-level`, `bad-owner`, `bad-teststate`); only known `children` sub-keys (`unknown-child-key`); an id that exists and is a UUID (`missing-id`, `malformed-id`); for interface files, one operation per file with a success and a refusal JSON example (`multi-op`, `too-many-examples`); and for code nodes the node-text standard (`node_text_findings`): an action-first `description` (`description-not-action`, see `description_is_action`), a `simple_description`, and a Long Description of real substance (`thin-long-description`). Findings split into two kinds. Mechanical fixes whose result is certain (a `...` closing fence, junk before the opening fence, an unquoted value containing `: `) are applied with `--fix`; everything that needs judgement is only reported. `template_for` prints the empty template for a file's role (`iter validate --file x --template`).

The iter command line runs it (`iter validate`, exit 1 on any error or warning), and the Map uploader and test sweep files one `ingest` work item per code node that fails the same `node_text_findings` rules.

Why it matters: the scanner, the map and agent prompts all trust these files. A missing key or a label-only description means a broken map or a confused agent, and the checker catches it before it lands.

Example: a new code node whose description reads "The billing parts: invoices, refunds, credits" gets `description-not-action`, telling the author to say what the part does.
