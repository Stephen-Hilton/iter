# iter engine: Linux

Makes this Linux machine an iter engine, run by systemd as the `iter-engine` service. It works on Debian 11+, Ubuntu 20.04+ and Arch/Omarchy, and mostly on Fedora. The binary needs glibc 2.31 or newer. A machine runs one engine, and that engine serves every project the webui assigns to it. The engine only connects outward: to iter_data, to Anthropic, to the projects' git remote, and once to Hugging Face for the embedding model.

For a distro inside WSL2 on Windows, use `../linux_wsl` instead.

## Install

```bash
curl -fsSL https://raw.githubusercontent.com/Stephen-Hilton/iter/main/installers/iter_engine/linux/install.sh | bash -s -- --version v2026.10.17
```

From a downloaded copy of this folder: `./install.sh`. It uses the `iter_engine-linux-<arch>` file next to it when one is there. Otherwise it downloads that file and `SHA256SUMS` from the GitHub Release (`--version`, else the latest) and checks the sha256.

Run it as your own user; it uses sudo where it must. The steps:

0. The iter dir (below): `/opt/iter/iter_engine`, or under `--iter-dir`. An older `~/.iter` is moved into it.
1. Packages: git, curl, jq, openssl, poppler, and gh where the distro has it. It asks before installing.
2. The `claude` CLI, from Anthropic's installer into `~/.local/bin`.
3. The iter_data URL and the engine name, saved to `config/engine.conf`.
4. `bin/iter_engine`, written to a temp name and then moved into place, and linked as `/usr/local/bin/iter_engine`. Then iter_data's certificate: `--fingerprint` (or the one you paste) is pinned with `iter_engine key trust` (see [HTTPS](#https-and-iter_datas-certificate)). With iter_data on this machine (below), the script reads the certificate from its file instead.
5. The embedding model into `models/`, checksum-verified.
6. The engine key. `iter_engine key new` makes it; you paste its public key into the webui's Engine keys screen, and the script waits until iter_data accepts it. Each try shows the one line to paste, the key id the webui will list, and why iter_data refused. With iter_data on this machine (below), nothing is pasted.
7. On a laptop: no suspend, and on Omarchy no lock screen. It asks first.
8. The `iter-engine` systemd service.
9. Account tokens (`claude setup-token`, saved to `secrets/.env`, or none to paste with `--secrets-command`) and clones of the served projects.

Prerequisites: sudo, systemd, outbound HTTPS. No Rust and no compiling.

## Where it installs: one folder

Everything goes in one folder, the **iter dir**: `/opt/iter` unless you give `--iter-dir DIR` (or `ITER_DIR`). The folder always ends in `iter`, so `--iter-dir ~/stuff`, `--iter-dir ~/stuff/iter` and `--iter-dir ~/stuff/iter/` all mean `~/stuff/iter`. The engine's part of it is `<iter dir>/iter_engine`, owned by you. iter_data on the same machine goes beside it, in `<iter dir>/iter_data`.

```
<iter dir>/iter_engine/
  bin/iter_engine
  config/    engine.conf, server_trust.json (iter_data's pinned certificate)
  secrets/   engine.key, .env (the account tokens); mode 700
  models/    the embedding model
  logs/      engine.log, engine.err.log
  systemd/   iter-engine.service, owned by root (systemd runs it, so you and the agents cannot change it)
```

Only links live outside the folder: `/etc/systemd/system/iter-engine.service` and `/usr/local/bin/iter_engine`. When the iter dir is on another filesystem than `/`, the unit is copied there instead, because systemd reads units before other disks are mounted. The service sets `ITER_HOME` to the engine's folder. `iter_engine` run by hand (through the link) finds its folder from its own path. Later runs (`status`, `update`, `uninstall`) find the iter dir from the service, so `--iter-dir` is only needed the first time. A run with another `--iter-dir` moves the folder there.

