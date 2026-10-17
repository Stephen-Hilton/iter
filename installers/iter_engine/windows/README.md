# iter engine: Windows (native)

Makes this Windows machine an iter engine that runs natively, with no WSL. It suits projects that build with Windows tools (Unreal-style). For Bash- and Linux-heavy projects, use an engine in a WSL2 distro (`../linux_wsl`). One engine serves every project the webui assigns to it.

## Install

In PowerShell:

```powershell
irm https://raw.githubusercontent.com/Stephen-Hilton/iter/main/installers/iter_engine/windows/install.ps1 -OutFile $env:TEMP\iter-install.ps1; powershell -NoProfile -ExecutionPolicy Bypass -File $env:TEMP\iter-install.ps1 -Version v2026.10.17
```

From a downloaded copy of this folder: `install.cmd`, which runs `install.ps1` with ExecutionPolicy Bypass and passes the arguments through. It uses `iter_engine-windows-x86_64.exe` next to it if present. Otherwise it downloads that file from the GitHub Release and checks it against `SHA256SUMS`. ARM64 Windows runs the x64 build through emulation.

The steps:

1. Git for Windows, installed with winget if it is missing (the script asks first). Its `bash.exe` runs the shell steps; WSL's bash is never used.
2. The `claude` CLI (Anthropic's `install.ps1`). The script warns when `pdftoppm` is missing.
3. `~\.iter\engine.conf` (the iter_data URL and the engine name).
4. `~\.iter\bin\iter_engine.exe`. Windows will not overwrite a running exe, so the script stops the engine first, then writes a temp file and moves it into place. Then iter_data's certificate: `-Fingerprint` (or the one you paste) is pinned with `iter_engine key trust` (see [HTTPS](#https-and-iter_datas-certificate)).
5. The embedding model into `~\.iter\models`.
6. The engine key. You paste its public key into the webui's Engine keys screen, and the script waits until iter_data accepts it.
7. On a laptop: no sleep on AC power, and closing the lid does nothing on AC (powercfg). The script asks first.
8. The logon task `iter engine`. It runs `~\.iter\bin\iter-engine.ps1 start`, a copy of this script, which starts the engine hidden and writes its pid to `~\.iter\engine.pid`. The script then starts the engine.
9. Account tokens and project checkouts. Give `serves` edges Windows topdirs with forward slashes, e.g. `C:/Users/me/dev/project`.

The engine itself writes `.iter\bin\iter` and `iter.cmd` into each checkout.

## Options

`-DataUrl URL` (`https://`), `-Name NAME`, `-Version TAG`, `-Binary PATH`, `-Fingerprint FP` (iter_data's certificate, pinned before the first sign-in), `-TrustCa` (a TLS proxy with a real certificate), `-SecretsCommand CMD` / `-SecretsEvery N` (account tokens from a secret store, [below](#account-tokens-from-a-secret-store)), `-Yes`, `-DryRun`.
Modes: `install` (default), `status`, `update`, `accounts`, `projects`, `start`, `stop`, `logs`, `uninstall`.

## Account tokens from a secret store

By default the account tokens (`claude setup-token`, one per `token_envar`) sit in `~/.iter/.env` and the accounts step asks you to paste them. Instead, `-SecretsCommand CMD` names a command that prints `KEY=value` lines. The engine runs it at startup and every `-SecretsEvery N` minutes (default 5) and keeps the values in memory only; it never logs them, and it logs the names of tokens it is still missing. The script keeps the command in `~/.iter/engine.conf`, puts it on the engine's command line (the logon task runs `iter-engine.ps1 start`, which reads `engine.conf`), and the accounts step then asks for no tokens: it runs the command once and reports which needed names it prints (names only). `-SecretsCommand none` removes it. On Windows it runs with `cmd /C`, so a PowerShell recipe is `powershell -NoProfile -Command "…"`, and the `aws`/`op` lines below work in cmd as long as their quoting suits cmd (or put them in a `.cmd`/`.ps1` file and name that).

The sources rank: environment variables the service is started with (highest) > the secrets command > `~/.iter/.env`. Variables set in the environment the engine starts with (user or machine environment variables) already work and win over everything.

Recipes (each prints `DEV1_TOKEN=…` lines; use your own names and paths):

- **AWS SSM Parameter Store**, one SecureString per token under `/iter/engine/<name>/` (the parameter's last path part is the variable name):
  `aws ssm get-parameters-by-path --path /iter/engine/<name>/ --with-decryption --query 'Parameters[].[Name,Value]' --output text | awk '{n=$1; sub(".*/","",n); print n"="$2}'`
- **AWS Secrets Manager**, one JSON secret `{"DEV1_TOKEN": "…", "DEV2_TOKEN": "…"}`:
  `aws secretsmanager get-secret-value --secret-id iter/engine --query SecretString --output text | jq -r 'to_entries[]|"\(.key)=\(.value)"'`
- **1Password CLI**: `op inject -i ~/.iter/tokens.tpl` (a template of `DEV1_TOKEN={{ op://vault/item/field }}` lines), or `printf 'DEV1_TOKEN=%s\n' "$(op read op://vault/item/field)"`

## HTTPS and iter_data's certificate

The iter_data URL must be `https://`. iter_data serves HTTPS by default, and the engine never accepts plain `http://` across a network: the script stops on an `http://` URL unless the host is this machine (`127.0.0.1`, `localhost`, `::1`).

iter_data's certificate is usually self-signed, so the engine does not ask a certificate authority about it. It pins the certificate's **fingerprint** (`sha256:` + 64 hex) in `~\.iter\server_trust.json`, one entry per server:

- **Given up front** (recommended): `-Fingerprint sha256:…`, or paste it when the script asks. The iter_data installer prints it, and the webui shows it in the 🔑 Engine keys screen. Right after the binary is installed, the script runs `iter_engine key trust --data-url URL --fingerprint …`, which pins it only if the server presents that same certificate. If it does not, the script stops: something else answers at that URL.
- **Trust on first use**: press Enter at the question, and the first signed sign-in pins the certificate it sees. Compare it afterwards with `iter_engine key status`, which prints the trusted certificate.
- **A TLS proxy with a real certificate** in front of iter_data: `-TrustCa` (`iter_engine key trust --ca`) checks it against certificate authorities instead.

When iter_data's certificate changes on purpose, iter_data announces the next fingerprint at each sign-in, so the engine follows it. A certificate replaced by hand (for example `--tls-cert` on the server) needs `iter_engine key trust --data-url URL --fingerprint …` again; `iter_engine key trust --forget` drops the entry.

The script's own API calls check the certificate the same way (a pinned .NET request). Key commands: `iter_engine key show` prints this engine's public key (never the private one), `iter_engine key status` checks the key and the trusted certificate, and `iter_engine key rotate --data-url URL` replaces the key: the current key signs the new one, so no webui step is needed (a revoked key cannot rotate). The engine signs in only with its key.

## Upgrade

`install.cmd update -Version vYYYY.MM.DD` replaces the binary and the `iter-engine.ps1` copy, then restarts the engine. It refuses while work runs, unless you pass `-Yes`.

## Uninstall

`install.cmd uninstall` stops the engine (`taskkill /T`, which stops its agent sessions too) and removes the logon task, the exe and `iter-engine.ps1`. It keeps `~\.iter`.

## Not part of this installer

The WSL server orchestration in the repo's `deploy.ps1` (`startup`, `autostart`, `restart-wsl`, the `iter startup` logon task, the `.wslconfig` memory reclaim, and the Docker Desktop stop and start around `wsl --shutdown`) concerns iter_data running in a WSL distro, not the engine. This installer replaces only `deploy.ps1`'s engine parts: `build`, `start`, `stop`, `status`. It takes over the logon task name `iter engine` that the old root `install.ps1` registered. If the `iter startup` task still exists, it also starts a Windows engine from the repo, and the script warns about it.

To compile it yourself, clone the repo and run `cargo build --release -p iter_engine` (or `installers/build.sh`).
