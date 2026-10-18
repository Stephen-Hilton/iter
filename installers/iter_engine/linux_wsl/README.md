# iter engine: Linux in WSL2

Makes a WSL2 distro on a Windows machine (Debian or Ubuntu from `wsl --install`) an iter engine. It runs by systemd as the `iter-engine` service inside the distro. Use it for Bash- and Linux-heavy projects: Git Bash makes every process start expensive on native Windows. For Windows-native builds (Unreal and similar), use `../windows`. One engine serves every project the webui assigns to it.

## Install

Inside the distro, as your own user:

```bash
curl -fsSL https://raw.githubusercontent.com/Stephen-Hilton/iter/main/installers/iter_engine/linux_wsl/install.sh | bash -s -- --version v2026.10.17
```

The steps are the same as `../linux`, and so is the one folder everything goes in: `/opt/iter/iter_engine` unless `--iter-dir` says otherwise (see [../linux](../linux/README.md#where-it-installs-one-folder); an older `~/.iter` is moved in). Keep it on the distro's own disk, never under `/mnt/c`. Plus these WSL specifics:

- **`/etc/wsl.conf`**: `[boot] systemd=true` is required. The script also offers `[interop] appendWindowsPath=false`, which keeps Windows' `claude.exe` and `git.exe` off the Linux PATH. After it writes the file, it stops and asks you to run `wsl --terminate <distro>` in Windows, reopen the distro, and run the script again.
- **The `claude` CLI** must be the Linux one; a `claude` that resolves under `/mnt/c` does not count.
- **Keepalive**: WSL stops a distro, and its services with it, once no Windows program holds it open. The script offers to register a hidden Windows logon task, `iter WSL keepalive (<distro>)`, that runs `wsl.exe -d <distro> --exec /bin/sleep infinity` through `powershell.exe`. To do it by hand, create that task in Task Scheduler.
- **Checkouts** belong on the distro's own disk (`~/dev/<project>`), never on `/mnt/c`, where git and builds are slow. The script warns about topdirs under `/mnt/`.
- **No laptop-suspend step**. Windows' power settings decide sleep. On a laptop, run `powercfg /change standby-timeout-ac 0` in Windows.
- If iter_data runs in the same distro (the iter_data Linux installer), its URL is `https://127.0.0.1:8400` (iter_data serves HTTPS; `http://` only if it runs with `--tls off`).
- **HTTPS**: as in `../linux`, the URL must be `https://` unless iter_data runs in this distro, and the engine pins iter_data's certificate fingerprint in `iter_engine/config/server_trust.json`: `--fingerprint sha256:…` (from the iter_data installer or the webui's 🔑 Engine keys screen), or paste it when asked, or let the first sign-in trust it and compare with `iter_engine key status`. `--trust-ca` for a TLS proxy with a real certificate. See [../linux](../linux/README.md#https-and-iter_datas-certificate) for `key trust`, `key show` and `key rotate`.
- **Account tokens from a secret store**: `--secrets-command CMD` (and `--secrets-every N`), as in [../linux](../linux/README.md#account-tokens-from-a-secret-store), with the recipes there (AWS SSM / Secrets Manager, 1Password). It goes on the unit's `ExecStart`; the accounts step then asks for no tokens.

Options, verbs, files and the binary lookup are the same as `../linux`: `--data-url`, `--name`, `--version`, `--binary`, `--fingerprint`, `--trust-ca`, `--secrets-command`, `--secrets-every`, `--yes`, `--dry-run`; `install`, `status`, `update`, `accounts`, `projects`, `logs`, `uninstall`.

## Upgrade

`install.sh update` (or the curl line with `update`). It refuses while the engine runs work, unless you pass `--yes`.

## Uninstall

`install.sh uninstall` removes the service and the binary and keeps `<iter dir>/iter_engine`. The Windows keepalive task stays, because other WSL services may need it; remove it in Task Scheduler if nothing does.

To compile it yourself, clone the repo and run `cargo build --release -p iter_engine` (or `installers/build.sh`).
