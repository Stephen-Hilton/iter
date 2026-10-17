# iter engine: macOS

Makes this Mac (Apple silicon or Intel) an iter engine. launchd runs it as a per-user agent, `~/Library/LaunchAgents/com.iter.engine.plist`, with `KeepAlive` set. One engine serves every project the webui assigns to it. The engine only connects outward: to iter_data, Anthropic, the projects' git remote, and once to Hugging Face.

## Install

```bash
curl -fsSL https://raw.githubusercontent.com/Stephen-Hilton/iter/main/installers/iter_engine/macos/install.sh | bash -s -- --version v2026.10.17
```

From a downloaded copy of this folder: `./install.sh`. It uses the `iter_engine-macos-aarch64` or `-x86_64` file next to it, chosen by `uname -m`. Otherwise it downloads that file from the GitHub Release and checks it against `SHA256SUMS`. It runs under macOS's own bash 3.2.

The steps:

1. git and jq. The script installs them from Homebrew only when macOS has none, and installs Homebrew first if needed. poppler is optional. It asks before each install.
2. The `claude` CLI (Anthropic's installer).
3. `~/.iter/engine.conf` (the iter_data URL and the engine name).
4. `~/.iter/bin/iter_engine`, written to a temp file and then moved into place. A `cp` over a live binary gets it SIGKILLed on macOS. Then iter_data's certificate: `--fingerprint` (or the one you paste) is pinned with `iter_engine key trust` (see [HTTPS](#https-and-iter_datas-certificate)).
5. The embedding model into `~/.iter/models`.
6. The engine key. You paste its public key into the webui's Engine keys screen, and the script waits until iter_data accepts it.
7. On a MacBook, the script asks (default no) whether to run the engine under `caffeinate -s`. That stops system sleep while the Mac is on power, and only while the engine runs. Closing the lid still sleeps the Mac unless an external display is attached. To keep it awake system-wide instead, run `sudo pmset -c sleep 0`.
8. Loads the launchd agent.
9. Account tokens and project checkouts.

The agent runs while you are logged in and starts at login. For an unattended Mac, turn on automatic login. If an `iter_engine` was started by hand before, stop it: one engine per machine.

## Options

`--data-url URL` (`https://`), `--name NAME`, `--version TAG`, `--binary PATH`, `--fingerprint FP` (iter_data's certificate, pinned before the first sign-in), `--trust-ca` (a TLS proxy with a real certificate), `--secrets-command CMD` / `--secrets-every N` (account tokens from a secret store, [below](#account-tokens-from-a-secret-store)), `--yes`, `--dry-run` (prints everything and writes nothing: no `~/.iter`, no launchctl).
Verbs: `install` (default), `status`, `update`, `accounts`, `projects`, `logs`, `uninstall`.

## Account tokens from a secret store

By default the account tokens (`claude setup-token`, one per `token_envar`) sit in `~/.iter/.env` and the accounts step asks you to paste them. Instead, `--secrets-command CMD` names a command that prints `KEY=value` lines. The engine runs it at startup and every `--secrets-every N` minutes (default 5) and keeps the values in memory only; it never logs them, and it logs the names of tokens it is still missing. The script keeps the command in `~/.iter/engine.conf`, puts it on the launchd agent's `ProgramArguments`, and the accounts step then asks for no tokens: it runs the command once and reports which needed names it prints (names only). `--secrets-command none` removes it. It runs with `sh -c` as you, with the agent's `PATH` (`~/.local/bin:/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin`) and `HOME`.

The sources rank: environment variables the service is started with (highest) > the secrets command > `~/.iter/.env`. Variables injected at start already work and win over everything (an `EnvironmentVariables` entry in the plist, which the script rewrites on install/update).

Recipes (each prints `DEV1_TOKEN=…` lines; use your own names and paths):

- **AWS SSM Parameter Store**, one SecureString per token under `/iter/engine/<name>/` (the parameter's last path part is the variable name):
  `aws ssm get-parameters-by-path --path /iter/engine/<name>/ --with-decryption --query 'Parameters[].[Name,Value]' --output text | awk '{n=$1; sub(".*/","",n); print n"="$2}'`
- **AWS Secrets Manager**, one JSON secret `{"DEV1_TOKEN": "…", "DEV2_TOKEN": "…"}`:
  `aws secretsmanager get-secret-value --secret-id iter/engine --query SecretString --output text | jq -r 'to_entries[]|"\(.key)=\(.value)"'`
- **1Password CLI**: `op inject -i ~/.iter/tokens.tpl` (a template of `DEV1_TOKEN={{ op://vault/item/field }}` lines), or `printf 'DEV1_TOKEN=%s\n' "$(op read op://vault/item/field)"`
- **macOS Keychain**: `printf 'DEV1_TOKEN=%s\n' "$(security find-generic-password -s iter-DEV1_TOKEN -w)"` (store it with `security add-generic-password -s iter-DEV1_TOKEN -a "$USER" -w`)

## HTTPS and iter_data's certificate

The iter_data URL must be `https://`. iter_data serves HTTPS by default, and the engine never accepts plain `http://` across a network: the script stops on an `http://` URL unless the host is this machine (`127.0.0.1`, `localhost`, `::1`).

iter_data's certificate is usually self-signed, so the engine does not ask a certificate authority about it. It pins the certificate's **fingerprint** (`sha256:` + 64 hex) in `~/.iter/server_trust.json`, one entry per server:

- **Given up front** (recommended): `--fingerprint sha256:…`, or paste it when the script asks. The iter_data installer prints it, and the webui shows it in the 🔑 Engine keys screen. Right after the binary is installed, the script runs `iter_engine key trust --data-url URL --fingerprint …`, which pins it only if the server presents that same certificate. If it does not, the script stops: something else answers at that URL.
- **Trust on first use**: press Enter at the question, and the first signed sign-in pins the certificate it sees. Compare it afterwards with `iter_engine key status`, which prints the trusted certificate.
- **A TLS proxy with a real certificate** in front of iter_data: `--trust-ca` (`iter_engine key trust --ca`) checks it against certificate authorities instead.

When iter_data's certificate changes on purpose, iter_data announces the next fingerprint at each sign-in, so the engine follows it. A certificate replaced by hand (for example `--tls-cert` on the server) needs `iter_engine key trust --data-url URL --fingerprint …` again; `iter_engine key trust --forget` drops the entry.

Key commands: `iter_engine key show` prints this engine's public key (never the private one), `iter_engine key status` checks the key and the trusted certificate, and `iter_engine key rotate --data-url URL` replaces the key: the current key signs the new one, so no webui step is needed (a revoked key cannot rotate). The engine signs in only with its key.

## Upgrade

`install.sh update` swaps the binary (temp file plus `mv`) and runs `launchctl kickstart -k gui/<uid>/com.iter.engine`. It refuses while work runs, unless you pass `--yes`.

## Uninstall

`install.sh uninstall` runs `launchctl bootout` and removes the plist and the binary. It keeps `~/.iter` (the key, tokens, model and `logs/`).

Files: `~/.iter/` (`bin/iter_engine`, `engine.key`, `server_trust.json`, `.env`, `engine.conf`, `models/`, `logs/engine.log`, `logs/engine.err.log`) and the plist.

To compile it yourself, clone the repo and run `cargo build --release -p iter_engine` (or `installers/build.sh`).
