# iter engine: Windows (native)

Makes this Windows machine an iter engine that runs natively, with no WSL. It suits projects that build with Windows tools (Unreal-style). For Bash- and Linux-heavy projects, use an engine in a WSL2 distro (`../linux_wsl`). One engine serves every project the webui assigns to it.

## Install

In the webui, an admin opens Engine keys and clicks Add an engine, which makes a one-time join code. Then, in PowerShell on this machine:

```powershell
irm https://raw.githubusercontent.com/Stephen-Hilton/iter/main/installers/iter_engine/windows/install.ps1 -OutFile $env:TEMP\iter-install.ps1; powershell -NoProfile -ExecutionPolicy Bypass -File $env:TEMP\iter-install.ps1 -Join CODE
```

The join code carries iter_data's URL, the certificate to trust and a one-time secret that registers this machine's key, so nothing is pasted back into the webui. It can name the engine too; if another machine has that name already, iter_data records another one (for example `host-2`) and the script writes it to `config\engine.conf`. The code expires; make a new one if it has.

Without a join code, the script asks for the URL and the certificate fingerprint and shows a key to paste into the webui:

```powershell
irm https://raw.githubusercontent.com/Stephen-Hilton/iter/main/installers/iter_engine/windows/install.ps1 -OutFile $env:TEMP\iter-install.ps1; powershell -NoProfile -ExecutionPolicy Bypass -File $env:TEMP\iter-install.ps1 -Version v2026.10.17
```

From a downloaded copy of this folder: `install.cmd`, which runs `install.ps1` with ExecutionPolicy Bypass and passes the arguments through. It uses `iter_engine-windows-x86_64.exe` next to it if present. Otherwise it downloads that file from the GitHub Release and checks it against `SHA256SUMS`. ARM64 Windows runs the x64 build through emulation.

Everything the script puts on this machine for iter goes in one folder, the **iter dir**: `C:\iter` unless you pass `-IterDir` (or set `ITER_DIR`). The engine's part is `<iter dir>\iter_engine`:

| Folder | What |
|---|---|
| `bin\` | `iter_engine.exe`, and `iter-engine.ps1` (the logon task's copy of this script); on your user PATH |
| `config\` | `engine.conf` (iter_data URL, engine name, secrets command), `server_trust.json` (iter_data's pinned certificate) |
| `secrets\` | `engine.key`, `.env` (account tokens); readable by you (and SYSTEM) only |
| `models\` | the embedding model |
| `logs\` | `engine.log`, `engine.err.log`, `engine.pid` |

`-IterDir` always ends in a folder named `iter`: `-IterDir D:\tools\iter\`, `-IterDir D:\tools\iter` and `-IterDir D:\tools` all give `D:\tools\iter`. Later runs (`status`, `update`, `uninstall`) find the iter dir from the logon task, so you give it only once. Run `install` again with another `-IterDir` and the script stops the engine, moves `iter_engine\` there and registers the logon task again. The engine runs with `ITER_HOME` set to `<iter dir>\iter_engine`; from a shell, `iter_engine key status` finds the folder from where the exe sits.

**An install from before 2026-10-18** kept everything in `~\.iter`. `install` or `update` moves it once: each file to its folder above, anything else to `iter_engine\old-dot-iter\`. Nothing is deleted, and `~\.iter` is removed only when it is empty.

The steps:

1. Git for Windows, installed with winget if it is missing (the script asks first). Its `bash.exe` runs the shell steps; WSL's bash is never used.
2. The `claude` CLI (Anthropic's `install.ps1`). The script warns when `pdftoppm` is missing.
3. `config\engine.conf` (the iter_data URL and the engine name; with `-Join`, both from the code).
4. `bin\iter_engine.exe`, and `bin\` on your user PATH. Windows will not overwrite a running exe, so the script stops the engine first, then writes a temp file and moves it into place. Then iter_data's certificate: with `-Join` the code says which one, and step 6 trusts it; otherwise `-Fingerprint` (or the one you paste) is pinned with `iter_engine key trust` (see [HTTPS](#https-and-iter_datas-certificate)).
5. The embedding model into `models\`.
6. The engine key. With `-Join`, `iter_engine key join` trusts iter_data as the code says, makes the key, registers it and signs in (when the key exists and iter_data accepts it already, the code is not needed). Otherwise you paste its public key into the webui's Engine keys screen, and the script waits until iter_data accepts it. Then iter_data's local certificate authority goes into your Trusted Root Certification Authorities (Windows asks to confirm), so Edge and Chrome open the webui as secure ([below](#https-and-iter_datas-certificate)).
7. On a laptop: no sleep on AC power, and closing the lid does nothing on AC (powercfg). The script asks first.
8. The logon task `iter engine`. It runs `bin\iter-engine.ps1 start -IterDir <iter dir>`, a copy of this script, which starts the engine hidden and writes its pid to `logs\engine.pid`. The script then starts the engine.
9. Account tokens and project checkouts. Give `serves` edges Windows topdirs with forward slashes, e.g. `C:/Users/me/dev/project`.

The engine itself writes `.iter\bin\iter` and `iter.cmd` into each checkout.

## Options

`-Join CODE` (a join code from the webui's Engine keys, Add an engine; also `ITER_JOIN`: the URL, the certificate and the key's registration in one; it overrides `-DataUrl`, and `-Name` when it names the engine), `-IterDir DIR` (default `C:\iter`; also `-Iter_Dir`), `-DataUrl URL` (`https://`), `-Name NAME`, `-Version TAG`, `-Binary PATH`, `-Fingerprint FP` (without a join code: iter_data's certificate, pinned before the first sign-in), `-TrustCa` (without a join code: a TLS proxy with a public certificate; also chosen by itself when the engine meets one), `-SecretsCommand CMD` / `-SecretsEvery N` (account tokens from a secret store, [below](#account-tokens-from-a-secret-store)), `-Yes`, `-DryRun`.
Modes: `install` (default), `status`, `update`, `accounts`, `projects`, `start`, `stop`, `logs`, `uninstall`.

