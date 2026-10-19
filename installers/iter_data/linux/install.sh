#!/usr/bin/env bash
# Install iter_data natively as systemd services on Debian, Ubuntu or Arch Linux
# (also a WSL2 distro with [boot] systemd=true): ArangoDB CE 3.12 (localhost only),
# iter_data (API + webui on :8400) and an online backup every 6 hours. With
# --arango-url (or ARANGO_URL) it uses an ArangoDB you already run instead, and
# installs only the Arango client tools here, for the backups.
#
#   ./install.sh [install] [options]   install or upgrade (rerun it: idempotent)
#   ./install.sh status                services, health, the last backups
#   ./install.sh backup                take a backup now (online, nothing stops)
#   ./install.sh restore FILE          load a backup's database back (iter-data stops meanwhile)
#   ./install.sh uninstall             stop + remove services and programs; data is kept
#   ./install.sh migrate               only move an older install's files into --iter-dir (install does it too)
#   curl -fsSL https://raw.githubusercontent.com/Stephen-Hilton/iter/main/installers/iter_data/linux/install.sh | bash -s -- --version vYYYY.MM.DD
#
# Options:
#   --iter-dir DIR         the one folder everything goes in, always ending in /iter (/my/dir,
#                          /my/dir/iter and /my/dir/iter/ all mean /my/dir/iter); also $ITER_DIR;
#                          a rerun finds an existing install's folder itself [/opt/iter]
#   --version TAG          release to download iter_data (and, when piped, this folder) from [latest]
#   --binary PATH          use this iter_data binary instead of the one beside the script / the release
#   --env-file PATH        take ITER_ADMIN_PASSWORD, ITER_JWT_SECRET, ARANGO_ROOT_PASSWORD from it
#   --port N               iter_data port [8400]
#   --db-dir DIR           ArangoDB data directory, e.g. on a data disk [<iter dir>/iter_data/arango/data]
#   --backup-dir DIR       where backups go, e.g. on another disk [<iter dir>/iter_data/backups]
#   --memory MiB           memory cap for ArangoDB + iter_data together [min(6144, 85% of RAM)]
#   --arango-version V     ArangoDB CE version, from the official arangodb image [3.12.12]
#   --arango-url URL       use this ArangoDB (https:// to another machine) instead of installing one;
#                          also ARANGO_URL in the environment or --env-file; `local` = install here
#   --arango-fingerprint FP  pin that ArangoDB's certificate (sha256:..., for a self-signed one)
#   --arango-ca-cert PATH  or trust the CA (PEM) that signed it (copied to iter_data/config/arango-ca.crt)
#   --arango-db NAME       iter's database [iter_db; an existing install keeps its own]
#   --arango-user NAME     iter's ArangoDB user, given rw on that database only [iter_user]
#   --tls-cert PATH        serve this certificate (PEM, e.g. a real one for your domain) instead of
#                          the self-signed one; needs --tls-key; copied to iter_data/state (user iter)
#   --tls-key PATH         the private key (PEM) for --tls-cert (kept mode 600)
#   --tls-name NAME        an extra DNS name or IP on the self-signed certificate (repeatable);
#                          only read when the certificate is first made
#   --tls off|auto         off = plain HTTP, only behind a TLS proxy on this host; auto = the
#                          self-signed certificate again (drops an earlier --tls-cert or --tls off)
#   --dry-run              print every action, change nothing
#   -h, --help             this text
#
# ArangoDB sign-in: iter_data works as ARANGO_USER with rw on ARANGO_DB only. Its
# password (ARANGO_PASSWORD) is generated and kept unless given; root's password
# (ARANGO_ROOT_PASSWORD) lets iter_data create that user and database on its first
# start. An external ArangoDB needs ARANGO_PASSWORD (a user its admin made) or
# ARANGO_ROOT_PASSWORD, from the environment or --env-file; plain http:// to it
# only with ARANGO_ALLOW_PLAIN_HTTP=1 (a private network).
# Lands in one folder, <iter dir>/iter_data (D):
#   D/bin/        iter_data, iter-backup, iter-admin-cleanup
#   D/config/     data.env (secrets, root:iter 640), tls.env, backup.env, install.env
#   D/state/      iter_data's own files (user iter, 700): secrets, TLS certificate, marker
#   D/models/     the embedding model          D/backups/   the backups (--backup-dir)
#   D/systemd/    the units, linked into /etc/systemd/system
#   D/arango/     bin/ (arangod + clients), share/, config/ (arangod.conf, root.env),
#                 data/ (the database, user arangodb; --db-dir), apps/
# Outside it only links: the units in /etc/systemd/system and
# /etc/sysctl.d/60-arangodb.conf (a unit is copied there instead when the iter dir is
# not on the root filesystem, which systemd reads before other disks are mounted).
# An install from before 2026-10-18 (/opt/iter/bin, /etc/iter, /var/lib/iter,
# /var/lib/arangodb3, /usr/sbin/arangod) is moved into the iter dir by the next run.
# HTTPS by default: iter_data makes a local CA and a certificate signed by it on first
# start (D/state/tls-ca.crt, tls.crt, kept across reinstalls); the CA is added to this
# machine's system store and browser certificate databases, so the webui opens as secure
# here (other machines: the engine installer does it there). The script prints the fingerprint,
# to give to each engine installer (--fingerprint) or compare with `iter_engine key
# status`. The TLS choices are kept in D/config/tls.env, so a plain rerun keeps them.
# Logs: journalctl -u iter-data -u arangodb3. Run as root or as a user with sudo.
set -euo pipefail

REPO="Stephen-Hilton/iter"
FOLDER="installers/iter_data/linux"
# shellcheck disable=SC2015  # piped from curl there is no script dir: fall back to the cwd
HERE="$(cd "$(dirname "${BASH_SOURCE[0]:-$0}")" 2>/dev/null && pwd || pwd)"
SUPPORT=(arangod.conf iter-backup iter-admin-cleanup systemd/iter.slice systemd/arangodb3.service systemd/iter-data.service
         systemd/iter-backup.service systemd/iter-backup.timer systemd/iter-admin-cleanup.path systemd/iter-admin-cleanup.service)
UNITS=(iter.slice arangodb3.service iter-data.service iter-backup.service iter-backup.timer
       iter-admin-cleanup.path iter-admin-cleanup.service)
MODEL_SHA="53aa51172d142c89d9012cce15ae4d6cc0ca6895895114379cacb4fab128d9db"
ARANGO_CLIENTS=(arangosh arangodump arangorestore arangobackup arangoexport arangoimport)

CMD=install
VERSION="${ITER_VERSION:-}"
BINARY=""
ENV_SRC=""
PORT="${ITER_PORT:-8400}"
DB_DIR=""
BACKUP_DIR=""
ITER_DIR_OPT="${ITER_DIR:-}"
MEM_MIB=""
ARANGO_VERSION="${ARANGO_VERSION:-3.12.12}"
DRY=0
A_URL_OPT="" A_FP_OPT="" A_CA_SRC="" A_DB_OPT="" A_USER_OPT="" RESTORE_FILE=""
EXTERNAL=0
TLS_CERT_SRC="" TLS_KEY_SRC="" TLS_OPT="" TLS_NAMES_OPT=()

say() { echo "[iter_data] $*"; }
die() { echo "[iter_data] ERROR: $*" >&2; exit 1; }
usage() { sed -n '2,/^set -euo/p' "${BASH_SOURCE[0]:-$0}" 2>/dev/null | sed -e '$d' -e 's/^# \{0,1\}//' || echo "see $FOLDER/README.md"; }
SUDO=""; [ "$(id -u)" = 0 ] || SUDO=sudo
# run a command as root (printed; skipped in a dry run)
run() { echo "  + $*"; [ "$DRY" = 1 ] || $SUDO "$@"; }
# write stdin to a root-owned file: put MODE OWNER PATH
put() {
  local mode="$1" owner="$2" path="$3" t
  if [ "$DRY" = 1 ]; then echo "  + write $path ($mode $owner)"; cat >/dev/null; return; fi
  t="$(mktemp)"; cat >"$t"
  $SUDO install -D -m "$mode" -o "${owner%%:*}" -g "${owner##*:}" "$t" "$path.tmp" && $SUDO mv -f "$path.tmp" "$path"
  rm -f "$t"
}
envval() { [ -r "$2" ] && grep -E "^\s*(export\s+)?$1=" "$2" | tail -1 | sed -E 's/^[^=]*=//; s/^["'\'']//; s/["'\'']$//' | tr -d '\r' || true; }
rootenvval() { $SUDO test -f "$2" && $SUDO grep -E "^\s*(export\s+)?$1=" "$2" | tail -1 | sed -E 's/^[^=]*=//; s/^["'\'']//; s/["'\'']$//' | tr -d '\r' || true; }
gen() { LC_ALL=C tr -dc 'A-Za-z0-9' </dev/urandom | head -c "$1" || true; }
# every temp folder lives under one, removed on exit
TMPROOT="$(mktemp -d)"
trap 'rm -rf "$TMPROOT"' EXIT
tmpdir() { mktemp -d "$TMPROOT/t.XXXXXX"; }
have_systemd() { [ "$(ps -p 1 -o comm= 2>/dev/null)" = systemd ]; }

