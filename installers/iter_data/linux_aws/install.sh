#!/usr/bin/env bash
# Install iter_data natively as systemd services on an EC2 instance running Amazon
# Linux 2023 (x86_64 or Graviton aarch64): ArangoDB CE 3.12 (localhost only),
# iter_data (API + webui on :8400) and an online backup every 6 hours.
#
#   ./install.sh [install] [options]   install or upgrade (rerun it: idempotent)
#   ./install.sh status                services, health, the last backups
#   ./install.sh backup                take a backup now (online, nothing stops)
#   ./install.sh uninstall             stop + remove services and programs; data is kept
#   curl -fsSL https://raw.githubusercontent.com/Stephen-Hilton/iter/main/installers/iter_data/linux_aws/install.sh | bash -s -- --version vYYYY.MM.DD
#
# Options:
#   --version TAG          release to download iter_data (and, when piped, this folder) from [latest]
#   --binary PATH          use this iter_data binary instead of the one beside the script / the release
#   --env-file PATH        take ITER_ADMIN_PASSWORD, ITER_JWT_SECRET, ARANGO_ROOT_PASSWORD from it
#   --port N               iter_data port [8400]
#   --db-dir DIR           ArangoDB data directory, e.g. on a mounted EBS volume [/var/lib/arangodb3]
#   --backup-dir DIR       where backups go [/var/backups/iter]
#   --memory MiB           memory cap for ArangoDB + iter_data together [min(6144, 85% of RAM)]
#   --arango-version V     ArangoDB CE version, from the official arangodb image [3.12.12]
#   --tls-cert PATH        serve this certificate (PEM, e.g. a real one for your domain) instead of
#                          the self-signed one; needs --tls-key; copied to /var/lib/iter (user iter)
#   --tls-key PATH         the private key (PEM) for --tls-cert (kept mode 600)
#   --tls-name NAME        an extra DNS name or IP on the self-signed certificate (repeatable), e.g.
#                          the instance's public DNS name or Elastic IP;
#                          only read when the certificate is first made
#   --tls off|auto         off = plain HTTP, only behind a TLS proxy on this host; auto = the
#                          self-signed certificate again (drops an earlier --tls-cert or --tls off)
#   --dry-run              print every action, change nothing
#   -h, --help             this text
#
# Lands in: /opt/iter/bin/iter_data, /opt/iter/models/ (embedding model),
# /etc/iter/data.env (secrets, root:iter 640), /var/lib/iter (iter_data's files, user
# iter), the database in --db-dir (user arangodb), /usr/sbin/arangod + clients.
# HTTPS by default: iter_data makes a self-signed certificate on first start
# (/var/lib/iter/tls.crt, kept across reinstalls); the script prints its fingerprint,
# to give to each engine installer (--fingerprint) or compare with `iter_engine key
# status`. The TLS choices are kept in /etc/iter/tls.env, so a plain rerun keeps them.
# Logs: journalctl -u iter-data -u arangodb3. Run as ec2-user (sudo) or root. Needs
# no instance metadata (IMDS) and no AWS credentials.
set -euo pipefail

REPO="Stephen-Hilton/iter"
FOLDER="installers/iter_data/linux_aws"
# shellcheck disable=SC2015  # piped from curl there is no script dir: fall back to the cwd
HERE="$(cd "$(dirname "${BASH_SOURCE[0]:-$0}")" 2>/dev/null && pwd || pwd)"
SUPPORT=(arangod.conf iter-backup iter-admin-cleanup systemd/iter.slice systemd/arangodb3.service systemd/iter-data.service
         systemd/iter-backup.service systemd/iter-backup.timer systemd/iter-admin-cleanup.path systemd/iter-admin-cleanup.service)
UNITS=(iter.slice arangodb3.service iter-data.service iter-backup.service iter-backup.timer
       iter-admin-cleanup.path iter-admin-cleanup.service)
# iter_data writes this once the first admin has chosen their own password
MARKER=/var/lib/iter/admin_password_changed
MODEL_SHA="53aa51172d142c89d9012cce15ae4d6cc0ca6895895114379cacb4fab128d9db"
ARANGO_CLIENTS=(arangosh arangodump arangorestore arangobackup arangoexport arangoimport)

