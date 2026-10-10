---
id: f9719834-9b88-43b8-9c3c-7f03c299177e
name: "iter_rag unit tests"
desc: "Runs cargo test -p iter_rag — extraction of html, docx and pptx text with headings kept, chapter and chunk splitting within the size budget, frontmatter splitting, and (when the all-MiniLM-L6-v2 weights are on disk) that similar sentences embed closer than unrelated ones — and reports one standard result line counted per test. Needs only cargo."
creator: "agent.code"
teststate: inherit
children:
  codedirs:  []
  codenodes: []
  tests:     ["{thisfiledir}/cargo_test.sh"]
  reqs:      []
timestamps: {create: "2026-10-02 23:15:00Z", last_modified: "2026-10-02 23:15:00Z", last_tested: ""}
---

# iter_rag — unit tests

One script, `cargo_test.sh`, runs the crate's whole `cargo test` through `tools/cargo-test-crate.sh iter_rag` and prints the standard result JSON (spec §3.5) as its last line: every test in the `normal` bucket, one `details` row per test. Exit 0 green, 1 red, 2 could not run.

The embedding test needs the model weights (`models/all-MiniLM-L6-v2`, `$ITER_EMBED_MODEL` or `~/.cache/iter/models`); without them it prints "skipped" and passes, so a machine without the weights still runs the rest.
