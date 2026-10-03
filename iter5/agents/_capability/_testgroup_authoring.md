# Capability: testgroups — RETIRED in iter5

Tests files (`*.tests.iter.md`, `*.testgroup.iter.md`), the testgroup block, `testlist`
registration, group labels and `iter runtests --group` no longer exist. A **test node**
(`*.test.iter.md`) now lists its scripts in `children.tests`, and each script prints the
standard result JSON as its last stdout line.

Read the `_test_node_authoring` capability instead
(`"$ITER_BIN" capability _test_node_authoring`), and `_runtests` for running them.
