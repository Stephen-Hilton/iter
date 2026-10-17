# iter engine: Docker

Runs this machine's iter engine in a container: Debian bookworm-slim, git, bash, the `claude` CLI and the prebuilt `iter_engine-linux-<arch>` for the Docker host's platform. It works on a Linux host, or on macOS with Docker Desktop. Nothing is compiled.

## Limits

- **One engine per machine still.** Do not run this beside a native engine on the same host.
- **The agents run inside the container.** That covers claude sessions, shell steps and tests. They see only what the image has (git, bash, curl, jq, poppler) and the mounted folders. A project that needs the host's toolchain, Docker, a GUI or Windows tools wants a native engine instead. To add tools, extend the `Dockerfile` in `~/.iter/docker`.
- **Credentials come from files.** Claude logs in with the account tokens in `~/.iter/.env` (`claude setup-token`), and the host's own claude login is not shared. Git uses the host's `~/.ssh`, `~/.gitconfig`, `~/.git-credentials` and `~/.config/gh`, mounted read-only when present. macOS keychain or ssh-agent credentials are not visible inside.
- **Same paths inside and outside.** `~/.iter` and the checkouts folder (default `~/dev`) are mounted at the same absolute paths. The container runs as your uid/gid with HOME set to your home path, so `~/dev/<project>` topdirs match and the files it writes are yours. Every serves edge's topdir must be inside the checkouts folder. The `.iter/bin/iter` shim the engine writes into a checkout works only inside the container.
- **The network is the container's.** For iter_data on the same host, use `https://host.docker.internal:<port>`; the script rewrites `127.0.0.1` and `localhost` for you. Plain `http://` is refused: from inside the container iter_data is always across a network.
- Bind mounts on Docker Desktop for Mac are slower than a native disk. The image is about 850 MB.

## Install

```bash
curl -fsSL https://raw.githubusercontent.com/Stephen-Hilton/iter/main/installers/iter_engine/docker/install.sh | bash -s -- --version v2026.10.17
```

Prerequisites: Docker with compose v2, with your user able to run `docker`, plus curl and git on the host.

The script sets up `~/.iter/docker` as the build folder. Into it go `Dockerfile`, `entrypoint.sh` and `compose.yml` (copied from beside the script, else downloaded from GitHub at `--version`), the binary (from beside the script, `--binary`, or the release, sha256-checked), `.env` with the compose settings, and `compose.override.yml` with the git credential mounts. It then fetches the embedding model into `~/.iter/models` and builds `iter-engine:local`. Next it makes the engine key inside the image and waits until the webui's Engine keys screen has its public key. Last, `docker compose up -d` starts the container `iter-engine` with `restart: unless-stopped` and docker's init.

## Options

