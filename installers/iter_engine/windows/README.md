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
4. `~\.iter\bin\iter_engine.exe`. Windows will not overwrite a running exe, so the script stops the engine first, then writes a temp file and moves it into place.
5. The embedding model into `~\.iter\models`.
6. The engine key. You paste its public key into the webui's Engine keys screen, and the script waits until iter_data accepts it.
7. On a laptop: no sleep on AC power, and closing the lid does nothing on AC (powercfg). The script asks first.
8. The logon task `iter engine`. It runs `~\.iter\bin\iter-engine.ps1 start`, a copy of this script, which starts the engine hidden and writes its pid to `~\.iter\engine.pid`. The script then starts the engine.
9. Account tokens and project checkouts. Give `serves` edges Windows topdirs with forward slashes, e.g. `C:/Users/me/dev/project`.

The engine itself writes `.iter\bin\iter` and `iter.cmd` into each checkout.

## Options

`-DataUrl URL`, `-Name NAME`, `-Version TAG`, `-Binary PATH`, `-Yes`, `-DryRun`.
Modes: `install` (default), `status`, `update`, `accounts`, `projects`, `start`, `stop`, `logs`, `uninstall`.

## Upgrade

`install.cmd update -Version vYYYY.MM.DD` replaces the binary and the `iter-engine.ps1` copy, then restarts the engine. It refuses while work runs, unless you pass `-Yes`.

## Uninstall

`install.cmd uninstall` stops the engine (`taskkill /T`, which stops its agent sessions too) and removes the logon task, the exe and `iter-engine.ps1`. It keeps `~\.iter`.

## Not part of this installer

The WSL server orchestration in the repo's `deploy.ps1` (`startup`, `autostart`, `restart-wsl`, the `iter startup` logon task, the `.wslconfig` memory reclaim, and the Docker Desktop stop and start around `wsl --shutdown`) concerns iter_data running in a WSL distro, not the engine. This installer replaces only `deploy.ps1`'s engine parts: `build`, `start`, `stop`, `status`. It takes over the logon task name `iter engine` that the old root `install.ps1` registered. If the `iter startup` task still exists, it also starts a Windows engine from the repo, and the script warns about it.

To compile it yourself, clone the repo and run `cargo build --release -p iter_engine` (or `installers/build.sh`).
