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
4. `~/.iter/bin/iter_engine`, written to a temp file and then moved into place. A `cp` over a live binary gets it SIGKILLed on macOS.
5. The embedding model into `~/.iter/models`.
6. The engine key. You paste its public key into the webui's Engine keys screen, and the script waits until iter_data accepts it.
7. On a MacBook, the script asks (default no) whether to run the engine under `caffeinate -s`. That stops system sleep while the Mac is on power, and only while the engine runs. Closing the lid still sleeps the Mac unless an external display is attached. To keep it awake system-wide instead, run `sudo pmset -c sleep 0`.
8. Loads the launchd agent.
9. Account tokens and project checkouts.

The agent runs while you are logged in and starts at login. For an unattended Mac, turn on automatic login. If an `iter_engine` was started by hand before, stop it: one engine per machine.

## Options

`--data-url URL`, `--name NAME`, `--version TAG`, `--binary PATH`, `--yes`, `--dry-run` (prints everything and writes nothing: no `~/.iter`, no launchctl).
Verbs: `install` (default), `status`, `update`, `accounts`, `projects`, `logs`, `uninstall`.

## Upgrade

`install.sh update` swaps the binary (temp file plus `mv`) and runs `launchctl kickstart -k gui/<uid>/com.iter.engine`. It refuses while work runs, unless you pass `--yes`.

## Uninstall

`install.sh uninstall` runs `launchctl bootout` and removes the plist and the binary. It keeps `~/.iter` (the key, tokens, model and `logs/`).

Files: `~/.iter/` (`bin/iter_engine`, `engine.key`, `.env`, `engine.conf`, `models/`, `logs/engine.log`, `logs/engine.err.log`) and the plist.

To compile it yourself, clone the repo and run `cargo build --release -p iter_engine` (or `installers/build.sh`).