`--data-url URL` (`https://`), `--name NAME`, `--checkouts DIR` (default `~/dev`), `--version TAG`, `--binary PATH`, `--fingerprint FP` (iter_data's certificate, pinned before the first sign-in), `--trust-ca` (a TLS proxy with a real certificate), `--secrets-command CMD` / `--secrets-every N` (account tokens from a secret store, [below](#account-tokens-from-a-secret-store)), `--yes`, `--dry-run`.
Verbs: `install` (default), `status`, `update`, `accounts`, `projects`, `logs`, `uninstall`.

By hand, from `~/.iter/docker`: `docker compose up -d`, `docker compose logs -f`, `docker compose down`. To run a command as the engine's user: `docker compose run --rm --no-deps engine <cmd>`. Note that `docker exec` runs as root.

## Account tokens from a secret store

By default the account tokens (`claude setup-token`, one per `token_envar`) sit in `~/.iter/.env` and the accounts step asks you to paste them. Instead, `--secrets-command CMD` names a command that prints `KEY=value` lines. The engine runs it at startup and every `--secrets-every N` minutes (default 5) and keeps the values in memory only; it never logs them, and it logs the names of tokens it is still missing. The script keeps the command in `~/.iter/engine.conf`, puts it on the container (as `ITER_SECRETS_COMMAND="sh ~/.iter/docker/secrets_command"`, the command in that file, plus `ITER_SECRETS_EVERY`, through `~/.iter/docker/.env` and compose.yml), and the accounts step then asks for no tokens: it runs the command once and reports which needed names it prints (names only). `--secrets-command none` removes it. It runs **inside the container**, which has `sh`, curl, jq and openssl but no `aws`, `op` or Keychain: add the CLI to `~/.iter/docker/Dockerfile` (and its credentials as a mount or variables), then `install.sh update`.

The sources rank: environment variables the service is started with (highest) > the secrets command > `~/.iter/.env`. Variables injected at start already work and win over everything: `environment:` in `compose.override.yml` or `docker run -e`.

Recipes (each prints `DEV1_TOKEN=…` lines; use your own names and paths):

- **AWS SSM Parameter Store**, one SecureString per token under `/iter/engine/<name>/` (the parameter's last path part is the variable name):
  `aws ssm get-parameters-by-path --path /iter/engine/<name>/ --with-decryption --query 'Parameters[].[Name,Value]' --output text | awk '{n=$1; sub(".*/","",n); print n"="$2}'`
- **AWS Secrets Manager**, one JSON secret `{"DEV1_TOKEN": "…", "DEV2_TOKEN": "…"}`:
  `aws secretsmanager get-secret-value --secret-id iter/engine --query SecretString --output text | jq -r 'to_entries[]|"\(.key)=\(.value)"'`
- **1Password CLI**: `op inject -i ~/.iter/tokens.tpl` (a template of `DEV1_TOKEN={{ op://vault/item/field }}` lines), or `printf 'DEV1_TOKEN=%s\n' "$(op read op://vault/item/field)"`

## HTTPS and iter_data's certificate

The iter_data URL must be `https://`. iter_data serves HTTPS by default, and the engine never accepts plain `http://` across a network: the script stops on an `http://` URL unless the host is this machine (`127.0.0.1`, `localhost`, `::1`). In the container nothing but the container itself is local, so every URL is `https://`.

iter_data's certificate is usually self-signed, so the engine does not ask a certificate authority about it. It pins the certificate's **fingerprint** (`sha256:` + 64 hex) in `~/.iter/server_trust.json`, one entry per server:

- **Given up front** (recommended): `--fingerprint sha256:…`, or paste it when the script asks. The iter_data installer prints it, and the webui shows it in the 🔑 Engine keys screen. Right after the image is built, the script runs, inside it, `iter_engine key trust --data-url URL --fingerprint …`, which pins it only if the server presents that same certificate. If it does not, the script stops: something else answers at that URL.
- **Trust on first use**: press Enter at the question, and the first signed sign-in pins the certificate it sees. Compare it afterwards with `iter_engine key status`, which prints the trusted certificate.
- **A TLS proxy with a real certificate** in front of iter_data: `--trust-ca` (`iter_engine key trust --ca`) checks it against certificate authorities instead.

When iter_data's certificate changes on purpose, iter_data announces the next fingerprint at each sign-in, so the engine follows it. A certificate replaced by hand (for example `--tls-cert` on the server) needs `iter_engine key trust --data-url URL --fingerprint …` again; `iter_engine key trust --forget` drops the entry.

The container sees `~/.iter/server_trust.json` through its `~/.iter` mount, and the script checks the certificate the same way for its own API calls (with curl and openssl inside the image). Key commands, from `~/.iter/docker` as `docker compose run --rm --no-deps engine iter_engine key …`: `iter_engine key show` prints this engine's public key (never the private one), `iter_engine key status` checks the key and the trusted certificate, and `iter_engine key rotate --data-url URL` replaces the key: the current key signs the new one, so no webui step is needed (a revoked key cannot rotate). The engine signs in only with its key.

## Upgrade

`install.sh update --version vYYYY.MM.DD` fetches the new binary, rebuilds the image with `--no-cache` (which also picks up a newer `claude` CLI) and recreates the container. It refuses while work runs, unless you pass `--yes`.

## Uninstall

`install.sh uninstall` runs `docker compose down --rmi local`. It keeps `~/.iter` (the key, tokens, model, `claude/` and `docker/`) and the checkouts.

To compile it yourself, clone the repo and run `cargo build --release -p iter_engine` (or `installers/build.sh`).
