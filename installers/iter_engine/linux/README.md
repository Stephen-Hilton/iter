# iter engine: Linux

Makes this Linux machine an iter engine, run by systemd as the `iter-engine` service. It works on Debian 11+, Ubuntu 20.04+ and Arch/Omarchy, and mostly on Fedora. The binary needs glibc 2.31 or newer. A machine runs one engine, and that engine serves every project the webui assigns to it. The engine only connects outward: to iter_data, to Anthropic, to the projects' git remote, and once to Hugging Face for the embedding model.

For a distro inside WSL2 on Windows, use `../linux_wsl` instead.

## Install

```bash
curl -fsSL https://raw.githubusercontent.com/Stephen-Hilton/iter/main/installers/iter_engine/linux/install.sh | bash -s -- --version v2026.10.17
```

From a downloaded copy of this folder: `./install.sh`. It uses the `iter_engine-linux-<arch>` file next to it when one is there. Otherwise it downloads that file and `SHA256SUMS` from the GitHub Release (`--version`, else the latest) and checks the sha256.

Run it as your own user; it uses sudo where it must. The steps:

1. Packages: git, curl, jq, poppler, and gh where the distro has it. It asks before installing.
2. The `claude` CLI, from Anthropic's installer into `~/.local/bin`.
3. The iter_data URL and the engine name, saved to `~/.iter/engine.conf`.
4. `~/.iter/bin/iter_engine`, written to a temp name and then moved into place.
5. The embedding model into `~/.iter/models`, checksum-verified.
6. The engine key. `iter_engine key new` makes it; you paste its public key into the webui's Engine keys screen, and the script waits until iter_data accepts it.
7. On a laptop: no suspend, and on Omarchy no lock screen. It asks first.
8. The `iter-engine` systemd service.
9. Account tokens (`claude setup-token`, saved to `~/.iter/.env`) and clones of the served projects.

Prerequisites: sudo, systemd, outbound HTTPS. No Rust and no compiling.

## Options

| | |
|---|---|
| `--data-url URL` | the iter_data URL (otherwise it asks) |
| `--name NAME` | the engine name, unique per machine (default: the hostname) |
| `--version TAG` | the release to download (else `$ITER_VERSION`, else the latest) |
| `--binary PATH` | install this iter_engine file instead of downloading one |
| `--yes` | answer yes to every question |
| `--dry-run` | print every action and change nothing |

Verbs: `install` (the default), `status`, `update`, `accounts`, `projects`, `logs`, `uninstall`.

Projects: in the webui's Settings tab, draw a `serves` edge from the engine to the project with the topdir `~/dev/<project>`, then run `install.sh projects`.

## Upgrade

```bash
curl -fsSL https://raw.githubusercontent.com/Stephen-Hilton/iter/main/installers/iter_engine/linux/install.sh | bash -s -- update --version v2026.10.17
```

The script swaps the binary and restarts the service. It refuses while the engine runs work: set the projects to Draining first, or pass `--yes`.

## Uninstall

`install.sh uninstall` stops and removes the service and the binary. It keeps `~/.iter` (the key, tokens, model and logs) and the checkouts. Remove the engine's key and serves edges in the webui.

Files: `~/.iter/` (`bin/iter_engine`, `engine.key`, `.env`, `engine.conf`, `models/`, `engine.log`, `engine.err.log`) and `/etc/systemd/system/iter-engine.service`.

To compile it yourself, clone the repo and run `cargo build --release -p iter_engine` (or `installers/build.sh`).
