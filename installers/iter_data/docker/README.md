# iter_data in Docker

One container with ArangoDB Community Edition 3.12 and iter_data. The API and webui are on `https://<host>:8400` (HTTPS with a self-signed certificate, see below), and the Arango web console is on `127.0.0.1:8630` (never published beyond localhost). Everything that persists is in one host folder, `~/.iter/iter_data` by default: the database, Arango's app folder and iter_data's own files.

The image is Debian bookworm-slim. It holds ArangoDB's static binaries, copied from the official `arangodb:3.12.12` image, the **prebuilt** `iter_data-linux-<arch>` binary (picked by Docker's `TARGETARCH`) and the embedding model. Nothing is compiled, so no Rust is needed.

## Prerequisites

Docker Engine or Docker Desktop with the `docker compose` plugin, `curl`, and 4 GB of RAM or more for the container (`--memory`, default `6g`).

## Install

From a copy of this folder:

```bash
./install.sh
```

Or with one line (the script fetches this folder's files and the binary from the release):

```bash
curl -fsSL https://raw.githubusercontent.com/Stephen-Hilton/iter/main/installers/iter_data/docker/install.sh | bash -s -- --version v2026.10.17
```

The script checks Docker and gets the binary: `--binary PATH`, else the one in this folder, else the GitHub Release, with its sha256 checked against `SHA256SUMS`. It fetches the embedding model (all-MiniLM-L6-v2, about 91 MB) into `models/`, writes the secrets, builds the image, starts the container, waits for `https://127.0.0.1:<port>/health` and prints the URL and the TLS certificate fingerprint.

| Option | Default | |
|---|---|---|
| `--version TAG` | latest release | release to download the binary from |
| `--binary PATH` | | use this `iter_data` linux binary |
| `--env-file PATH` | | read `ITER_ADMIN_PASSWORD`, `ITER_JWT_SECRET`, `ARANGO_ROOT_PASSWORD` from it |
| `--port N` | 8400 | host port for the API + webui |
| `--arango-port N` | 8630 | console port on 127.0.0.1; `0` leaves it unpublished |
| `--data-dir DIR` | `~/.iter/iter_data` | host folder for everything that persists |
| `--name NAME` | `iter5` | container, compose project and image name |
| `--model-dir DIR` | | copy an existing all-MiniLM-L6-v2 folder instead of downloading it |
| `--memory SIZE` | `6g` | container memory cap; ArangoDB is sized 1G below it |
| `--tls-cert PATH` `--tls-key PATH` | | serve this certificate (PEM) instead of the self-signed one |
| `--tls-name NAME` | | another DNS name or IP on the self-signed certificate (repeatable) |
| `--tls off\|auto` | `auto` | `off` = plain HTTP behind a TLS proxy; `auto` = self-signed again |
| `--dry-run` | | print every action, change nothing |

The secrets are kept in `~/.iter/<name>.docker.env` (mode 600). A fresh install without `--env-file` generates them and prints the admin password once. iter_data marks that admin `must_change_password`: the webui makes you choose your own password at the first sign-in (12 characters or more), and the API refuses everything else until then. iter_data then writes `admin_password_changed` in `<data-dir>/iter`, and the next `install.sh` run or `install.sh status` removes `ITER_ADMIN_PASSWORD` from the env file (and says so). Keep `ITER_JWT_SECRET`, because every engine and user token is signed with it.

To compile the binary yourself, clone the repo and run `cargo build --release -p iter_data` (or `installers/build.sh`).

## HTTPS and the certificate fingerprint

iter_data serves HTTPS on `:8400`. On its first start it makes a self-signed certificate, `tls.crt` + `tls.key` (mode 600), in `<data-dir>/iter` beside its secrets, and keeps it across reinstalls and upgrades. The engine-session signing secret, `engine_session.secret`, lives there too. The script makes the data folder owner-only.

After a successful install the script prints the certificate's **fingerprint** (`sha256:` + 64 hex: the SHA-256 of the certificate, the same as `openssl x509 -in tls.crt -noout -fingerprint -sha256` without the colons). Engines pin it. Give it to each engine installer (`--fingerprint sha256:…`, or `-Fingerprint` on Windows), or let the engine trust it at its first sign-in and compare it afterwards with `iter_engine key status`. It is also in the webui's 🔑 Engine keys screen, and in the container log: `docker logs iter5 2>&1 | grep fingerprint`.

Browsers warn about a self-signed certificate. Accept it once, or:

- `--tls-cert PATH --tls-key PATH`: serve a certificate of your own (a real one for your domain). The script copies both into `<data-dir>/custom-tls/` (key mode 600). The fingerprint changes, so engines that pinned the old one need `iter_engine key trust --data-url URL --fingerprint …` again, or `--ca` for a certificate from a public authority.
- `--tls off`: plain HTTP, only for iter_data behind a TLS proxy on this host (Caddy, nginx, a load balancer). Engines refuse `http://` to anything but 127.0.0.1 / localhost / ::1, so they must reach it through the proxy's `https://` URL, with `iter_engine key trust --ca` when the proxy has a real certificate.
- `--tls-name NAME` (repeatable): put another DNS name or IP on the self-signed certificate, such as the name browsers use. Names are read only when the certificate is first made; engines pin the fingerprint and do not check names.

The script remembers these choices in `~/.iter/<name>.docker.env` (`ITER_TLS=off`, `ITER_TLS_CERT`/`ITER_TLS_KEY`, `ITER_TLS_NAMES`), so a plain rerun keeps them. `--tls auto` goes back to the self-signed certificate. The container's health check uses `curl -fsSk https://127.0.0.1:8400/health` (and plain http when TLS is off).

## Status

`./install.sh status [--name NAME]`: the container and its health check, `https://127.0.0.1:<port>/health`, the TLS certificate fingerprint, and the first-boot admin password cleanup.

## Upgrade

Run `install.sh` again, with `--version` for a specific release. It replaces the binary, rebuilds the image and recreates the container. The data folder is kept, and with it the secrets and the TLS certificate, so the fingerprint engines pinned stays the same.

## Uninstall

```bash
ITER_DATA_DIR=~/.iter/iter_data docker compose -p iter5 -f compose.yml down --rmi local   # in this folder (or ~/.iter/iter5-docker)
```

The data folder (`~/.iter/iter_data`) and `~/.iter/iter5.docker.env` stay. Remove them yourself if you no longer need the data.
