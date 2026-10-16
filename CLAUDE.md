# iter

Two programs, one shared library (cargo workspace at the repo root):

| crate | builds | notes |
|---|---|---|
| `iter_engine` | binary | one per machine; also the `iter` CLI (`iter_engine cli …`); checkout tools in `src/local/` |
| `iter_data` | binary | the server; embeds `webui/` and `tools/iter_engine_setup.sh` |
| `iter_core` | library | compiled into both binaries; see `iter_core/CLAUDE.md` |

## Before deploying a rebuilt iter_engine or iter_data

Both binaries compile iter_core in, so a binary is only as current as the iter_core it was built from.
Before any deploy (`deploy.sh`, the docker image, `linux/install.sh`, or copying a binary to another
machine), check that the latest iter_core is in it:

1. The checkout you build from has the latest iter_core: `git status iter_core/` is clean or holds the change
   you mean to ship, and the branch is pulled up to date.
2. Build with `cargo build --release -p iter_engine -p iter_data` (what `deploy.sh` runs), not a bare
   `cargo build` and not one package alone.
3. The binary is newer than the last iter_core change. Compare the time of the last iter_core commit with
   the binary's modification time:
   `git log -1 --format=%cd --date=iso -- iter_core` and `ls -l target/release/iter_engine target/release/iter_data`.
   For uncommitted iter_core edits, rebuild first; cargo rebuilds whenever iter_core's source changed.
4. If iter_core changed since the other binary was last deployed, redeploy both, so the server and the
   engines never run different iter_core rules.