## Account tokens from a secret store

By default the account tokens (`claude setup-token`, one per `token_envar`) sit in `secrets\.env` and the accounts step asks you to paste them. Instead, `-SecretsCommand CMD` names a command that prints `KEY=value` lines. The engine runs it at startup and every `-SecretsEvery N` minutes (default 5) and keeps the values in memory only; it never logs them, and it logs the names of tokens it is still missing. The script keeps the command in `config\engine.conf`, puts it on the engine's command line (the logon task runs `iter-engine.ps1 start`, which reads `engine.conf`), and the accounts step then asks for no tokens: it runs the command once and reports which needed names it prints (names only). `-SecretsCommand none` removes it. On Windows it runs with `cmd /C`, so a PowerShell recipe is `powershell -NoProfile -Command "…"`, and the `aws`/`op` lines below work in cmd as long as their quoting suits cmd (or put them in a `.cmd`/`.ps1` file and name that).

The sources rank: environment variables the service is started with (highest) > the secrets command > `secrets\.env`. Variables set in the environment the engine starts with (user or machine environment variables) already work and win over everything.

Recipes (each prints `DEV1_TOKEN=…` lines; use your own names and paths):

- **AWS SSM Parameter Store**, one SecureString per token under `/iter/engine/<name>/` (the parameter's last path part is the variable name):
  `aws ssm get-parameters-by-path --path /iter/engine/<name>/ --with-decryption --query 'Parameters[].[Name,Value]' --output text | awk '{n=$1; sub(".*/","",n); print n"="$2}'`
- **AWS Secrets Manager**, one JSON secret `{"DEV1_TOKEN": "…", "DEV2_TOKEN": "…"}`:
  `aws secretsmanager get-secret-value --secret-id iter/engine --query SecretString --output text | jq -r 'to_entries[]|"\(.key)=\(.value)"'`
- **1Password CLI**: `op inject -i C:/iter/iter_engine/config/tokens.tpl` (a template of `DEV1_TOKEN={{ op://vault/item/field }}` lines), or `printf 'DEV1_TOKEN=%s\n' "$(op read op://vault/item/field)"`

## HTTPS and iter_data's certificate

The iter_data URL must be `https://`. iter_data serves HTTPS by default, and the engine never accepts plain `http://` across a network: the script stops on an `http://` URL unless the host is this machine (`127.0.0.1`, `localhost`, `::1`).

iter_data's certificate is usually its own (signed by a local certificate authority iter_data makes, not a public one), so the engine does not ask a certificate authority about it. It pins the certificate's **fingerprint** (`sha256:` + 64 hex) in `config\server_trust.json`, one entry per server:

- **A join code** (recommended): `-Join CODE` carries the certificate to trust, so there is nothing to compare. When iter_data is behind a proxy with a publicly trusted certificate, the engine checks it against certificate authorities instead.
- **Given up front**, without a join code: `-Fingerprint sha256:…`, or paste it when the script asks. The iter_data installer prints it, and the webui shows it in the 🔑 Engine keys screen. Right after the binary is installed, the script runs `iter_engine key trust --data-url URL --fingerprint …`, which pins it only if the server presents that same certificate. If it does not, the script stops: something else answers at that URL.
- **Trust on first use**: press Enter at the question, and the first signed sign-in pins the certificate it sees. Compare it afterwards with `iter_engine key status`, which prints the trusted certificate.
- **A TLS proxy with a public certificate** in front of iter_data, without a join code: `-TrustCa` (`iter_engine key trust --ca`) checks it against certificate authorities instead. The engine also chooses this by itself when it meets a publicly trusted certificate for the host.

When iter_data's certificate changes on purpose, iter_data announces the next fingerprint at each sign-in, so the engine follows it. A certificate replaced by hand (for example `--tls-cert` on the server) needs `iter_engine key trust --data-url URL --fingerprint …` again; `iter_engine key trust --forget` drops the entry.

**Browsers on this machine.** iter_data (`--tls auto`) signs its certificate with a local certificate authority of its own, name-constrained to that server's names, `.local`/`.lan`/`.internal`/`.home.arpa` and loopback/private addresses, so it cannot vouch for any public site. Once iter_data accepts the key, the script fetches that CA over the pinned connection (`iter_engine key ca --data-url URL`) and imports it into `Cert:\CurrentUser\Root` (Windows asks you to confirm), or `Cert:\LocalMachine\Root` when the script runs as administrator. Edge and Chrome use that store, so the webui opens as secure after a browser restart. Skipped when a certificate with the same thumbprint is there already, for plain `http://`, and when iter_data is trusted through certificate authorities (`-TrustCa`, or `mode` `ca` in `server_trust.json`); a failure is only a warning.

The script's own API calls check the certificate the same way (a pinned .NET request). Key commands: `iter_engine key show` prints this engine's public key (never the private one), `iter_engine key status` checks the key and the trusted certificate, and `iter_engine key rotate --data-url URL` replaces the key: the current key signs the new one, so no webui step is needed (a revoked key cannot rotate). The engine signs in only with its key.

## Upgrade

`install.cmd update -Version vYYYY.MM.DD` replaces the binary and the `iter-engine.ps1` copy, then restarts the engine. It refuses while work runs, unless you pass `-Yes`.

## Uninstall

`install.cmd uninstall` stops the engine (`taskkill /T`, which stops its agent sessions too) and removes the logon task, the exe, `iter-engine.ps1` and the PATH entry. It keeps the rest of `<iter dir>\iter_engine` (config, key, tokens, model, logs).

## Not part of this installer

The WSL server orchestration in the repo's `deploy.ps1` (`startup`, `autostart`, `restart-wsl`, the `iter startup` logon task, the `.wslconfig` memory reclaim, and the Docker Desktop stop and start around `wsl --shutdown`) concerns iter_data running in a WSL distro, not the engine. This installer replaces only `deploy.ps1`'s engine parts: `build`, `start`, `stop`, `status`. It takes over the logon task name `iter engine` that the old root `install.ps1` registered. If the `iter startup` task still exists, it also starts a Windows engine from the repo, and the script warns about it.

To compile it yourself, clone the repo and run `cargo build --release -p iter_engine` (or `installers/build.sh`).
