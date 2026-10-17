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
timestamps: {create: "2026-10-02 23:05:35Z", last_modified: "2026-10-16 23:00:00Z", last_tested: ""}
---
# installers — prebuilt binaries and their install scripts

## Summary

Everything a person needs to put iter on a machine without a Rust toolchain. Each folder under
`installers/<program>/<platform>/` is self-contained: download that folder (or run its install script
straight from raw.githubusercontent.com) and it installs one program on one kind of machine.

| folder | installs |
|---|---|
| `iter_data/docker/` | the all-in-one image (ArangoDB 3.12.12 + iter_data on :8400, Arango console on 127.0.0.1:8630) on Debian bookworm-slim |
| `iter_data/linux/` | ArangoDB + iter_data + backups as systemd services on Debian, Ubuntu, Arch or a WSL2 distro |
| `iter_data/linux_aws/` | the same on EC2 Amazon Linux 2023 (x86_64 or Graviton), with EBS and security-group guidance |
| `iter_data/aws/` | a CloudFormation stack (`iter_data.cfn.yaml`): an AL2023 instance whose first boot runs the release's `linux_aws/install.sh`, an encrypted EBS data volume, an Elastic IP, a port-8400-only security group and the admin password in SSM Parameter Store |
| `iter_engine/linux/` | the engine as the systemd service `iter-engine` |
| `iter_engine/linux_wsl/` | the same inside a WSL2 distro (systemd=true check, keepalive logon task) |
| `iter_engine/macos/` | the engine as the launchd agent `com.iter.engine` |
| `iter_engine/windows/` | the native Windows engine as the logon task `iter engine` (`install.cmd` / `install.ps1`) |
| `iter_engine/docker/` | the engine in a container, with `~/.iter` and the checkouts mounted at the host's own paths |

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
