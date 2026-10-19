---
id: a9040402-89d2-4e8e-8c61-04c88f0876b8
name: "installers — prebuilt binaries and their install scripts"
desc: "Installs iter_data or iter_engine on a machine from prebuilt binaries, never compiling: build.sh builds the seven release binaries (Linux x86_64 and aarch64 for both programs, macOS and Windows for the engine) and publishes them as a GitHub Release, and one self-contained folder per program and platform holds the install script that fetches the matching binary (from beside it or from the release, sha256-checked) and sets it up as a service."
creator: "iter migrate5"
teststate: inherit
level: component
owner: bespoke
children:
  codedirs:  ["{thisfiledir}/"]
  codenodes: []
  tests:     []
  reqs:      []
timestamps: {create: "2026-10-02 23:05:35Z", last_modified: "2026-10-19 23:01:38Z", last_tested: ""}
---
# installers — prebuilt binaries and their install scripts

## Summary

Everything a person needs to put iter on a machine without a Rust toolchain. Each folder under
`installers/<program>/<platform>/` is self-contained: download that folder (or run its install script
straight from raw.githubusercontent.com) and it installs one program on one kind of machine.

| folder | installs |
|---|---|
| `iter_data/docker/` | the all-in-one image (ArangoDB 3.12.12 + iter_data on https :8400, Arango console on 127.0.0.1:8630) on Debian bookworm-slim, or iter_data alone against an existing ArangoDB (`--arango-url`) |
| `iter_data/linux/` | ArangoDB + iter_data + per-database backups (and `restore`) as systemd services on Debian, Ubuntu, Arch or a WSL2 distro, or iter_data + backups against an existing ArangoDB (`--arango-url`) |
| `iter_data/linux_aws/` | the same on EC2 Amazon Linux 2023 (x86_64 or Graviton), with EBS and security-group guidance |
| `iter_data/aws/` | a CloudFormation stack (`iter_data.cfn.yaml`): an AL2023 instance whose first boot runs the release's `linux_aws/install.sh`, an encrypted EBS data volume, an Elastic IP, a port-8400-only security group, and the admin password and TLS certificate fingerprint in SSM Parameter Store |
| `iter_engine/linux/` | the engine as the systemd service `iter-engine` |
| `iter_engine/linux_wsl/` | the same inside a WSL2 distro (systemd=true check, keepalive logon task) |
| `iter_engine/macos/` | the engine as the launchd agent `com.iter.engine` |
| `iter_engine/windows/` | the native Windows engine as the logon task `iter engine` (`install.cmd` / `install.ps1`) |
| `iter_engine/docker/` | the engine in a container, with `<iter dir>/iter_engine` and the checkouts mounted at the host's own paths |

## How it works

`build.sh` builds the seven assets into `dist/` (git-ignored) with `SHA256SUMS` and copies each into the
folders that use it: `iter_data-linux-{x86_64,aarch64}`, `iter_engine-linux-{x86_64,aarch64}`,
`iter_engine-macos-{aarch64,x86_64}`, `iter_engine-windows-x86_64.exe`. Linux builds run in a
`rust:1-bullseye` container (glibc 2.31 baseline, so they run on Debian 11+, Ubuntu 20.04+ and Amazon
Linux 2023); Windows is cross-built with `cargo xwin` and a static C runtime. `build.sh --release` tags
`vYYYY.MM.DD[.N]` and publishes the assets as a GitHub Release; binaries are never committed.

An install script takes its binary from `--binary`, else from beside the script, else from the release
(`--version <tag>`, or the latest), checking the sha256 against the release's `SHA256SUMS`. Scripts are
idempotent, support `--dry-run`, and install a binary by writing a temp file and moving it into place.
`fetch_model.sh` downloads the all-MiniLM-L6-v2 embedding model iter_data needs for GraphRAG.

## What goes in and out

In: a release tag (or a local binary) and the machine's package manager, systemd, launchd, Task
Scheduler or Docker. Out: an installed, running `iter_data` or `iter_engine`. The engine installers also
make the machine's engine key and wait until iter_data accepts it.

iter_data serves HTTPS with a certificate signed by a local CA it makes on first start (`--tls auto`); the
iter_data installers wait on `https://127.0.0.1:<port>/health` (curl -k), print the certificate's
fingerprint (and `status` prints it again), pass `--tls-cert/--tls-key`, `--tls-name` and `--tls off`
through to iter_data, and keep `tls-ca.crt`/`tls-ca.key`, `tls.crt`/`tls.key` and `engine_session.secret`
beside `iter_data.secret` across reinstalls. The engine installers refuse a plain `http://` iter_data URL unless it is this machine,
pin the certificate with `iter_engine key trust --fingerprint` (from `--fingerprint`, or pasted) right
after installing the binary and before the key step, and check the pinned certificate the same way in
their own API calls (curl `--pinnedpubkey` after an openssl fingerprint check; a pinned .NET request on
Windows).

Every installer then adds iter_data's local CA (name-constrained to its names, local suffixes and private
addresses) to the trust stores of the machine it runs on, so a browser there shows the webui as secure: the
iter_data installers from `state/tls-ca.crt` (docker: copied out of the container), the engine installers from
`iter_engine key ca` over the pinned connection after the key step. Stores: Linux = the system store
(`update-ca-certificates` / `update-ca-trust`) + the browser NSS databases when a browser is there; macOS =
the login keychain (`security add-trusted-cert -r trustRoot -p ssl`); Windows = `Cert:\CurrentUser\Root`
(`LocalMachine\Root` elevated); WSL = the distro's stores + Windows' through `powershell.exe`; docker = the
host's; linux_aws = the system store only (headless). Skipped with `--tls off`, a custom certificate or
`--trust-ca`; failures warn, never stop the install.

ArangoDB: iter_data signs in as `iter_user` with `rw` on `iter_db` only. The installers generate and keep
that user's password, and keep root's password for iter_data to create the user and database on first
start. `--arango-url` (or `ARANGO_URL`) points them at an existing ArangoDB instead, with
`--arango-fingerprint` / `--arango-ca-cert` for its certificate and `--arango-db` / `--arango-user`.
linux and linux_aws then install only the Arango clients and drop the `arangodb3` unit and dependency;
docker skips arangod; aws passes them as stack parameters, with the passwords and CA in SSM. `iter-backup` dumps only that database, as iter's user, checking an
https certificate first; `install.sh restore FILE` loads it back.

The first-boot admin password must be changed at the first sign-in; once iter_data writes
`admin_password_changed`, the installers drop `ITER_ADMIN_PASSWORD` (linux/linux_aws: the
`iter-admin-cleanup.path` unit, plus the SSM copy on an aws stack; docker: the next run or `status`).
The engine installers take `--secrets-command`/`--secrets-every` (account tokens from a secret store),
keep them in `engine.conf`, pass them on the service's command line, and then ask for no tokens.