while [ $# -gt 0 ]; do
  case "$1" in
    install|status|backup|uninstall|migrate) CMD="$1"; shift ;;
    restore) CMD=restore; RESTORE_FILE="${2:-}"; shift; [ $# = 0 ] || shift ;;
    --iter-dir|--iter_dir) ITER_DIR_OPT="$2"; shift 2 ;;
    --iter-dir=*|--iter_dir=*) ITER_DIR_OPT="${1#*=}"; shift ;;
    --version) VERSION="$2"; shift 2 ;;
    --binary) BINARY="$2"; shift 2 ;;
    --env-file) ENV_SRC="$2"; shift 2 ;;
    --port) PORT="$2"; shift 2 ;;
    --db-dir) DB_DIR="${2%/}"; shift 2 ;;
    --backup-dir) BACKUP_DIR="${2%/}"; shift 2 ;;
    --memory) MEM_MIB="$2"; shift 2 ;;
    --arango-version) ARANGO_VERSION="$2"; shift 2 ;;
    --arango-url) A_URL_OPT="$2"; shift 2 ;;
    --arango-fingerprint) A_FP_OPT="$2"; shift 2 ;;
    --arango-ca-cert) A_CA_SRC="$2"; shift 2 ;;
    --arango-db) A_DB_OPT="$2"; shift 2 ;;
    --arango-user) A_USER_OPT="$2"; shift 2 ;;
    --tls-cert) TLS_CERT_SRC="$2"; shift 2 ;;
    --tls-key) TLS_KEY_SRC="$2"; shift 2 ;;
    --tls-name) TLS_NAMES_OPT+=("$2"); shift 2 ;;
    --tls) TLS_OPT="$2"; shift 2 ;;
    --dry-run) DRY=1; shift ;;
    -h|--help) usage; exit 0 ;;
    *) usage; die "unknown option $1" ;;
  esac
done
[ "$DRY" = 1 ] && say "dry run: nothing is changed"
case "$TLS_OPT" in ""|auto|off) ;; *) die "--tls is auto or off (a certificate of your own: --tls-cert/--tls-key)" ;; esac
{ [ -n "$TLS_CERT_SRC" ] && [ -n "$TLS_KEY_SRC" ]; } || { [ -z "$TLS_CERT_SRC" ] && [ -z "$TLS_KEY_SRC" ]; } \
  || die "--tls-cert and --tls-key go together"
[ -z "$TLS_CERT_SRC" ] || [ "$TLS_OPT" != off ] || die "--tls off and --tls-cert contradict each other"
[ "$A_USER_OPT" != root ] || die "--arango-user root: iter_data signs in as a user of its own (rw on its database only)"
[ "$CMD" != restore ] || [ -n "$RESTORE_FILE" ] || die "restore needs the backup file: $0 restore <iter dir>/iter_data/backups/iter-<ts>.tar"
for n in ${TLS_NAMES_OPT[@]+"${TLS_NAMES_OPT[@]}"}; do
  case "$n" in ""|*[!A-Za-z0-9.:-]*) die "--tls-name $n: a DNS name or an IP address" ;; esac
done

case "$(uname -m)" in x86_64|amd64) ARCH=x86_64; OCI_ARCH=amd64 ;; aarch64|arm64) ARCH=aarch64; OCI_ARCH=arm64 ;; *) die "unsupported CPU $(uname -m)" ;; esac
ASSET="iter_data-linux-$ARCH"

