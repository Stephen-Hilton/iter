#!/usr/bin/env bash
# install.sh - run this machine's iter engine in a Docker container (Linux, or
# macOS with Docker Desktop). ONE engine per machine still: not beside a native
# engine on the same host. The agents (claude sessions, shell steps, tests)
# run inside the container, so a project whose steps need tools from the host
# wants a native engine instead (installers/iter_engine/linux, macos, windows).
#
#   curl -fsSL https://raw.githubusercontent.com/Stephen-Hilton/iter/main/installers/iter_engine/docker/install.sh | bash
#   ./install.sh [verb] [options]
#
# Verbs:
#   install     (default) set up or repair; safe to run again
#   status      the container, the key, the served projects
#   update      a newer iter_engine (and claude CLI): rebuild the image, recreate the container
#               (refuses while work runs, without --yes)
#   accounts    ask for each account token the served projects name
#   projects    clone each served project that has no checkout yet (on the host)
#   logs        follow the container's log
#   uninstall   stop and remove the container and its image (keeps ~/.iter and the checkouts)
#
# Options:
#   --data-url URL    iter_data URL, https:// (else asked; kept in ~/.iter/engine.conf).
#                     iter_data on this same host: https://host.docker.internal:<port>, not
#                     127.0.0.1 (the script rewrites 127.0.0.1 and localhost)
#   --name NAME       this engine's name, unique per machine (default: the hostname)
#   --checkouts DIR   the host folder holding the project checkouts [~/dev], mounted at the
#                     same path; every serves edge's topdir must be inside it
#   --version TAG     release to download, e.g. v2026.10.17 (else $ITER_VERSION, else latest)
#   --binary PATH     put this iter_engine-linux-<arch> file in the image instead
#   --fingerprint FP  iter_data's certificate fingerprint (sha256:..., from the iter_data
#                     installer or the webui's Engine keys screen): pinned before sign-in
#   --trust-ca        iter_data is behind a TLS proxy with a real certificate: check it
#                     against certificate authorities instead of pinning
#   --secrets-command CMD  a shell command printing KEY=value lines (the account tokens,
#                     from a secret store); it runs INSIDE the container (add the CLI it
#                     needs to the Dockerfile), at startup and every --secrets-every
#                     minutes, values in memory only. Kept in ~/.iter/engine.conf and
#                     ~/.iter/docker/secrets_command; "none" removes it. With it, the
#                     accounts step asks for no tokens.
#   --secrets-every N  minutes between secrets-command runs [5]
#   --yes             answer yes to every question; update while work runs
#   --dry-run         print every action, change nothing
#   -h, --help        this text
#   ($ITER_RELEASE_URL: download the binary and SHA256SUMS from this base URL instead)
#
# What it sets up: ~/.iter/docker = the build folder (Dockerfile, entrypoint.sh,
# compose.yml from beside this script, else from GitHub at --version; the
# iter_engine-linux-<arch> binary; .env with the compose settings;
# compose.override.yml mounting the git credentials it finds read-only:
# ~/.ssh, ~/.gitconfig, ~/.git-credentials, ~/.config/gh). ~/.iter (key, .env
# with account tokens, engine.conf, models/, claude/ = the container's claude
# config) and the checkouts are mounted at the same absolute paths, and the
# container runs as your uid/gid with HOME = your home path, so files it
# writes are yours and ~/dev/<project> topdirs match. Credentials for claude
# come from the account tokens in ~/.iter/.env (`claude setup-token`); the
# host's own claude login is not shared.
#
# HTTPS: iter_data must be https:// (it serves HTTPS by default); plain http://
# is refused, because from inside the container iter_data is always across a
# network, even on this host. The engine pins iter_data's certificate
# fingerprint in ~/.iter/server_trust.json: --fingerprint (or the fingerprint
# pasted when asked) runs `iter_engine key trust` in the image right after it
# is built and stops if the server presents another certificate; without one,
# the first sign-in trusts the certificate it sees (compare: install.sh status).
# Key commands, in ~/.iter/docker: docker compose run --rm --no-deps engine
# iter_engine key show (the public key) | key rotate (a new key, signed by the
# current one, no webui step) | key status.
set -euo pipefail

REPO_SLUG="Stephen-Hilton/iter"
HERE=""
if [ -n "${BASH_SOURCE[0]:-}" ] && [ -f "${BASH_SOURCE[0]}" ]; then HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"; fi
IHOME="$HOME/.iter"
# iter_engine (in the image, with HOME = your home path) keeps its key and
# server_trust.json in ~/.iter, the mounted folder
CONF="$IHOME/engine.conf"
ENVF="$IHOME/.env"
KEYF="$IHOME/engine.key"
MODEL="$IHOME/models/all-MiniLM-L6-v2"
CTX="$IHOME/docker"

