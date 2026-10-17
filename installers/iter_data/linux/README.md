# iter_data on Linux (systemd): Debian, Ubuntu, Arch, WSL2

The script installs three things as systemd services:

- **ArangoDB CE 3.12** (`arangodb3`), listening on localhost only.
- **iter_data** (`iter-data`), serving the API and webui on `https://<host>:8400` (a self-signed certificate; see [HTTPS](#https-and-the-certificate-fingerprint)).
- **An online backup every 6 hours** (`iter-backup.timer`, using arangodump, with no downtime).

ArangoDB and iter_data share a memory cap (`iter.slice`). The script works on Debian 11+, Ubuntu 20.04+ and Arch on x86_64 or aarch64. It also works inside a WSL2 distro that has `[boot] systemd=true` in `/etc/wsl.conf`.

**Where ArangoDB comes from.** ArangoDB stopped publishing Community Edition packages after 3.12.4. Later 3.12 releases ship only as the official `arangodb` Docker image, and its binaries are static. The script downloads that image's layers straight from Docker Hub, checks each layer's sha256 and unpacks `arangod` and its clients. Docker does not need to be installed. The server then runs the same ArangoDB build as the docker install (`--arango-version`, default 3.12.12), so a database can move between the two.

Where things land:

| Path | What |
|---|---|
| `/opt/iter/bin/iter_data` | the program |
| `/opt/iter/models/` | the embedding model |
| `/etc/iter/data.env` | secrets (root:iter, mode 640) |
| `/var/lib/iter` | iter_data's files (user `iter`, mode 700): `iter_data.secret`, `engine_session.secret`, `tls.crt` + `tls.key` |
| `/etc/iter/tls.env` | the TLS choice (`--tls`, `--tls-cert`, `--tls-name`), kept for reruns |
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
| `--tls-cert PATH` `--tls-key PATH` | | serve this certificate (PEM) instead of the self-signed one |
| `--tls-name NAME` | | another DNS name or IP on the self-signed certificate (repeatable) |
| `--tls off\|auto` | `auto` | `off` = plain HTTP behind a TLS proxy; `auto` = self-signed again |
| `--dry-run` | | print every action, change nothing |

A fresh install without `--env-file` generates the secrets and prints the admin password once; you must change it at the first sign-in ([below](#the-first-boot-admin-password)). Keep `ITER_JWT_SECRET`, because every engine and user token is signed with it. A machine set up with the older `linux/install.sh server` keeps its secrets: the script reads the invoking user's `~/.iter/data.env` and copies `~/.iter/iter_data/{iter_data.secret,.env}`. It also keeps the database, which stays in `/var/lib/arangodb3`.

Other commands: `install.sh status` (services, health, the certificate fingerprint, the last backups), `install.sh backup` (back up now), `install.sh uninstall`.

The Arango console is never exposed. To use it, run `ssh -L 8529:127.0.0.1:8529 <host>` and open http://127.0.0.1:8529/.

Restore a backup:

```bash
mkdir /tmp/r && tar -xf iter5-<ts>.tar -C /tmp/r
arangorestore --server.password "$ARANGO_PASSWORD" --server.database iter5 --create-database true --include-system-collections true --input-directory /tmp/r/db
```

## The first-boot admin password

A fresh install generates the `admin` password, prints it once and keeps it as `ITER_ADMIN_PASSWORD` in `/etc/iter/data.env`. iter_data marks that admin `must_change_password`: the webui makes you choose your own password at the first sign-in (12 characters or more), and the API refuses everything else until then. At that moment iter_data writes `/var/lib/iter/admin_password_changed`, and `iter-admin-cleanup.path` starts `iter-admin-cleanup.service` (root, oneshot), which removes the `ITER_ADMIN_PASSWORD` line from `/etc/iter/data.env` (everything else and its `root:iter 640` stay). `install.sh status` and a reinstall run the same cleanup when the marker exists, and a reinstall never writes the line back.



iter_data serves HTTPS. On its first start it makes a self-signed certificate, `/var/lib/iter/tls.crt` + `tls.key` (user `iter`, mode 600), beside `iter_data.secret` and `engine_session.secret` (the engine-session signing secret). `/var/lib/iter` stays owner-only (700), and all of it is kept across reinstalls, upgrades and `uninstall`.

After a successful install, and in `install.sh status`, the script prints the certificate's **fingerprint** (`sha256:` + 64 hex, the SHA-256 of the certificate: `openssl x509 -in /var/lib/iter/tls.crt -noout -fingerprint -sha256` without the colons; the script installs openssl). Engines pin it. Give it to each engine installer (`--fingerprint sha256:…`, or `-Fingerprint` on Windows), or let the engine trust it at its first sign-in and compare it afterwards with `iter_engine key status`. It is also in the webui's 🔑 Engine keys screen and in `journalctl -u iter-data | grep fingerprint`.

Browsers warn about a self-signed certificate. Accept it once, or:

- `--tls-cert PATH --tls-key PATH`: serve a certificate of your own (a real one for your domain). The script copies both to `/var/lib/iter/custom-tls.{crt,key}` (user `iter`, key mode 600). The fingerprint changes, so engines that pinned the old one need `iter_engine key trust --data-url URL --fingerprint …` again, or `--ca` for a certificate from a public authority.
- `--tls off`: plain HTTP, only for iter_data behind a TLS proxy on this host (Caddy, nginx). Engines refuse `http://` to anything but 127.0.0.1 / localhost / ::1, so they must use the proxy's `https://` URL, with `iter_engine key trust --ca` when the proxy has a real certificate.
- `--tls-name NAME` (repeatable): put another DNS name or IP on the self-signed certificate, such as the name browsers use. Names are read only when the certificate is first made; engines pin the fingerprint and do not check names.

The flags go on the `iter-data` unit's `ExecStart`. The choice is kept in `/etc/iter/tls.env`, so a plain rerun keeps it; `--tls auto` goes back to the self-signed certificate.

## Upgrade

Rerun `install.sh`, with `--version` for a specific release. The script is idempotent: it replaces the binary (written to a temp name, then moved over), re-renders the units and restarts `iter-data`. Secrets, data and the TLS certificate are kept, so the fingerprint engines pinned stays the same.

## Uninstall

```bash
sudo ./install.sh uninstall
```

This stops and removes the services, `iter_data`, `arangod` and the clients. The database, `/var/lib/iter`, `/etc/iter` and the backups are kept, and the script prints their paths in case you want to delete them.