# ---------------------------------------------------------------- the iter dir
# --iter-dir: the folder that holds everything, always ending in /iter
# (/my/dir/iter/, /my/dir/iter and /my/dir all give /my/dir/iter)
iter_dir_of() {
  local d="$1"
  # shellcheck disable=SC2088  # a literal ~ typed in quotes
  case "$d" in "~") d="$HOME" ;; "~/"*) d="$HOME/${d#\~/}" ;; esac
  case "$d" in /*) ;; *) d="$PWD/$d" ;; esac
  while [ "${#d}" -gt 1 ] && [ "${d%/}" != "$d" ]; do d="${d%/}"; done
  [ "${d##*/}" = iter ] || d="${d%/}/iter"
  printf '%s' "$d"
}
SYSD=/etc/systemd/system
# the iter dir of an install made since 2026-10-18, from its iter-data unit
# (a link into <iter dir>/iter_data/systemd, or a copy whose ExecStart names it)
installed_iter_dir() {
  local u="$SYSD/iter-data.service" t
  # the link's own target, so a dangling link (a folder moved by hand) still counts
  t="$(readlink "$u" 2>/dev/null || true)"
  case "$t" in /*/iter_data/systemd/iter-data.service) printf '%s' "${t%/iter_data/systemd/iter-data.service}"; return ;; esac
  [ -f "$u" ] || return 0
  sed -n 's|^ExecStart=\(/.*\)/iter_data/bin/iter_data .*|\1|p' "$u" 2>/dev/null | head -1 || true
}
OLD_ROOT="$(installed_iter_dir)"
if [ -n "$ITER_DIR_OPT" ]; then ROOT="$(iter_dir_of "$ITER_DIR_OPT")"; else ROOT="${OLD_ROOT:-/opt/iter}"; fi
case "$ROOT" in *[[:space:]]*) die "--iter-dir $ROOT: a folder without spaces (systemd units name it)" ;; esac
D="$ROOT/iter_data"
BIN_DIR="$D/bin" CONF_DIR="$D/config" STATE="$D/state" UNIT_DIR="$D/systemd"
MODEL_DIR="$D/models/all-MiniLM-L6-v2"
DATA_ENV="$CONF_DIR/data.env" TLS_ENV="$CONF_DIR/tls.env" BACKUP_ENV="$CONF_DIR/backup.env" INSTALL_ENV="$CONF_DIR/install.env"
AR="$D/arango"
AR_BIN="$AR/bin" AR_SHARE="$AR/share/arangodb3" AR_CONF="$AR/config" APPS_DIR="$AR/apps"
# iter_data writes this once the first admin has chosen their own password
MARKER="$STATE/admin_password_changed"

# ---------------------------------------------------------------- distro + tools
detect_distro() {
  [ -f /etc/os-release ] || die "no /etc/os-release: unsupported system"
  # in a subshell: os-release sets VERSION, which would clobber --version
  local ID ID_LIKE PRETTY_NAME
  IFS='|' read -r ID ID_LIKE PRETTY_NAME < <(
    # shellcheck disable=SC1091
    . /etc/os-release; echo "${ID:-unknown}|${ID_LIKE:-none}|${PRETTY_NAME:-$ID}")
  [ "$ID_LIKE" != none ] || ID_LIKE=""
  case " $ID $ID_LIKE " in
    *" debian "*|*" ubuntu "*) PKG=apt ;;
    *" arch "*) PKG=pacman ;;
    *) die "${PRETTY_NAME:-this distro} is not Debian, Ubuntu or Arch (Amazon Linux: use installers/iter_data/linux_aws)" ;;
  esac
  say "found ${PRETTY_NAME:-$ID} ($ARCH), package manager $PKG"
  if grep -qi microsoft /proc/version 2>/dev/null; then say "WSL2 detected: needs [boot] systemd=true in /etc/wsl.conf"; fi
}

ensure_tools() {
  # command:package pairs (the package is the same on apt and pacman unless noted)
  local missing=() c p pair
  for pair in curl:curl jq:jq tar:tar gzip:gzip sha256sum:coreutils useradd:passwd sysctl:procps install:coreutils ps:procps openssl:openssl; do
    c="${pair%%:*}"; p="${pair##*:}"
    if [ "$PKG" = pacman ]; then case "$p" in passwd) p=shadow ;; procps) p=procps-ng ;; esac; fi
    command -v "$c" >/dev/null || missing+=("$p")
  done
  # the iter_data binary links libstdc++.so.6 and libgcc_s.so.1
  { ldconfig -p 2>/dev/null || true; } | grep -q 'libstdc++\.so\.6' || missing+=("$([ "$PKG" = apt ] && echo libstdc++6 || echo gcc-libs)")
  [ -s /etc/ssl/certs/ca-certificates.crt ] || [ -s /etc/pki/tls/certs/ca-bundle.crt ] || missing+=(ca-certificates)
  if [ ${#missing[@]} = 0 ]; then say "tools: curl jq tar gzip sha256sum useradd sysctl openssl present"; return; fi
  mapfile -t missing < <(printf '%s\n' "${missing[@]}" | sort -u)
  say "installing ${missing[*]}"
  if [ "$PKG" = apt ]; then
    run env DEBIAN_FRONTEND=noninteractive apt-get update -qq
    run env DEBIAN_FRONTEND=noninteractive apt-get install -y -qq "${missing[@]}"
  else
    run pacman -Sy --noconfirm --needed "${missing[@]}"
  fi
  [ "$DRY" = 1 ] || command -v curl >/dev/null || die "curl still missing"
}

# ---------------------------------------------------------------- support files
# Beside the script in a checkout or a downloaded folder; piped from curl they
# are fetched from the same tag (or main) into a temp folder.
SRC="$HERE"
get_support() {
  local f absent=0
  for f in "${SUPPORT[@]}"; do [ -f "$HERE/$f" ] || absent=1; done
  [ "$absent" = 1 ] || { say "support files: $HERE"; return; }
  local ref="${VERSION:-main}"
  SRC="$(tmpdir)"
  say "support files: not beside the script, fetching $FOLDER ($ref) into $SRC"
  for f in "${SUPPORT[@]}"; do
    mkdir -p "$SRC/$(dirname "$f")"
    curl -fsSL --retry 3 -o "$SRC/$f" "https://raw.githubusercontent.com/$REPO/$ref/$FOLDER/$f" \
      || die "could not fetch $f from $REPO@$ref"
  done
}

# ---------------------------------------------------------------- the binary
get_binary() {  # -> $BIN_FILE (a local file to install)
  if [ -n "$BINARY" ]; then
    [ -f "$BINARY" ] || die "--binary $BINARY does not exist"
    BIN_FILE="$BINARY"; say "binary: $BINARY (--binary)"; return
  fi
  if [ -z "$VERSION" ] && [ -f "$HERE/$ASSET" ]; then
    BIN_FILE="$HERE/$ASSET"; say "binary: $BIN_FILE (beside the script)"; return
  fi
  local base t want
  if [ -n "$VERSION" ]; then base="https://github.com/$REPO/releases/download/$VERSION"; else base="https://github.com/$REPO/releases/latest/download"; fi
  say "binary: downloading $ASSET from $base"
  if [ "$DRY" = 1 ]; then
    echo "  + curl -fsSL $base/$ASSET and $base/SHA256SUMS; verify sha256"
    BIN_FILE="(downloaded $ASSET)"; return
  fi
  t="$(tmpdir)"
  curl -fsSL --retry 3 -o "$t/$ASSET" "$base/$ASSET" || die "could not download $base/$ASSET"
  curl -fsSL --retry 3 -o "$t/SHA256SUMS" "$base/SHA256SUMS" || die "could not download $base/SHA256SUMS"
  want="$(awk -v a="$ASSET" '$2==a || $2=="*"a {print $1}' "$t/SHA256SUMS")"
  [ -n "$want" ] || die "$ASSET is not listed in SHA256SUMS"
  [ "$(sha256sum "$t/$ASSET" | cut -d' ' -f1)" = "$want" ] || die "$ASSET sha256 does not match SHA256SUMS"
  say "sha256 verified"
  BIN_FILE="$t/$ASSET"
}

install_binary() {
  say "installing iter_data -> $BIN_DIR/iter_data"
  run install -d -m 755 "$BIN_DIR"
  # a temp name in the same dir, then mv over: never cp over a running executable
  run install -m 755 "$BIN_FILE" "$BIN_DIR/.iter_data.new"
  run mv -f "$BIN_DIR/.iter_data.new" "$BIN_DIR/iter_data"
}

# ---------------------------------------------------------------- embedding model
install_model() {
  local dir="$MODEL_DIR" hf=https://huggingface.co/sentence-transformers/all-MiniLM-L6-v2/resolve/main f
  if [ -f "$dir/model.safetensors" ] && [ "$(sha256sum "$dir/model.safetensors" | cut -d' ' -f1)" = "$MODEL_SHA" ] && [ -f "$dir/tokenizer.json" ]; then
    say "model: all-MiniLM-L6-v2 present in $dir"; return
  fi
  say "model: downloading all-MiniLM-L6-v2 (~91 MB) into $dir"
  run install -d -m 755 "$dir" "$dir/1_Pooling"
  for f in config.json tokenizer.json model.safetensors special_tokens_map.json tokenizer_config.json \
           modules.json sentence_bert_config.json 1_Pooling/config.json README.md; do
    run curl -fsSL --retry 3 -o "$dir/$f.part" "$hf/$f"
    run mv -f "$dir/$f.part" "$dir/$f"
  done
  if [ "$DRY" = 0 ] && [ "$(sha256sum "$dir/model.safetensors" | cut -d' ' -f1)" != "$MODEL_SHA" ]; then
    say "WARNING: model.safetensors sha256 differs from the verified $MODEL_SHA (upstream changed?)"
  fi
}

# ---------------------------------------------------------------- ArangoDB
# ArangoDB stopped publishing Community Edition packages after 3.12.4; 3.12.5+ CE
# ships only as the official docker image, whose binaries are static. So the
# binaries are taken from that image, straight from Docker Hub (no docker needed):
# each layer is checked against its sha256 digest, then unpacked.
fetch_arango() {  # fetch_arango DEST
  local dest="$1" img="library/arangodb" reg="https://registry-1.docker.io/v2" tok idx dig man layer
  tok="$(curl -fsSL "https://auth.docker.io/token?service=registry.docker.io&scope=repository:$img:pull" | jq -r .token)" \
    || die "Docker Hub token request failed"
  idx="$(curl -fsSL -H "Authorization: Bearer $tok" \
    -H 'Accept: application/vnd.oci.image.index.v1+json, application/vnd.docker.distribution.manifest.list.v2+json' \
    "$reg/$img/manifests/$ARANGO_VERSION")" || die "arangodb:$ARANGO_VERSION not found on Docker Hub"
  dig="$(jq -r --arg a "$OCI_ARCH" '[.manifests[] | select(.platform.os=="linux" and .platform.architecture==$a)][0].digest' <<<"$idx")"
  [ -n "$dig" ] && [ "$dig" != null ] || die "arangodb:$ARANGO_VERSION has no linux/$OCI_ARCH image"
  man="$(curl -fsSL -H "Authorization: Bearer $tok" \
    -H 'Accept: application/vnd.oci.image.manifest.v1+json, application/vnd.docker.distribution.manifest.v2+json' \
    "$reg/$img/manifests/$dig")" || die "manifest $dig failed"
  mkdir -p "$dest/root"
  for layer in $(jq -r '.layers[].digest' <<<"$man"); do
    curl -fsSL --retry 3 -H "Authorization: Bearer $tok" -o "$dest/layer" "$reg/$img/blobs/$layer" || die "layer $layer failed"
    [ "sha256:$(sha256sum "$dest/layer" | cut -d' ' -f1)" = "$layer" ] || die "layer $layer: checksum mismatch"
    tar -xzf "$dest/layer" -C "$dest/root" --no-same-owner --exclude='dev/*' 2>/dev/null || tar -xf "$dest/layer" -C "$dest/root" --no-same-owner --exclude='dev/*'
    rm -f "$dest/layer"
  done
  [ -x "$dest/root/usr/sbin/arangod" ] || die "arangod not found in arangodb:$ARANGO_VERSION"
}

install_arango() {
  local have=""
  if [ "$EXTERNAL" = 1 ]; then
    say "ArangoDB: using $A_URL (nothing installed or run here but the client tools, for backups)"
    stop_local_arango
    [ -x "$AR_BIN/arangodump" ] && have="$("$AR_BIN/arangodump" --configuration none --version 2>/dev/null | head -1 || true)"
    [ "$have" = "$ARANGO_VERSION" ] && { say "Arango client tools $ARANGO_VERSION already installed"; return; }
    local t="<arangodb:$ARANGO_VERSION>"
    if [ "$DRY" = 1 ]; then
      echo "  + download arangodb:$ARANGO_VERSION linux/$OCI_ARCH layers from Docker Hub, verify each sha256, unpack"
    else
      t="$(tmpdir)"; fetch_arango "$t"
    fi
    install_arango_clients "$t"
    return
  fi
  [ -x "$AR_BIN/arangod" ] && have="$(ICU_DATA="$AR_SHARE" ICU_DATA_LEGACY="$AR_SHARE" "$AR_BIN/arangod" --version 2>/dev/null | head -1 || true)"
  if [ "$have" = "$ARANGO_VERSION" ]; then
    say "ArangoDB $ARANGO_VERSION already installed"
  else
    say "ArangoDB: ${have:+$have installed, }installing $ARANGO_VERSION (static binaries from the official arangodb:$ARANGO_VERSION image, linux/$OCI_ARCH)"
    local t="<arangodb:$ARANGO_VERSION>"
    if [ "$DRY" = 1 ]; then
      echo "  + download arangodb:$ARANGO_VERSION linux/$OCI_ARCH layers from Docker Hub, verify each sha256, unpack"
    else
      t="$(tmpdir)"; fetch_arango "$t"
    fi
    have_systemd && run systemctl stop arangodb3 2>/dev/null || true
    run install -d -m 755 "$AR" "$AR_BIN" "$AR/share"
    run install -m 755 "$t/root/usr/sbin/arangod" "$AR_BIN/.arangod.new"
    run mv -f "$AR_BIN/.arangod.new" "$AR_BIN/arangod"
    # the old share/ goes to a temp folder that is removed on exit
    [ ! -e "$AR_SHARE" ] || run mv "$AR_SHARE" "$(tmpdir)/"
    run cp -r "$t/root/usr/share/arangodb3" "$AR_SHARE"
    install_arango_clients "$t"
  fi
  sed -e "s|@DB_DIR@|$DB_DIR|g" -e "s|@APPS_DIR@|$APPS_DIR|g" -e "s|@AR_SHARE@|$AR_SHARE|g" "$SRC/arangod.conf" | put 644 root:root "$AR_CONF/arangod.conf"
  id arangodb >/dev/null 2>&1 || run useradd --system --user-group --home-dir "$DB_DIR" --shell "$NOLOGIN" arangodb
  run install -d -o arangodb -g arangodb -m 750 "$DB_DIR" "$APPS_DIR"
  echo 'vm.max_map_count = 1024000' | put 644 root:root "$AR_CONF/60-arangodb.conf"
  link_or_copy "$AR_CONF/60-arangodb.conf" /etc/sysctl.d/60-arangodb.conf
  run sysctl -q -p /etc/sysctl.d/60-arangodb.conf || say "sysctl vm.max_map_count not applied (fine in a container)"
  printf 'ARANGODB_DEFAULT_ROOT_PASSWORD=%s\n' "$ARANGO_PW" | put 600 root:root "$AR_CONF/root.env"
}

# the Arango client tools into arango/bin, and the image's client configs into
# arango/config with their paths pointed at arango/share (arangosh -c <that conf>)
install_arango_clients() {  # install_arango_clients <fetched image root's parent>
  local t="$1" b f
  run install -d -m 755 "$AR" "$AR_BIN" "$AR_CONF"
  for b in "${ARANGO_CLIENTS[@]}"; do run install -m 755 "$t/root/usr/bin/$b" "$AR_BIN/$b"; done
  for f in "$t"/root/etc/arangodb3/*.conf; do
    [ "$DRY" = 1 ] && { echo "  + install the image's /etc/arangodb3/*.conf (client configs) into $AR_CONF"; break; }
    [ "$(basename "$f")" = arangod.conf ] || sed "s|/usr/share/arangodb3|$AR_SHARE|g" "$f" | put 644 root:root "$AR_CONF/$(basename "$f")"
  done
}

# a file the system reads from its own folder (/etc/sysctl.d, /etc/systemd/system):
# a link to the file in the iter dir, or a copy when the iter dir is on another
# filesystem than / (mounted after systemd reads its units)
ON_ROOT_FS=""
link_or_copy() {  # link_or_copy SOURCE TARGET
  if [ -z "$ON_ROOT_FS" ]; then
    local p="$ROOT"; while [ ! -e "$p" ]; do p="$(dirname "$p")"; done
    if [ "$(stat -c %d "$p")" = "$(stat -c %d /)" ]; then ON_ROOT_FS=1; else ON_ROOT_FS=0
      say "$ROOT is not on the root filesystem: the units and the sysctl file are copied into /etc, not linked"; fi
  fi
  if [ "$ON_ROOT_FS" = 1 ]; then run ln -sfn "$1" "$2"
  else [ ! -L "$2" ] || run rm -f "$2"; run install -m 644 "$1" "$2"; fi
}

# an earlier install's local ArangoDB, now that iter uses another: stopped, its data kept
stop_local_arango() {
  have_systemd || return 0
  [ -e "$SYSD/arangodb3.service" ] || return 0
  say "ArangoDB: stopping the local arangodb3 service (its data in $DB_DIR is kept)"
  run systemctl disable --now arangodb3 || true
  run rm -f "$SYSD/arangodb3.service" "$UNIT_DIR/arangodb3.service"
}

# ---------------------------------------------------------------- secrets
# config/data.env, kept across reinstalls. Values: --env-file, else the existing
# config/data.env, else an older ~/.iter/data.env (linux/install.sh server), else new.
LEGACY=""
secrets() {
  local cur u h
  cur="$(cfgfile data.env)"
  u="${SUDO_USER:-$(id -un)}"; h="$(getent passwd "$u" | cut -d: -f6)"
  [ -n "$h" ] && [ -f "$h/.iter/data.env" ] && LEGACY="$h/.iter"
  [ -z "$ENV_SRC" ] || [ -f "$ENV_SRC" ] || die "--env-file $ENV_SRC does not exist"
  pickv() {
    local v="${!1:-}"
    [ -n "$v" ] || [ -z "$ENV_SRC" ] || v="$(envval "$1" "$ENV_SRC")"
    [ -n "$v" ] || v="$(rootenvval "$1" "$cur")"
    [ -n "$v" ] || [ -z "$LEGACY" ] || v="$(envval "$1" "$LEGACY/data.env")"
    printf '%s' "$v"
  }
  ADMIN_PW="$(pickv ITER_ADMIN_PASSWORD)"
  JWT="$(pickv ITER_JWT_SECRET)"
  # ArangoDB. Before 2026-10-17 iter_data signed in as root (ARANGO_USER=root,
  # ARANGO_PASSWORD = root's); now as its own user, with rw on its database only.
  A_URL="$A_URL_OPT"; [ -n "$A_URL" ] || A_URL="$(pickv ARANGO_URL)"
  case "$A_URL" in ""|local|http://127.0.0.1:8529|http://localhost:8529) EXTERNAL=0; A_URL=http://127.0.0.1:8529 ;; *) EXTERNAL=1 ;; esac
  A_USER="$A_USER_OPT"; [ -n "$A_USER" ] || A_USER="$(pickv ARANGO_USER)"
  A_PW="$(pickv ARANGO_PASSWORD)"
  ARANGO_PW="$(pickv ARANGO_ROOT_PASSWORD)"
  if [ -z "$A_USER" ] || [ "$A_USER" = root ]; then
    # an older install: its ARANGO_PASSWORD is root's
    [ -n "$ARANGO_PW" ] || ARANGO_PW="$A_PW"
    [ -z "$A_USER" ] || say "ArangoDB: iter_data signed in as root until now; from now on as iter_user (rw on its own database only)"
    A_USER=iter_user; A_PW=""
  fi
  local fresh=1; $SUDO test -f "$DB_DIR/SERVER" && fresh=0
  [ -z "$MOVED_DB_FROM" ] || ! $SUDO test -f "$MOVED_DB_FROM/SERVER" || fresh=0   # a dry run: not moved yet
  FRESH_ADMIN=""
  if $SUDO test -f "$MARKER" || $SUDO test -f /var/lib/iter/admin_password_changed; then
    # the admin chose their own password: the first-boot one is not written back
    [ -z "$ADMIN_PW" ] || say "the admin has chosen their own password: ITER_ADMIN_PASSWORD is left out of $cur"
    ADMIN_PW=""
  fi
  if [ -z "$ADMIN_PW" ] && [ "$fresh" = 1 ]; then ADMIN_PW="$(gen 20)"; FRESH_ADMIN=1; fi
  if [ -z "$JWT" ]; then
    if $SUDO test -s "$STATE/iter_data.secret" || $SUDO test -s /var/lib/iter/iter_data.secret || { [ -n "$LEGACY" ] && [ -s "$LEGACY/iter_data/iter_data.secret" ]; }; then
      say "no ITER_JWT_SECRET given: iter_data keeps its iter_data.secret file"
    else JWT="$(gen 48)"; say "generated a new ITER_JWT_SECRET"; fi
  fi
  if [ -z "$ARANGO_PW" ] && [ "$EXTERNAL" = 0 ]; then
    if [ "$fresh" = 0 ]; then ARANGO_PW=iterdev; say "existing database, no Arango password given: assuming the old default"
    else ARANGO_PW="$(gen 24)"; say "generated an Arango root password (kept in $DATA_ENV)"; fi
  fi
  arango_settings
  [ -z "$LEGACY" ] || say "found an older install's settings in $LEGACY (used where nothing newer is given)"
  {
    [ -z "$ADMIN_PW" ] || echo "ITER_ADMIN_PASSWORD=$ADMIN_PW"
    [ -z "$JWT" ] || echo "ITER_JWT_SECRET=$JWT"
    [ -z "$ARANGO_PW" ] || echo "ARANGO_ROOT_PASSWORD=$ARANGO_PW"
    echo "ARANGO_URL=$A_URL"
    echo "ARANGO_USER=$A_USER"
    echo "ARANGO_PASSWORD=$A_PW"
    echo "ARANGO_DB=$A_DB"
    [ -z "$A_FP" ] || echo "ARANGO_CERT_FINGERPRINT=$A_FP"
    [ -z "$A_CA" ] || echo "ARANGO_CA_CERT=$A_CA"
    [ -z "$A_PLAIN" ] || echo "ARANGO_ALLOW_PLAIN_HTTP=$A_PLAIN"
    echo "ITER_EMBED_MODEL=$MODEL_DIR"
  } | put 640 root:iter "$DATA_ENV"
}

# a config file: the one in config/, else (a dry run before the move) the old
# /etc/iter one
cfgfile() {
  if $SUDO test -f "$CONF_DIR/$1" || ! $SUDO test -f "/etc/iter/$1"; then printf '%s' "$CONF_DIR/$1"; else printf '%s' "/etc/iter/$1"; fi
}

# iter's ArangoDB user, password and database, and how an external one is reached
arango_settings() {
  [ -z "$A_PW" ] || [ "$A_PW" != "$ARANGO_PW" ] || A_PW=""   # never root's password
  if [ -z "$A_PW" ]; then
    [ "$EXTERNAL" = 0 ] || [ -n "$ARANGO_PW" ] || die "the ArangoDB at $A_URL needs ARANGO_PASSWORD ($A_USER's password) or ARANGO_ROOT_PASSWORD (to create $A_USER), in the environment or --env-file"
    A_PW="$(gen 24)"; say "ArangoDB: generated a password for user $A_USER (kept in $DATA_ENV)"
  fi
  A_DB="$A_DB_OPT"; [ -n "$A_DB" ] || A_DB="$(pickv ARANGO_DB)"
  if [ -z "$A_DB" ]; then A_DB=iter_db
  elif [ -z "$A_DB_OPT" ] && [ "$A_DB" != iter_db ]; then say "ArangoDB: keeping database $A_DB from the existing install (--arango-db to choose another)"; fi
  A_FP="" A_CA="" A_PLAIN=""
  [ "$EXTERNAL" = 1 ] || { say "ArangoDB: local, user $A_USER, database $A_DB"; return 0; }
  A_FP="$A_FP_OPT"; [ -n "$A_FP" ] || A_FP="$(pickv ARANGO_CERT_FINGERPRINT)"
  A_PLAIN="$(pickv ARANGO_ALLOW_PLAIN_HTTP)"
  if [ -n "$A_CA_SRC" ]; then
    [ -r "$A_CA_SRC" ] || die "--arango-ca-cert $A_CA_SRC: cannot read it"
    put 644 root:root "$CONF_DIR/arango-ca.crt" <"$A_CA_SRC"
    A_CA="$CONF_DIR/arango-ca.crt"
  else
    A_CA="$(pickv ARANGO_CA_CERT)"
    # moved with the rest of /etc/iter (or of another iter dir)
    case "$A_CA" in /etc/iter/*) A_CA="$CONF_DIR/${A_CA#/etc/iter/}" ;; "$OLD_D"/config/*) A_CA="$CONF_DIR/${A_CA#"$OLD_D"/config/}" ;; esac
  fi
  case "$A_URL" in
    https://*) [ -n "$A_FP$A_CA" ] || say "ArangoDB: $A_URL is checked against the public CAs (self-signed? give --arango-fingerprint)" ;;
    http://*) case "$A_PLAIN" in 1|true|yes) say "ArangoDB: plain http to $A_URL (ARANGO_ALLOW_PLAIN_HTTP: a private network only)" ;;
                *) die "--arango-url $A_URL: use https:// (or ARANGO_ALLOW_PLAIN_HTTP=1 on a private network)" ;; esac ;;
    *) die "--arango-url $A_URL: not an http(s):// URL" ;;
  esac
  say "ArangoDB: $A_URL, user $A_USER, database $A_DB${A_FP:+, pinned certificate}${A_CA:+, CA $A_CA}"
}

# ---------------------------------------------------------------- iter user + units
NOLOGIN=/usr/sbin/nologin
setup_iter_user() {
  NOLOGIN="$(command -v nologin || echo /usr/sbin/nologin)"
  getent group iter >/dev/null 2>&1 || run groupadd --system iter
  id iter >/dev/null 2>&1 || run useradd --system --gid iter --home-dir "$STATE" --shell "$NOLOGIN" iter
  [ "$(getent passwd iter 2>/dev/null | cut -d: -f6)" = "$STATE" ] || [ "$DRY" = 1 ] || run usermod -d "$STATE" iter
  run install -d -m 755 "$ROOT" "$D" "$BIN_DIR" "$UNIT_DIR" "$D/models"
  run install -d -o root -g iter -m 750 "$CONF_DIR"
  run install -d -o iter -g iter -m 700 "$STATE"
  # an older ~/.iter install: bring iter_data's own files over once
  local f
  if [ -n "$LEGACY" ]; then
    for f in iter_data.secret engine_session.secret tls.crt tls.key .env; do
      if [ -f "$LEGACY/iter_data/$f" ] && ! $SUDO test -f "$STATE/$f"; then
        run install -o iter -g iter -m 600 "$LEGACY/iter_data/$f" "$STATE/$f"
      fi
    done
  fi
}

# ---------------------------------------------------------------- TLS
# iter_data serves HTTPS (--tls auto): a self-signed certificate it makes on first start
# beside its secret file (state/tls.crt + tls.key, user iter, 600), kept across
# reinstalls like iter_data.secret and engine_session.secret. The choice made here is
# kept in config/tls.env, so a rerun without TLS options leaves it as it was.
CUSTOM_CRT="$STATE/custom-tls.crt"
CUSTOM_KEY="$STATE/custom-tls.key"
tls_setup() {
  local names
  TLS_MODE="$(rootenvval ITER_TLS_MODE "$(cfgfile tls.env)")"; TLS_MODE="${TLS_MODE:-auto}"
  names="$(rootenvval ITER_TLS_NAMES "$(cfgfile tls.env)")"
  [ -z "$TLS_OPT" ] || TLS_MODE="$TLS_OPT"
  [ -z "$TLS_CERT_SRC" ] || TLS_MODE=custom
  if [ ${#TLS_NAMES_OPT[@]} -gt 0 ]; then names="$(IFS=,; echo "${TLS_NAMES_OPT[*]}")"; fi
  if [ -n "$TLS_CERT_SRC" ]; then
    [ -r "$TLS_CERT_SRC" ] && [ -r "$TLS_KEY_SRC" ] || die "--tls-cert / --tls-key: cannot read $TLS_CERT_SRC or $TLS_KEY_SRC"
    say "TLS: serving your certificate $TLS_CERT_SRC (copied to $CUSTOM_CRT)"
    run install -o iter -g iter -m 644 "$TLS_CERT_SRC" "$CUSTOM_CRT"
    run install -o iter -g iter -m 600 "$TLS_KEY_SRC" "$CUSTOM_KEY"
  elif [ "$TLS_MODE" = custom ]; then
    $SUDO test -f "$CUSTOM_CRT" || [ "$DRY" = 1 ] || die "$TLS_ENV says custom, but $CUSTOM_CRT is gone: rerun with --tls-cert/--tls-key (or --tls auto)"
    say "TLS: your certificate $CUSTOM_CRT (kept from the last install)"
  fi
  TLS_ARGS=""
  case "$TLS_MODE" in
    off) TLS_ARGS="--tls off"
         say "TLS: off (plain HTTP): only for iter_data behind a TLS proxy on this host; engines refuse plain http across a network" ;;
    custom) TLS_ARGS="--tls-cert $CUSTOM_CRT --tls-key $CUSTOM_KEY" ;;
    *) TLS_MODE=auto
       local n
       for n in ${names//,/ }; do TLS_ARGS="$TLS_ARGS --tls-name $n"; done
       TLS_ARGS="${TLS_ARGS# }"
       say "TLS: a self-signed certificate (made on first start, kept in $STATE)${names:+, extra names $names}" ;;
  esac
  [ -z "$TLS_ARGS" ] || say "TLS: iter-data runs with $TLS_ARGS"
  SCHEME=https; [ "$TLS_MODE" != off ] || SCHEME=http
  printf 'ITER_TLS_MODE=%s\nITER_TLS_NAMES=%s\n' "$TLS_MODE" "$names" | put 644 root:root "$TLS_ENV"
}

# the certificate's fingerprint (sha256:<hex> of its DER, what engines pin): openssl on
# the certificate file, else the line iter_data logs at every start
fingerprint() {
  local crt="$STATE/tls.crt" fp=""
  [ "${TLS_MODE:-auto}" != custom ] || crt="$CUSTOM_CRT"
  if command -v openssl >/dev/null && $SUDO test -f "$crt"; then
    fp="$($SUDO openssl x509 -in "$crt" -noout -fingerprint -sha256 2>/dev/null | sed 's/.*=//; s/://g' | tr 'A-F' 'a-f')"
    [ -z "$fp" ] || fp="sha256:$fp"
  fi
  [ -n "$fp" ] || fp="$($SUDO journalctl -u iter-data --no-pager -o cat 2>/dev/null | sed -n 's/.*TLS certificate fingerprint.*\(sha256:[0-9a-f]\{64\}\).*/\1/p' | tail -1 || true)"
  printf '%s' "$fp"
}

