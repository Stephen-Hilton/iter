# installers

One folder per program and platform. Each folder works on its own: download just that folder (or
run its one-liner) and its install script does the rest. The script finds its binary in this order:
`--binary <path>`, then the matching asset sitting next to it, then a download from a GitHub Release
(sha256-checked against that release's `SHA256SUMS`).

| program | platform | folder | asset(s) | install |
|---|---|---|---|---|
| iter_data | Docker (any host) | [iter_data/docker/](iter_data/docker/README.md) | `iter_data-linux-x86_64`, `iter_data-linux-aarch64` | `./install.sh` |
| iter_data | Debian / Ubuntu / Arch, WSL2 | [iter_data/linux/](iter_data/linux/README.md) | `iter_data-linux-x86_64`, `iter_data-linux-aarch64` | `./install.sh` |
| iter_data | EC2 Amazon Linux 2023 | [iter_data/linux_aws/](iter_data/linux_aws/README.md) | `iter_data-linux-x86_64`, `iter_data-linux-aarch64` | `./install.sh` |
| iter_data | AWS, whole stack (CloudFormation) | [iter_data/aws/](iter_data/aws/README.md) | none: the instance runs `linux_aws/` from the release | `./install.sh --vpc … --subnet … --allowed-cidr …` |
| iter_engine | Docker (any host) | [iter_engine/docker/](iter_engine/docker/README.md) | `iter_engine-linux-x86_64`, `iter_engine-linux-aarch64` | `./install.sh` |
| iter_engine | Debian / Ubuntu / Arch | [iter_engine/linux/](iter_engine/linux/README.md) | `iter_engine-linux-x86_64`, `iter_engine-linux-aarch64` | `./install.sh` |
| iter_engine | Windows, inside WSL2 | [iter_engine/linux_wsl/](iter_engine/linux_wsl/README.md) | `iter_engine-linux-x86_64`, `iter_engine-linux-aarch64` | `./install.sh` |
| iter_engine | macOS (Apple silicon / Intel) | [iter_engine/macos/](iter_engine/macos/README.md) | `iter_engine-macos-aarch64`, `iter_engine-macos-x86_64` | `./install.sh` |
| iter_engine | Windows (native) | [iter_engine/windows/](iter_engine/windows/README.md) | `iter_engine-windows-x86_64.exe` | `install.cmd` |

Without a checkout, run a folder's script straight from GitHub (it downloads its binary from the release):

```sh
curl -fsSL https://raw.githubusercontent.com/Stephen-Hilton/iter/main/installers/iter_engine/linux/install.sh | bash -s -- --version v2026.10.17
```

Every script takes `--help` and `--dry-run` (print every action, change nothing) and can be run
again at any time. The folder READMEs above have the details for each platform.

Installed locations: engine `~/.iter/bin/iter_engine` (Windows `%USERPROFILE%\.iter\bin\iter_engine.exe`),
iter_data `/opt/iter/bin/iter_data`.

## Versions

A release is a git tag `v<YYYY>.<MM>.<DD>` (UTC build day); a second release the same day is
`v<YYYY>.<MM>.<DD>.2`, then `.3`, ... Each release carries the 7 binaries plus `SHA256SUMS`:

```
iter_data-linux-x86_64     iter_data-linux-aarch64
iter_engine-linux-x86_64   iter_engine-linux-aarch64
iter_engine-macos-aarch64  iter_engine-macos-x86_64
iter_engine-windows-x86_64.exe
```

Download URLs:

- latest: `https://github.com/Stephen-Hilton/iter/releases/latest/download/<asset>`
- a specific version: `https://github.com/Stephen-Hilton/iter/releases/download/<tag>/<asset>`

Install scripts take the latest release unless given `--version <tag>` (or `ITER_VERSION=<tag>`).
List releases at https://github.com/Stephen-Hilton/iter/releases or with `gh release list -R Stephen-Hilton/iter`.

Linux binaries are built against glibc 2.31 (Debian 11 bullseye), so they run on Debian 11+,
Ubuntu 20.04+, Amazon Linux 2023 and current Arch.

## Rebuilding

`installers/build.sh` (on an Apple-silicon Mac with Docker running) builds every asset into
`installers/dist/`, writes `dist/SHA256SUMS`, and copies each asset into the folders that use it:

```sh
installers/build.sh                               # all 7 assets
installers/build.sh --only 'iter_engine-macos-*'  # a subset (glob, repeatable)
installers/build.sh --skip-windows
installers/build.sh --dry-run                     # show the plan only
installers/build.sh --release                     # build all, tag HEAD, push the tag, gh release create
```

`--release` refuses when the working tree is dirty or HEAD is not on origin. How each asset is built:

| asset | toolchain |
|---|---|
| `iter_engine-macos-aarch64` | native `cargo build --release -p iter_engine -p iter_data` |
| `iter_engine-macos-x86_64` | `cargo build --release --target x86_64-apple-darwin -p iter_engine` |
| `iter_*-linux-aarch64` | `rust:1-bullseye` container (linux/arm64), docker volumes `iter-build-cargo` + `iter-build-target` |
| `iter_*-linux-x86_64` | same container, cross-compiled with `gcc-x86-64-linux-gnu` |
| `iter_engine-windows-x86_64.exe` | `cargo xwin build --target x86_64-pc-windows-msvc` (`cargo install --locked cargo-xwin`, `brew install llvm`) |

Binaries are never committed: `dist/` and the copies inside the folders are git-ignored; the only
published copies are GitHub Release assets.

`fetch_model.sh` downloads and checksum-verifies the embedding model (all-MiniLM-L6-v2, ~91 MB)
that iter_data and iter_engine use.