CMD=install
VERSION="${ITER_VERSION:-}"
BINARY=""
ENV_SRC=""
PORT="${ITER_PORT:-8400}"
DB_DIR=/var/lib/arangodb3
BACKUP_DIR=""
MEM_MIB=""
ARANGO_VERSION="${ARANGO_VERSION:-3.12.12}"
DRY=0
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
    install|status|backup|uninstall) CMD="$1"; shift ;;
    --version) VERSION="$2"; shift 2 ;;
    --binary) BINARY="$2"; shift 2 ;;
    --env-file) ENV_SRC="$2"; shift 2 ;;
    --port) PORT="$2"; shift 2 ;;
    --db-dir) DB_DIR="${2%/}"; shift 2 ;;
    --backup-dir) BACKUP_DIR="${2%/}"; shift 2 ;;
    --memory) MEM_MIB="$2"; shift 2 ;;
    --arango-version) ARANGO_VERSION="$2"; shift 2 ;;
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
for n in ${TLS_NAMES_OPT[@]+"${TLS_NAMES_OPT[@]}"}; do
  case "$n" in ""|*[!A-Za-z0-9.:-]*) die "--tls-name $n: a DNS name or an IP address" ;; esac
done

case "$(uname -m)" in x86_64|amd64) ARCH=x86_64; OCI_ARCH=amd64 ;; aarch64|arm64) ARCH=aarch64; OCI_ARCH=arm64 ;; *) die "unsupported CPU $(uname -m)" ;; esac
ASSET="iter_data-linux-$ARCH"

# ---------------------------------------------------------------- distro + tools
detect_distro() {
  [ -f /etc/os-release ] || die "no /etc/os-release: unsupported system"
  # in a subshell: os-release sets VERSION, which would clobber --version
  local ID ID_LIKE PRETTY_NAME VERSION_ID
  IFS='|' read -r ID ID_LIKE PRETTY_NAME VERSION_ID < <(
    # shellcheck disable=SC1091
    . /etc/os-release; echo "${ID:-unknown}|${ID_LIKE:-none}|${PRETTY_NAME:-$ID}|${VERSION_ID:-}")
  [ "$ID_LIKE" != none ] || ID_LIKE=""
  case " $ID $ID_LIKE " in
    *" amzn "*)
      [ "${VERSION_ID%%.*}" = 2023 ] || die "${PRETTY_NAME} is not Amazon Linux 2023 (Amazon Linux 2 is not supported: glibc 2.26)" ;;
    *" fedora "*|*" rhel "*) say "WARNING: ${PRETTY_NAME} is not Amazon Linux 2023; carrying on with dnf" ;;
    *) die "${PRETTY_NAME:-this distro} is not Amazon Linux 2023 (Debian/Ubuntu/Arch: use installers/iter_data/linux)" ;;
  esac
  command -v dnf >/dev/null || die "dnf not found"
  PKG=dnf
  say "found ${PRETTY_NAME:-$ID} ($ARCH), package manager $PKG"
}