memory() {
  local total
  total=$(( $(awk '/^MemTotal:/ {print $2}' /proc/meminfo) / 1024 ))
  if [ -z "$MEM_MIB" ]; then MEM_MIB=$(( total * 85 / 100 )); [ "$MEM_MIB" -le 6144 ] || MEM_MIB=6144; fi
  ARANGO_MIB=$(( MEM_MIB - 1024 )); [ "$ARANGO_MIB" -ge 1024 ] || ARANGO_MIB=1024
  say "memory: ${total} MiB in the machine; ArangoDB + iter_data capped at ${MEM_MIB} MiB (arangod sized for ${ARANGO_MIB} MiB)"
  [ "$total" -ge 3500 ] || say "WARNING: under ~4 GB of RAM; ArangoDB + iter_data want 4 GB or more"
}

install_units() {
  local u keep
  keep="$(rootenvval ITER_BACKUP_KEEP "$(cfgfile backup.env)" | grep . || echo 28)"
  run install -d -o iter -g iter -m 700 "$BACKUP_DIR"
  printf 'ITER_BACKUP_DIR=%s\nITER_BACKUP_KEEP=%s\n' "$BACKUP_DIR" "$keep" | put 644 root:root "$BACKUP_ENV"
  printf 'ITER_DB_DIR=%s\n' "$DB_DIR" | put 644 root:root "$INSTALL_ENV"
  put 755 root:root "$BIN_DIR/iter-backup" <"$SRC/iter-backup"
  put 755 root:root "$BIN_DIR/iter-admin-cleanup" <"$SRC/iter-admin-cleanup"
  for u in "${UNITS[@]}"; do
    # an external ArangoDB: no local arangodb3 unit to need or wait for
    [ "$EXTERNAL" = 0 ] || [ "$u" != arangodb3.service ] || continue
    sed -e "s|@PORT@|$PORT|g" -e "s|@TLS_ARGS@|$TLS_ARGS|g" -e "s|@DB_DIR@|$DB_DIR|g" -e "s|@MEM_MAX@|${MEM_MIB}M|g" -e "s|@ARANGO_MEM@|${ARANGO_MIB}M|g" \
        -e "s|@D@|$D|g" -e "s|@AR@|$AR|g" -e "s|@AR_SHARE@|$AR_SHARE|g" \
      "$SRC/systemd/$u" | { if [ "$EXTERNAL" = 1 ]; then sed -e '/^Requires=arangodb3.service$/d' -e '/^After=arangodb3.service$/d' -e 's/^After=arangodb3.service /After=/'; else cat; fi; } \
      | put 644 root:root "$UNIT_DIR/$u"
    link_or_copy "$UNIT_DIR/$u" "$SYSD/$u"
  done
}

