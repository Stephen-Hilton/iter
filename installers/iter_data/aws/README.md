# iter_data on AWS (CloudFormation)

One CloudFormation stack gives you a running iter_data server on EC2. It is the
[`linux_aws/`](../linux_aws/README.md) installer, run for you at first boot, plus the AWS pieces around it:

| resource | what it is |
|---|---|
| EC2 instance | Amazon Linux 2023 (newest AMI, resolved by AWS), Graviton `arm64` or `x86_64`, IMDSv2 only |
| first-boot script | resolves the iter release (the latest, or `IterVersion`), runs that release's `installers/iter_data/linux_aws/install.sh` (ArangoDB + iter_data + backups as systemd services), waits for `/health` |
| EBS data volume | encrypted gp3, mounted at `/data` (database `/data/arangodb3`, backups `/data/backups`); kept as a snapshot when the stack is deleted or replaced |
| security group | only the iter_data port (8400) from `AllowedCidr`; SSH only if you give `SshCidr`; ArangoDB is never reachable |
| Elastic IP | a fixed address for the web page and for engines |
| IAM role | Session Manager access, and put/get/delete on its own `/iter/<stack>/*` parameters only |
| SSM parameters | `/iter/<stack>/admin-password` (SecureString) and `/iter/<stack>/tls-fingerprint` (String), written by the instance; the password never passes through CloudFormation, and the instance deletes it once the admin has chosen their own |

The stack reports complete only after iter_data answers `https://127.0.0.1:<port>/health`, typically about 10 minutes. It needs an iter release that includes HTTPS (any release after v2026.10.17): the first boot runs that release's `linux_aws/install.sh` with `--tls-name`.

## Launch it

**Console:** CloudFormation → Create stack → upload `iter_data.cfn.yaml` → fill in VPC, a public subnet
and `AllowedCidr` → acknowledge that it creates an IAM role.

**Command line** (aws CLI v2, signed in):

```sh
curl -fsSL https://raw.githubusercontent.com/Stephen-Hilton/iter/main/installers/iter_data/aws/install.sh \
  | bash -s -- --vpc vpc-0123 --subnet subnet-0123 --allowed-cidr 203.0.113.0/24
```

or from this folder: `./install.sh --vpc … --subnet … --allowed-cidr …`. Options: `--version v2026.10.17`
(default latest), `--arch arm64|x86_64`, `--instance-type`, `--volume-gib`, `--port`, `--ssh-cidr`,
`--key-name`, `--stack-name` (default `iter-data`), `--region`, `--profile`, `--dry-run`.

Then:

```sh
./install.sh status        # stack state and outputs: Url, AdminPasswordParameter, TlsFingerprintParameter, InstanceId, DataVolumeId
./install.sh password      # the first-boot admin password (from SSM; gone once you have changed it)
./install.sh fingerprint   # iter_data's TLS certificate fingerprint (from SSM)
```

Open the `Url` output (`https://<Elastic IP>:8400`), sign in as `admin` and choose your own password (the webui asks at the first sign-in; 12 characters or more). The instance then deletes `/iter/<stack>/admin-password` and the `ITER_ADMIN_PASSWORD` line in `/etc/iter/data.env` (`iter-admin-cleanup`, told the stack by `/etc/iter/aws.env`). Then and point engines at that URL
(`installers/iter_engine/<platform>/`) with the fingerprint: `install.sh --data-url https://<Elastic IP>:8400 --fingerprint sha256:…`.

## HTTPS

iter_data serves HTTPS with a self-signed certificate it makes on first boot (`/var/lib/iter/tls.crt`, kept
across reinstalls). Its names are the Elastic IP and, once the instance metadata (IMDSv2) shows the Elastic IP
attached, its public DNS name; engines ignore names, because they pin the certificate's **fingerprint**
(`sha256:` + 64 hex). The first boot stores it in `/iter/<stack>/tls-fingerprint`. Give it to each engine
installer (`--fingerprint`), or let the engine trust it at its first sign-in and compare with
`iter_engine key status`; the webui's 🔑 Engine keys screen shows it too. Engines never accept plain `http://`
to another machine.

Browsers warn about the self-signed certificate until you accept it. For one they trust, put an Application
Load Balancer with an ACM certificate (or a proxy such as Caddy) in front, rerun `linux_aws/install.sh` on the
instance with `--tls off` (the proxy then speaks to iter_data over plain HTTP) and narrow `AllowedCidr` to the
proxy; engines then use the proxy's URL with `iter_engine key trust --ca`. Or rerun it with
`--tls-cert/--tls-key` for a certificate of your own.

## Sizing

| instance | memory | for |
|---|---|---|
| t4g.medium / t3.medium | 4 GiB | a handful of people and engines |
| t4g.large / t3.large (default) | 8 GiB | a team, comfortable GraphRAG ingestion |
| m7g.large / m7i.large | 8 GiB, no CPU credits | steady heavy use |

ArangoDB and iter_data share one memory cap (min(6 GiB, 85% of RAM)). The Community Edition license
allows internal business use of a dataset under 100 GB.

## Upgrade, access, delete

- **Upgrade iter_data:** connect with Session Manager (`aws ssm start-session --target <InstanceId>`) and run
  `curl -fsSL https://raw.githubusercontent.com/Stephen-Hilton/iter/main/installers/iter_data/linux_aws/install.sh | sudo bash -s -- install --version <tag> --db-dir /data/arangodb3 --backup-dir /data/backups`.
  Changing `IterVersion` on the stack does not upgrade a running instance: it only applies at first boot.
- **Logs:** `/var/log/iter-bootstrap.log` (first boot), `journalctl -u iter-data -u arangodb3`.
- **Delete:** `./install.sh delete`. The data volume is kept as an EBS snapshot (restore it to a new volume
  to recover the database); remove the snapshot and the `/iter/<stack>/admin-password` and `/iter/<stack>/tls-fingerprint` parameters yourself when done.

Nothing here compiles anything: the instance downloads the prebuilt `iter_data-linux-<arch>` from the
release. To compile it yourself, clone the repo and run `cargo build --release -p iter_data` (or
`installers/build.sh`).
