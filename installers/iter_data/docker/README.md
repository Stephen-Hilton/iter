# iter_data in Docker

One container with ArangoDB Community Edition 3.12 and iter_data. The API and webui are on `https://<host>:8400` (HTTPS with a certificate signed by iter_data's own local CA, see below), and the Arango web console is on `127.0.0.1:8630` (never published beyond localhost). Everything the install makes is in one host folder, `<iter dir>/iter_data` (`/opt/iter/iter_data` by default, `~/iter/iter_data` under Docker Desktop), mounted into the container at `/var/lib/iter_data`:

| Path | What |
|---|---|
| `docker/` | the build folder: `Dockerfile`, `compose.yml`, `compose.console.yml`, `entrypoint.sh`, the binary, the embedding model |
| `config/data.env` | the secrets (mode 600): admin password, JWT secret, the `ARANGO_*` and TLS settings |
| `config/custom-tls.crt` `.key`, `config/arango-ca.crt` | a certificate of your own, an ArangoDB CA (when given) |
| `state/` | iter_data's own files: `iter_data.secret`, `engine_session.secret`, `tls.crt` + `tls.key` |
| `arango/data`, `arango/apps` | the database and Arango's app folder |
| `backups/` | where the backup commands below write |

The folder is owner-only (700). On a Linux host the container (root) owns what it writes in `state/` and `arango/`.

You can also point the container at an ArangoDB you already run (another app's, or one in AWS): then it runs iter_data only. See [Using an existing ArangoDB](#using-an-existing-arangodb).

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

The script picks the iter dir: `--iter-dir DIR` (also `--iter_dir`, or `ITER_DIR`), else where this container's files already are, else `/opt/iter` (`~/iter` under Docker Desktop, below). It is forgiving about the ending: `/my/dir/iter/`, `/my/dir/iter` and `/my/dir` all mean `/my/dir/iter`. It creates `/opt/iter` with sudo when it must (root-owned, 755) and `iter_data/` under it as yours. Given an iter dir other than the one the container uses, it stops the container and moves the folder there.

**Docker Desktop** (macOS, Windows, and Docker Desktop for Linux) runs containers in a VM that sees only the folders shared with it (Settings → Resources → File sharing: the home folder is, `/opt` is not). So under Docker Desktop the default iter dir is `~/iter`; with Docker Engine on Linux it is `/opt/iter`. The script tells them apart by `docker info`. An `--iter-dir` Docker Desktop cannot see is caught before anything starts: the script checks the mount and says to share that folder or pick one under your home.

The script checks Docker and gets the binary: `--binary PATH`, else the one in this folder, else the GitHub Release, with its sha256 checked against `SHA256SUMS`. It copies this folder's files into `docker/`, fetches the embedding model (all-MiniLM-L6-v2, about 91 MB) into `docker/models/`, writes the secrets, builds the image, starts the container, waits for `https://127.0.0.1:<port>/health` and prints the URL and the TLS certificate fingerprint.

| Option | Default | |
|---|---|---|
| `--iter-dir DIR` | `/opt/iter`; Docker Desktop `~/iter` (later runs: where it is) | the folder that holds everything; iter_data's files go in `<iter dir>/iter_data` |
| `--version TAG` | latest release | release to download the binary from |
| `--binary PATH` | | use this `iter_data` linux binary |
| `--env-file PATH` | | read `ITER_ADMIN_PASSWORD`, `ITER_JWT_SECRET` and the `ARANGO_*` settings from it |
| `--port N` | 8400 | host port for the API + webui |
| `--arango-port N` | 8630 | console port on 127.0.0.1; `0` leaves it unpublished |
| `--data-dir DIR` | the container's mount, else `~/.iter/iter_data` | an older install's data folder, moved in once (see [below](#moving-an-install-from-before-2026-10-18)) |
| `--adopt-volumes` | | copy a hand-made setup's docker volumes into the iter dir (see below) |
| `--name NAME` | `iter` | container, compose project and image name; another name keeps its files in `<iter dir>/iter_data_<name>` |
| `--model-dir DIR` | | copy an existing all-MiniLM-L6-v2 folder instead of downloading it |
| `--memory SIZE` | `6g` | container memory cap; ArangoDB is sized 1G below it |
| `--tls-cert PATH` `--tls-key PATH` | | serve this certificate (PEM) instead of iter_data's own |
| `--tls-name NAME` | | another DNS name or IP on iter_data's own certificate (repeatable) |
| `--tls off\|auto` | `auto` | `off` = plain HTTP behind a TLS proxy; `auto` = iter_data's own certificate again |
| `--arango-url URL` | `local` | use an existing ArangoDB at `URL` instead of the container's own (also read from `ARANGO_URL`); `local` switches back |
| `--arango-fingerprint FP` | | pin that ArangoDB's certificate, `sha256:<64 hex>` (comma-separated during a rotation) |
| `--arango-ca-cert PATH` | | trust this CA (PEM) for that ArangoDB's certificate; copied to `config/arango-ca.crt` |
| `--arango-db NAME` | `iter_db` | iter's database (an existing install keeps its own) |
| `--arango-user NAME` | `iter_user` | the Arango user iter_data signs in as |
| `--dry-run` | | print every action, change nothing |

The secrets are kept in `config/data.env` (mode 600). A fresh install without `--env-file` generates them and prints the admin password once. iter_data marks that admin `must_change_password`: the webui makes you choose your own password at the first sign-in (12 characters or more), and the API refuses everything else until then. iter_data then writes `admin_password_changed` in `state/`, and the next `install.sh` run or `install.sh status` removes `ITER_ADMIN_PASSWORD` from the env file (and says so). Keep `ITER_JWT_SECRET`, because every engine and user token is signed with it.

### Moving an install from before 2026-10-18

Older installs kept their files in `~/.iter`: the data folder `~/.iter/iter_data` (`arango/`, `arango-apps/`, `iter/`, `custom-tls/`, `arango-ca/`), the secrets `~/.iter/<name>.docker.env` and, piped from curl, the build folder `~/.iter/<name>-docker`. The next `install.sh` run stops the container and moves them in with `mv`: `arango/` → `arango/data`, `arango-apps/` → `arango/apps`, `iter/` → `state/`, the certificate files → `config/`, the secrets → `config/data.env` (with the container paths in it rewritten). The old secrets file, the old build folder and anything else that was in the data folder go to `<iter dir>/iter_data/old-dot-iter/`, kept rather than deleted. Delete that folder once all is well. Nothing else changes: same database, same secrets, same certificate. `--data-dir` names the old data folder when it was somewhere else. On a Linux host the container's root-owned folders move with sudo.

A container made by hand (not by this script) that keeps its database in **named docker volumes** is not moved. The script stops and says so, because it would otherwise start an empty database beside the old one. `--adopt-volumes` (with `--env-file` giving that setup's `ARANGO_ROOT_PASSWORD`) copies the volumes into `arango/data`, `arango/apps` and `state/`. The volumes themselves are kept as they are.

To compile the binary yourself, clone the repo and run `cargo build --release -p iter_data` (or `installers/build.sh`).

## HTTPS and the certificate fingerprint

iter_data serves HTTPS on `:8400`. On its first start it makes a local certificate authority (`tls-ca.crt` + `tls-ca.key`) and a certificate signed by it (`tls.crt` + `tls.key`, keys mode 600) in `state/` beside its secrets, and keeps it across reinstalls and upgrades. The engine-session signing secret, `engine_session.secret`, lives there too. The script makes the data folder owner-only.

After a successful install the script prints the certificate's **fingerprint** (`sha256:` + 64 hex: the SHA-256 of the certificate, the same as `openssl x509 -in tls.crt -noout -fingerprint -sha256` without the colons). Engines pin it. Give it to each engine installer (`--fingerprint sha256:…`, or `-Fingerprint` on Windows), or let the engine trust it at its first sign-in and compare it afterwards with `iter_engine key status`. It is also in the webui's 🔑 Engine keys screen, and in the container log: `docker logs iter 2>&1 | grep fingerprint`.

**Browsers.** The CA is name-constrained to this server's names (with every `--tls-name`), `.local`/`.lan`/`.internal`/`.home.arpa` and loopback/private addresses, so it cannot vouch for any public site. iter_data serves it, without sign-in, at `https://<host>:<port>/tls/ca.crt`. The script copies it out of the container and trusts it on this host: on macOS in the login keychain as a root for SSL (macOS asks for your password; Firefox reads it while `security.enterprise_roots.enabled` is on, the default), on Linux in the system store and the desktop user's NSS databases (Chrome, Chromium, Firefox). The webui then opens as secure here after a browser restart. On another machine the engine installer does the same; without an engine there, open `https://<this host>:8400/tls/ca.crt` and trust it as a certificate authority. Other choices:

- `--tls-cert PATH --tls-key PATH`: serve a certificate of your own (a real one for your domain). The script copies both to `config/custom-tls.crt` and `config/custom-tls.key` (key mode 600). The fingerprint changes, so engines that pinned the old one need `iter_engine key trust --data-url URL --fingerprint …` again, or `--ca` for a certificate from a public authority.
- `--tls off`: plain HTTP, only for iter_data behind a TLS proxy on this host (Caddy, nginx, a load balancer). Engines refuse `http://` to anything but 127.0.0.1 / localhost / ::1, so they must reach it through the proxy's `https://` URL, with `iter_engine key trust --ca` when the proxy has a real certificate.
- `--tls-name NAME` (repeatable): put another DNS name or IP on iter_data's own certificate (and its CA's name constraints), such as the name browsers use. Names are read only when the certificate is first made; engines pin the fingerprint and do not check names.

The script remembers these choices in `config/data.env` (`ITER_TLS=off`, `ITER_TLS_CERT`/`ITER_TLS_KEY`, `ITER_TLS_NAMES`), so a plain rerun keeps them. `--tls auto` goes back to iter_data's own certificate. The container's health check uses `curl -fsSk https://127.0.0.1:8400/health` (and plain http when TLS is off).

## ArangoDB: iter's own user and database

iter_data does not sign in to ArangoDB as root. It uses a user of its own, `iter_user` (`ARANGO_USER`), which holds `rw` (Administrate) on one database, `iter_db` (`ARANGO_DB`), and nothing else, so other apps' databases on the same server stay out of its reach. That user creates every collection and index iter needs, including the GraphRAG vector indexes once a project grows large enough.

On a start where `iter_user` cannot sign in (it does not exist yet, the database is missing, or it has too little access), iter_data uses `ARANGO_ROOT_PASSWORD` once: root creates the user (with `ARANGO_PASSWORD`) and the database, and grants `rw`. Then iter_data signs in as the user again. A user that already exists keeps its password; when `ARANGO_PASSWORD` is wrong for it, iter_data stops and says so. `ARANGO_PASSWORD` is never root's password, and the script refuses one that is.

- **The container's own ArangoDB** (the default): the script keeps the root password and the user's password in `config/data.env`, and iter_data sets everything up on its first start. An install from before 2026-10-17 kept its data in a database named `iter5`. When `ARANGO_DB` is not set, `iter5` exists and `iter_db` does not, the container keeps using `iter5` and logs one line saying so.
- **An existing ArangoDB**: either give `ARANGO_ROOT_PASSWORD` for the first start, or have that server's admin create the user and the database and grant `rw` beforehand. Then `ARANGO_PASSWORD` alone is enough.

## Using an existing ArangoDB

```bash
ARANGO_PASSWORD=… ARANGO_ROOT_PASSWORD=… ./install.sh --arango-url https://arango.example.com:8529 \
  --arango-fingerprint sha256:<64 hex>
```

With `--arango-url` (or `ARANGO_URL` in the environment or `--env-file`), the container does not start arangod and does not publish the Arango console. The settings are kept in `config/data.env`, so a plain rerun keeps them, and `--arango-url local` goes back to the container's own ArangoDB. The container's own root and user passwords are kept aside in that file meanwhile (`ITER_LOCAL_ARANGO_*`).

iter_data reaches an ArangoDB on another machine over HTTPS only:

- **A self-signed certificate**: pin it with `--arango-fingerprint sha256:…` (`ARANGO_CERT_FINGERPRINT`). Get the value from the server's certificate with `openssl x509 -in cert.pem -noout -fingerprint -sha256`; the colon form is accepted. A pinned certificate needs no CA and no matching name. When the certificate changes, give both the old and the new fingerprint, comma-separated, until the switch is done.
- **A certificate from your own CA**: `--arango-ca-cert ca.pem` (`ARANGO_CA_CERT`). The name in the certificate must match the URL's host.
- **A certificate from a public CA** (for example one behind a load balancer): no option needed.
- **Plain `http://`** to another machine is refused unless `ARANGO_ALLOW_PLAIN_HTTP=1` is set, for a private network only. That includes `http://host.docker.internal:8529` (an ArangoDB on the Docker host).

To make ArangoDB itself serve HTTPS, give arangod an `ssl://` endpoint and a PEM file holding the certificate and its key:

```bash
openssl req -x509 -newkey ec -pkeyopt ec_paramgen_curve:prime256v1 -nodes -days 825 \
  -subj /CN=arango -addext "subjectAltName=DNS:arango.example.com" -keyout key.pem -out cert.pem
cat cert.pem key.pem > server.pem      # arangod --server.endpoint ssl://0.0.0.0:8529 --ssl.keyfile server.pem
openssl x509 -in cert.pem -noout -fingerprint -sha256    # -> --arango-fingerprint
```

## Backup and restore (iter's database only)

`arangodump` and `arangorestore` work on one database at a time, so iter's data can be saved and loaded without touching anything else on the server, as `iter_user`:

```bash
# backup (online: iter_data keeps serving); the dump lands in <iter dir>/iter_data/backups on the host
docker exec iter bash -c 'arangodump --server.endpoint tcp://127.0.0.1:8529 \
  --server.username "$ARANGO_USER" --server.password "$ARANGO_PASSWORD" --server.database "${ARANGO_DB:-iter_db}" \
  --include-system-collections false --output-directory /var/lib/iter_data/backups/backup-$(date +%F)'

# restore into the same database (overwrites iter's collections; best with no engines connected)
docker exec iter bash -c 'arangorestore --server.endpoint tcp://127.0.0.1:8529 \
  --server.username "$ARANGO_USER" --server.password "$ARANGO_PASSWORD" --server.database "${ARANGO_DB:-iter_db}" \
  --include-system-collections false --overwrite true --input-directory /var/lib/iter_data/backups/backup-YYYY-MM-DD'
docker restart iter
```

An install still on the older `iter` database (see above) has no `ARANGO_DB` set: name `iter` in place of `${ARANGO_DB:-iter_db}`. For an external ArangoDB, run the same commands with that server's endpoint (`ssl://host:8529` for HTTPS) from any machine with the Arango client tools. Leave the system collections out: `iter_user` may not restore Arango's own (`_queues` and the like), and iter does not need them, because iter_data recreates its named graph (`_graphs`) at every start. To restore into a database that does not exist yet, create it first (start iter_data once, or as root with `--create-database true`).

## Status

`./install.sh status [--name NAME]`: where its files are, the container and its health check, `https://127.0.0.1:<port>/health`, the TLS certificate fingerprint, and the first-boot admin password cleanup.

## Upgrade

Run `install.sh` again, with `--version` for a specific release (no `--iter-dir` needed: it finds the container's folder). It replaces the binary, rebuilds the image and recreates the container. The folder is kept, and with it the secrets and the TLS certificate, so the fingerprint engines pinned stays the same.

## Uninstall

```bash
cd /opt/iter/iter_data/docker
ITER_DATA_DIR=/opt/iter/iter_data ITER_ENV_PATH=/opt/iter/iter_data/config/data.env docker compose -p iter -f compose.yml down --rmi local
```

`/opt/iter/iter_data` stays, with the database and the secrets. Remove it yourself if you no longer need the data.