# where the database and the backups go: --db-dir / --backup-dir, else what this
# install had (config/install.env, config/backup.env), else inside the iter dir
paths() {
  [ -n "$DB_DIR" ] || DB_DIR="$(rootenvval ITER_DB_DIR "$INSTALL_ENV")"
  [ -n "$DB_DIR" ] || DB_DIR="$AR/data"
  [ -n "$BACKUP_DIR" ] || BACKUP_DIR="$(rootenvval ITER_BACKUP_DIR "$BACKUP_ENV")"
  [ -n "$BACKUP_DIR" ] || BACKUP_DIR="$D/backups"
  say "iter dir: $ROOT (database $DB_DIR, backups $BACKUP_DIR)"
}

start_services() {
  if ! have_systemd; then
    [ "$DRY" = 1 ] || die "systemd is not running (in WSL: [boot] systemd=true in /etc/wsl.conf, then wsl --terminate <distro>)"
    say "systemd is not running here (fine for a dry run; a real install needs it)"
  fi
  run systemctl daemon-reload
  [ "$EXTERNAL" = 1 ] || run systemctl enable --now arangodb3
  run systemctl enable iter-data
  run systemctl restart iter-data
  run systemctl enable --now iter-backup.timer
  # removes the first-boot ITER_ADMIN_PASSWORD once the admin has chosen their own
  run systemctl enable --now iter-admin-cleanup.path
}

