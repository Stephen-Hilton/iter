# iter_data on Linux (systemd): Debian, Ubuntu, Arch, WSL2

The script installs three things as systemd services:

- **ArangoDB CE 3.12** (`arangodb3`), listening on localhost only.
- **iter_data** (`iter-data`), serving the API and webui on `https://<host>:8400` (a certificate signed by iter_data's own local CA; see [HTTPS](#https-and-the-certificate-fingerprint)).
- **An online backup every 6 hours** (`iter-backup.timer`, using arangodump, with no downtime).

ArangoDB and iter_data share a memory cap (`iter.slice`). The script works on Debian 11+, Ubuntu 20.04+ and Arch on x86_64 or aarch64. It also works inside a WSL2 distro that has `[boot] systemd=true` in `/etc/wsl.conf`.

**Where ArangoDB comes from.** ArangoDB stopped publishing Community Edition packages after 3.12.4. Later 3.12 releases ship only as the official `arangodb` Docker image, and its binaries are static. The script downloads that image's layers straight from Docker Hub, checks each layer's sha256 and unpacks `arangod` and its clients. Docker does not need to be installed. The server then runs the same ArangoDB build as the docker install (`--arango-version`, default 3.12.12), so a database can move between the two.

## Where it installs: one folder

Everything goes in one folder, the **iter dir**: `/opt/iter` unless you give `--iter-dir DIR` (or `ITER_DIR`). The folder always ends in `iter`, so `--iter-dir /data`, `--iter-dir /data/iter` and `--iter-dir /data/iter/` all mean `/data/iter`. iter_data's part of it is `<iter dir>/iter_data` (below, `D`). An engine on the same machine goes beside it, in `<iter dir>/iter_engine`.

| Path | What |
|---|---|
| `D/bin/` | `iter_data`, `iter-backup`, `iter-admin-cleanup` |
| `D/config/` | `data.env` (secrets, root:iter, mode 640), `tls.env` (the TLS choice, kept for reruns), `backup.env`, `install.env` (where the database is) |
| `D/state/` | iter_data's own files (user `iter`, mode 700): `iter_data.secret`, `engine_session.secret`, `tls.crt` + `tls.key` |
| `D/models/` | the embedding model |
| `D/backups/` | backups (`--backup-dir` puts them elsewhere, e.g. another disk) |
| `D/systemd/` | the units |
| `D/arango/` | ArangoDB: `bin/` (arangod and its clients), `share/`, `config/` (`arangod.conf`, `root.env`), `data/` (the database, user `arangodb`; `--db-dir` puts it elsewhere), `apps/` |

Only links live outside the folder: each unit in `/etc/systemd/system` and `/etc/sysctl.d/60-arangodb.conf` point into it. When the iter dir is on another filesystem than `/`, the script copies those files instead, because systemd reads its units before other disks are mounted. Later runs (`status`, `backup`, `uninstall`, an upgrade) find the iter dir from the `iter-data` unit, so `--iter-dir` is only needed the first time. A rerun with another `--iter-dir` moves the whole folder there.

**An install from before 2026-10-18** (files in `/opt/iter/bin`, `/etc/iter`, `/var/lib/iter`, `/var/lib/arangodb3`, `/usr/sbin/arangod`, `/var/backups/iter`) is moved into the iter dir by the next `install.sh` run (or `install.sh migrate`), with the services stopped meanwhile. Files are moved, never deleted. The database and the backups move only from their default places and only within one filesystem (a rename). Otherwise they stay where they are, and the script records that. The certificate, and so the fingerprint engines pinned, stays the same.

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
| `--iter-dir DIR` | `/opt/iter` (a rerun: where it is installed) | the one folder everything goes in; always ends in `iter` |
| `--version TAG` | latest release | release to download from |
| `--binary PATH` | | use this `iter_data` binary |
| `--env-file PATH` | | read `ITER_ADMIN_PASSWORD`, `ITER_JWT_SECRET` and the `ARANGO_*` settings ([below](#arangodb-iters-own-user-and-database-or-an-arangodb-you-already-run)) from it |
| `--port N` | 8400 | iter_data port |
| `--db-dir DIR` | `D/arango/data` | ArangoDB data directory, e.g. on a data disk |
| `--backup-dir DIR` | `D/backups` | backup folder, e.g. on another disk |
| `--memory MiB` | min(6144, 85% of RAM) | cap for ArangoDB + iter_data together |
| `--arango-version V` | 3.12.12 | ArangoDB CE version |
| `--arango-url URL` | local | use this ArangoDB instead of installing one (`https://` to another machine); `local` = install here |
| `--arango-fingerprint FP` | | pin that ArangoDB's (self-signed) certificate, `sha256:…` |
| `--arango-ca-cert PATH` | | or trust the CA (PEM) that signed it |
| `--arango-db NAME` | `iter_db` | iter's database (an existing install keeps its own) |
| `--arango-user NAME` | `iter_user` | iter's ArangoDB user, `rw` on that database only |
| `--tls-cert PATH` `--tls-key PATH` | | serve this certificate (PEM) instead of iter_data's own |
| `--tls-name NAME` | | another DNS name or IP on iter_data's own certificate (repeatable) |
| `--tls off\|auto` | `auto` | `off` = plain HTTP behind a TLS proxy; `auto` = iter_data's own certificate again |
| `--dry-run` | | print every action, change nothing |

A fresh install without `--env-file` generates the secrets and prints the admin password once; you must change it at the first sign-in ([below](#the-first-boot-admin-password)). Keep `ITER_JWT_SECRET`, because every engine and user token is signed with it. A machine set up with the older `linux/install.sh server` keeps its secrets: the script reads the invoking user's `~/.iter/data.env` and copies `~/.iter/iter_data/{iter_data.secret,.env}`. It also keeps the database.

Other commands: `install.sh status` (services, the iter dir, health, the certificate fingerprint, the ArangoDB in use, the last backups), `install.sh backup` (back up now), `install.sh restore FILE` ([below](#backups-iters-database-only)), `install.sh uninstall`.

The local Arango console is never exposed. To use it, run `ssh -L 8529:127.0.0.1:8529 <host>` and open http://127.0.0.1:8529/. The Arango shell is in `D/arango/bin` and needs its data folder named: `ICU_DATA=D/arango/share/arangodb3 ICU_DATA_LEGACY=D/arango/share/arangodb3 D/arango/bin/arangosh -c D/arango/config/arangosh.conf`.

## ArangoDB: iter's own user and database, or an ArangoDB you already run

iter_data signs in to ArangoDB as **its own user** (`ARANGO_USER`, default `iter_user`) and keeps everything in **its own database** (`ARANGO_DB`, default `iter_db`). That user holds `rw` (Administrate) on that one database and nothing else, so the same ArangoDB server can hold other apps' databases too. iter_data creates its collections and indexes itself, at every start and as projects grow (GraphRAG's vector indexes once a project has 1,024 chunks; arangod must run with `--vector-index`, which the local install does).

On its first start iter_data signs in as that user. When it can't (the user or the database doesn't exist yet, or the user has too little access), it signs in as `root` with `ARANGO_ROOT_PASSWORD`. As root it creates the user (with `ARANGO_PASSWORD`, never root's own password) and the database, and grants the user `rw` on it. Then it tries the user again. An existing user keeps its password: a wrong `ARANGO_PASSWORD` is reported, never reset. With the local ArangoDB the script generates both passwords and keeps them in `D/config/data.env`.

Use `--arango-db` to choose another database.

**An ArangoDB you already run.** Give `--arango-url` (or `ARANGO_URL` in the environment or in `--env-file`). The script then installs neither arangod nor its service, only the Arango client tools for the backups, and `iter-data` no longer waits for a local `arangodb3`. A local ArangoDB from an earlier install is stopped and its data is kept; `--arango-url local` goes back to it. Give either:

- `ARANGO_PASSWORD`, for a user the ArangoDB admin already made with `rw` on the database, or
- `ARANGO_ROOT_PASSWORD`, so iter_data makes them itself.

Both come from the environment or `--env-file`.

**HTTPS to it.** iter_data refuses plain `http://` to another machine. Use `https://` and either:

- `--arango-fingerprint sha256:…`, which pins a **self-signed** certificate: only that exact certificate is accepted, whatever its name or issuer. Get the value with `openssl x509 -in server.crt -noout -fingerprint -sha256`; the colons and case don't matter. Several fingerprints, separated by commas, cover a certificate change.
- `--arango-ca-cert PATH`, for a certificate signed by a CA of your own. The CA is copied to `D/config/arango-ca.crt`, and the name in the URL must match the certificate.
- neither, for a certificate from a public CA.

`ARANGO_ALLOW_PLAIN_HTTP=1` allows `http://` on a private network only. To serve ArangoDB over HTTPS, put the certificate and its key in one PEM file and start arangod with `--server.endpoint ssl://0.0.0.0:8529 --ssl.keyfile /path/server.pem`. For a self-signed certificate:

```bash
openssl req -x509 -newkey ec -pkeyopt ec_paramgen_curve:prime256v1 -nodes -days 825 -subj /CN=arango \
  -addext "subjectAltName=DNS:arango.example.com" -keyout key.pem -out cert.pem
cat cert.pem key.pem > server.pem
openssl x509 -in cert.pem -noout -fingerprint -sha256    # for --arango-fingerprint
```

Everything chosen here is kept in `D/config/data.env`: `ARANGO_URL`, `ARANGO_USER`, `ARANGO_PASSWORD`, `ARANGO_DB`, `ARANGO_ROOT_PASSWORD` when given, `ARANGO_CERT_FINGERPRINT`, `ARANGO_CA_CERT` and `ARANGO_ALLOW_PLAIN_HTTP`. A plain rerun keeps it.

## Backups: iter's database only

`iter-backup` dumps only iter's database (`ARANGO_DB`), as iter's own user. Other databases on a shared ArangoDB are never touched. Arango's own system collections are left out; iter_data recreates what it needs from them (its named graph) at start. Each tar also holds `D/config/data.env` and iter_data's secret and certificate files. Over HTTPS the script first checks the server's certificate against `ARANGO_CERT_FINGERPRINT` or `ARANGO_CA_CERT`, because arangodump does not check it.

To load a backup back into iter's database:

```bash
sudo ./install.sh restore /opt/iter/iter_data/backups/iter-<ts>.tar
```

This stops `iter-data`, replaces the collections the backup holds (`arangorestore --overwrite true` as iter's user), and starts `iter-data` again. The env and secret files in the tar are not touched; they are there for a move to a new machine.

## An engine on the same machine

`iter_data engine-key add --public-key iter_… --engine NAME` registers an engine's key straight in the database, as an admin would in the Engine keys screen. Run it as the `iter` user with `--env-file D/config/data.env`; it starts no server. The Linux engine installer (`installers/iter_engine/linux`) runs it by itself when its iter_data URL is this machine, and also pins iter_data's certificate from `D/state`. See its README.

## The first-boot admin password

A fresh install generates the `admin` password, prints it once and keeps it as `ITER_ADMIN_PASSWORD` in `D/config/data.env` (read it again with `sudo sed -n 's/^ITER_ADMIN_PASSWORD=//p' /opt/iter/iter_data/config/data.env`; an engine installed here afterwards prints it at its end). iter_data marks that admin `must_change_password`: the webui makes you choose your own password at the first sign-in (12 characters or more), and the API refuses everything else until then. At that moment iter_data writes `D/state/admin_password_changed`, and `iter-admin-cleanup.path` starts `iter-admin-cleanup.service` (root, oneshot), which removes the `ITER_ADMIN_PASSWORD` line from `D/config/data.env` (everything else and its `root:iter 640` stay). `install.sh status` and a reinstall run the same cleanup when the marker exists, and a reinstall never writes the line back.



iter_data serves HTTPS. On its first start it makes a local certificate authority, `D/state/tls-ca.crt` + `tls-ca.key`, and a certificate signed by it, `D/state/tls.crt` + `tls.key` (user `iter`, keys mode 600), beside `iter_data.secret` and `engine_session.secret` (the engine-session signing secret). `D/state` stays owner-only (700), and all of it is kept across reinstalls, upgrades and `uninstall`.

After a successful install, and in `install.sh status`, the script prints the certificate's **fingerprint** (`sha256:` + 64 hex, the SHA-256 of the certificate: `openssl x509 -in D/state/tls.crt -noout -fingerprint -sha256` without the colons; the script installs openssl). Engines pin it. Give it to each engine installer (`--fingerprint sha256:…`, or `-Fingerprint` on Windows), or let the engine trust it at its first sign-in and compare it afterwards with `iter_engine key status`. It is also in the webui's 🔑 Engine keys screen and in `journalctl -u iter-data | grep fingerprint`.

**Browsers.** The CA is name-constrained to this server's names (with every `--tls-name`), `.local`/`.lan`/`.internal`/`.home.arpa` and loopback/private addresses, so it cannot vouch for any public site. iter_data serves it, without sign-in, at `https://<host>:<port>/tls/ca.crt`. The script adds it to this machine's system store and, for the desktop user who ran sudo, to the NSS databases Chrome, Chromium and Firefox read, so the webui opens as secure here (after a browser restart). On another machine the engine installer does the same; without an engine there, open `https://<this host>:8400/tls/ca.crt` and trust it as a certificate authority. Other choices:

- `--tls-cert PATH --tls-key PATH`: serve a certificate of your own (a real one for your domain). The script copies both to `D/state/custom-tls.{crt,key}` (user `iter`, key mode 600). The fingerprint changes, so engines that pinned the old one need `iter_engine key trust --data-url URL --fingerprint …` again, or `--ca` for a certificate from a public authority.
- `--tls off`: plain HTTP, only for iter_data behind a TLS proxy on this host (Caddy, nginx). Engines refuse `http://` to anything but 127.0.0.1 / localhost / ::1, so they must use the proxy's `https://` URL, with `iter_engine key trust --ca` when the proxy has a real certificate.
- `--tls-name NAME` (repeatable): put another DNS name or IP on iter_data's own certificate (and its CA's name constraints), such as the name browsers use. Names are read only when the certificate is first made; engines pin the fingerprint and do not check names.

The flags go on the `iter-data` unit's `ExecStart`. The choice is kept in `D/config/tls.env`, so a plain rerun keeps it; `--tls auto` goes back to iter_data's own certificate.

## Upgrade

Rerun `install.sh`, with `--version` for a specific release, or run `installers/upgrade_iter.sh`, which also backs up first and upgrades an engine on the same machine. The script is idempotent: it replaces the binary (written to a temp name, then moved over), re-renders the units and restarts `iter-data`. Secrets, data and the TLS certificate are kept, so the fingerprint engines pinned stays the same. The port and a `--memory` given earlier are kept too (`D/config/install.env`).

## Uninstall

```bash
sudo ./install.sh uninstall
```

This stops and removes the services and their links, `iter_data`, `arangod` and the clients. `D/config`, `D/state`, the database and the backups are kept. To remove iter_data completely afterwards, delete `D` (and the database or backup folder if you put them elsewhere).
