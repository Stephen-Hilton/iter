---
id: cac0e3d9-a824-491a-bd07-4cc292e2eb4d
name: "linux — native systemd install (server, engine, backups)"
desc: "install.sh installs iter5 natively on a systemd Linux, including a WSL2 distro on Windows: ArangoDB (the docker image's own static build), iter_data on :8400 and an online backup timer as systemd services in one 6 GB slice, and this machine's iter_engine as a service. Everything except the database lives in ~/.iter5. On Windows this replaces the docker container: deploy.ps1 startup/autostart bring the distro up at logon."
creator: "stephen"
teststate: inherit
level: container
owner: bespoke
children:
  codedirs:  ["{thisfiledir}/"]
  codenodes: []
  tests:     []
  reqs:      []
timestamps: {create: "2026-10-09 03:58:48Z", last_modified: "2026-10-09 03:58:48Z", last_tested: ""}
---
# linux — native systemd install (server, engine, backups)

The docker container (`docker/`) runs ArangoDB and iter_data in one box. This
directory installs the same two programs, plus the engine and a backup timer,
as systemd services on the host itself. On Windows the host is a WSL2 distro
(Debian, `[boot] systemd=true`): the database lives on the distro's own disk
instead of a slow Windows-drive mount, and a Linux-heavy project gets a native
Linux engine beside the Windows one.

```bash
./install.sh server              # ArangoDB + iter_data (:8400) + backups every 6h
./install.sh engine --name NAME  # this machine's iter_engine as a service
./install.sh update [--yes]      # rebuild + reinstall binaries, restart what runs
./install.sh backup | status
```

## What `server` installs

- **ArangoDB**, copied out of the official image (`$ARANGO_IMAGE`, default
  `arangodb:3.12.12`). Its binaries are static, so the server runs the exact
  build the container ran; the Debian package repository stops at an older
  3.12. Config `arangod.conf` (localhost only, `vector-index = true`), data in
  `/var/lib/arangodb3` (linked as `~/.iter5/iter_data/arango`), first-start root
  password from `/etc/arangodb3/root.env`.
- **iter_data**, built from this workspace into `~/.iter5/bin`, with the
  embedding model in `~/.iter5/models` and its environment in
  `~/.iter5/data.env` (admin password and JWT secret from `$ITER_ENV_FILE`,
  default an existing `data.env`, else the repo `.env`; the Arango URL,
  credentials and model path). It listens on `0.0.0.0:8400`, which a WSL distro
  forwards to Windows as `127.0.0.1:8400`. Log: `~/.iter5/iter_data.log`.
- **`iter.slice`** holds both services: `MemoryMax=6G`, no swap, and arangod is
  told it has 5 GB (`ARANGODB_OVERRIDE_DETECTED_TOTAL_MEMORY`), because it sizes
  caches and query limits from the machine's RAM, not from a cgroup cap. These
  are the container's limits.
- **Backups**: `iter-backup.timer` runs `/usr/local/bin/iter-backup` every 6
  hours. `arangodump` reads a consistent snapshot while arangod keeps serving,
  so nothing stops. Each run writes one `iter5-<utc time>.tar` (the dump plus
  `data.env` and iter_data's secret files) to `$ITER_BACKUP_DIR` (default
  `~/.iter5/backups`; set it in `~/.iter5/backup.env`, e.g. a `/mnt/c/...` path
  so backups outlive the distro) and keeps the newest `$ITER_BACKUP_KEEP` (28).
  The restore command is in the script's header.

## `engine`

Builds iter_engine into `~/.iter5/bin` and runs it as `iter-engine.service`
under the invoking user, with `~/.local/bin` and `~/.cargo/bin` on PATH for the
agents' tools. It needs `~/.iter5/.env` with `ITER_ENGINE_TOKEN` (written by
`tools/iter_engine_setup.sh`). A stop kills the whole control group, so the
agent sessions it started stop with it: `update` refuses to restart a running
engine without `--yes`.

## Unit templates

`systemd/*` are rendered with `@USER@`, `@HOME@`, `@PORT@` and `@ENGINE@` into
`/etc/systemd/system/`.