**An install from before 2026-10-18** (`~/.iter`, or `~/.iter5`) is moved in by the next `install` or `update`, with the service stopped meanwhile. Each file goes to its place above. Files the engine does not use go to `iter_engine/old-dot-iter/`; nothing is deleted. iter_data's files from an old install (`~/.iter/iter_data`, `data.env`) stay, for its own installer. The key, and so the webui's record of this engine, stays the same.

## Options

| | |
|---|---|
| `--iter-dir DIR` | the one folder everything goes in (default `/opt/iter`; later runs: where it is installed); always ends in `iter` |
| `--data-url URL` | the iter_data URL, `https://` (otherwise it asks) |
| `--name NAME` | the engine name, unique per machine (default: the hostname) |
| `--version TAG` | the release to download (else `$ITER_VERSION`, else the latest) |
| `--binary PATH` | install this iter_engine file instead of downloading one |
| `--fingerprint FP` | iter_data's certificate fingerprint (`sha256:…`), pinned before the first sign-in |
| `--trust-ca` | iter_data is behind a TLS proxy with a real certificate: check it against certificate authorities |
| `--secrets-command CMD` | a command printing the account tokens as `KEY=value` lines, from a secret store ([below](#account-tokens-from-a-secret-store)); `none` removes it |
| `--secrets-every N` | minutes between its runs (default 5) |
| `--yes` | answer yes to every question |
| `--dry-run` | print every action and change nothing |

Verbs: `install` (the default), `status`, `update`, `accounts`, `projects`, `logs`, `uninstall`.

Projects: in the webui's Settings tab, draw a `serves` edge from the engine to the project with the topdir `~/dev/<project>`, then run `install.sh projects`.

## iter_data on the same machine

When the iter_data URL is this machine (`https://127.0.0.1:8400`, `localhost`, `::1`) and iter_data is installed here by `installers/iter_data/linux` (its folder is found from the `iter-data` unit), the two are set up together and nothing is copied by hand:

- **The certificate** is read from iter_data's own file (`iter_data/state/tls.crt`, or `custom-tls.crt`) and pinned, so no fingerprint is asked for.
- **The engine key** is registered with `sudo -u iter <iter dir>/iter_data/bin/iter_data engine-key add --env-file <iter dir>/iter_data/config/data.env --public-key iter_… --engine NAME`. This runs as iter_data's own user with its own env file, the way the `iter-data` service reaches the database, so no admin password is needed. The key shows in the webui's Engine keys screen like one an admin added. A rerun is a no-op. An iter_data without this command falls back to pasting.
- **The webui sign-in** is printed at the end: the URL, and the first-boot admin password while it is still the first-boot one.

Install iter_data first, then the engine with `--data-url https://127.0.0.1:8400`. An engine on another machine still has its key pasted in the webui, and an engine here that reports to another iter_data is set up as usual.

## Account tokens from a secret store

By default the account tokens (`claude setup-token`, one per `token_envar`) sit in `secrets/.env` and the accounts step asks you to paste them. Instead, `--secrets-command CMD` names a command that prints `KEY=value` lines. The engine runs it at startup and every `--secrets-every N` minutes (default 5) and keeps the values in memory only; it never logs them, and it logs the names of tokens it is still missing. The script keeps the command in `config/engine.conf`, puts it on the `iter-engine` unit's `ExecStart`, and the accounts step then asks for no tokens: it runs the command once and reports which needed names it prints (names only). `--secrets-command none` removes it. It runs with `sh -c` as you, with the service's `PATH` and `HOME`, so `~/.aws` credentials and profiles work; give the full path of a CLI outside `/usr/local/bin:/usr/bin:/bin` and `~/.local/bin`.

The sources rank: environment variables the service is started with (highest) > the secrets command > `secrets/.env`. Variables injected at start already work and win over everything: `Environment=` / `EnvironmentFile=` in a drop-in (`sudo systemctl edit iter-engine`).

Recipes (each prints `DEV1_TOKEN=…` lines; use your own names and paths):

- **AWS SSM Parameter Store**, one SecureString per token under `/iter/engine/<name>/` (the parameter's last path part is the variable name):
  `aws ssm get-parameters-by-path --path /iter/engine/<name>/ --with-decryption --query 'Parameters[].[Name,Value]' --output text | awk '{n=$1; sub(".*/","",n); print n"="$2}'`
- **AWS Secrets Manager**, one JSON secret `{"DEV1_TOKEN": "…", "DEV2_TOKEN": "…"}`:
  `aws secretsmanager get-secret-value --secret-id iter/engine --query SecretString --output text | jq -r 'to_entries[]|"\(.key)=\(.value)"'`
- **1Password CLI**: `op inject -i /opt/iter/iter_engine/secrets/tokens.tpl` (a template of `DEV1_TOKEN={{ op://vault/item/field }}` lines), or `printf 'DEV1_TOKEN=%s\n' "$(op read op://vault/item/field)"`
- **macOS Keychain**: `printf 'DEV1_TOKEN=%s\n' "$(security find-generic-password -s iter-DEV1_TOKEN -w)"` (store it with `security add-generic-password -s iter-DEV1_TOKEN -a "$USER" -w`)

## HTTPS and iter_data's certificate

The iter_data URL must be `https://`. iter_data serves HTTPS by default, and the engine never accepts plain `http://` across a network: the script stops on an `http://` URL unless the host is this machine (`127.0.0.1`, `localhost`, `::1`).

iter_data's certificate is usually self-signed, so the engine does not ask a certificate authority about it. It pins the certificate's **fingerprint** (`sha256:` + 64 hex) in `config/server_trust.json`, one entry per server:

- **Given up front** (recommended): `--fingerprint sha256:…`, or paste it when the script asks. The iter_data installer prints it, and the webui shows it in the 🔑 Engine keys screen. Right after the binary is installed, the script runs `iter_engine key trust --data-url URL --fingerprint …`, which pins it only if the server presents that same certificate. If it does not, the script stops: something else answers at that URL.
- **Trust on first use**: press Enter at the question, and the first signed sign-in pins the certificate it sees. Compare it afterwards with `iter_engine key status`, which prints the trusted certificate.
- **A TLS proxy with a real certificate** in front of iter_data: `--trust-ca` (`iter_engine key trust --ca`) checks it against certificate authorities instead.

When iter_data's certificate changes on purpose, iter_data announces the next fingerprint at each sign-in, so the engine follows it. A certificate replaced by hand (for example `--tls-cert` on the server) needs `iter_engine key trust --data-url URL --fingerprint …` again; `iter_engine key trust --forget` drops the entry.

Key commands: `iter_engine key show` prints this engine's public key (never the private one), `iter_engine key status` checks the key and the trusted certificate, and `iter_engine key rotate --data-url URL` replaces the key: the current key signs the new one, so no webui step is needed (a revoked key cannot rotate). The engine signs in only with its key.

## Upgrade

```bash
curl -fsSL https://raw.githubusercontent.com/Stephen-Hilton/iter/main/installers/iter_engine/linux/install.sh | bash -s -- update --version v2026.10.17
```

The script swaps the binary and restarts the service. It refuses while the engine runs work: set the projects to Draining first, or pass `--yes`.

## Uninstall

`install.sh uninstall` stops and removes the service and the binary. It keeps `<iter dir>/iter_engine` (the key, tokens, model and logs) and the checkouts; delete that folder to remove the engine completely. Remove the engine's key and serves edges in the webui.

Files: `<iter dir>/iter_engine/` ([above](#where-it-installs-one-folder)), and the links `/etc/systemd/system/iter-engine.service` and `/usr/local/bin/iter_engine`.

To compile it yourself, clone the repo and run `cargo build --release -p iter_engine` (or `installers/build.sh`).
