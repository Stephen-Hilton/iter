# iter_data in Docker

One container with ArangoDB Community Edition 3.12 and iter_data. The API and webui are on `https://<host>:8400` (HTTPS with a self-signed certificate, see below), and the Arango web console is on `127.0.0.1:8630` (never published beyond localhost). Everything that persists is in one host folder, `~/.iter/iter_data` by default: the database, Arango's app folder and iter_data's own files.

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

The script checks Docker and gets the binary: `--binary PATH`, else the one in this folder, else the GitHub Release, with its sha256 checked against `SHA256SUMS`. It fetches the embedding model (all-MiniLM-L6-v2, about 91 MB) into `models/`, writes the secrets, builds the image, starts the container, waits for `https://127.0.0.1:<port>/health` and prints the URL and the TLS certificate fingerprint.

| Option | Default | |
|---|---|---|
| `--version TAG` | latest release | release to download the binary from |
| `--binary PATH` | | use this `iter_data` linux binary |
| `--env-file PATH` | | read `ITER_ADMIN_PASSWORD`, `ITER_JWT_SECRET` and the `ARANGO_*` settings from it |
| `--port N` | 8400 | host port for the API + webui |
| `--arango-port N` | 8630 | console port on 127.0.0.1; `0` leaves it unpublished |
| `--data-dir DIR` | `~/.iter/iter_data` | host folder for everything that persists |
| `--name NAME` | `iter5` | container, compose project and image name |
| `--model-dir DIR` | | copy an existing all-MiniLM-L6-v2 folder instead of downloading it |
| `--memory SIZE` | `6g` | container memory cap; ArangoDB is sized 1G below it |
| `--tls-cert PATH` `--tls-key PATH` | | serve this certificate (PEM) instead of the self-signed one |
| `--tls-name NAME` | | another DNS name or IP on the self-signed certificate (repeatable) |
| `--tls off\|auto` | `auto` | `off` = plain HTTP behind a TLS proxy; `auto` = self-signed again |
| `--arango-url URL` | `local` | use an existing ArangoDB at `URL` instead of the container's own (also read from `ARANGO_URL`); `local` switches back |
| `--arango-fingerprint FP` | | pin that ArangoDB's certificate, `sha256:<64 hex>` (comma-separated during a rotation) |
| `--arango-ca-cert PATH` | | trust this CA (PEM) for that ArangoDB's certificate; copied into `<data-dir>/arango-ca/` |
| `--arango-db NAME` | `iter_db` | iter's database (an existing install keeps its own) |
| `--arango-user NAME` | `iter_user` | the Arango user iter_data signs in as |
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

## ArangoDB: iter's own user and database

iter_data does not sign in to ArangoDB as root. It uses a user of its own, `iter_user` (`ARANGO_USER`), which holds `rw` (Administrate) on one database, `iter_db` (`ARANGO_DB`), and nothing else, so other apps' databases on the same server stay out of its reach. That user creates every collection and index iter needs, including the GraphRAG vector indexes once a project grows large enough.

On a start where `iter_user` cannot sign in (it does not exist yet, the database is missing, or it has too little access), iter_data uses `ARANGO_ROOT_PASSWORD` once: root creates the user (with `ARANGO_PASSWORD`) and the database, and grants `rw`. Then iter_data signs in as the user again. A user that already exists keeps its password; when `ARANGO_PASSWORD` is wrong for it, iter_data stops and says so. `ARANGO_PASSWORD` is never root's password, and the script refuses one that is.

- **The container's own ArangoDB** (the default): the script keeps the root password and the user's password in `~/.iter/<name>.docker.env`, and iter_data sets everything up on its first start. An install from before 2026-10-17 kept its data in a database named `iter5`. When `ARANGO_DB` is not set, `iter5` exists and `iter_db` does not, the container keeps using `iter5` and logs one line saying so.
- **An existing ArangoDB**: either give `ARANGO_ROOT_PASSWORD` for the first start, or have that server's admin create the user and the database and grant `rw` beforehand. Then `ARANGO_PASSWORD` alone is enough.

## Using an existing ArangoDB

```bash
ARANGO_PASSWORD=… ARANGO_ROOT_PASSWORD=… ./install.sh --arango-url https://arango.example.com:8529 \
  --arango-fingerprint sha256:<64 hex>
```

With `--arango-url` (or `ARANGO_URL` in the environment or `--env-file`), the container does not start arangod and does not publish the Arango console. The settings are kept in `~/.iter/<name>.docker.env`, so a plain rerun keeps them, and `--arango-url local` goes back to the container's own ArangoDB. The container's own root and user passwords are kept aside in that file meanwhile (`ITER_LOCAL_ARANGO_*`).

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
# backup (online: iter_data keeps serving); the dump lands in the data folder on the host
docker exec iter5 bash -c 'arangodump --server.endpoint tcp://127.0.0.1:8529 \
  --server.username "$ARANGO_USER" --server.password "$ARANGO_PASSWORD" --server.database "${ARANGO_DB:-iter_db}" \
  --include-system-collections false --output-directory /var/lib/iter_data/backup-$(date +%F)'

# restore into the same database (overwrites iter's collections; best with no engines connected)
docker exec iter5 bash -c 'arangorestore --server.endpoint tcp://127.0.0.1:8529 \
  --server.username "$ARANGO_USER" --server.password "$ARANGO_PASSWORD" --server.database "${ARANGO_DB:-iter_db}" \
  --include-system-collections false --overwrite true --input-directory /var/lib/iter_data/backup-YYYY-MM-DD'
docker restart iter5
```

An install still on the older `iter5` database (see above) has no `ARANGO_DB` set: name `iter5` in place of `${ARANGO_DB:-iter_db}`. For an external ArangoDB, run the same commands with that server's endpoint (`ssl://host:8529` for HTTPS) from any machine with the Arango client tools. Leave the system collections out: `iter_user` may not restore Arango's own (`_queues` and the like), and iter does not need them, because iter_data recreates its named graph (`_graphs`) at every start. To restore into a database that does not exist yet, create it first (start iter_data once, or as root with `--create-database true`).

## Status

`./install.sh status [--name NAME]`: the container and its health check, `https://127.0.0.1:<port>/health`, the TLS certificate fingerprint, and the first-boot admin password cleanup.

## Upgrade

Run `install.sh` again, with `--version` for a specific release. It replaces the binary, rebuilds the image and recreates the container. The data folder is kept, and with it the secrets and the TLS certificate, so the fingerprint engines pinned stays the same.

## Uninstall

```bash
ITER_DATA_DIR=~/.iter/iter_data docker compose -p iter5 -f compose.yml down --rmi local   # in this folder (or ~/.iter/iter5-docker)
```

The data folder (`~/.iter/iter_data`) and `~/.iter/iter5.docker.env` stay. Remove them yourself if you no longer need the data.