wait_health() {
  if [ "$DRY" = 1 ]; then echo "  + wait for $SCHEME://127.0.0.1:$PORT/health to answer {\"ok\":true}$([ "$SCHEME" = http ] || echo ' (curl -k: the certificate is self-signed)')"; return; fi
  local h
  for _ in $(seq 1 180); do
    if h="$(curl -fsSk "$SCHEME://127.0.0.1:$PORT/health" 2>/dev/null)" && grep -q '"ok":true' <<<"$h"; then say "iter_data up: $h"; return 0; fi
    sleep 1
  done
  $SUDO journalctl -u iter-data -u arangodb3 --no-pager -n 30 || true
  die "iter_data did not answer on :$PORT within 3 minutes"
}

# ---------------------------------------------------------------- browsers trust iter_data
# --tls auto: iter_data signs its certificate with a local certificate authority
# of its own (D/state/tls-ca.crt), limited by name constraints to this server's
# names, local DNS suffixes (.local .lan .internal .home.arpa) and loopback /
# private addresses: it cannot vouch for any public site. Adding it to this
# machine's trust stores makes a browser here show the webui as secure: the
# system store (curl and most programs) and, for the desktop user who ran sudo,
# the NSS databases Chrome, Chromium and Firefox read. On another machine, the
# engine installer does the same; without one there, open
# https://<this host>:<port>/tls/ca.crt and trust it as a certificate authority.
DESK_USER="${SUDO_USER:-}"; [ "$DESK_USER" != root ] || DESK_USER=""
[ -n "$DESK_USER" ] || [ "$(id -u)" = 0 ] || DESK_USER="$(id -un)"
desk_home() { getent passwd "$DESK_USER" 2>/dev/null | cut -d: -f6; }
as_desk() { if [ "$(id -un)" = "$DESK_USER" ]; then "$@"; else sudo -u "$DESK_USER" "$@"; fi; }  # root: DESK_USER is who ran sudo
nss_dbs() {  # the desktop user's browser certificate databases
  local h d; h="$(desk_home)"; [ -n "$h" ] || return 0
  for d in "$h/.pki/nssdb" "$h"/snap/chromium/current/.pki/nssdb "$h"/.var/app/*/.pki/nssdb \
           "$h"/.mozilla/firefox/*/ "$h"/snap/firefox/common/.mozilla/firefox/*/ "$h"/.var/app/org.mozilla.firefox/.mozilla/firefox/*/; do
    $SUDO test -f "${d%/}/cert9.db" && printf '%s\n' "${d%/}"
  done
  return 0
}
has_browser() {
  local b
  for b in google-chrome-stable google-chrome chromium chromium-browser brave brave-browser firefox; do command -v "$b" >/dev/null && return 0; done
  [ -n "$(nss_dbs)" ]
}
browsers() {
  [ "$TLS_MODE" = auto ] || return 0
  local ca cn file dir db n=0 h
  say "browsers on this machine: trusting iter_data's local certificate authority ($STATE/tls-ca.crt)"
  [ "$DRY" = 0 ] || { echo "  + add $STATE/tls-ca.crt to the system store and the desktop user's browser certificate databases"; return 0; }
  $SUDO test -f "$STATE/tls-ca.crt" || { say "  no $STATE/tls-ca.crt (an iter_data older than v2026.10.19): browsers warn about its certificate"; return 0; }
  ca="$(tmpdir)/iter_data-ca.crt"; $SUDO cat "$STATE/tls-ca.crt" > "$ca"; chmod 644 "$ca"; chmod 755 "$(dirname "$ca")" "$TMPROOT"
  cn="$(openssl x509 -in "$ca" -noout -subject -nameopt multiline 2>/dev/null | sed -n 's/^ *commonName *= *//p')"
  [ -n "$cn" ] || { say "  WARNING: $STATE/tls-ca.crt is not a certificate"; return 0; }
  file="iter_data-$(printf '%s' "$cn" | tr -c 'A-Za-z0-9_-' '_' | sed 's/__*/_/g; s/_$//').crt"
  if [ -d /usr/local/share/ca-certificates ] && command -v update-ca-certificates >/dev/null; then
    run install -m 644 "$ca" "/usr/local/share/ca-certificates/$file"; run update-ca-certificates >/dev/null
    say "  system store: /usr/local/share/ca-certificates/$file"
  elif command -v update-ca-trust >/dev/null; then
    dir=/etc/pki/ca-trust/source/anchors; [ -d "$dir" ] || dir=/etc/ca-certificates/trust-source/anchors
    run install -D -m 644 "$ca" "$dir/$file"; run update-ca-trust extract
    say "  system store: $dir/$file"
  else
    say "  no update-ca-certificates or update-ca-trust: the system store is left as it is"
  fi
  { [ -n "$DESK_USER" ] && has_browser; } || { say "  no desktop user with a browser here: nothing more to trust it"; return 0; }
  if ! command -v certutil >/dev/null; then
    if [ "$PKG" = apt ]; then run env DEBIAN_FRONTEND=noninteractive apt-get install -y -qq libnss3-tools; else run pacman -S --noconfirm --needed nss; fi
  fi
  command -v certutil >/dev/null || { say "  WARNING: certutil (NSS tools) missing: browsers here do not trust iter_data yet"; return 0; }
  # Chrome and Chromium read ~/.pki/nssdb: made now if the user has none yet
  h="$(desk_home)"
  $SUDO test -f "$h/.pki/nssdb/cert9.db" || { as_desk mkdir -p "$h/.pki/nssdb" && as_desk certutil -d "sql:$h/.pki/nssdb" -N --empty-password; }
  while IFS= read -r db; do
    as_desk certutil -d "sql:$db" -D -n "$cn" >/dev/null 2>&1 || true
    as_desk certutil -d "sql:$db" -A -t "C,," -n "$cn" -i "$ca" && n=$((n + 1))
  done < <(nss_dbs)
  say "  browsers: \"$cn\" trusted in $n certificate database(s) for $DESK_USER (restart the browser)"
}

