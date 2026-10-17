# iter_data on EC2: Amazon Linux 2023 (systemd)

The script installs three things as systemd services:

- **ArangoDB CE 3.12** (`arangodb3`), listening on localhost only.
- **iter_data** (`iter-data`), serving the API and webui on `https://<host>:8400` (a self-signed certificate; see [HTTPS](#https-and-the-certificate-fingerprint)).
- **An online backup every 6 hours** (`iter-backup.timer`, using arangodump, with no downtime).

ArangoDB and iter_data share a memory cap (`iter.slice`). The script is for Amazon Linux 2023 on x86_64 or Graviton (aarch64), and the CPU is detected for you. Amazon Linux 2 is not supported (its glibc is too old). For Debian, Ubuntu or Arch use `../linux`.

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

Amazon Linux 2023, and `ec2-user` (sudo) or root. The script installs any missing tools (`curl`, `jq`, `tar`, `procps`, …) with dnf (the stock `curl-minimal` is enough). The machine needs 4 GB of RAM or more.

## Install

From a copy of this folder (or a repo checkout after `installers/build.sh`):

```bash
sudo ./install.sh
```

Or with one line (the script fetches this folder's files and the binary from the release):

```bash
curl -fsSL https://raw.githubusercontent.com/Stephen-Hilton/iter/main/installers/iter_data/linux_aws/install.sh | bash -s -- --version v2026.10.17
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
| `--tls-name NAME` | | another DNS name or IP on the self-signed certificate (repeatable); e.g. the public DNS name or Elastic IP |
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

A fresh install generates the `admin` password, prints it once and keeps it as `ITER_ADMIN_PASSWORD` in `/etc/iter/data.env` (the CloudFormation stack in `../aws` also copies it to SSM). iter_data marks that admin `must_change_password`: the webui makes you choose your own password at the first sign-in (12 characters or more), and the API refuses everything else until then. At that moment iter_data writes `/var/lib/iter/admin_password_changed`, and `iter-admin-cleanup.path` starts `iter-admin-cleanup.service` (root, oneshot), which removes the `ITER_ADMIN_PASSWORD` line from `/etc/iter/data.env` (everything else and its `root:iter 640` stay); on a stack from `../aws` (`/etc/iter/aws.env` names `STACK=` and `REGION=`) it also deletes the SSM parameter `/iter/<stack>/admin-password`. `install.sh status` and a reinstall run the same cleanup when the marker exists, and a reinstall never writes the line back.



iter_data serves HTTPS. On its first start it makes a self-signed certificate, `/var/lib/iter/tls.crt` + `tls.key` (user `iter`, mode 600), beside `iter_data.secret` and `engine_session.secret` (the engine-session signing secret). `/var/lib/iter` stays owner-only (700), and all of it is kept across reinstalls, upgrades and `uninstall`.

After a successful install, and in `install.sh status`, the script prints the certificate's **fingerprint** (`sha256:` + 64 hex, the SHA-256 of the certificate: `openssl x509 -in /var/lib/iter/tls.crt -noout -fingerprint -sha256` without the colons; the script installs openssl). Engines pin it. Give it to each engine installer (`--fingerprint sha256:…`, or `-Fingerprint` on Windows), or let the engine trust it at its first sign-in and compare it afterwards with `iter_engine key status`. It is also in the webui's 🔑 Engine keys screen and in `journalctl -u iter-data | grep fingerprint`.

Browsers warn about a self-signed certificate. Accept it once, or:

- `--tls-cert PATH --tls-key PATH`: serve a certificate of your own (a real one for your domain). The script copies both to `/var/lib/iter/custom-tls.{crt,key}` (user `iter`, key mode 600). The fingerprint changes, so engines that pinned the old one need `iter_engine key trust --data-url URL --fingerprint …` again, or `--ca` for a certificate from a public authority.
- `--tls off`: plain HTTP, only for iter_data behind a TLS proxy on this host (Caddy, nginx, an ALB). Engines refuse `http://` to anything but 127.0.0.1 / localhost / ::1, so they must use the proxy's `https://` URL, with `iter_engine key trust --ca` when the proxy has a real certificate.
- `--tls-name NAME` (repeatable): put another DNS name or IP on the self-signed certificate, such as the name browsers use: on EC2, `--tls-name <public DNS name or Elastic IP>`. Names are read only when the certificate is first made; engines pin the fingerprint and do not check names.

The flags go on the `iter-data` unit's `ExecStart`. The choice is kept in `/etc/iter/tls.env`, so a plain rerun keeps it; `--tls auto` goes back to the self-signed certificate.

## Upgrade

Rerun `install.sh`, with `--version` for a specific release. The script is idempotent: it replaces the binary (written to a temp name, then moved over), re-renders the units and restarts `iter-data`. Secrets, data and the TLS certificate are kept, so the fingerprint engines pinned stays the same.

## Uninstall

```bash
sudo ./install.sh uninstall
```

This stops and removes the services, `iter_data`, `arangod` and the clients. The database, `/var/lib/iter`, `/etc/iter` and the backups are kept, and the script prints their paths in case you want to delete them.

## EC2

### Instance size

ArangoDB and iter_data need 4 GB of RAM or more. The script caps them together at the smaller of 6 GB and 85% of RAM (`--memory`).

| Use | Graviton (aarch64) | x86_64 |
|---|---|---|
| Trying it, one small team | `t4g.medium` (2 vCPU, 4 GB) | `t3.medium` |
| Recommended | `t4g.large` / `m7g.large` (8 GB) | `t3.large` / `m7i.large` |
| Large GraphRAG indexes | `r7g.large` (16 GB) with `--memory 12000` | `r7i.large` |

On burstable `t*` instances, file ingestion (embeddings) can use up the CPU credits. Pick unlimited mode or an `m`-class instance if you upload a lot of documents.

### Security group

| Inbound | Source | Why |
|---|---|---|
| TCP 8400 | your office/VPN CIDRs, or the load balancer's security group | API + webui, the only public port |
| TCP 22 | your IP only, or none with SSM Session Manager | admin |

- **Never open 8529.** ArangoDB listens on 127.0.0.1 only. Reach the console through an SSH tunnel: `ssh -L 8529:127.0.0.1:8529 ec2-user@<host>`.
- iter_data speaks HTTPS with its own self-signed certificate, which engines pin, so engines can reach it directly. For a certificate browsers trust, use `--tls-cert/--tls-key`, or put a proxy in front (an ALB with an ACM certificate forwarding to :8400 with `--tls off`, or Caddy/nginx on the instance) and allow 8400 only from that proxy.
- Outbound: HTTPS to github.com, objects.githubusercontent.com, huggingface.co (the model) and registry-1.docker.io / auth.docker.io (ArangoDB) during install. After that, outbound is needed only for what the engines and agents call.

### EBS data volume

Keep the database on its own gp3 volume, so it survives the instance being replaced and can be snapshotted. 20 GB, 3000 IOPS and 125 MB/s are plenty to start with.

```bash
lsblk                                   # find the new volume, e.g. /dev/nvme1n1
sudo mkfs.xfs /dev/nvme1n1
sudo mkdir -p /data
echo "UUID=$(sudo blkid -s UUID -o value /dev/nvme1n1) /data xfs defaults,nofail 0 2" | sudo tee -a /etc/fstab
sudo mount -a
sudo ./install.sh --db-dir /data/arangodb3 --backup-dir /data/backups
```

The unit waits for the mount (`RequiresMountsFor`). For off-instance copies, add EBS snapshots (Data Lifecycle Manager) or sync `/data/backups` to S3 yourself. The installer itself never calls AWS.

### Instance metadata (IMDS) and IAM

Neither iter_data nor the installer uses IMDS or AWS credentials. You can launch with IMDSv2 required (hop limit 1) or with the metadata endpoint turned off. No instance role is needed.
