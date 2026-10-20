# iter

Two programs, one shared library (cargo workspace at the repo root; build output in the root `target/`):

| crate | builds | notes |
|---|---|---|
| `iter_engine` | binary | one per machine; also the `iter` CLI (`iter_engine cli …`); checkout tools in `src/local/` |
| `iter_data` | binary | the server; embeds `iter_data/webui/` and `iter_data/iter_engine_setup.sh` |
| `iter_core` | library | compiled into both binaries; see `iter_core/CLAUDE.md` |

Other folders: `installers/` (release binaries + one install script per program and platform), `test/`
(every test run from outside the crates), `docs/` (spec, guide, and `docs/iter_project_docs/`, iter's own
project graph: project node, use cases, connections, `map/`).

## Before deploying a rebuilt iter_engine or iter_data

Both binaries compile iter_core in, so a binary is only as current as the iter_core it was built from.
Before any deploy (`deploy.sh`, an installer, or copying a binary to another machine), check that the
latest iter_core is in it:

1. The checkout you build from has the latest iter_core: `git status iter_core/` is clean or holds the change
   you mean to ship, and the branch is pulled up to date.
2. Build with `cargo build --release -p iter_engine -p iter_data` (what `deploy.sh` runs) or
   `installers/build.sh`, not a bare `cargo build` and not one package alone.
3. The binary is newer than the last iter_core change. Compare the time of the last iter_core commit with
   the binary's modification time:
   `git log -1 --format=%cd --date=iso -- iter_core` and `ls -l target/release/iter_engine target/release/iter_data`.
   For uncommitted iter_core edits, rebuild first; cargo rebuilds whenever iter_core's source changed.
4. If iter_core changed since the other binary was last deployed, redeploy both, so the server and the
   engines never run different iter_core rules.

## Keep installers/ current

`installers/` is how people get iter: prebuilt binaries published as GitHub Release assets (never
committed) and one self-contained install script per program and platform. Installers never compile.

- **After changing iter_engine, iter_data or iter_core** in a way that should reach users: run
  `installers/build.sh` (all 7 assets: Linux x86_64/aarch64 for both programs in the `rust:1-bullseye`
  docker image, macOS aarch64/x86_64 and Windows x86_64 for the engine), check its summary (every Linux
  binary at glibc ≤ 2.31), commit, push, then `installers/build.sh --release` to tag
  `vYYYY.MM.DD<a-z>` (`v2026.10.20a`, then `b`, ...) and publish the release (the `/iter_release` skill does all of this). `--release` refuses a dirty tree or an unpushed HEAD.
- **When a change alters what an installed machine needs** (a new flag or env var, a port, a file under
  the iter dir, a package, a service setting, a new platform), update every affected install script and its
  README under `installers/<program>/<platform>/`, keep each folder self-contained (no folder sources
  another's files), and test with `bash -n`, `shellcheck` and `--dry-run` (Linux ones inside a container
  of the target distro: debian:bookworm, ubuntu:24.04, amazonlinux:2023).
- Everything an installer makes lives in one folder, the iter dir (`/opt/iter`, Windows `C:\iter`, AWS stack
  `/data/iter`, docker installers under Docker Desktop `~/iter`; `--iter-dir` / `-IterDir` / `ITER_DIR`, always ending in `iter`): `iter_data/` and
  `iter_engine/` (layout in `installers/README.md`). Outside it only OS-required links (systemd units,
  `/usr/local/bin/iter_engine`) and the plist / logon task. Never add files elsewhere; older installs
  (`~/.iter`, `/etc/iter`, `/var/lib/iter`, …) are moved in by the next run, never deleted.
- Services the installers create: engine = systemd `iter-engine` (Linux, WSL), launchd `com.iter.engine`
  (macOS), logon task `iter engine` (Windows), container `iter-engine` (docker); iter_data = systemd
  `arangodb3` + `iter-data` (Linux, AL2023, binary `<iter dir>/iter_data/bin/iter_data`, config
  `<iter dir>/iter_data/config/data.env`) or container `iter` (docker). `installers/iter_data/aws/iter_data.cfn.yaml` is a
  CloudFormation stack whose first boot runs the release's `linux_aws/install.sh`; keep its parameters in
  step with that script's options and lint it with `cfn-lint`.
- `installers/README.md` and `installers/installers.code.iter.md` describe the layout; update them when a
  folder is added or removed.

## Secrets and sign-in (do not weaken)

- Engines sign in only with their own Ed25519 key (`iter_engine key new|show|rotate`); never add a user-token
  path for engines. Engine sessions and people's tokens use separate signing secrets.
- iter_data serves HTTPS (`--tls auto` by default); engines pin its certificate fingerprint (`iter_engine/config/server_trust.json`,
  `iter_engine key trust`) and refuse plain `http://` except to this machine. Tests and the local dev loop use `--tls off`
  on 127.0.0.1 only.
- Browsers: `--tls auto` signs iter_data's certificate with a local CA of its own (`state/tls-ca.crt`), which the
  installers add to each machine's trust stores. Keep that CA name-constrained to the server's names, local
  suffixes and private addresses (iter_data/src/tls.rs): an unconstrained one in a trust store could vouch for any site.
- Account tokens: process environment > `--secrets-command` > `iter_engine/secrets/.env`. Never log a value, never write the
  secrets command's output to disk, and keep `envstore::scrub` on every child process the engine starts.
- People: the first admin and admin-set passwords must be changed at first sign-in (`must_change_password`);
  password changes need the current password and 12+ characters and end other sessions; sign-ins are rate-limited.
- ArangoDB: iter_data signs in as its own user (`iter_user`, rw on `iter_db` only), never as root; root's password only
  creates that user and database when missing, and the user never gets root's password. ArangoDB on another machine
  is HTTPS with a pinned fingerprint or CA (`ARANGO_CERT_FINGERPRINT` / `ARANGO_CA_CERT`); plain http only to this
  machine or with `ARANGO_ALLOW_PLAIN_HTTP` on a private network (iter_data/src/arango.rs).
