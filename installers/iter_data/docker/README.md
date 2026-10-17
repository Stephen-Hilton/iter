# iter_data in Docker

One container with ArangoDB Community Edition 3.12 and iter_data. The API and webui are on `:8400`, and the Arango web console is on `127.0.0.1:8630` (never published beyond localhost). Everything that persists is in one host folder, `~/.iter/iter_data` by default: the database, Arango's app folder and iter_data's own files.

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

The script checks Docker and gets the binary: `--binary PATH`, else the one in this folder, else the GitHub Release, with its sha256 checked against `SHA256SUMS`. It fetches the embedding model (all-MiniLM-L6-v2, about 91 MB) into `models/`, writes the secrets, builds the image, starts the container, waits for `/health` and prints the URL.

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
| `--dry-run` | | print every action, change nothing |

The secrets are kept in `~/.iter/<name>.docker.env` (mode 600). A fresh install without `--env-file` generates them and prints the admin password once. Keep `ITER_JWT_SECRET`, because every engine and user token is signed with it.

To compile the binary yourself, clone the repo and run `cargo build --release -p iter_data` (or `installers/build.sh`).

## Upgrade

Run `install.sh` again, with `--version` for a specific release. It replaces the binary, rebuilds the image and recreates the container. The data folder is kept.

## Uninstall

```bash
docker compose -p iter5 -f compose.yml down --rmi local   # in this folder (or ~/.iter/iter5-docker)
```

The data folder (`~/.iter/iter_data`) and `~/.iter/iter5.docker.env` stay. Remove them yourself if you no longer need the data.
