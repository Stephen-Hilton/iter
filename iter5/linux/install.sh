#!/usr/bin/env bash
# Install iter5 natively on a systemd Linux (Debian/Ubuntu; also a WSL2 distro
# with systemd=true), as systemd services instead of the docker container.
#
#   ./install.sh server              ArangoDB + iter_data (:8400) + backups every 6h
#   ./install.sh engine --name NAME  this machine's iter_engine as a service
#   ./install.sh update [--yes]      rebuild + reinstall the binaries, restart what runs
#   ./install.sh backup              take a backup now (online, nothing stops)
#   ./install.sh status              the services, health and the last backups
#
# Everything lands in the invoking user's ~/.iter5 (binaries in bin/, the
# embedding model in models/, data.env, logs), except the database:
# /var/lib/arangodb3 (linked as ~/.iter5/iter_data/arango). ArangoDB comes from
# the official arangodb image ($ARANGO_IMAGE): its binaries are static, so the
# server runs the same build the container did. Needs docker (once, for that
# copy), cargo, curl, jq and sudo.
#
# server reads ITER_ADMIN_PASSWORD and ITER_JWT_SECRET from $ITER_ENV_FILE
# (default: an existing ~/.iter5/data.env, else the repo's ../.env). Keep the
# JWT secret: every engine and user token is signed with it.
set -euo pipefail

HERE="$(cd "$(dirname "$0")" && pwd)"
ROOT="$(cd "$HERE/.." && pwd)"
ME="$(id -un)"
H5="$HOME/.iter5"
PORT="${ITER_PORT:-8400}"
ARANGO_PW="${ARANGO_ROOT_PASSWORD:-iter4dev}"
ARANGO_IMAGE="${ARANGO_IMAGE:-arangodb:3.12.12}"
CARGO="${CARGO:-$(command -v cargo || echo "$HOME/.cargo/bin/cargo")}"

say() { echo "[install] $*"; }
die() { echo "[install] $*" >&2; exit 1; }
envval() { [ -f "$2" ] && grep -E "^\s*(export\s+)?$1=" "$2" | tail -1 | sed -E 's/^[^=]*=//; s/^["'\'']//; s/["'\'']$//' | tr -d '\r' || true; }

# render a unit template (@USER@ @HOME@ @PORT@ @ENGINE@) into /etc/systemd/system
unit() {
  sed -e "s|@USER@|$ME|g" -e "s|@HOME@|$HOME|g" -e "s|@PORT@|$PORT|g" -e "s|@ENGINE@|${ENGINE:-}|g" \
    "$HERE/systemd/$1" | sudo tee "/etc/systemd/system/$1" >/dev/null
}

need_systemd() { [ "$(ps -p 1 -o comm=)" = systemd ] || die "systemd is not running (in WSL: [boot] systemd=true in /etc/wsl.conf, then wsl --terminate <distro>)"; }

