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
| IAM role | Session Manager access, and put/get/delete on its own `/iter/<stack>/*` parameters only (which include an external ArangoDB's `arango-*` ones) |
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
`--key-name`, `--stack-name` (default `iter-data`), `--region`, `--profile`, `--dry-run`, and the
`--arango-*` options below for an ArangoDB you already run.

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

## ArangoDB: on the instance, or one you already run

iter_data signs in to ArangoDB as a user of its own, **`iter_user`**, which holds rw (Administrate) on
its own database, **`iter_db`**, and nothing else, so one ArangoDB server can hold other apps' databases
too. At every start iter_data signs in as that user. When it cannot (the user or the database does not
exist yet, or the user has less than rw), and root's password is known, root creates the user and the
database and grants rw, and iter_data tries again. The user itself creates every collection and index,
including GraphRAG's vector indexes once a project is large. An installation from before iter_user existed
keeps its `iter5` database (iter_data then signs in as `iter_user` to it).

By default the first boot installs ArangoDB on the instance (localhost only) and does all of this for you.
To use an ArangoDB you already run, for example one shared with another app, give its URL:

```sh
ARANGO_PASSWORD='a password for iter_user' ARANGO_ROOT_PASSWORD='root, only if iter_user or iter_db is missing' \
  ./install.sh --vpc … --subnet … --allowed-cidr … \
    --arango-url https://arango.example.com:8529 --arango-fingerprint sha256:…
```

| option (stack parameter) | what it does |
|---|---|
| `--arango-url` (`ArangoUrl`) | the ArangoDB to use; nothing is installed on the instance. Empty or `local` installs one there |
| `--arango-fingerprint` (`ArangoCertFingerprint`) | pins the ArangoDB's self-signed certificate (`sha256:` + 64 hex); iter_data accepts exactly that certificate |
| `--arango-ca-cert PATH` (SSM `/iter/<stack>/arango-ca-cert`) | trusts this PEM CA instead, for a certificate a CA of your own signed |
| `--arango-db` (`ArangoDb`), `--arango-user` (`ArangoUser`) | another database or user name [`iter_db`, `iter_user`]; never `root` |
| `--arango-allow-plain-http` (`ArangoAllowPlainHttp`) | allows an `http://` URL on a network you trust (inside the VPC); otherwise refused |
| `ARANGO_PASSWORD` (SSM SecureString `/iter/<stack>/arango-password`) | the user's own password, never root's; made on the instance when absent |
| `ARANGO_ROOT_PASSWORD` (SSM SecureString `/iter/<stack>/arango-root-password`) | only needed while the user or database does not exist yet; leave it out when your ArangoDB admin created them |

The passwords never pass through CloudFormation: `install.sh` stores them in SSM (it prints `<$ARANGO_PASSWORD>`,
never the value), and the first boot reads them into `/etc/iter/data.env` (root:iter 640). From the
console, create those SSM parameters yourself before the stack. A certificate from a public CA (for
example ACM behind a load balancer) needs neither the fingerprint nor the CA file. With an external ArangoDB
the data volume holds only the backups, which dump iter's database alone (`arangodump --server.database`),
so `--volume-gib 10` is plenty.

**HTTPS for an ArangoDB you run:** iter_data refuses plain `http://` to another machine. Give arangod a
certificate and key in one PEM file and an `ssl://` endpoint, then pin the certificate:

```sh
openssl req -x509 -newkey ec -pkeyopt ec_paramgen_curve:prime256v1 -nodes -days 3650 -subj /CN=arango \
  -addext 'subjectAltName=DNS:arango.example.com' -keyout key.pem -out cert.pem
cat cert.pem key.pem > /etc/arangodb3/server.pem && chmod 600 /etc/arangodb3/server.pem   # owned by arangodb
# arangod.conf: [server] endpoint = ssl://0.0.0.0:8529   [ssl] keyfile = /etc/arangodb3/server.pem
openssl x509 -in cert.pem -noout -fingerprint -sha256 | sed 's/.*=//; s/://g' | tr A-F a-f   # -> sha256:<this>
```

and let only this stack's security group (or its Elastic IP) reach port 8529.

## Sizing

| instance | memory | for |
|---|---|---|
| t4g.medium / t3.medium | 4 GiB | a handful of people and engines |
| t4g.large / t3.large (default) | 8 GiB | a team, comfortable GraphRAG ingestion |
| m7g.large / m7i.large | 8 GiB, no CPU credits | steady heavy use |

ArangoDB and iter_data share one memory cap (min(6 GiB, 85% of RAM)); with `--arango-url` the instance
runs iter_data alone, and a t4g.medium is enough. The Community Edition license
allows internal business use of a dataset under 100 GB.

## Upgrade, access, delete

- **Upgrade iter_data:** connect with Session Manager (`aws ssm start-session --target <InstanceId>`) and run
  `curl -fsSL https://raw.githubusercontent.com/Stephen-Hilton/iter/main/installers/iter_data/linux_aws/install.sh | sudo bash -s -- install --version <tag> --db-dir /data/arangodb3 --backup-dir /data/backups`.
  Changing `IterVersion` on the stack does not upgrade a running instance: it only applies at first boot.
- **Logs:** `/var/log/iter-bootstrap.log` (first boot), `journalctl -u iter-data -u arangodb3`.
- **Delete:** `./install.sh delete`. The data volume is kept as an EBS snapshot (restore it to a new volume
  to recover the database); remove the snapshot and the `/iter/<stack>/admin-password` and `/iter/<stack>/tls-fingerprint` parameters (and any `/iter/<stack>/arango-*` ones) yourself when done.
- **Changing ArangoDB later:** the `Arango*` parameters, like the others, apply only at first boot. On a
  running instance rerun `linux_aws/install.sh` with the `--arango-*` options (and the passwords in an
  `--env-file`).

Nothing here compiles anything: the instance downloads the prebuilt `iter_data-linux-<arch>` from the
release. To compile it yourself, clone the repo and run `cargo build --release -p iter_data` (or
`installers/build.sh`).