VERB=install DRY=0 YES=0 BINARY="" OPT_URL="" OPT_NAME="" OPT_CHECKOUTS="" OPT_FP="" TRUST_CA=0 OPT_SC="" SC_GIVEN=0 OPT_SE="" VERSION="${ITER_VERSION:-}"
while [ $# -gt 0 ]; do
  case "$1" in
    install|status|update|accounts|projects|logs|uninstall) VERB="$1" ;;
    --data-url) OPT_URL="${2:?--data-url needs a URL}"; shift ;;
    --name) OPT_NAME="${2:?--name needs a name}"; shift ;;
    --checkouts) OPT_CHECKOUTS="${2:?--checkouts needs a folder}"; shift ;;
    --version) VERSION="${2:?--version needs a tag}"; shift ;;
    --binary) BINARY="${2:?--binary needs a path}"; shift ;;
    --fingerprint) OPT_FP="${2:?--fingerprint needs sha256:<64 hex>}"; shift ;;
    --trust-ca) TRUST_CA=1 ;;
    --secrets-command) OPT_SC="${2?--secrets-command needs a command (or none)}"; SC_GIVEN=1; shift ;;
    --secrets-every) OPT_SE="${2:?--secrets-every needs minutes}"; shift ;;
    --yes|-y) YES=1 ;;
    --dry-run) DRY=1 ;;
    -h|--help|help) sed -n '2,/^set -euo/p' "${BASH_SOURCE[0]:-$0}" 2>/dev/null | sed '$d' | sed 's/^# \{0,1\}//'; exit 0 ;;
    *) printf 'unknown argument %s (try --help)\n' "$1" >&2; exit 2 ;;
  esac
  shift
done

say()  { printf '\n\033[1m==>\033[0m %s\n' "$*"; }
ok()   { printf '    \033[32mok\033[0m  %s\n' "$*"; }
warn() { printf '    \033[33m!!\033[0m  %s\n' "$*" >&2; }
die()  { printf '\n\033[31mstopped:\033[0m %s\n' "$*" >&2; exit 1; }
run() { if [ $DRY = 1 ]; then printf '    [dry-run] %s\n' "$*"; else "$@"; fi; }
write_file() {
  if [ $DRY = 1 ]; then printf '    [dry-run] write %s:\n' "$1"; sed 's/^/      | /'; return; fi
  cat > "$1"
}
tty_ok() { [ -r /dev/tty ] && { : < /dev/tty; } 2>/dev/null; }
ask() {
  local a=""
  if tty_ok; then printf '    %s ' "$1" > /dev/tty; IFS= read -r a < /dev/tty || true; fi
  printf '%s' "${a:-$2}"
}
yes_no() { [ $YES = 1 ] && return 0; case "$(ask "$1 [Y/n]" y)" in [nN]*) return 1 ;; *) return 0 ;; esac; }
ask_secret() {
  local a=""
  if tty_ok; then printf '    %s ' "$1" > /dev/tty; IFS= read -rs a < /dev/tty || true; printf '\n' > /dev/tty; fi
  printf '%s' "$a"
}
confval() { [ -f "$CONF" ] && grep -E "^$1=" "$CONF" | tail -1 | cut -d= -f2- || true; }
envval() { [ -f "$2" ] && grep -E "^[[:space:]]*$1=" "$2" | tail -1 | cut -d= -f2- | sed -e 's/^["'\'']//' -e 's/["'\'']$//' | tr -d '\r' || true; }
# shellcheck disable=SC2088  # the topdir arrives as text with a literal ~
expand() { case "$1" in "~") printf '%s' "$HOME" ;; "~/"*) printf '%s/%s' "$HOME" "${1#\~/}" ;; *) printf '%s' "$1" ;; esac; }
sha() { if command -v sha256sum >/dev/null; then sha256sum "$1" | cut -d' ' -f1; else shasum -a 256 "$1" | cut -d' ' -f1; fi; }

[ "$(id -u)" != 0 ] || die "run this as your own user, not root (the container runs as you)"
[ $DRY = 0 ] || printf '\033[1mdry run:\033[0m every action is printed, nothing is changed\n'

# the image's platform is the Docker host's (Docker Desktop on Apple silicon = arm64)
case "$(uname -m)" in
  x86_64|amd64) ARCH=x86_64 ;;
  aarch64|arm64) ARCH=aarch64 ;;
  *) die "no iter_engine build for $(uname -m)" ;;
esac
ASSET="iter_engine-linux-$ARCH"

DATA_URL="" ENGINE="" CHECKOUTS=""
load_conf() {
  [ -n "$DATA_URL" ] || DATA_URL="$(confval DATA_URL)"
  [ -n "$ENGINE" ] || ENGINE="$(confval ENGINE)"
  [ -n "$CHECKOUTS" ] || CHECKOUTS="${OPT_CHECKOUTS:-$(envval ITER_CHECKOUTS "$CTX/.env")}"
  [ $SC_GIVEN = 1 ] || [ -n "$SECRETS_COMMAND" ] || SECRETS_COMMAND="$(confval SECRETS_COMMAND)"
  [ -n "$SECRETS_EVERY" ] || SECRETS_EVERY="$(confval SECRETS_EVERY)"
}