ensure_tools() {
  # command:package pairs. curl: AL2023 ships curl-minimal, which is enough (installing
  # the curl package over it conflicts), so it is only added when no curl exists at all.
  local missing=() c p pair
  for pair in curl:curl-minimal jq:jq tar:tar gzip:gzip sha256sum:coreutils useradd:shadow-utils sysctl:procps-ng \
              install:coreutils ps:procps-ng getent:glibc-common hostname:hostname nologin:util-linux \
              openssl:openssl; do
    c="${pair%%:*}"; p="${pair##*:}"
    command -v "$c" >/dev/null || missing+=("$p")
  done
  # the iter_data binary links libstdc++.so.6 and libgcc_s.so.1
  { ldconfig -p 2>/dev/null || true; } | grep -q 'libstdc++\.so\.6' || missing+=(libstdc++)
  [ -s /etc/ssl/certs/ca-certificates.crt ] || [ -s /etc/pki/tls/certs/ca-bundle.crt ] || missing+=(ca-certificates)
  if [ ${#missing[@]} = 0 ]; then say "tools: curl jq tar gzip sha256sum useradd sysctl openssl present"; return; fi
  mapfile -t missing < <(printf '%s\n' "${missing[@]}" | sort -u)
  say "installing ${missing[*]}"
  run dnf install -y -q "${missing[@]}"
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
  say "installing iter_data -> /opt/iter/bin/iter_data"
  run install -d -m 755 /opt/iter /opt/iter/bin
  # a temp name in the same dir, then mv over: never cp over a running executable
  run install -m 755 "$BIN_FILE" /opt/iter/bin/.iter_data.new
  run mv -f /opt/iter/bin/.iter_data.new /opt/iter/bin/iter_data
}

# ---------------------------------------------------------------- embedding model
install_model() {
  local dir=/opt/iter/models/all-MiniLM-L6-v2 hf=https://huggingface.co/sentence-transformers/all-MiniLM-L6-v2/resolve/main f
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
  [ -x /usr/sbin/arangod ] && have="$(/usr/sbin/arangod --version 2>/dev/null | head -1 || true)"
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
    run install -m 755 "$t/root/usr/sbin/arangod" /usr/sbin/.arangod.new
    run mv -f /usr/sbin/.arangod.new /usr/sbin/arangod
    for b in "${ARANGO_CLIENTS[@]}"; do run install -m 755 "$t/root/usr/bin/$b" "/usr/bin/$b"; done
    [ -e /usr/share/arangodb3 ] && run mv /usr/share/arangodb3 "/usr/share/.arangodb3.old.$$"
    run cp -r "$t/root/usr/share/arangodb3" /usr/share/arangodb3
    run rm -rf "/usr/share/.arangodb3.old.$$"
    run install -d -m 755 /etc/arangodb3
    for f in "$t"/root/etc/arangodb3/*.conf; do
      [ "$DRY" = 1 ] && { echo "  + install the image's /etc/arangodb3/*.conf (client configs)"; break; }
      [ "$(basename "$f")" = arangod.conf ] || run install -m 644 "$f" /etc/arangodb3/
    done
  fi
  sed "s|@DB_DIR@|$DB_DIR|g" "$SRC/arangod.conf" | put 644 root:root /etc/arangodb3/arangod.conf
  id arangodb >/dev/null 2>&1 || run useradd --system --user-group --home-dir "$DB_DIR" --shell "$NOLOGIN" arangodb
  run install -d -o arangodb -g arangodb -m 750 "$DB_DIR" "$DB_DIR-apps"
  echo 'vm.max_map_count = 1024000' | put 644 root:root /etc/sysctl.d/60-arangodb.conf
  run sysctl -q -p /etc/sysctl.d/60-arangodb.conf || say "sysctl vm.max_map_count not applied (fine in a container)"
  printf 'ARANGODB_DEFAULT_ROOT_PASSWORD=%s\n' "$ARANGO_PW" | put 600 root:root /etc/arangodb3/root.env
}

# ---------------------------------------------------------------- secrets
# /etc/iter/data.env, kept across reinstalls. Values: --env-file, else the existing
# /etc/iter/data.env, else an older ~/.iter/data.env (linux/install.sh server), else new.
LEGACY=""
secrets() {
  local cur=/etc/iter/data.env u h
  u="${SUDO_USER:-$(id -un)}"; h="$(getent passwd "$u" | cut -d: -f6)"
  [ -n "$h" ] && [ -f "$h/.iter/data.env" ] && LEGACY="$h/.iter"
  [ -z "$ENV_SRC" ] || [ -f "$ENV_SRC" ] || die "--env-file $ENV_SRC does not exist"
  pickv() {
    local v=""
    [ -n "$ENV_SRC" ] && v="$(envval "$1" "$ENV_SRC")"
    [ -n "$v" ] || v="$(rootenvval "$1" "$cur")"
    [ -n "$v" ] || [ -z "$LEGACY" ] || v="$(envval "$1" "$LEGACY/data.env")"
    printf '%s' "$v"
  }
  ADMIN_PW="$(pickv ITER_ADMIN_PASSWORD)"
  JWT="$(pickv ITER_JWT_SECRET)"
  ARANGO_PW="$(pickv ARANGO_ROOT_PASSWORD)"; [ -n "$ARANGO_PW" ] || ARANGO_PW="$(pickv ARANGO_PASSWORD)"
  local fresh=1; $SUDO test -f "$DB_DIR/SERVER" && fresh=0
  FRESH_ADMIN=""
  if $SUDO test -f "$MARKER"; then
    # the admin chose their own password: the first-boot one is not written back
    [ -z "$ADMIN_PW" ] || say "the admin has chosen their own password: ITER_ADMIN_PASSWORD is left out of $cur"
    ADMIN_PW=""
  fi
  if [ -z "$ADMIN_PW" ] && [ "$fresh" = 1 ]; then ADMIN_PW="$(gen 20)"; FRESH_ADMIN=1; fi
  if [ -z "$JWT" ]; then
    if $SUDO test -s /var/lib/iter/iter_data.secret || { [ -n "$LEGACY" ] && [ -s "$LEGACY/iter_data/iter_data.secret" ]; }; then
      say "no ITER_JWT_SECRET given: iter_data keeps its iter_data.secret file"
    else JWT="$(gen 48)"; say "generated a new ITER_JWT_SECRET"; fi
  fi
  if [ -z "$ARANGO_PW" ]; then
    if [ "$fresh" = 0 ]; then ARANGO_PW=iter4dev; say "existing database, no Arango password given: assuming the old default"
    else ARANGO_PW="$(gen 24)"; say "generated an Arango root password (kept in $cur)"; fi
  fi
  [ -z "$LEGACY" ] || say "found an older install's settings in $LEGACY (used where nothing newer is given)"
  {
    [ -z "$ADMIN_PW" ] || echo "ITER_ADMIN_PASSWORD=$ADMIN_PW"
    [ -z "$JWT" ] || echo "ITER_JWT_SECRET=$JWT"
    echo "ARANGO_ROOT_PASSWORD=$ARANGO_PW"
    echo "ARANGO_URL=http://127.0.0.1:8529"
    echo "ARANGO_USER=root"
    echo "ARANGO_PASSWORD=$ARANGO_PW"
    echo "ARANGO_DB=iter5"
    echo "ITER_EMBED_MODEL=/opt/iter/models/all-MiniLM-L6-v2"
  } | put 640 root:iter "$cur"
}

# ---------------------------------------------------------------- iter user + units
NOLOGIN=/usr/sbin/nologin
setup_iter_user() {
  NOLOGIN="$(command -v nologin || echo /usr/sbin/nologin)"
  getent group iter >/dev/null 2>&1 || run groupadd --system iter
  id iter >/dev/null 2>&1 || run useradd --system --gid iter --home-dir /var/lib/iter --shell "$NOLOGIN" iter
  run install -d -o iter -g iter -m 700 /var/lib/iter
  run install -d -m 755 /etc/iter
  # an older ~/.iter install: bring iter_data's own files over once
  local f
  if [ -n "$LEGACY" ]; then
    for f in iter_data.secret engine_session.secret tls.crt tls.key .env; do
      if [ -f "$LEGACY/iter_data/$f" ] && ! $SUDO test -f "/var/lib/iter/$f"; then
        run install -o iter -g iter -m 600 "$LEGACY/iter_data/$f" "/var/lib/iter/$f"
      fi
    done
  fi
}

# ---------------------------------------------------------------- TLS
# iter_data serves HTTPS (--tls auto): a self-signed certificate it makes on first start
# beside its secret file (/var/lib/iter/tls.crt + tls.key, user iter, 600), kept across
# reinstalls like iter_data.secret and engine_session.secret. The choice made here is
# kept in /etc/iter/tls.env, so a rerun without TLS options leaves it as it was.
CUSTOM_CRT=/var/lib/iter/custom-tls.crt
CUSTOM_KEY=/var/lib/iter/custom-tls.key
tls_setup() {
  local names
  TLS_MODE="$(rootenvval ITER_TLS_MODE /etc/iter/tls.env)"; TLS_MODE="${TLS_MODE:-auto}"
  names="$(rootenvval ITER_TLS_NAMES /etc/iter/tls.env)"
  [ -z "$TLS_OPT" ] || TLS_MODE="$TLS_OPT"
  [ -z "$TLS_CERT_SRC" ] || TLS_MODE=custom
  if [ ${#TLS_NAMES_OPT[@]} -gt 0 ]; then names="$(IFS=,; echo "${TLS_NAMES_OPT[*]}")"; fi
  TLS_NAMES="$names"
  if [ -n "$TLS_CERT_SRC" ]; then
    [ -r "$TLS_CERT_SRC" ] && [ -r "$TLS_KEY_SRC" ] || die "--tls-cert / --tls-key: cannot read $TLS_CERT_SRC or $TLS_KEY_SRC"
    say "TLS: serving your certificate $TLS_CERT_SRC (copied to $CUSTOM_CRT)"
    run install -o iter -g iter -m 644 "$TLS_CERT_SRC" "$CUSTOM_CRT"
    run install -o iter -g iter -m 600 "$TLS_KEY_SRC" "$CUSTOM_KEY"
  elif [ "$TLS_MODE" = custom ]; then
    $SUDO test -f "$CUSTOM_CRT" || [ "$DRY" = 1 ] || die "/etc/iter/tls.env says custom, but $CUSTOM_CRT is gone: rerun with --tls-cert/--tls-key (or --tls auto)"
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
       say "TLS: a self-signed certificate (made on first start, kept in /var/lib/iter)${names:+, extra names $names}" ;;
  esac
  [ -z "$TLS_ARGS" ] || say "TLS: iter-data runs with $TLS_ARGS"
  SCHEME=https; [ "$TLS_MODE" != off ] || SCHEME=http
  printf 'ITER_TLS_MODE=%s\nITER_TLS_NAMES=%s\n' "$TLS_MODE" "$names" | put 644 root:root /etc/iter/tls.env
}

# the certificate's fingerprint (sha256:<hex> of its DER, what engines pin): openssl on
# the certificate file, else the line iter_data logs at every start
fingerprint() {
  local crt=/var/lib/iter/tls.crt fp=""
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
  local u
  [ -n "$BACKUP_DIR" ] || BACKUP_DIR="$(rootenvval ITER_BACKUP_DIR /etc/iter/backup.env)"
  [ -n "$BACKUP_DIR" ] || BACKUP_DIR=/var/backups/iter
  run install -d -o iter -g iter -m 700 "$BACKUP_DIR"
  printf 'ITER_BACKUP_DIR=%s\nITER_BACKUP_KEEP=%s\n' "$BACKUP_DIR" "$(rootenvval ITER_BACKUP_KEEP /etc/iter/backup.env | grep . || echo 28)" \
    | put 644 root:root /etc/iter/backup.env
  put 755 root:root /usr/local/bin/iter-backup <"$SRC/iter-backup"
  put 755 root:root /usr/local/bin/iter-admin-cleanup <"$SRC/iter-admin-cleanup"
  for u in "${UNITS[@]}"; do
    sed -e "s|@PORT@|$PORT|g" -e "s|@TLS_ARGS@|$TLS_ARGS|g" -e "s|@DB_DIR@|$DB_DIR|g" -e "s|@MEM_MAX@|${MEM_MIB}M|g" -e "s|@ARANGO_MEM@|${ARANGO_MIB}M|g" \
      "$SRC/systemd/$u" | put 644 root:root "/etc/systemd/system/$u"
  done
}

start_services() {
  if ! have_systemd; then
    [ "$DRY" = 1 ] || die "systemd is not running (in WSL: [boot] systemd=true in /etc/wsl.conf, then wsl --terminate <distro>)"
    say "systemd is not running here (fine for a dry run; a real install needs it)"
  fi
  run systemctl daemon-reload
  run systemctl enable --now arangodb3
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

# ---------------------------------------------------------------- commands
do_install() {
  detect_distro
  ensure_tools
  have_systemd || [ "$DRY" = 1 ] || die "systemd is not running (in WSL: [boot] systemd=true in /etc/wsl.conf, then wsl --terminate <distro>)"
  get_support
  get_binary
  memory
  setup_iter_user
  secrets
  tls_setup
  install_arango
  install_binary
  install_model
  install_units
  start_services
  wait_health
  [ "$DRY" = 1 ] && { say "dry run done"; return; }
  local ip; ip="$(hostname -I 2>/dev/null | awk '{print $1}')"
  say "webui: $SCHEME://${ip:-127.0.0.1}:$PORT/ inside the VPC, or $SCHEME://<the instance's public DNS or IP>:$PORT/ (open :$PORT in its security group)"
  if $SUDO grep -q '^ITER_ADMIN_PASSWORD=' /etc/iter/data.env 2>/dev/null; then say "first sign-in: admin / ITER_ADMIN_PASSWORD in /etc/iter/data.env"
  else say "sign in as admin with the password you chose"; fi
  tls_report
  if [ -n "$FRESH_ADMIN" ]; then
    say "admin password (shown once; also in /etc/iter/data.env until you change it): $ADMIN_PW"
    say "  the webui makes you change it at the first sign-in (a new password needs 12+ characters); then it is removed from /etc/iter/data.env"
  fi
  admin_cleanup
  say "backups every 6h -> $BACKUP_DIR; Arango console: ssh -L 8529:127.0.0.1:8529 <this host>, then http://127.0.0.1:8529/"
}

# the first-boot admin password, once the admin has chosen their own (see iter-admin-cleanup)
admin_cleanup() {
  [ "$DRY" = 0 ] || { echo "  + /usr/local/bin/iter-admin-cleanup (when $MARKER exists)"; return 0; }
  $SUDO test -f "$MARKER" || return 0
  $SUDO test -x /usr/local/bin/iter-admin-cleanup || return 0
  $SUDO /usr/local/bin/iter-admin-cleanup | sed 's/^iter-admin-cleanup: /[iter_data] /' || true
}

tls_report() {
  if [ "$TLS_MODE" = off ]; then
    say "TLS is off: put a TLS proxy in front; engines refuse plain http to another machine"; return
  fi
  local fp; fp="$(fingerprint)"
  say "TLS certificate fingerprint: ${fp:-(not found: journalctl -u iter-data | grep fingerprint)}"
  say "  give it to each engine installer (--fingerprint ${fp:-sha256:...}), or compare it with \`iter_engine key status\` on the engine"
  say "  (also in the webui's Engine keys screen, and: $0 status)"
  [ "$TLS_MODE" = custom ] || say "  browsers warn about this self-signed certificate: accept it once, or rerun with --tls-cert/--tls-key (or put a TLS proxy in front)"
    [ -n "$TLS_NAMES" ] || say "  (engines pin the fingerprint, so names do not matter to them; for browsers, rerun with --tls-name <public DNS or Elastic IP> before the first start, or use --tls-cert)"
}

do_status() {
  local s
  admin_cleanup
  for s in arangodb3 iter-data iter-backup.timer; do printf '%-18s %s\n' "$s" "$(systemctl is-active "$s" 2>/dev/null || true)"; done
  printf '%-18s %s\n' iter_data "$(stat -c 'installed %y' /opt/iter/bin/iter_data 2>/dev/null | cut -c1-20 || echo "not installed")"
  TLS_MODE="$(rootenvval ITER_TLS_MODE /etc/iter/tls.env)"; TLS_MODE="${TLS_MODE:-auto}"
  local scheme=https fp; [ "$TLS_MODE" != off ] || scheme=http
  curl -fsSk "$scheme://127.0.0.1:$PORT/health" 2>/dev/null && echo || echo "iter_data: no answer on $scheme://127.0.0.1:$PORT"
  if [ "$TLS_MODE" = off ]; then printf '%-18s %s\n' tls "off (plain HTTP, behind a TLS proxy)"
  else fp="$(fingerprint)"; printf '%-18s %s\n' tls "$TLS_MODE, certificate fingerprint ${fp:-unknown (not started yet?)}"; fi
  systemctl list-timers iter-backup.timer --no-pager 2>/dev/null | sed -n 2p || true
  local dir; dir="$(rootenvval ITER_BACKUP_DIR /etc/iter/backup.env)"
  $SUDO sh -c "ls -1t '${dir:-/var/backups/iter}'/iter5-*.tar 2>/dev/null | head -3" | grep . || echo "no backups yet"
}

do_uninstall() {
  local u
  say "stopping and removing the services and programs; the database ($DB_DIR, or the --db-dir it was given), /var/lib/iter, /etc/iter and the backups are kept"
  run systemctl disable --now iter-backup.timer iter-admin-cleanup.path iter-admin-cleanup.service iter-data arangodb3 || true
  for u in "${UNITS[@]}"; do run rm -f "/etc/systemd/system/$u"; done
  run systemctl daemon-reload || true
  run rm -f /opt/iter/bin/iter_data /usr/local/bin/iter-backup /usr/local/bin/iter-admin-cleanup /usr/sbin/arangod /etc/sysctl.d/60-arangodb.conf
  for u in "${ARANGO_CLIENTS[@]}"; do run rm -f "/usr/bin/$u"; done
  say "to remove the data too: /var/lib/arangodb3 (or your --db-dir and its -apps), /var/lib/iter, /etc/iter, /etc/arangodb3, /opt/iter, the backup folder; users iter and arangodb"
}

case "$CMD" in
  install) do_install ;;
  status) do_status ;;
  backup) run systemctl start iter-backup.service; [ "$DRY" = 1 ] || $SUDO journalctl -u iter-backup --no-pager -n 1 -o cat ;;
  uninstall) do_uninstall ;;
esac