wait_health() {
  for _ in $(seq 1 120); do
    curl -fsS "http://127.0.0.1:$PORT/health" 2>/dev/null | grep -q '"ok":true' && { say "iter_data up: $(curl -fsS "http://127.0.0.1:$PORT/health")"; return 0; }
    sleep 1
  done
  tail -20 "$H5/iter_data.log" 2>/dev/null; die "iter_data did not come up"
}

build() {  # build <crate>... -> ~/.iter5/bin
  say "building $* (release)"
  (cd "$ROOT" && "$CARGO" build --release $(printf -- '-p %s ' "$@"))
  mkdir -p "$H5/bin"
  for b in "$@"; do install -m 755 "$ROOT/target/release/$b" "$H5/bin/$b"; done
}

install_arango() {
  local want; want="${ARANGO_IMAGE##*:}"
  if [ -x /usr/sbin/arangod ] && /usr/sbin/arangod --version 2>/dev/null | head -1 | grep -qx "$want"; then
    say "ArangoDB $want already installed"
  else
    command -v docker >/dev/null || die "docker is needed once, to copy ArangoDB out of $ARANGO_IMAGE"
    say "copying ArangoDB out of $ARANGO_IMAGE"
    docker pull -q "$ARANGO_IMAGE" >/dev/null
    local c t; c=$(docker create "$ARANGO_IMAGE"); t=$(mktemp -d)
    docker cp "$c:/usr/sbin/arangod" "$t/"
    docker cp "$c:/usr/share/arangodb3" "$t/share"
    docker cp "$c:/etc/arangodb3" "$t/etc"
    for b in arangodump arangorestore arangosh arangobackup arangoexport arangoimport; do docker cp "$c:/usr/bin/$b" "$t/"; done
    docker rm -f "$c" >/dev/null
    sudo systemctl stop arangodb3 2>/dev/null || true
    sudo install -m 755 "$t/arangod" /usr/sbin/arangod
    for b in arangodump arangorestore arangosh arangobackup arangoexport arangoimport; do sudo install -m 755 "$t/$b" "/usr/bin/$b"; done
    sudo rm -rf /usr/share/arangodb3 && sudo cp -r "$t/share" /usr/share/arangodb3
    sudo mkdir -p /etc/arangodb3 && sudo cp "$t"/etc/*.conf /etc/arangodb3/ 2>/dev/null || true
    rm -rf "$t"
  fi
  sudo install -m 644 "$HERE/arangod.conf" /etc/arangodb3/arangod.conf
  id arangodb >/dev/null 2>&1 || sudo useradd --system --home-dir /var/lib/arangodb3 --shell /usr/sbin/nologin arangodb
  sudo install -d -o arangodb -g arangodb -m 750 /var/lib/arangodb3 /var/lib/arangodb3-apps
  echo 'vm.max_map_count = 1024000' | sudo tee /etc/sysctl.d/60-arangodb.conf >/dev/null
  sudo sysctl -q -p /etc/sysctl.d/60-arangodb.conf || true
  # read by arangod only when it initialises an empty database directory
  printf 'ARANGODB_DEFAULT_ROOT_PASSWORD=%s\n' "$ARANGO_PW" | sudo tee /etc/arangodb3/root.env >/dev/null
  sudo chmod 600 /etc/arangodb3/root.env
}

server() {
  need_systemd
  local src="${ITER_ENV_FILE:-}"
  [ -n "$src" ] || { [ -f "$H5/data.env" ] && src="$H5/data.env" || src="$ROOT/../.env"; }
  [ -n "$(envval ITER_JWT_SECRET "$src")" ] || die "ITER_JWT_SECRET is not set in $src (set ITER_ENV_FILE)"

  install_arango
  build iter_data
  "$ROOT/tools/fetch_model.sh"
  mkdir -p "$H5/models" && rsync -a "$ROOT/models/all-MiniLM-L6-v2" "$H5/models/"
  install -d -m 700 "$H5/iter_data"
  ln -sfn /var/lib/arangodb3 "$H5/iter_data/arango"

  local tmp; tmp=$(mktemp)
  ( umask 077; cat > "$tmp" <<EOF
ITER_ADMIN_PASSWORD=$(envval ITER_ADMIN_PASSWORD "$src")
ITER_JWT_SECRET=$(envval ITER_JWT_SECRET "$src")
ARANGO_URL=http://127.0.0.1:8529
ARANGO_USER=root
ARANGO_PASSWORD=$ARANGO_PW
ARANGO_DB=iter5
ITER_EMBED_MODEL=$H5/models/all-MiniLM-L6-v2
EOF
  )
  install -m 600 "$tmp" "$H5/data.env" && rm -f "$tmp"

  sudo install -m 755 "$HERE/iter-backup" /usr/local/bin/iter-backup
  for u in iter.slice arangodb3.service iter-data.service iter-backup.service iter-backup.timer; do unit "$u"; done
  sudo systemctl daemon-reload
  sudo systemctl enable -q --now arangodb3
  sudo systemctl enable -q iter-data && sudo systemctl restart iter-data
  sudo systemctl enable -q --now iter-backup.timer
  wait_health
  local dir; dir="$(envval ITER_BACKUP_DIR "$H5/backup.env")"
  say "webui: http://127.0.0.1:$PORT/  (login: admin / ITER_ADMIN_PASSWORD); backups every 6h -> ${dir:-$H5/backups} (set ITER_BACKUP_DIR in $H5/backup.env)"
}

engine() {
  need_systemd
  ENGINE="${ENGINE:-$(hostname -s)}"
  [ -n "$(envval ITER_ENGINE_TOKEN "$H5/.env")" ] || die "no ITER_ENGINE_TOKEN in $H5/.env: run tools/iter_engine_setup.sh first (it writes the env file)"
  build iter_engine
  unit iter-engine.service
  sudo systemctl daemon-reload
  sudo systemctl enable -q iter-engine && sudo systemctl restart iter-engine
  sleep 3
  systemctl is-active -q iter-engine || { tail -20 "$H5/engine.err.log"; die "iter-engine did not start"; }
  tail -3 "$H5/engine.log"
}

update() {
  local yes=0; [ "${1:-}" = --yes ] && yes=1
  if [ $yes = 0 ] && systemctl is-active -q iter-engine; then
    die "restarting iter-engine stops its running agent sessions: check the projects' in-progress items, then rerun with --yes"
  fi
  local crates=()
  systemctl is-enabled -q iter-data 2>/dev/null && crates+=(iter_data)
  systemctl is-enabled -q iter-engine 2>/dev/null && crates+=(iter_engine)
  [ ${#crates[@]} -gt 0 ] || die "nothing installed yet (install.sh server | engine)"
  build "${crates[@]}"
  systemctl is-enabled -q iter-data 2>/dev/null && { sudo systemctl restart iter-data; wait_health; }
  systemctl is-enabled -q iter-engine 2>/dev/null && sudo systemctl restart iter-engine
  say "updated: ${crates[*]}"
}

status() {
  for s in arangodb3 iter-data iter-engine iter-backup.timer; do printf '%-18s %s\n' "$s" "$(systemctl is-active "$s" 2>/dev/null)"; done
  curl -fsS "http://127.0.0.1:$PORT/health" 2>/dev/null && echo || echo "iter_data: no answer on :$PORT"
  systemctl list-timers iter-backup.timer --no-pager 2>/dev/null | sed -n 2p
  local dir; dir="$(envval ITER_BACKUP_DIR "$H5/backup.env")"
  ls -1t "${dir:-${ITER_BACKUP_DIR:-$H5/backups}}"/iter5-*.tar 2>/dev/null | head -3 || echo "no backups yet"
}

case "${1:-}" in
  server) server ;;
  engine)
    shift; while [ $# -gt 0 ]; do case "$1" in --name) ENGINE="$2"; shift 2 ;; *) die "unknown option $1" ;; esac; done
    engine ;;
  update) shift; update "${1:-}" ;;
  backup) sudo systemctl start iter-backup.service && journalctl -u iter-backup --no-pager -n 1 -o cat ;;
  status) status ;;
  *) sed -n '2,13p' "$0" | sed 's/^# \{0,1\}//'; exit 2 ;;
esac
