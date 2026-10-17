# iter_data on Linux (systemd): Debian, Ubuntu, Arch, WSL2

The script installs three things as systemd services:

- **ArangoDB CE 3.12** (`arangodb3`), listening on localhost only.
- **iter_data** (`iter-data`), serving the API and webui on `:8400`.
- **An online backup every 6 hours** (`iter-backup.timer`, using arangodump, with no downtime).

ArangoDB and iter_data share a memory cap (`iter.slice`). The script works on Debian 11+, Ubuntu 20.04+ and Arch on x86_64 or aarch64. It also works inside a WSL2 distro that has `[boot] systemd=true` in `/etc/wsl.conf`.

**Where ArangoDB comes from.** ArangoDB stopped publishing Community Edition packages after 3.12.4. Later 3.12 releases ship only as the official `arangodb` Docker image, and its binaries are static. The script downloads that image's layers straight from Docker Hub, checks each layer's sha256 and unpacks `arangod` and its clients. Docker does not need to be installed. The server then runs the same ArangoDB build as the docker install (`--arango-version`, default 3.12.12), so a database can move between the two.

Where things land:

| Path | What |
|---|---|
| `/opt/iter/bin/iter_data` | the program |
| `/opt/iter/models/` | the embedding model |
| `/etc/iter/data.env` | secrets (root:iter, mode 640) |
| `/var/lib/iter` | iter_data's files (user `iter`) |
| `/var/lib/arangodb3` | the database (`--db-dir`, user `arangodb`) |
| `/var/backups/iter` | backups (`--backup-dir`) |

Logs: `journalctl -u iter-data -u arangodb3`.

## Prerequisites

systemd, and root or sudo. The script installs any missing tools (`curl`, `jq`, `tar`, `procps`, …) with apt or pacman. The machine needs 4 GB of RAM or more.

## Install

From a copy of this folder (or a repo checkout after `installers/build.sh`):

```bash
sudo ./install.sh
```

Or with one line (the script fetches this folder's files and the binary from the release):

```bash
curl -fsSL https://raw.githubusercontent.com/Stephen-Hilton/iter/main/installers/iter_data/linux/install.sh | bash -s -- --version v2026.10.17
```

The binary is taken from `--binary PATH`, else from beside the script, else from the GitHub Release (with its sha256 checked against `SHA256SUMS`). Nothing is compiled. To compile it yourself, clone the repo and run `cargo build --release -p iter_data` (or `installers/build.sh`).

| Option | Default | |
|---|---|---|
| `--version TAG` | latest release | release to download from |
| `--binary PATH` | | use this `iter_data` binary |
| `--env-file PATH` | | read `ITER_ADMIN_PASSWORD`, `ITER_JWT_SECRET`, `ARANGO_ROOT_PASSWORD` from it |
| `--port N` | 8400 | iter_data port |
| `--db-dir DIR` | `/var/lib/arangodb3` | ArangoDB data directory |
| `--backup-dir DIR` | `/var/backups/iter` | backup folder |
| `--memory MiB` | min(6144, 85% of RAM) | cap for ArangoDB + iter_data together |
| `--arango-version V` | 3.12.12 | ArangoDB CE version |
| `--dry-run` | | print every action, change nothing |

A fresh install without `--env-file` generates the secrets and prints the admin password once. Keep `ITER_JWT_SECRET`, because every engine and user token is signed with it. A machine set up with the older `linux/install.sh server` keeps its secrets: the script reads the invoking user's `~/.iter/data.env` and copies `~/.iter/iter_data/{iter_data.secret,.env}`. It also keeps the database, which stays in `/var/lib/arangodb3`.

Other commands: `install.sh status`, `install.sh backup` (back up now), `install.sh uninstall`.

The Arango console is never exposed. To use it, run `ssh -L 8529:127.0.0.1:8529 <host>` and open http://127.0.0.1:8529/.

Restore a backup:

```bash
mkdir /tmp/r && tar -xf iter5-<ts>.tar -C /tmp/r
arangorestore --server.password "$ARANGO_PASSWORD" --server.database iter5 --create-database true --include-system-collections true --input-directory /tmp/r/db
```

## Upgrade

Rerun `install.sh`, with `--version` for a specific release. The script is idempotent: it replaces the binary (written to a temp name, then moved over), re-renders the units and restarts `iter-data`. Secrets and data are kept.

## Uninstall

```bash
sudo ./install.sh uninstall
```

This stops and removes the services, `iter_data`, `arangod` and the clients. The database, `/var/lib/iter`, `/etc/iter` and the backups are kept, and the script prints their paths in case you want to delete them.