# ---------------------------------------------------------------- moving an older install
# Into the iter dir, once, with the services stopped: an install from before
# 2026-10-18 (files under /etc/iter, /var/lib/iter, /opt/iter/bin, /usr/sbin, the
# database in /var/lib/arangodb3), or one made in another iter dir. Everything
# is moved (mv), nothing deleted; emptied folders are removed with rmdir. The
# database and the backups move only from their default places and only within
# one filesystem (a rename); otherwise they stay where they are, recorded in
# config/install.env and config/backup.env.
MOVED_DB_FROM="" OLD_D=""
legacy_install() { [ -e /etc/iter/data.env ] || [ -e /opt/iter/bin/iter_data ] || { [ -f "$SYSD/iter-data.service" ] && [ ! -L "$SYSD/iter-data.service" ] && [ -z "$OLD_ROOT" ]; }; }
same_fs() {  # same_fs A B: both on one filesystem (B may not exist yet)
  local b="$2"; while [ ! -e "$b" ]; do b="$(dirname "$b")"; done
  [ "$($SUDO stat -c %d "$1")" = "$($SUDO stat -c %d "$b")" ]
}
# move every entry of folder FROM into folder TO (made OWNER:MODE); a name that
# is taken in TO goes to TO/old-<date>/ instead; FROM is removed once empty
move_into() {  # move_into FROM TO OWNER MODE
  local from="$1" to="$2" e n
  $SUDO test -d "$from" || return 0
  run install -d -o "${3%%:*}" -g "${3##*:}" -m "$4" "$to"
  while IFS= read -r e; do
    [ -n "$e" ] || continue
    n="$(basename "$e")"
    if $SUDO test -e "$to/$n"; then run install -d -m 700 "$to/old-$(date +%Y%m%d)"; run mv "$e" "$to/old-$(date +%Y%m%d)/$n"
    else run mv "$e" "$to/$n"; fi
  done < <($SUDO find "$from" -mindepth 1 -maxdepth 1 2>/dev/null)
  run rmdir "$from" 2>/dev/null || say "left $from (not empty)"
}
move_one() {  # move_one FILE DEST_FILE: only when DEST_FILE is not there yet
  $SUDO test -e "$1" || return 0
  if $SUDO test -e "$2"; then say "kept $2 (also found $1, left as it is)"; else run install -d -m 755 "$(dirname "$2")"; run mv "$1" "$2"; fi
}
stop_all() {
  have_systemd || return 0
  run systemctl stop iter-backup.timer iter-admin-cleanup.path iter-data arangodb3 2>/dev/null || true
}
migrate() {
  if [ -n "$OLD_ROOT" ] && [ "$OLD_ROOT" != "$ROOT" ] && $SUDO test -d "$OLD_ROOT/iter_data"; then
    OLD_D="$OLD_ROOT/iter_data"
    $SUDO test -e "$D" && die "iter_data is installed in $OLD_ROOT, and $D exists too: move or remove one of them first"
    say "moving iter_data from $OLD_ROOT to $ROOT (services stopped meanwhile)"
    stop_all
    run install -d -m 755 "$ROOT"
    run mv "$OLD_D" "$D"
    run rmdir "$OLD_ROOT" 2>/dev/null || true
    # a database or backup folder that was inside the old iter dir moved with it
    local v
    v="$(rootenvval ITER_DB_DIR "$INSTALL_ENV")"; case "$v" in "$OLD_D"/*) [ -n "$DB_DIR" ] || DB_DIR="$D/${v#"$OLD_D"/}" ;; esac
    v="$(rootenvval ITER_BACKUP_DIR "$BACKUP_ENV")"; case "$v" in "$OLD_D"/*) [ -n "$BACKUP_DIR" ] || BACKUP_DIR="$D/${v#"$OLD_D"/}" ;; esac
    [ "$DRY" = 0 ] || { D_DRY_NOTE="(dry run: still in $OLD_D)"; say "$D_DRY_NOTE"; }
    return 0
  fi
  legacy_install || return 0
  say "an install from before 2026-10-18 is here: moving its files into $D (services stopped meanwhile)"
  stop_all
  local old_db old_bk
  old_db="$($SUDO sed -n 's/^directory = //p' /etc/arangodb3/arangod.conf 2>/dev/null | head -1)"; old_db="${old_db:-/var/lib/arangodb3}"
  old_bk="$(rootenvval ITER_BACKUP_DIR /etc/iter/backup.env)"; old_bk="${old_bk:-/var/backups/iter}"
  run install -d -m 755 "$ROOT" "$D" "$BIN_DIR" "$D/models" "$AR" "$AR_BIN" "$AR/share"
  getent group iter >/dev/null 2>&1 || run groupadd --system iter
  move_into /etc/iter "$CONF_DIR" root:iter 750
  move_into /var/lib/iter "$STATE" iter:iter 700
  move_one /opt/iter/bin/iter_data "$BIN_DIR/iter_data"
  run rmdir /opt/iter/bin 2>/dev/null || true
  move_into /opt/iter/models "$D/models" root:root 755
  move_one /usr/local/bin/iter-backup "$BIN_DIR/iter-backup"
  move_one /usr/local/bin/iter-admin-cleanup "$BIN_DIR/iter-admin-cleanup"
  move_one /usr/sbin/arangod "$AR_BIN/arangod"
  local b; for b in "${ARANGO_CLIENTS[@]}"; do move_one "/usr/bin/$b" "$AR_BIN/$b"; done
  move_one /usr/share/arangodb3 "$AR_SHARE"
  move_into /etc/arangodb3 "$AR_CONF" root:root 755
  local f; for f in "$AR_CONF"/*.conf; do
    [ -f "$f" ] && [ "$(basename "$f")" != arangod.conf ] || continue
    run sed -i "s|/usr/share/arangodb3|$AR_SHARE|g" "$f"
  done
  if $SUDO test -L /etc/sysctl.d/60-arangodb.conf || ! $SUDO test -e /etc/sysctl.d/60-arangodb.conf; then :; else move_one /etc/sysctl.d/60-arangodb.conf "$AR_CONF/60-arangodb.conf"; fi
  # the database: from its default place, within one filesystem; else where it is
  if [ -z "$DB_DIR" ] && $SUDO test -d "$old_db"; then
    if [ "$old_db" = /var/lib/arangodb3 ] && same_fs "$old_db" "$AR" && ! $SUDO test -e "$AR/data"; then
      run mv "$old_db" "$AR/data"; MOVED_DB_FROM="$old_db"
      ! $SUDO test -d "$old_db-apps" || $SUDO test -e "$APPS_DIR" || run mv "$old_db-apps" "$APPS_DIR"
      DB_DIR="$AR/data"
    else
      DB_DIR="$old_db"; say "the database stays in $DB_DIR (not the default place, or another filesystem than $ROOT): --db-dir to choose"
      APPS_DIR="$old_db-apps"
    fi
  fi
  if [ -z "$BACKUP_DIR" ] && $SUDO test -d "$old_bk"; then
    if [ "$old_bk" = /var/backups/iter ] && same_fs "$old_bk" "$D" && ! $SUDO test -e "$D/backups"; then run mv "$old_bk" "$D/backups"; BACKUP_DIR="$D/backups"
    else BACKUP_DIR="$old_bk"; say "the backups stay in $BACKUP_DIR: --backup-dir to choose"; fi
  fi
  # the old units: replaced by links into $UNIT_DIR (install_units)
  local u; for u in "${UNITS[@]}"; do
    if $SUDO test -f "$SYSD/$u" && ! $SUDO test -L "$SYSD/$u"; then run mv "$SYSD/$u" "$(tmpdir)/"; fi
  done
  have_systemd && run systemctl daemon-reload || true
  id arangodb >/dev/null 2>&1 && [ -n "$DB_DIR" ] && run usermod -d "$DB_DIR" arangodb 2>/dev/null || true
  run rmdir /opt/iter 2>/dev/null || true
  say "moved: everything of iter_data is in $D now"
}

# ---------------------------------------------------------------- commands
do_install() {
  detect_distro
  ensure_tools
  have_systemd || [ "$DRY" = 1 ] || die "systemd is not running (in WSL: [boot] systemd=true in /etc/wsl.conf, then wsl --terminate <distro>)"
  get_support
  get_binary
  memory
  migrate
  paths
  setup_iter_user
  secrets
  tls_setup
  install_arango
  install_binary
  install_model
  install_units
  start_services
  wait_health
  browsers
  [ "$DRY" = 1 ] && { say "dry run done"; return; }
  # || true: Arch has no hostname command without inetutils
  local ip; ip="$(hostname -I 2>/dev/null | awk '{print $1}' || true)"
  say "webui: $SCHEME://${ip:-127.0.0.1}:$PORT/"
  if $SUDO grep -q '^ITER_ADMIN_PASSWORD=' "$DATA_ENV" 2>/dev/null; then say "first sign-in: admin, password: sudo sed -n 's/^ITER_ADMIN_PASSWORD=//p' $DATA_ENV"
  else say "sign in as admin with the password you chose"; fi
  tls_report
  if [ -n "$FRESH_ADMIN" ]; then
    say "admin password (shown once; also in $DATA_ENV until you change it): $ADMIN_PW"
    say "  the webui makes you change it at the first sign-in (a new password needs 12+ characters); then it is removed from $DATA_ENV"
  fi
  admin_cleanup
  if [ "$EXTERNAL" = 1 ]; then say "backups of database $A_DB every 6h -> $BACKUP_DIR (from $A_URL)"
  else say "backups of database $A_DB every 6h -> $BACKUP_DIR; Arango console: ssh -L 8529:127.0.0.1:8529 <this host>, then http://127.0.0.1:8529/"; fi
}

# the first-boot admin password, once the admin has chosen their own (see iter-admin-cleanup)
admin_cleanup() {
  [ "$DRY" = 0 ] || { echo "  + $BIN_DIR/iter-admin-cleanup (when $MARKER exists)"; return 0; }
  $SUDO test -f "$MARKER" || return 0
  $SUDO test -x "$BIN_DIR/iter-admin-cleanup" || return 0
  $SUDO "$BIN_DIR/iter-admin-cleanup" | sed 's/^iter-admin-cleanup: /[iter_data] /' || true
}

tls_report() {
  if [ "$TLS_MODE" = off ]; then
    say "TLS is off: put a TLS proxy in front; engines refuse plain http to another machine"; return
  fi
  local fp; fp="$(fingerprint)"
  say "TLS certificate fingerprint: ${fp:-(not found: journalctl -u iter-data | grep fingerprint)}"
  say "  give it to each engine installer (--fingerprint ${fp:-sha256:...}), or compare it with \`iter_engine key status\` on the engine"
  say "  (also in the webui's Engine keys screen, and: $0 status)"
  [ "$TLS_MODE" = custom ] || say "  browsers here trust it (its local CA was added above); on another machine the engine installer adds it, or open $SCHEME://<this host>:$PORT/tls/ca.crt and trust it as a certificate authority"
}

do_status() {
  local s
  if legacy_install; then say "an install from before 2026-10-18 is here: \`$0 install\` (or migrate) moves it into $D"; fi
  admin_cleanup
  local aurl; aurl="$(rootenvval ARANGO_URL "$DATA_ENV")"
  for s in arangodb3 iter-data iter-backup.timer; do
    [ "$s" != arangodb3 ] || [ -e "$SYSD/arangodb3.service" ] || continue
    printf '%-18s %s\n' "$s" "$(systemctl is-active "$s" 2>/dev/null || true)"
  done
  printf '%-18s %s\n' "iter dir" "$ROOT (database $(rootenvval ITER_DB_DIR "$INSTALL_ENV"), backups $(rootenvval ITER_BACKUP_DIR "$BACKUP_ENV"))"
  printf '%-18s %s, user %s, database %s\n' arangodb "${aurl:-?}" "$(rootenvval ARANGO_USER "$DATA_ENV")" "$(rootenvval ARANGO_DB "$DATA_ENV")"
  printf '%-18s %s\n' iter_data "$(stat -c 'installed %y' "$BIN_DIR/iter_data" 2>/dev/null | cut -c1-20 || echo "not installed")"
  TLS_MODE="$(rootenvval ITER_TLS_MODE "$TLS_ENV")"; TLS_MODE="${TLS_MODE:-auto}"
  local scheme=https fp; [ "$TLS_MODE" != off ] || scheme=http
  curl -fsSk "$scheme://127.0.0.1:$PORT/health" 2>/dev/null && echo || echo "iter_data: no answer on $scheme://127.0.0.1:$PORT"
  if [ "$TLS_MODE" = off ]; then printf '%-18s %s\n' tls "off (plain HTTP, behind a TLS proxy)"
  else fp="$(fingerprint)"; printf '%-18s %s\n' tls "$TLS_MODE, certificate fingerprint ${fp:-unknown (not started yet?)}"; fi
  systemctl list-timers iter-backup.timer --no-pager 2>/dev/null | sed -n 2p || true
  local dir; dir="$(rootenvval ITER_BACKUP_DIR "$BACKUP_ENV")"
  $SUDO sh -c "ls -1t '${dir:-$D/backups}'/iter-*.tar '${dir:-$D/backups}'/iter5-*.tar 2>/dev/null | head -3" | grep . || echo "no backups yet"
}

# load a backup's database back into iter's database (only that one), with
# iter-data stopped so nothing writes meanwhile; its env and secret files are
# left as they are (they are in the tar, for a move to a new machine)
do_restore() {
  [ "$DRY" = 1 ] || $SUDO test -f "$RESTORE_FILE" || die "$RESTORE_FILE not found"
  say "restoring database $(rootenvval ARANGO_DB "$DATA_ENV") from $RESTORE_FILE (its collections are replaced)"
  run systemctl stop iter-data
  # shellcheck disable=SC2016  # expanded by the inner shell, which reads data.env
  run bash -c 'set -a; . "$2/data.env"; [ ! -f "$2/backup.env" ] || . "$2/backup.env"; set +a; exec "$3/iter-backup" --restore "$1"' _ "$RESTORE_FILE" "$CONF_DIR" "$BIN_DIR" \
    || { run systemctl start iter-data; die "restore failed; iter-data started again"; }
  run systemctl start iter-data
  PORT="$(sed -n 's/.*--listen 0.0.0.0:\([0-9]*\).*/\1/p' "$SYSD/iter-data.service" 2>/dev/null | head -1)"; PORT="${PORT:-8400}"
  TLS_MODE="$(rootenvval ITER_TLS_MODE "$TLS_ENV")"; SCHEME=https; [ "${TLS_MODE:-auto}" != off ] || SCHEME=http
  wait_health
}

do_uninstall() {
  local u
  paths
  say "stopping and removing the services and programs; $CONF_DIR, $STATE, the database ($DB_DIR) and the backups ($BACKUP_DIR) are kept"
  run systemctl disable --now iter-backup.timer iter-admin-cleanup.path iter-admin-cleanup.service iter-data arangodb3 || true
  for u in "${UNITS[@]}"; do run rm -f "$SYSD/$u"; done
  run systemctl daemon-reload || true
  run rm -f "$BIN_DIR/iter_data" "$BIN_DIR/iter-backup" "$BIN_DIR/iter-admin-cleanup" "$AR_BIN/arangod" /etc/sysctl.d/60-arangodb.conf
  for u in "${ARANGO_CLIENTS[@]}"; do run rm -f "$AR_BIN/$u"; done
  say "to remove the data too: $D (and the database or backup folder if they are elsewhere); users iter and arangodb"
}

case "$CMD" in
  install) do_install ;;
  migrate) get_support; migrate; paths ;;
  status) do_status ;;
  backup) run systemctl start iter-backup.service; [ "$DRY" = 1 ] || $SUDO journalctl -u iter-backup --no-pager -n 1 -o cat ;;
  restore) do_restore ;;
  uninstall) do_uninstall ;;
esac
