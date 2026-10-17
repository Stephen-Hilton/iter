# iter engine: Docker

Runs this machine's iter engine in a container: Debian bookworm-slim, git, bash, the `claude` CLI and the prebuilt `iter_engine-linux-<arch>` for the Docker host's platform. It works on a Linux host, or on macOS with Docker Desktop. Nothing is compiled.

## Limits

- **One engine per machine still.** Do not run this beside a native engine on the same host.
- **The agents run inside the container.** That covers claude sessions, shell steps and tests. They see only what the image has (git, bash, curl, jq, poppler) and the mounted folders. A project that needs the host's toolchain, Docker, a GUI or Windows tools wants a native engine instead. To add tools, extend the `Dockerfile` in `~/.iter/docker`.
- **Credentials come from files.** Claude logs in with the account tokens in `~/.iter/.env` (`claude setup-token`), and the host's own claude login is not shared. Git uses the host's `~/.ssh`, `~/.gitconfig`, `~/.git-credentials` and `~/.config/gh`, mounted read-only when present. macOS keychain or ssh-agent credentials are not visible inside.
- **Same paths inside and outside.** `~/.iter` and the checkouts folder (default `~/dev`) are mounted at the same absolute paths. The container runs as your uid/gid with HOME set to your home path, so `~/dev/<project>` topdirs match and the files it writes are yours. Every serves edge's topdir must be inside the checkouts folder. The `.iter/bin/iter` shim the engine writes into a checkout works only inside the container.
- **The network is the container's.** For iter_data on the same host, use `http://host.docker.internal:<port>`; the script rewrites `127.0.0.1` and `localhost` for you.
- Bind mounts on Docker Desktop for Mac are slower than a native disk. The image is about 850 MB.

## Install

```bash
curl -fsSL https://raw.githubusercontent.com/Stephen-Hilton/iter/main/installers/iter_engine/docker/install.sh | bash -s -- --version v2026.10.17
```

Prerequisites: Docker with compose v2, with your user able to run `docker`, plus curl and git on the host.

The script sets up `~/.iter/docker` as the build folder. Into it go `Dockerfile`, `entrypoint.sh` and `compose.yml` (copied from beside the script, else downloaded from GitHub at `--version`), the binary (from beside the script, `--binary`, or the release, sha256-checked), `.env` with the compose settings, and `compose.override.yml` with the git credential mounts. It then fetches the embedding model into `~/.iter/models` and builds `iter-engine:local`. Next it makes the engine key inside the image and waits until the webui's Engine keys screen has its public key. Last, `docker compose up -d` starts the container `iter-engine` with `restart: unless-stopped` and docker's init.

## Options

`--data-url URL`, `--name NAME`, `--checkouts DIR` (default `~/dev`), `--version TAG`, `--binary PATH`, `--yes`, `--dry-run`.
Verbs: `install` (default), `status`, `update`, `accounts`, `projects`, `logs`, `uninstall`.

By hand, from `~/.iter/docker`: `docker compose up -d`, `docker compose logs -f`, `docker compose down`. To run a command as the engine's user: `docker compose run --rm --no-deps engine <cmd>`. Note that `docker exec` runs as root.

## Upgrade

`install.sh update --version vYYYY.MM.DD` fetches the new binary, rebuilds the image with `--no-cache` (which also picks up a newer `claude` CLI) and recreates the container. It refuses while work runs, unless you pass `--yes`.

## Uninstall

`install.sh uninstall` runs `docker compose down --rmi local`. It keeps `~/.iter` (the key, tokens, model, `claude/` and `docker/`) and the checkouts.

To compile it yourself, clone the repo and run `cargo build --release -p iter_engine` (or `installers/build.sh`).
