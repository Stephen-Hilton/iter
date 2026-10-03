---
id: 5cae72b3-ef35-4433-ba16-20e625aea203
name: "Developer"
desc: "A person who works on a project through iter5: designs it in the Project graph and builds it, edits nodes in the graph or node files in the repo, files and steers work items in the Work queue, answers agents' questions, runs tests and reads results, and searches the project with GraphRAG. Works in the web page, in their editor, and with the iter command line in a checkout."
creator: "iter migrate5"
teststate: inherit
drives: ["{topdir}/global/usecases/design_a_project_and_build_it.usecase.iter.md", "{topdir}/global/usecases/edit_a_node_in_the_graph.usecase.iter.md", "{topdir}/global/usecases/edit_a_node_file_in_the_repo.usecase.iter.md", "{topdir}/global/usecases/run_tests_and_see_the_results.usecase.iter.md", "{topdir}/global/usecases/a_red_test_becomes_one_work_item.usecase.iter.md"]
touches: ["{topdir}/webui/webui.code.iter.md", "{topdir}/webui/intro.code.iter.md", "{topdir}/webui/queue.code.iter.md", "{topdir}/webui/projectgraph.code.iter.md", "{topdir}/webui/grapheditor.code.iter.md", "{topdir}/webui/rag.code.iter.md", "{topdir}/iter_engine/src/cli.code.iter.md"]
children:
  codedirs:  []
  codenodes: []
  tests:     []
  reqs:      []
timestamps: {create: "2026-10-02 23:05:35Z", last_modified: "2026-10-02 23:12:30Z", last_tested: ""}
---
# Developer

A person (user role `user`, or `viewer` for read-only) with a `member` edge
to the projects they work on; the project's creator gets one automatically.

Where they touch iter5:

- **Web page** — Intro (the new-project wizard), Work queue (file, answer,
  approve, reopen), Project graph (design, edit nodes and edges, Build, Run
  tests, conflicts), GraphRAG (search, upload documents).
- **Their editor and git** — node files are ordinary files; an edit shows up
  in the graph within a few seconds.
- **The `iter` command line** in a checkout — `validate`, `sync`,
  `runtests`, `sweep`, `init`, `migrate5`.
