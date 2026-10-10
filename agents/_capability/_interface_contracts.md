# Capability: interface contracts — RETIRED in iter5

Interface files (`*.interface.iter.md`), `ITER_INTERFACE_DIR`, `children.inputs` /
`children.outputs` and `iter validate --template` no longer exist. The iter4 → iter5
conversion replaced every interface with one **connection** node per kind of connection
(`level: connection` code nodes under `global/connections/`), whose `connects.from` /
`connects.to` list the parts that supply and use it.

Read the `_connections` capability instead (`"$ITER_BIN" capability _connections`).
The messages of one operation are now a requirement section in the supplying code
node's techreq file.