# ------------------------------------------------------------------ secrets command
# --secrets-command / --secrets-every: given now, else kept in engine.conf. The
# container gets it as ITER_SECRETS_COMMAND="sh ~/.iter/docker/secrets_command"
# (a file, so the command needs no quoting through compose) and ITER_SECRETS_EVERY.
SECRETS_COMMAND="" SECRETS_EVERY=""
SC_FILE="$CTX/secrets_command"
secrets_settings() {
  if [ $SC_GIVEN = 1 ]; then SECRETS_COMMAND="$OPT_SC"; else SECRETS_COMMAND="$(confval SECRETS_COMMAND)"; fi
  [ "$SECRETS_COMMAND" != none ] || SECRETS_COMMAND=""
  case "$SECRETS_COMMAND" in *"
"*) die "--secrets-command: one line only" ;; esac
  SECRETS_EVERY="${OPT_SE:-$(confval SECRETS_EVERY)}"
  case "$SECRETS_EVERY" in ""|*[!0-9]*|0) [ -z "$OPT_SE" ] || die "--secrets-every: whole minutes, 1 or more"; SECRETS_EVERY="" ;; esac
  [ -n "$SECRETS_COMMAND" ] || SECRETS_EVERY=""
  if [ -n "$SECRETS_COMMAND" ]; then ok "account tokens: from the secrets command (run inside the container), every ${SECRETS_EVERY:-5} min (values stay in the engine's memory)"; fi
}
conf_text() {
  printf 'DATA_URL=%s\nENGINE=%s\n' "$DATA_URL" "$ENGINE"
  [ -z "$SECRETS_COMMAND" ] || printf 'SECRETS_COMMAND=%s\n' "$SECRETS_COMMAND"
  [ -z "$SECRETS_EVERY" ] || printf 'SECRETS_EVERY=%s\n' "$SECRETS_EVERY"
}
# accounts step with a secrets command: nothing to paste; say which names it covers
accounts_from_command() { # VARS...
  ok "the secrets command supplies the account tokens (not $ENVF): nothing to paste here"
  if [ $DRY = 1 ] || ! image_ready; then printf '    [dry-run] run the secrets command once in the image and check it prints %s (names only)\n' "$*"; return; fi
  local names v
  names="$(in_image sh "$SC_FILE" 2>/dev/null </dev/null | sed -n 's/^[[:space:]]*\(export[[:space:]][[:space:]]*\)\{0,1\}\([A-Za-z_][A-Za-z0-9_]*\)=.*/\2/p' || true)"
  for v in "$@"; do
    if printf '%s\n' "$names" | grep -qx "$v"; then ok "$v (from the secrets command)"
    elif [ -n "$(envval "$v" "$ENVF")" ]; then ok "$v (from $ENVF)"
    else warn "$v: the secrets command does not print it in the container (is its CLI in the image? the engine logs missing token names)"; fi
  done
}

