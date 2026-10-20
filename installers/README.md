# installers

One folder per program and platform. Each folder works on its own: download just that folder (or
run its one-liner) and its install script does the rest. The script finds its binary in this order:
`--binary <path>`, then the matching asset sitting next to it, then a download from a GitHub Release
(sha256-checked against that release's `SHA256SUMS`).

| program | platform | folder | asset(s) | install |
|---|---|---|---|---|
| iter_data | Docker (any host) | [iter_data/docker/](iter_data/docker/README.md) | `iter_data-linux-x86_64`, `iter_data-linux-aarch64` | `./install.sh` |
| iter_data | Debian / Ubuntu / Arch, WSL2 | [iter_data/linux/](iter_data/linux/README.md) | `iter_data-linux-x86_64`, `iter_data-linux-aarch64` | `./install.sh` |
| iter_data | EC2 Amazon Linux 2023 | [iter_data/linux_aws/](iter_data/linux_aws/README.md) | `iter_data-linux-x86_64`, `iter_data-linux-aarch64` | `./install.sh` |
| iter_data | AWS, whole stack (CloudFormation) | [iter_data/aws/](iter_data/aws/README.md) | none: the instance runs `linux_aws/` from the release | `./install.sh --vpc … --subnet … --allowed-cidr …` |
| iter_engine | Docker (any host) | [iter_engine/docker/](iter_engine/docker/README.md) | `iter_engine-linux-x86_64`, `iter_engine-linux-aarch64` | `./install.sh` |
| iter_engine | Debian / Ubuntu / Arch | [iter_engine/linux/](iter_engine/linux/README.md) | `iter_engine-linux-x86_64`, `iter_engine-linux-aarch64` | `./install.sh` |
| iter_engine | Windows, inside WSL2 | [iter_engine/linux_wsl/](iter_engine/linux_wsl/README.md) | `iter_engine-linux-x86_64`, `iter_engine-linux-aarch64` | `./install.sh` |
| iter_engine | macOS (Apple silicon / Intel) | [iter_engine/macos/](iter_engine/macos/README.md) | `iter_engine-macos-aarch64`, `iter_engine-macos-x86_64` | `./install.sh` |
| iter_engine | Windows (native) | [iter_engine/windows/](iter_engine/windows/README.md) | `iter_engine-windows-x86_64.exe` | `install.cmd` |

Without a checkout, run a folder's script straight from GitHub (it downloads its binary from the release):

```sh
curl -fsSL https://raw.githubusercontent.com/Stephen-Hilton/iter/main/installers/iter_engine/linux/install.sh | bash -s -- --version v2026.10.17
```

Every script takes `--help` and `--dry-run` (print every action, change nothing) and can be run
again at any time. The folder READMEs above have the details for each platform.

## Upgrade a machine: upgrade_iter.sh

`upgrade_iter.sh` (Linux, macOS) upgrades every iter program installed natively on the machine to one
release, latest unless `--version` says otherwise. Run it as your own user; it uses sudo where it must:

```sh
curl -fsSL https://raw.githubusercontent.com/Stephen-Hilton/iter/main/installers/upgrade_iter.sh | bash
```

It finds iter_data (`iter-data` systemd unit) and the engine (`iter-engine` unit, or launchd
`com.iter.engine`), skips a program whose binary already matches the release's `SHA256SUMS`, and fetches each
program's `install.sh` from that release's tag. It asks for sudo once. Before it changes anything it runs the
engine's `update` as a dry run, which refuses while the engine runs work: on a terminal it asks, otherwise
`--yes` restarts it anyway. Then it backs up iter_data (`install.sh backup`, `--no-backup` skips it), reruns
iter_data's `install.sh` (secrets, database, TLS certificate, port and memory cap are kept), and runs the
engine's `install.sh update` (`--full`: its whole `install --yes`, which also makes the browsers here trust
iter_data's certificate authority). Last it checks the binaries, the services, that iter_data accepts the
engine's key, that the account tokens the served projects name are set (names only), and that iter_data
answers as a trusted https site, and lists what is left for a person. A failed iter_data step stops before the
engine is touched; a rerun skips what is done. `--only data|engine`, `--force`, `--log FILE|none` (default
`<iter dir>/iter_engine/logs/upgrade_iter.log`, passwords masked) and `--dry-run` change what it does. Docker
installs are only reported: rerun their `install.sh`.

## One folder: the iter dir

Every installer puts everything it makes in one folder, the **iter dir**: `/opt/iter` by default
(Windows `C:\iter`; the AWS stack `/data/iter`, on its data volume; the docker installers under Docker
Desktop `~/iter`). Choose another with `--iter-dir DIR`
(Windows `-IterDir`, or `ITER_DIR` in the environment). The folder always ends in `iter`:
`--iter-dir /my/dir`, `--iter-dir /my/dir/iter` and `--iter-dir /my/dir/iter/` all mean `/my/dir/iter`.

```
<iter dir>/
  iter_data/      bin/ config/ state/ models/ backups/ systemd/ arango/{bin,share,config,data,apps}
  iter_engine/    bin/ config/ secrets/ models/ logs/ systemd/
```

- `iter_data/` is root's, run by the `iter` user. `iter_engine/` belongs to the user the engine runs as,
  except `iter_engine/systemd/`, which is root's: systemd runs that unit, so the user and its agents must
  not be able to change it.
- Outside the folder there are only what the OS requires, as links where it can follow them: the systemd
  units (links in `/etc/systemd/system`, or copies when the iter dir is on another disk than `/`),
  `/etc/sysctl.d/60-arangodb.conf`, the `/usr/local/bin/iter_engine` link, the macOS launchd plist in
  `~/Library/LaunchAgents`, and the Windows logon task.
- Later runs (`status`, `update`, `uninstall`) find the iter dir from the installed service, so
  `--iter-dir` is only needed the first time. A run with another `--iter-dir` moves the folder there.
- The engine's service sets `ITER_HOME=<iter dir>/iter_engine`. `iter_engine` run by hand finds its folder
  from its own path (`<folder>/bin/iter_engine`).
- **An install from before 2026-10-18** is moved into the iter dir by the next `install` or `update`.
  For the engine that is `~/.iter`; for iter_data, `/etc/iter`, `/var/lib/iter`, `/opt/iter/bin`,
  `/var/lib/arangodb3` and `/usr/sbin/arangod`. Files are moved, never deleted, with the services
  stopped meanwhile. Files the engine does not use go to `iter_engine/old-dot-iter/`, and files an
  earlier install left in the way go to `replaced-<time>/`. The database and the backups move only
  from their default places and within one filesystem; otherwise they stay where they are and the
  script records that. Keys, certificates, and so the fingerprints engines pinned, stay the same.
- Docker Desktop (macOS, Windows, and Docker Desktop for Linux) runs containers in a VM that sees only
  the folders shared with it: the home folder is, `/opt` is not (this is file sharing, not permissions,
  so `chmod` does not help). So the docker installers default to `~/iter` under Docker Desktop (they ask
  `docker info`) and to `/opt/iter` with Docker Engine on Linux. The native installers use no Docker.

## The first-boot admin password

Every iter_data installer generates the `admin` password on a fresh install and prints it once. It must be
changed at the first sign-in: the webui asks, and a new password needs 12 characters or more (the API refuses
everything else until then). Then iter_data writes `admin_password_changed` beside its secrets, and the
installers remove `ITER_ADMIN_PASSWORD` from where they put it: linux / linux_aws through the
`iter-admin-cleanup.path` systemd unit (on an aws stack also the SSM parameter), docker on the next
`install.sh` run or `install.sh status`.

## ArangoDB: iter's own user and database, or one you already run

iter_data signs in to ArangoDB as its own user (`iter_user`), which holds `rw` on its own database
(`iter_db`) and nothing else. On the first start, when either is missing, iter_data uses
`ARANGO_ROOT_PASSWORD` once to create them and grant `rw`. iter's user never gets root's password.

By default every iter_data installer runs ArangoDB beside iter_data. To use an ArangoDB you already run
(shared with other apps, or on another machine), give it `--arango-url`:

| option | linux, linux_aws, docker | aws (CloudFormation) |
|---|---|---|
| use an existing ArangoDB | `--arango-url URL` (or `ARANGO_URL`) | `--arango-url` / `ArangoUrl` |
| pin its self-signed certificate | `--arango-fingerprint sha256:…` | `--arango-fingerprint` / `ArangoCertFingerprint` |
| or trust your own CA | `--arango-ca-cert PATH` | `--arango-ca-cert` (kept in SSM) |
| iter's database | `--arango-db NAME` [iter_db] | `--arango-db` / `ArangoDb` |
| iter's user | `--arango-user NAME` [iter_user] | `--arango-user` / `ArangoUser` |
| plain http on a private network | `ARANGO_ALLOW_PLAIN_HTTP=1` | `--arango-allow-plain-http` / `ArangoAllowPlainHttp` |
| iter's user's password | `ARANGO_PASSWORD` (environment or `--env-file`) | `ARANGO_PASSWORD` in the shell (SSM SecureString) |
| root's password, to create them | `ARANGO_ROOT_PASSWORD` (environment or `--env-file`) | `ARANGO_ROOT_PASSWORD` in the shell (SSM SecureString) |

With an external ArangoDB, give either `ARANGO_PASSWORD` (a user its admin made, with `rw` on the
database) or `ARANGO_ROOT_PASSWORD`. iter_data reaches another machine over `https://` only. The
fingerprint is `openssl x509 -in server.crt -noout -fingerprint -sha256`. The linux and docker READMEs
show how to serve ArangoDB over HTTPS with a self-signed certificate. linux and linux_aws then install
only the Arango client tools, for backups. The docker container runs iter_data alone, with no Arango
console port.

**Backups are per database.** `iter-backup` (linux, linux_aws, every 6 hours) dumps only `ARANGO_DB`, as
iter's user, from a local or remote ArangoDB. Over HTTPS it first checks the certificate the way iter_data
does. `install.sh restore FILE` stops iter-data, loads that database back (`arangorestore --overwrite`) and
starts iter-data. Arango's system collections are left out: iter_data recreates what it uses at start. The
docker README has the same commands for the container.

## Engine account tokens from a secret store

Every engine installer takes `--secrets-command CMD` (`-SecretsCommand` on Windows) and `--secrets-every N`
(minutes, default 5): a command printing the account tokens as `KEY=value` lines (AWS SSM Parameter Store or
Secrets Manager, 1Password, the macOS Keychain; recipes in each folder's README). The engine runs it at
startup and every N minutes and keeps the values in memory only; environment variables the service starts
with win over it, and it wins over `iter_engine/secrets/.env`. With it, the installers ask for no tokens.

## HTTPS and the certificate fingerprint

iter_data serves HTTPS by default. On its first start it makes a local certificate authority
(`tls-ca.crt` + `tls-ca.key`) and a certificate signed by it (`tls.crt` + `tls.key`, keys mode 600) beside its secret file, next to `engine_session.secret`; the installers keep all of
them across reinstalls. Every iter_data installer waits on `https://127.0.0.1:<port>/health` and then
prints the certificate's **fingerprint**, `sha256:` + 64 hex (the SHA-256 of the certificate: `openssl x509
-in tls.crt -noout -fingerprint -sha256`, colons removed, lowercased). The webui shows it in the 🔑 Engine
keys screen too, and `install.sh status` (linux, linux_aws) or `install.sh fingerprint` (aws) prints it
again.

Engines never accept plain `http://` across a network: an engine installer stops on an `http://` URL
unless iter_data runs on that same machine (127.0.0.1, localhost, ::1). Each engine pins iter_data's
fingerprint in `iter_engine/config/server_trust.json`:

1. Install iter_data and copy the fingerprint it prints.
2. Install each engine with `--data-url https://<host>:8400 --fingerprint sha256:…` (Windows:
   `-DataUrl … -Fingerprint …`), or paste the fingerprint when the script asks. The script runs
   `iter_engine key trust --data-url URL --fingerprint …` right after the binary is installed. If the
   server presents a different certificate it stops, because something else answers at that URL.
3. Or press Enter at that question: the engine trusts the certificate at its first sign-in. Compare it
   afterwards with `iter_engine key status`, which prints the trusted certificate.

Browsers: the local CA is name-constrained to iter_data's names (with every `--tls-name`), `.local`/`.lan`/
`.internal`/`.home.arpa` and loopback/private addresses, so it cannot vouch for any public site; iter_data
serves it without sign-in at `<data url>/tls/ca.crt`. Every installer adds it to the trust stores of the machine
it runs on, so a browser there opens the webui as secure: the iter_data installers from iter_data's own folder
(linux: system store + the desktop user's NSS databases; docker: the host's login keychain on macOS, system
store + NSS on Linux; linux_aws: the system store only), the engine installers after the key step through
`iter_engine key ca` over the pinned connection (linux, linux_wsl: system store + NSS, and on WSL Windows'
`Cert:\CurrentUser\Root`; macos: the login keychain, a password dialog; windows: `Cert:\CurrentUser\Root`,
a confirmation dialog, or `LocalMachine\Root` when elevated; docker: the host). It is skipped with `--tls off`,
a certificate of your own or an engine's `--trust-ca`, and a failure is only a warning. A machine with no
installer opens `<data url>/tls/ca.crt` and trusts it as a certificate authority. The iter_data installers take
`--tls-cert PATH --tls-key PATH` to serve a certificate of your own (copied into iter_data's folder, key mode
600), `--tls-name NAME` (repeatable) to add names to iter_data's own, and `--tls off` for iter_data behind a TLS proxy on the
same host. An engine that reaches iter_data through a proxy with a real certificate uses `--trust-ca`
(`iter_engine key trust --ca`) instead of a fingerprint.

Engine key commands: `iter_engine key show` prints the public key; `iter_engine key rotate --data-url URL`
replaces the key, signed by the current one, with no webui step (a revoked key cannot rotate);
`iter_engine key status` checks the key and the trusted certificate; `iter_engine key trust --forget`
drops a server's pin. Engines sign in only with their key; there is no engine token.

## Versions

A release is a git tag `v<YYYY>.<MM>.<DD>` (UTC build day); a second release the same day is
`v<YYYY>.<MM>.<DD>.2`, then `.3`, ... Each release carries the 7 binaries plus `SHA256SUMS`:

```
iter_data-linux-x86_64     iter_data-linux-aarch64
iter_engine-linux-x86_64   iter_engine-linux-aarch64
iter_engine-macos-aarch64  iter_engine-macos-x86_64
iter_engine-windows-x86_64.exe
```

Download URLs:

- latest: `https://github.com/Stephen-Hilton/iter/releases/latest/download/<asset>`
- a specific version: `https://github.com/Stephen-Hilton/iter/releases/download/<tag>/<asset>`

Install scripts take the latest release unless given `--version <tag>` (or `ITER_VERSION=<tag>`).
List releases at https://github.com/Stephen-Hilton/iter/releases or with `gh release list -R Stephen-Hilton/iter`.

Linux binaries are built against glibc 2.31 (Debian 11 bullseye), so they run on Debian 11+,
Ubuntu 20.04+, Amazon Linux 2023 and current Arch.

## Rebuilding

`installers/build.sh` (on an Apple-silicon Mac with Docker running) builds every asset into
`installers/dist/`, writes `dist/SHA256SUMS`, and copies each asset into the folders that use it:

```sh
installers/build.sh                               # all 7 assets
installers/build.sh --only 'iter_engine-macos-*'  # a subset (glob, repeatable)
installers/build.sh --skip-windows
installers/build.sh --dry-run                     # show the plan only
installers/build.sh --release                     # build all, tag HEAD, push the tag, gh release create
```

`--release` refuses when the working tree is dirty or HEAD is not on origin. How each asset is built:

| asset | toolchain |
|---|---|
| `iter_engine-macos-aarch64` | native `cargo build --release -p iter_engine -p iter_data` |
| `iter_engine-macos-x86_64` | `cargo build --release --target x86_64-apple-darwin -p iter_engine` |
| `iter_*-linux-aarch64` | `rust:1-bullseye` container (linux/arm64), docker volumes `iter-build-cargo` + `iter-build-target` |
| `iter_*-linux-x86_64` | same container, cross-compiled with `gcc-x86-64-linux-gnu` |
| `iter_engine-windows-x86_64.exe` | `cargo xwin build --target x86_64-pc-windows-msvc` (`cargo install --locked cargo-xwin`, `brew install llvm`) |

Binaries are never committed: `dist/` and the copies inside the folders are git-ignored; the only
published copies are GitHub Release assets.

`fetch_model.sh` downloads and checksum-verifies the embedding model (all-MiniLM-L6-v2, ~91 MB)
that iter_data and iter_engine use.