WORK="$(mktemp -d)"; trap 'rm -rf "$WORK"' EXIT
TMPF="$WORK/api"
# compose in the build folder: compose.yml + compose.override.yml + .env
dc() { (cd "$CTX" && docker compose --progress quiet "$@"); }
# a one-off command in the engine image, as you, with the same mounts
in_image() { dc run --rm --no-deps -T engine "$@"; }
image_ready() { [ -f "$CTX/compose.yml" ] && docker image inspect iter-engine:local >/dev/null 2>&1; }
# ------------------------------------------------------------------ iter_data over HTTPS
# iter_data serves HTTPS with its own (self-signed) certificate. The engine pins
# that certificate's fingerprint in ~/.iter/server_trust.json (one entry per
# scheme://host:port), and plain http:// is only for this machine. The script's
# own API calls check the certificate the same way.
TRUSTF="$IHOME/server_trust.json"
# the trust file's key for a URL: scheme://host:port, lowercased
trust_origin() {
  local u="${1%/}" s r hp port=80
  s="$(printf '%s' "${u%%://*}" | tr '[:upper:]' '[:lower:]')"; r="${u#*://}"; hp="${r%%[/?#]*}"
  hp="$(printf '%s' "$hp" | tr '[:upper:]' '[:lower:]')"
  [ "$s" != https ] || port=443
  case "$hp" in *\]:*) ;; *\]) hp="$hp:$port" ;; *:*) ;; *) hp="$hp:$port" ;; esac
  printf '%s://%s' "$s" "$hp"
}
url_host() {
  local r="${1#*://}"; r="${r%%[/?#]*}"
  case "$r" in \[*) r="${r#\[}"; r="${r%%\]*}" ;; *) r="${r%:*}" ;; esac
  printf '%s' "$r" | tr '[:upper:]' '[:lower:]'
}
is_ip() { case "$1" in *:*) return 0 ;; ""|*[!0-9.]*) return 1 ;; *) return 0 ;; esac; }
is_loopback() { case "$1" in localhost|::1) return 0 ;; 127.*) is_ip "$1" ;; *) return 1 ;; esac; }
# stop on a URL the engine would refuse: plain http to another machine
check_url() {
  case "$1" in
    https://*) ;;
    http://*) is_loopback "$(url_host "$1")" \
        || die "$1 is plain http: iter_data must be reached over https:// (iter_data serves HTTPS by default), so the engine's session token and the work never cross the network unencrypted; plain http:// is only for iter_data on this machine (127.0.0.1, localhost, ::1). Try https://${1#http://}" ;;
    *) die "$1: the iter_data URL starts with https:// (plain http:// only to 127.0.0.1 / localhost / ::1)" ;;
  esac
}
# does ~/.iter/server_trust.json have an entry for this URL yet?
has_trust() { [ -f "$TRUSTF" ] && grep -qF "\"$(trust_origin "$1")\"" "$TRUSTF"; }
# PINOPT = curl options (used inside the image, where curl, jq and openssl are) that check iter_data's certificate as the engine does:
# none for http:// to this machine or a --ca trust; for a pinned certificate, -k
# plus the public key of the certificate iter_data presents, once its fingerprint
# is the pinned one (so curl's one connection must reach that same key). False
# when nothing is trusted yet, openssl or jq is missing, or the certificate differs.
PINOPT=()
pin_opts() {
  PINOPT=()
  case "$DATA_URL" in https://*) ;; *) return 0 ;; esac
  [ -f "$TRUSTF" ] && command -v jq >/dev/null 2>&1 && command -v openssl >/dev/null 2>&1 || return 1
  local o e pins hp h sni="" pem fp spki
  o="$(trust_origin "$DATA_URL")"
  e="$(jq -c --arg o "$o" '.servers[$o] // empty' "$TRUSTF" 2>/dev/null)"
  [ -n "$e" ] || return 1
  [ "$(printf '%s' "$e" | jq -r '.mode // "pin"')" != ca ] || return 0
  pins="$(printf '%s' "$e" | jq -r '.pins[]?' | sed 's/^sha256://; s/://g' | tr 'A-F' 'a-f')"
  hp="${o#https://}"; h="$(url_host "$DATA_URL")"
  is_ip "$h" || sni="$h"
  pem="$(openssl s_client -connect "$hp" ${sni:+-servername "$sni"} </dev/null 2>/dev/null | openssl x509 2>/dev/null)" || true
  [ -n "$pem" ] || return 1
  fp="$(printf '%s\n' "$pem" | openssl x509 -noout -fingerprint -sha256 | sed 's/.*=//; s/://g' | tr 'A-F' 'a-f')"
  printf '%s\n' "$pins" | grep -qx "$fp" || return 1
  spki="$(printf '%s\n' "$pem" | openssl x509 -noout -pubkey | openssl pkey -pubin -outform DER 2>/dev/null | openssl dgst -sha256 -binary | openssl base64 -A)"
  [ -n "$spki" ] || return 1
  # shellcheck disable=SC2034  # read by api_script, inside the image
  PINOPT=(-k --pinnedpubkey "sha256//$spki")
}

# the script api() runs inside the image (curl, jq, openssl and the engine's key
# are there, and host.docker.internal resolves): body, then the status last
api_script() {
  declare -f trust_origin url_host is_ip pin_opts
  cat <<'EOF'
set -u
TRUSTF="$1" DATA_URL="$2"
tok="$(iter_engine key session --key-file "$3" --data-url "$2" 2>/dev/null || true)"
if [ -z "$tok" ] || ! pin_opts; then printf '\n000'; exit 0; fi
curl -sS --max-time 15 ${PINOPT[@]+"${PINOPT[@]}"} -H "authorization: Bearer $tok" -w '\n%{http_code}' "$2$4" 2>/dev/null || printf '\n000'
EOF
}
# GET an iter_data path as this engine (a session from its key, iter_data's
# certificate checked as the engine checks it): body in $TMPF, prints the status
# (000 when it cannot be reached or its certificate is not the trusted one). Read-only.
api() {
  if ! image_ready || [ ! -f "$KEYF" ]; then echo 000; return 0; fi
  api_script > "$WORK/api.sh"
  in_image bash -s -- "$TRUSTF" "$DATA_URL" "$KEYF" "$1" < "$WORK/api.sh" > "$WORK/api.out" 2>/dev/null || true
  sed '$d' "$WORK/api.out" > "$TMPF"
  tail -n 1 "$WORK/api.out" | grep -E '^[0-9]{3}$' || echo 000
}

# ------------------------------------------------------------------ steps

docker_ok() {
  say "1/8 docker"
  command -v docker >/dev/null || die "docker is not installed (https://docs.docker.com/engine/install/ ; macOS: Docker Desktop)"
  docker info >/dev/null 2>&1 || die "the docker daemon does not answer (start Docker Desktop / sudo systemctl start docker; on Linux your user needs the docker group)"
  docker compose version >/dev/null 2>&1 || die "docker compose (v2) is missing"
  ok "docker $(docker version --format '{{.Server.Version}}' 2>/dev/null), $(docker compose version --short 2>/dev/null | sed 's/^/compose /')"
  for t in curl git; do command -v "$t" >/dev/null || die "$t is missing on the host"; done
  if pgrep -x iter_engine >/dev/null 2>&1; then
    warn "an iter_engine runs natively on this host (pid $(pgrep -x iter_engine | head -1)): one engine per machine, stop and remove it first"
  fi
}

settings() {
  say "2/8 iter_data, this engine's name, the checkouts folder"
  load_conf
  local url name host
  url="${OPT_URL:-$(ask "iter_data URL [${DATA_URL:-https://iter.example.com}]:" "${DATA_URL:-}")}"
  url="${url%/}"
  if [ -z "$url" ]; then
    [ $DRY = 1 ] || die "the iter_data URL is required (--data-url URL)"
    url="https://iter.example.com"; warn "no iter_data URL given: the dry run goes on with $url"
  fi
  case "$url" in
    http://127.0.0.1*|http://localhost*|https://127.0.0.1*|https://localhost*)
      local fixed; fixed="$(printf '%s' "$url" | sed -E 's#^(https?)://(127\.0\.0\.1|localhost)#\1://host.docker.internal#')"
      warn "$url is the container itself in there: using $fixed (this host, through host-gateway)"; url="$fixed" ;;
  esac
  case "$url" in
    https://*) ;;
    http://*) die "$url is plain http: the engine in the container reaches iter_data across a network (even on this host, through host.docker.internal), so iter_data must be https:// (it serves HTTPS by default; behind a TLS proxy, the proxy's https:// URL). Try https://${url#http://}" ;;
    *) die "$url: the iter_data URL starts with https://" ;;
  esac
  host="$(hostname -s 2>/dev/null || hostname)"
  name="${OPT_NAME:-$(ask "engine name [${ENGINE:-$host}]:" "${ENGINE:-$host}")}"
  case "$name" in *[!A-Za-z0-9_.-]*|"") die "engine name \"$name\": letters, digits, - _ . only" ;; esac
  local co; co="$(ask "the host folder that holds the project checkouts [${CHECKOUTS:-$HOME/dev}]:" "${CHECKOUTS:-$HOME/dev}")"
  co="$(expand "$co")"; co="${co%/}"
  case "$co" in /*) ;; *) die "the checkouts folder must be an absolute path" ;; esac
  case "$co" in "$HOME"|"$IHOME"|"$IHOME"/*) die "the checkouts folder must be a folder of its own (e.g. ~/dev), not $co" ;; esac
  run mkdir -p "$IHOME" "$co" "$IHOME/claude" "$IHOME/models"
  DATA_URL="$url" ENGINE="$name" CHECKOUTS="$co"
  secrets_settings
  conf_text | write_file "$CONF"
  ok "engine \"$ENGINE\" -> $DATA_URL (in $CONF); checkouts: $CHECKOUTS"
}

# the build folder's Dockerfile, entrypoint.sh, compose.yml: from beside this
# script, else from GitHub at --version (main without one)
context_files() {
  run mkdir -p "$CTX"
  local f ref="${VERSION:-main}"
  for f in Dockerfile entrypoint.sh compose.yml; do
    if [ -n "$HERE" ] && [ -f "$HERE/$f" ]; then
      [ "$HERE" = "$CTX" ] || run cp "$HERE/$f" "$CTX/$f"
    elif [ $DRY = 1 ]; then
      printf '    [dry-run] download %s from github.com/%s at %s into %s\n' "$f" "$REPO_SLUG" "$ref" "$CTX"
    else
      curl -fsSL --retry 3 -o "$CTX/$f" "https://raw.githubusercontent.com/$REPO_SLUG/$ref/installers/iter_engine/docker/$f" \
        || die "could not download $f (at $ref)"
    fi
  done
  ok "$CTX: Dockerfile, entrypoint.sh, compose.yml"
}

# the binary for the image: --binary, beside this script, else the release (sha256-checked)
binary() {
  local dest="$CTX/$ASSET"
  if [ -n "$BINARY" ]; then
    [ -f "$BINARY" ] || die "--binary $BINARY: no such file"
    run cp "$BINARY" "$dest"; ok "using $BINARY"; return
  fi
  if [ -n "$HERE" ] && [ -f "$HERE/$ASSET" ]; then
    if [ -f "$HERE/SHA256SUMS" ]; then
      local want; want="$(awk -v a="$ASSET" '$2 == a || $2 == "*"a { print $1 }' "$HERE/SHA256SUMS")"
      [ -z "$want" ] || [ "$(sha "$HERE/$ASSET")" = "$want" ] || die "$HERE/$ASSET does not match its SHA256SUMS entry"
    fi
    [ "$HERE" = "$CTX" ] || run cp "$HERE/$ASSET" "$dest"
    ok "using $HERE/$ASSET (next to this script)"; return
  fi
  local base
  if [ -n "${ITER_RELEASE_URL:-}" ]; then base="${ITER_RELEASE_URL%/}"
  elif [ -n "$VERSION" ]; then base="https://github.com/$REPO_SLUG/releases/download/$VERSION"
  else base="https://github.com/$REPO_SLUG/releases/latest/download"; fi
  if [ $DRY = 1 ]; then printf '    [dry-run] download %s/%s and SHA256SUMS, check its sha256, into %s\n' "$base" "$ASSET" "$CTX"; return; fi
  curl -fsSL --retry 3 -o "$WORK/$ASSET" "$base/$ASSET" || die "download failed: $base/$ASSET"
  curl -fsSL --retry 3 -o "$WORK/SHA256SUMS" "$base/SHA256SUMS" || die "download failed: $base/SHA256SUMS"
  local want got
  want="$(awk -v a="$ASSET" '$2 == a || $2 == "*"a { print $1 }' "$WORK/SHA256SUMS")"
  got="$(sha "$WORK/$ASSET")"
  [ -n "$want" ] || die "SHA256SUMS of that release has no $ASSET"
  [ "$got" = "$want" ] || die "$ASSET sha256 $got does not match the release's $want"
  mv "$WORK/$ASSET" "$dest"; ok "downloaded $ASSET ${VERSION:-(latest)}, sha256 ok"
}

compose_settings() {
  # compose's own settings (interpolated into compose.yml)
  cat <<EOF | write_file "$CTX/.env"
# written by installers/iter_engine/docker/install.sh
ITER_UID=$(id -u)
ITER_GID=$(id -g)
ITER_HOST_HOME=$HOME
ITER_CHECKOUTS=$CHECKOUTS
ITER_DATA_URL=$DATA_URL
ITER_ENGINE=$ENGINE
ITER_SECRETS_COMMAND=$([ -z "$SECRETS_COMMAND" ] || echo "sh $SC_FILE")
ITER_SECRETS_EVERY=$SECRETS_EVERY
EOF
  if [ -n "$SECRETS_COMMAND" ]; then
    printf '%s\n' "$SECRETS_COMMAND" | write_file "$SC_FILE"
  elif [ -f "$SC_FILE" ]; then
    run mv -f "$SC_FILE" "$SC_FILE.removed"
  fi
  # git credentials that exist on this host, read-only at the same paths
  local p mounts=""
  for p in .ssh .gitconfig .git-credentials .config/gh .config/git; do
    [ -e "$HOME/$p" ] && mounts="$mounts      - $HOME/$p:$HOME/$p:ro
"
  done
  if [ -n "$mounts" ]; then
    printf '# written by install.sh: git credentials from the host, read-only\nservices:\n  engine:\n    volumes:\n%s' "$mounts" | write_file "$CTX/compose.override.yml"
  else
    warn "no ~/.ssh, ~/.gitconfig or gh login on this host: the engine cannot push to the projects' remotes until one exists (then run install.sh again)"
    [ ! -f "$CTX/compose.override.yml" ] || run rm -f "$CTX/compose.override.yml"
  fi
}

build_files() {
  say "3/8 the image's files ($CTX)"
  context_files; binary; compose_settings
}

# the embedding model on the host, in ~/.iter/models (the container reads it
# through the ~/.iter mount; without it the engine downloads it itself)
model() {
  say "4/8 embedding model"
  local base="https://huggingface.co/sentence-transformers/all-MiniLM-L6-v2/resolve/main"
  local msum="53aa51172d142c89d9012cce15ae4d6cc0ca6895895114379cacb4fab128d9db"
  if [ -f "$MODEL/model.safetensors" ] && [ -f "$MODEL/tokenizer.json" ] && [ -f "$MODEL/config.json" ] \
     && [ "$(sha "$MODEL/model.safetensors")" = "$msum" ]; then ok "$MODEL"; return; fi
  if [ $DRY = 1 ]; then printf '    [dry-run] download all-MiniLM-L6-v2 from %s into %s, check its sha256\n' "$base" "$MODEL"; return; fi
  mkdir -p "$MODEL/1_Pooling"
  for f in config.json tokenizer.json model.safetensors special_tokens_map.json tokenizer_config.json \
           modules.json sentence_bert_config.json 1_Pooling/config.json README.md; do
    curl -sfL --retry 3 -o "$MODEL/$f.part" "$base/$f" || { warn "could not fetch $f: the engine downloads the model itself later"; return; }
    mv "$MODEL/$f.part" "$MODEL/$f"
  done
  [ "$(sha "$MODEL/model.safetensors")" = "$msum" ] || warn "model.safetensors differs from the verified build (upstream changed?)"
  ok "$MODEL"
}

image() {
  say "5/8 the image iter-engine:local"
  run dc build --pull "$@"
  [ $DRY = 1 ] || ok "built ($(docker image inspect iter-engine:local --format '{{.Size}}' | awk '{ printf "%d MB", $1 / 1e6 }'))"
}

# iter_data's certificate, before the first sign-in (in the image, which writes
# ~/.iter/server_trust.json through the ~/.iter mount): --fingerprint (or the one
# pasted here) pins it now and stops when the server presents another one;
# --trust-ca checks it against certificate authorities (a TLS proxy with a real
# certificate); else the first sign-in trusts what it sees (trust on first use).
trust() {
  say "5b/8 iter_data's certificate"
  case "$DATA_URL" in https://*) ;; *) ok "$DATA_URL is plain http to this machine: no certificate to trust"; return ;; esac
  if [ $TRUST_CA = 1 ]; then
    run in_image iter_engine key trust --data-url "$DATA_URL" --ca || die "could not reach $DATA_URL with a certificate-authority check"
    return
  fi
  local fp="$OPT_FP"
  if [ -z "$fp" ] && ! has_trust "$DATA_URL"; then
    if [ $DRY = 1 ]; then printf "    [dry-run] ask for iter_data's certificate fingerprint (Enter = trust it at the first sign-in)\n"
    else fp="$(ask "Paste iter_data's certificate fingerprint (shown in the webui's 🔑 Engine keys screen, or by the iter_data installer), or press Enter to trust it at the first sign-in:" "")"; fi
  fi
  if [ -z "$fp" ]; then
    if has_trust "$DATA_URL"; then ok "trusted already ($TRUSTF)"
    else warn "no fingerprint given: the first sign-in trusts the certificate $DATA_URL presents; compare it afterwards (iter_engine key status) with the webui's Engine keys screen"; fi
    return
  fi
  run in_image iter_engine key trust --data-url "$DATA_URL" --fingerprint "$fp" \
    || die "$DATA_URL does not present the certificate $fp: something else answers at that URL (or the fingerprint is mistyped). Nothing was trusted; check the URL and the fingerprint (iter_data installer output, webui Engine keys screen)"
}

key() {
  say "6/8 engine key"
  [ -f "$ENVF" ] || run touch "$ENVF"
  run chmod 600 "$ENVF"
  if [ $DRY = 1 ]; then
    if [ -f "$KEYF" ]; then ok "$KEYF exists"; else printf '    [dry-run] (in the image) iter_engine key new --key-file %s --name %s\n' "$KEYF" "$ENGINE"; fi
    printf '    [dry-run] wait until iter_data accepts it (iter_engine key status)\n'; return
  fi
  [ -f "$KEYF" ] || in_image iter_engine key new --key-file "$KEYF" --name "$ENGINE" | sed 's/^/    /'
  local first=1
  while :; do
    if in_image iter_engine key status --key-file "$KEYF" --data-url "$DATA_URL" > "$TMPF" 2>&1; then
      sed 's/^/    /' "$TMPF"; ok "iter_data accepts this engine's key"; return
    fi
    if [ $first = 1 ]; then
      sed 's/^/    /' "$TMPF"
      printf '\n    In the webui (%s): Engine keys (the key button in the header) -> Add: paste the iter_... public key above, engine name "%s".\n' "$DATA_URL" "$ENGINE"
      first=0
    fi
    case "$(ask "press Enter once it is added (n = a new key, s = skip):" "")" in
      n|N) mv "$KEYF" "$KEYF.replaced-$(date +%Y%m%d%H%M%S)"; in_image iter_engine key new --key-file "$KEYF" --name "$ENGINE" | sed 's/^/    /'; first=1 ;;
      s|S) warn "skipped: iter_data refuses this engine until its public key is added"; return ;;
      *) tty_ok || die "iter_data does not accept the key yet (no terminal to wait at): add it, then run this again"
         sleep 2; printf '    not accepted yet\n' ;;
    esac
  done
}

up() {
  say "7/8 the container iter-engine"
  run dc up -d --force-recreate
  [ $DRY = 1 ] && return
  sleep 5
  [ "$(docker inspect -f '{{.State.Running}}' iter-engine 2>/dev/null)" = true ] || { dc logs --tail 20 engine; die "the container did not stay up"; }
  ok "running (restart: unless-stopped; log: install.sh logs)"
}

assignments() {
  [ "$(api "/api/engines/$ENGINE/assignments")" = 200 ] && jq -e '.projects | length > 0' "$TMPF" >/dev/null 2>&1
}

accounts() {
  say "account tokens"
  load_conf
  command -v jq >/dev/null || { warn "jq is not on the host: skipped (tokens go in $ENVF as NAME=value)"; return; }
  if ! assignments; then ok "no projects served yet: the account tokens are asked for once projects are (install.sh accounts)"; return; fi
  local vars missing=0
  vars="$(jq -r '[.accounts[]?.token_envar, .projects[].accounts[]?.token_envar] | map(select(. != null and . != "")) | unique | .[]' "$TMPF")"
  [ -n "$vars" ] || { warn "the served projects name no account tokens: claude in the container has no login (give the projects an account with a token_envar)"; return; }
  # shellcheck disable=SC2086  # one name per word
  if [ -n "$SECRETS_COMMAND" ]; then accounts_from_command $vars; return; fi
  for v in $vars; do
    if [ -n "$(envval "$v" "$ENVF")" ]; then ok "$v"; continue; fi
    if [ $DRY = 1 ]; then printf '    [dry-run] ask for %s and add it to %s\n' "$v" "$ENVF"; continue; fi
    local val; val="$(ask_secret "$v: paste a token from \`claude setup-token\` (run while logged in to that account; blank = later):")"
    if [ -n "$val" ]; then
      { grep -v "^$v=" "$ENVF" || true; printf '%s=%s\n' "$v" "$val"; } > "$WORK/env"; cat "$WORK/env" > "$ENVF"; rm -f "$WORK/env"
      ok "$v (the engine picks it up without a restart)"
    else warn "$v not set: that account is never used here"; missing=1; fi
  done
  [ $missing = 0 ] || warn "add them later: install.sh accounts"
}

projects() {
  say "project checkouts (on the host, in $CHECKOUTS)"
  load_conf
  command -v jq >/dev/null || { warn "jq is not on the host: skipped"; return; }
  if ! assignments; then
    ok "no projects served yet: in the webui's Settings tab draw a serves edge from \"$ENGINE\" to each project, topdir inside $CHECKOUTS (e.g. ~/dev/<project>); then: install.sh projects"
    return
  fi
  local list; list="$(jq -r '.projects[] | "\(.project)\t\(.topdir)"' "$TMPF")"
  while IFS=$'\t' read -r p top; do
    [ -n "$p" ] || continue
    if [ -z "$top" ]; then warn "$p: its serves edge has no topdir"; continue; fi
    local dir; dir="$(expand "$top")"
    case "$dir" in "$CHECKOUTS"/*) ;; *) warn "$p: $dir is outside $CHECKOUTS, so the container cannot see it: move the topdir inside"; continue ;; esac
    if [ -d "$dir/.git" ]; then ok "$p: $dir"; continue; fi
    if [ -e "$dir" ] && [ -n "$(ls -A "$dir" 2>/dev/null)" ]; then warn "$p: $dir exists but is not a git checkout; left alone"; continue; fi
    local url=""
    [ "$(api "/api/projects/$p")" = 200 ] && url="$(jq -r '.gitrepo // ""' "$TMPF")"
    url="$(ask "$p: git URL to clone into $dir [${url:-none}]:" "$url")"
    [ -n "$url" ] || { warn "$p: no checkout (the engine skips it until $dir exists)"; continue; }
    if run git clone -q "$url" "$dir"; then ok "$p: cloned into $dir"; else warn "$p: clone failed (credentials?)"; fi
  done <<< "$list"
}

status() {
  load_conf
  [ -n "${ENGINE:-}" ] && [ -f "$CTX/compose.yml" ] || die "not installed yet: run install.sh"
  printf 'engine     %s -> %s\ncheckouts  %s\ncontainer  %s\n' "$ENGINE" "$DATA_URL" "$CHECKOUTS" \
    "$(docker inspect -f '{{.State.Status}} since {{.State.StartedAt}}' iter-engine 2>/dev/null || echo missing)"
  if image_ready; then in_image iter_engine key status --key-file "$KEYF" --data-url "$DATA_URL" || true; fi
  if command -v jq >/dev/null && assignments; then jq -r '.projects[] | "project    \(.project)  \(.topdir)  \(.state)"' "$TMPF"; else echo "project    (none served, or jq missing)"; fi
  docker logs --tail 3 iter-engine 2>&1 || true
}

update() {
  load_conf
  [ -n "${ENGINE:-}" ] && [ -f "$CTX/compose.yml" ] || die "not installed yet: run install.sh"
  say "update"
  if [ $YES = 0 ] && command -v jq >/dev/null && [ "$(api "/api/engines/$ENGINE")" = 200 ]; then
    local n; n="$(jq -r '.running // 0' "$TMPF" 2>/dev/null || echo 0)"
    [ "${n:-0}" -eq 0 ] 2>/dev/null || die "engine \"$ENGINE\" runs $n item(s) and recreating the container stops them: set its projects Draining and wait, or rerun with --yes"
  fi
  build_files
  # --no-cache: the claude CLI layer is fetched again too
  image --no-cache
  up
}

uninstall() {
  say "uninstall the iter engine container"
  if [ -f "$CTX/compose.yml" ]; then
    run dc down --rmi local
    ok "container and image removed"
  else ok "no $CTX/compose.yml: nothing to stop"; fi
  ok "kept $IHOME (key, tokens, model, claude/, docker/) and the checkouts; delete them by hand when this machine is done with iter"
  ok "in the webui, remove the engine's key (Engine keys) and its serves edges"
}

case "$VERB" in
  install)
    docker_ok; settings; build_files; model; image; trust; key; up
    say "8/8 accounts and projects"
    accounts; projects
    [ $DRY = 0 ] || { printf '\nDry run done: nothing was changed.\n'; exit 0; }
    cat <<EOF

Done. Engine "$ENGINE" runs in the container iter-engine and checks in with $DATA_URL.
Next, in the webui's Settings tab: draw a serves edge from $ENGINE to each project
(topdir inside $CHECKOUTS, e.g. ~/dev/<project>), then run \`install.sh projects\`.
  status:  install.sh status        log:  install.sh logs
  update:  install.sh update        (rebuilds the image: a newer iter_engine and claude CLI)
  a new key (in $CTX):  docker compose run --rm --no-deps engine iter_engine key rotate
                        (its public key: ... iter_engine key show)
EOF
    ;;
  projects) projects ;;
  accounts) accounts ;;
  status) status ;;
  update) update ;;
  logs) run docker logs -f --tail 100 iter-engine ;;
  uninstall) uninstall ;;
esac
