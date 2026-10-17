#!/usr/bin/env bash
# install.sh - make this Linux machine an iter engine (Debian, Ubuntu, Arch /
# Omarchy; Fedora mostly). ONE engine per machine; it serves every project the
# webui assigns to it. iter_data runs elsewhere; this machine only makes
# outbound connections: to iter_data, to Anthropic, to the projects' git remote
# and, once, to Hugging Face for the embedding model.
#
#   curl -fsSL https://raw.githubusercontent.com/Stephen-Hilton/iter/main/installers/iter_engine/linux/install.sh | bash
#   ./install.sh [verb] [options]
#
# Verbs:
#   install     (default) set up or repair; safe to run again
#   status      the service, the binary, the key, the served projects
#   update      install the newer binary and restart (refuses while work runs, without --yes)
#   accounts    ask for each account token the served projects name
#   projects    clone each served project that has no checkout yet
#   logs        follow the engine log
#   uninstall   stop and remove the service and the binary (keeps ~/.iter's key, tokens, model)
#
# Options:
#   --data-url URL    iter_data URL (else asked; kept in ~/.iter/engine.conf)
#   --name NAME       this engine's name, unique per machine (default: the hostname)
#   --version TAG     release to download, e.g. v2026.10.17 (else $ITER_VERSION, else latest)
#   --binary PATH     install this iter_engine file instead of downloading one
#   --fingerprint FP  iter_data's certificate fingerprint (sha256:..., from the iter_data
#                     installer or the webui's Engine keys screen): pinned before sign-in
#   --trust-ca        iter_data is behind a TLS proxy with a real certificate: check it
#                     against certificate authorities instead of pinning
#   --secrets-command CMD  a shell command printing KEY=value lines (the account tokens,
#                     from a secret store: aws ssm / secretsmanager, op, security, vault);
#                     the engine runs it at startup and every --secrets-every minutes and
#                     keeps the values in memory only. Kept in ~/.iter/engine.conf and put
#                     on the service's command line; "none" removes it. With it, the
#                     accounts step asks for no tokens.
#   --secrets-every N  minutes between secrets-command runs [5]
#   --yes             answer yes to every question; update while work runs
#   --dry-run         print every action, change nothing
#   -h, --help        this text
#   ($ITER_RELEASE_URL: download the binary and SHA256SUMS from this base URL instead)
#
# Steps of `install`: packages (git, curl, jq, poppler; asks first) -> the
# claude CLI (Anthropic's installer) -> iter_data URL + engine name
# (~/.iter/engine.conf) -> iter_engine into ~/.iter/bin (the asset next to this
# script, else the GitHub Release, sha256-checked) -> the embedding model into
# ~/.iter/models -> the engine key (`iter_engine key new`: paste its public key
# into the webui's Engine keys screen; the script waits until iter_data accepts
# it) -> a laptop: no suspend (asks) -> Omarchy: no lock screen (asks) -> the
# iter-engine systemd service -> account tokens and project checkouts.
#
# Everything this machine keeps for iter is in ~/.iter: bin/iter_engine, the key
# (engine.key), the env file with account tokens (.env), engine.conf, models/
# and the logs (engine.log, engine.err.log).
#
# HTTPS: iter_data must be https:// (iter_data serves HTTPS by default; plain
# http:// is refused unless iter_data runs on this machine: 127.0.0.1,
# localhost, ::1). The engine pins iter_data's certificate fingerprint in
# ~/.iter/server_trust.json: --fingerprint (or the fingerprint pasted when
# asked) runs `iter_engine key trust` right after the binary is installed and
# stops if the server presents another certificate; without one, the first
# sign-in trusts the certificate it sees (compare: iter_engine key status).
# Other key commands: `iter_engine key show` (print the public key),
# `iter_engine key rotate` (a new key, signed by the current one, no webui step).
set -euo pipefail

REPO_SLUG="Stephen-Hilton/iter"
HERE=""
if [ -n "${BASH_SOURCE[0]:-}" ] && [ -f "${BASH_SOURCE[0]}" ]; then HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"; fi
IHOME="$HOME/.iter"
# iter_engine keeps its files (key, server_trust.json) in $ITER_HOME, else ~/.iter:
# the service reads ~/.iter, so this script's own iter_engine calls do too
export ITER_HOME="$IHOME"
CONF="$IHOME/engine.conf"
ENVF="$IHOME/.env"
KEYF="$IHOME/engine.key"
BIN="$IHOME/bin/iter_engine"
MODEL="$IHOME/models/all-MiniLM-L6-v2"
UNIT=/etc/systemd/system/iter-engine.service
export PATH="$HOME/.local/bin:$PATH"

VERB=install DRY=0 YES=0 BINARY="" OPT_URL="" OPT_NAME="" OPT_FP="" TRUST_CA=0 OPT_SC="" SC_GIVEN=0 OPT_SE="" VERSION="${ITER_VERSION:-}"
while [ $# -gt 0 ]; do
  case "$1" in
    install|status|update|accounts|projects|logs|uninstall) VERB="$1" ;;
    --data-url) OPT_URL="${2:?--data-url needs a URL}"; shift ;;
    --name) OPT_NAME="${2:?--name needs a name}"; shift ;;
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
# run a command, or under --dry-run only print it
run() { if [ $DRY = 1 ]; then printf '    [dry-run] %s\n' "$*"; else "$@"; fi; }
# write stdin to a file (sudo when $2 = sudo), or under --dry-run print it
write_file() {
  if [ $DRY = 1 ]; then printf '    [dry-run] write %s:\n' "$1"; sed 's/^/      | /'; return; fi
  if [ "${2:-}" = sudo ]; then sudo tee "$1" >/dev/null; else cat > "$1"; fi
}
ask() { # ask "question" default -> the answer (the default without a terminal)
  local a=""
  if [ -r /dev/tty ] && { : < /dev/tty; } 2>/dev/null; then printf '    %s ' "$1" > /dev/tty; IFS= read -r a < /dev/tty || true; fi
  printf '%s' "${a:-$2}"
}
yes_no() { [ $YES = 1 ] && return 0; case "$(ask "$1 [Y/n]" y)" in [nN]*) return 1 ;; *) return 0 ;; esac; }
ask_secret() {
  local a=""
  if [ -r /dev/tty ] && { : < /dev/tty; } 2>/dev/null; then printf '    %s ' "$1" > /dev/tty; IFS= read -rs a < /dev/tty || true; printf '\n' > /dev/tty; fi
  printf '%s' "$a"
}
confval() { [ -f "$CONF" ] && grep -E "^$1=" "$CONF" | tail -1 | cut -d= -f2- || true; }
envval() { [ -f "$2" ] && grep -E "^[[:space:]]*$1=" "$2" | tail -1 | cut -d= -f2- | sed -e 's/^["'\'']//' -e 's/["'\'']$//' | tr -d '\r' || true; }
# shellcheck disable=SC2088  # the topdir arrives as text with a literal ~
expand() { case "$1" in "~") printf '%s' "$HOME" ;; "~/"*) printf '%s/%s' "$HOME" "${1#\~/}" ;; *) printf '%s' "$1" ;; esac; }
sha() { if command -v sha256sum >/dev/null; then sha256sum "$1" | cut -d' ' -f1; else shasum -a 256 "$1" | cut -d' ' -f1; fi; }

[ "$(id -u)" != 0 ] || die "run this as your own user, not root (it uses sudo where it must)"
[ "$(uname -s)" = Linux ] || die "this script is for Linux (macOS: installers/iter_engine/macos, Windows: installers/iter_engine/windows)"
[ $DRY = 0 ] || printf '\033[1mdry run:\033[0m every action is printed, nothing is changed\n'

case "$(uname -m)" in
  x86_64|amd64) ARCH=x86_64 ;;
  aarch64|arm64) ARCH=aarch64 ;;
  *) die "no iter_engine build for $(uname -m)" ;;
esac
ASSET="iter_engine-linux-$ARCH"

DATA_URL="" ENGINE=""
load_conf() { # fills what this run has not set yet
  [ -n "$DATA_URL" ] || DATA_URL="$(confval DATA_URL)"
  [ -n "$ENGINE" ] || ENGINE="$(confval ENGINE)"
  [ $SC_GIVEN = 1 ] || [ -n "$SECRETS_COMMAND" ] || SECRETS_COMMAND="$(confval SECRETS_COMMAND)"
  [ -n "$SECRETS_EVERY" ] || SECRETS_EVERY="$(confval SECRETS_EVERY)"
}

# ------------------------------------------------------------------ secrets command
# --secrets-command / --secrets-every: given now, else kept in engine.conf
SECRETS_COMMAND="" SECRETS_EVERY=""
secrets_settings() {
  if [ $SC_GIVEN = 1 ]; then SECRETS_COMMAND="$OPT_SC"; else SECRETS_COMMAND="$(confval SECRETS_COMMAND)"; fi
  [ "$SECRETS_COMMAND" != none ] || SECRETS_COMMAND=""
  case "$SECRETS_COMMAND" in *"
"*) die "--secrets-command: one line only" ;; esac
  SECRETS_EVERY="${OPT_SE:-$(confval SECRETS_EVERY)}"
  case "$SECRETS_EVERY" in ""|*[!0-9]*|0) [ -z "$OPT_SE" ] || die "--secrets-every: whole minutes, 1 or more"; SECRETS_EVERY="" ;; esac
  [ -n "$SECRETS_COMMAND" ] || SECRETS_EVERY=""
  if [ -n "$SECRETS_COMMAND" ]; then ok "account tokens: from the secrets command, every ${SECRETS_EVERY:-5} min (values stay in the engine's memory)"; fi
}
conf_text() {
  printf 'DATA_URL=%s\nENGINE=%s\n' "$DATA_URL" "$ENGINE"
  [ -z "$SECRETS_COMMAND" ] || printf 'SECRETS_COMMAND=%s\n' "$SECRETS_COMMAND"
  [ -z "$SECRETS_EVERY" ] || printf 'SECRETS_EVERY=%s\n' "$SECRETS_EVERY"
}
# systemd's quoting for one ExecStart argument
sd_quote() { local s="$1"; s="${s//\\/\\\\}"; s="${s//\"/\\\"}"; s="${s//\$/\$\$}"; s="${s//%/%%}"; printf '"%s"' "$s"; }
# the service's --secrets-command / --secrets-every flags, systemd-quoted
secrets_args() {
  [ -n "$SECRETS_COMMAND" ] || return 0
  printf ' --secrets-command %s' "$(sd_quote "$SECRETS_COMMAND")"
  [ -z "$SECRETS_EVERY" ] || printf ' --secrets-every %s' "$SECRETS_EVERY"
}
# the names (never the values) the secrets command prints
secrets_names() {
  sh -c "$SECRETS_COMMAND" 2>/dev/null </dev/null | sed -n 's/^[[:space:]]*\(export[[:space:]][[:space:]]*\)\{0,1\}\([A-Za-z_][A-Za-z0-9_]*\)=.*/\2/p' || true
}
# accounts step with a secrets command: nothing to paste; say which names it covers
accounts_from_command() { # VARS...
  ok "the secrets command supplies the account tokens (not $ENVF): nothing to paste here"
  if [ $DRY = 1 ]; then printf '    [dry-run] run the secrets command once and check it prints %s (names only)\n' "$*"; return; fi
  local names v; names="$(secrets_names)"
  for v in "$@"; do
    if printf '%s\n' "$names" | grep -qx "$v"; then ok "$v (from the secrets command)"
    elif [ -n "$(envval "$v" "$ENVF")" ]; then ok "$v (from $ENVF)"
    else warn "$v: the secrets command does not print it here (the engine logs missing token names; the service runs it with its own PATH and HOME)"; fi
  done
}

# ~/.iter5 became ~/.iter (2026-10-11): move it once; ~/.iter5 stays as a link
migrate_home() {
  if [ -d "$HOME/.iter5" ] && [ ! -L "$HOME/.iter5" ]; then
    if [ -e "$IHOME" ]; then warn "both ~/.iter5 and ~/.iter exist: using ~/.iter; move what you need from ~/.iter5 by hand"
    else run mv "$HOME/.iter5" "$IHOME" && run ln -s "$IHOME" "$HOME/.iter5" && ok "moved ~/.iter5 to ~/.iter (~/.iter5 links to it)"; fi
  fi
}

WORK="$(mktemp -d)"; trap 'rm -rf "$WORK"' EXIT
TMPF="$WORK/api"
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
# PINOPT = curl options that check iter_data's certificate as the engine does:
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
  PINOPT=(-k --pinnedpubkey "sha256//$spki")
}

# GET an iter_data path as this engine (a session from its key): body in $TMPF,
# prints the status (000 when iter_data cannot be reached or its certificate is
# not the trusted one). Read-only, so it runs in a dry run.
api() {
  local tok=""
  [ -x "$BIN" ] && [ -f "$KEYF" ] && tok="$("$BIN" key session --key-file "$KEYF" --data-url "$DATA_URL" 2>/dev/null || true)"
  if [ -z "$tok" ] || ! pin_opts; then echo 000; return 0; fi
  curl -sS --max-time 15 ${PINOPT[@]+"${PINOPT[@]}"} -H "authorization: Bearer $tok" -o "$TMPF" -w '%{http_code}' "$DATA_URL$1" 2>/dev/null || echo 000
}

# ------------------------------------------------------------------ the binary

# leaves the file to install in $NEWBIN ("" in a dry run that would download)
get_binary() {
  NEWBIN=""
  if [ -n "$BINARY" ]; then
    [ -f "$BINARY" ] || die "--binary $BINARY: no such file"
    NEWBIN="$BINARY"; ok "using $BINARY"; return
  fi
  if [ -n "$HERE" ] && [ -f "$HERE/$ASSET" ]; then
    NEWBIN="$HERE/$ASSET"
    if [ -f "$HERE/SHA256SUMS" ]; then
      local want; want="$(awk -v a="$ASSET" '$2 == a || $2 == "*"a { print $1 }' "$HERE/SHA256SUMS")"
      [ -z "$want" ] || [ "$(sha "$NEWBIN")" = "$want" ] || die "$NEWBIN does not match its SHA256SUMS entry"
    fi
    ok "using $NEWBIN (next to this script)"; return
  fi
  local base
  if [ -n "${ITER_RELEASE_URL:-}" ]; then base="${ITER_RELEASE_URL%/}"   # a mirror or a test server
  elif [ -n "$VERSION" ]; then base="https://github.com/$REPO_SLUG/releases/download/$VERSION"
  else base="https://github.com/$REPO_SLUG/releases/latest/download"; fi
  if [ $DRY = 1 ]; then
    printf '    [dry-run] download %s/%s and %s/SHA256SUMS, check its sha256\n' "$base" "$ASSET" "$base"; return
  fi
  curl -fsSL --retry 3 -o "$WORK/$ASSET" "$base/$ASSET" || die "download failed: $base/$ASSET"
  curl -fsSL --retry 3 -o "$WORK/SHA256SUMS" "$base/SHA256SUMS" || die "download failed: $base/SHA256SUMS"
  local want got
  want="$(awk -v a="$ASSET" '$2 == a || $2 == "*"a { print $1 }' "$WORK/SHA256SUMS")"
  got="$(sha "$WORK/$ASSET")"
  [ -n "$want" ] || die "SHA256SUMS of that release has no $ASSET"
  [ "$got" = "$want" ] || die "$ASSET sha256 $got does not match the release's $want"
  NEWBIN="$WORK/$ASSET"; ok "downloaded $ASSET ${VERSION:-(latest)}, sha256 ok"
}

# installs $NEWBIN as ~/.iter/bin/iter_engine (temp name + mv, never cp over a
# running binary); CHANGED=1 when the file is new
install_binary() {
  CHANGED=0
  get_binary
  if [ -z "$NEWBIN" ] || [ $DRY = 1 ]; then printf '    [dry-run] install it as %s (temp file + mv)\n' "$BIN"; CHANGED=1; return; fi
  if [ -x "$BIN" ] && [ "$(sha "$BIN")" = "$(sha "$NEWBIN")" ]; then ok "$BIN is already this build"; return; fi
  mkdir -p "$IHOME/bin"
  cp "$NEWBIN" "$BIN.new.$$"
  chmod 755 "$BIN.new.$$"
  "$BIN.new.$$" --help >/dev/null 2>&1 || { rm -f "$BIN.new.$$"; die "$ASSET does not run here (glibc older than 2.31, or libstdc++6 / libgcc_s missing?)"; }
  mv -f "$BIN.new.$$" "$BIN"
  CHANGED=1; ok "$BIN"
}

# ------------------------------------------------------------------ steps

packages() {
  say "1/9 packages"
  local need=()
  if command -v pacman >/dev/null; then
    local want=(git curl jq openssl poppler github-cli)
    for p in "${want[@]}"; do pacman -Qq "$p" >/dev/null 2>&1 || need+=("$p"); done
    if [ ${#need[@]} -gt 0 ]; then
      yes_no "install ${need[*]} with sudo pacman?" || die "these are needed: ${need[*]}"
      run sudo pacman -S --needed --noconfirm "${need[@]}"
    fi
  elif command -v apt-get >/dev/null; then
    local want=(git curl jq openssl ca-certificates poppler-utils)
    for p in "${want[@]}"; do dpkg -s "$p" >/dev/null 2>&1 || need+=("$p"); done
    # gh (credentials for HTTPS git remotes) only where the distro packages it
    if ! dpkg -s gh >/dev/null 2>&1; then
      if apt-cache policy gh 2>/dev/null | grep -q 'Candidate: [0-9]'; then need+=(gh); else warn "gh not available from apt here (optional: https://cli.github.com)"; fi
    fi
    if [ ${#need[@]} -gt 0 ]; then
      yes_no "install ${need[*]} with sudo apt-get?" || die "these are needed: ${need[*]}"
      run sudo apt-get update -q
      run sudo apt-get install -y -q "${need[@]}"
    fi
  elif command -v dnf >/dev/null; then
    local want=(git curl jq openssl poppler-utils gh)
    for p in "${want[@]}"; do rpm -q "$p" >/dev/null 2>&1 || need+=("$p"); done
    if [ ${#need[@]} -gt 0 ]; then
      yes_no "install ${need[*]} with sudo dnf?" || die "these are needed: ${need[*]}"
      run sudo dnf install -y -q "${need[@]}"
    fi
  else
    warn "no pacman, apt-get or dnf: install git, curl, jq, openssl and poppler (pdftoppm) yourself"
  fi
  [ $DRY = 1 ] && { ok "(dry run: the tools are checked after the packages are installed)"; return; }
  for t in git curl jq; do command -v "$t" >/dev/null || die "$t is still missing"; done
  command -v pdftoppm >/dev/null || warn "pdftoppm (poppler) missing: GraphRAG skips PDF pages"
  ok "git $(git --version | cut -d' ' -f3), curl, jq$(command -v pdftoppm >/dev/null && echo ', pdftoppm')"
}

claude_cli() {
  say "2/9 claude CLI"
  if command -v claude >/dev/null; then ok "claude $(claude --version 2>/dev/null | head -1)"; return; fi
  if yes_no "install the claude CLI (Anthropic's installer, into ~/.local/bin)?"; then
    run bash -c "curl -fsSL https://claude.ai/install.sh | bash"
    if [ $DRY = 0 ]; then
      if command -v claude >/dev/null; then ok "claude $(claude --version 2>/dev/null | head -1)"; else warn "claude is not on PATH yet: open a new shell"; fi
    fi
  else
    warn "agents cannot run until the claude CLI is installed"
  fi
}

settings() {
  say "3/9 iter_data and this engine's name"
  load_conf
  local url name host
  url="${OPT_URL:-$(ask "iter_data URL [${DATA_URL:-https://iter.example.com}]:" "${DATA_URL:-}")}"
  url="${url%/}"
  if [ -z "$url" ]; then
    [ $DRY = 1 ] || die "the iter_data URL is required (--data-url URL)"
    url="https://iter.example.com"; warn "no iter_data URL given: the dry run goes on with $url"
  fi
  check_url "$url"
  # -k: only "does it answer?"; the certificate is checked by iter_engine (pinned)
  if curl -fsSk --max-time 10 -o /dev/null "$url/health" 2>/dev/null; then ok "$url answers"
  else warn "no answer from $url/health (a proxy may hide it; the key check below tells)"; fi
  host="$(hostname -s 2>/dev/null || cat /etc/hostname 2>/dev/null || echo engine)"
  name="${OPT_NAME:-$(ask "engine name [${ENGINE:-$host}]:" "${ENGINE:-$host}")}"
  case "$name" in omarchy|archlinux|arch|localhost|ubuntu|debian|linux|fedora)
    warn "\"$name\" is a default hostname: every fresh machine has it, and two engines with one name are one engine to iter_data"
    name="$(ask "a name unique to this machine:" "")"
    [ -n "$name" ] || die "an engine name unique to this machine is needed (--name NAME)" ;;
  esac
  case "$name" in *[!A-Za-z0-9_.-]*|"") die "engine name \"$name\": letters, digits, - _ . only" ;; esac
  run mkdir -p "$IHOME"
  DATA_URL="$url" ENGINE="$name"
  secrets_settings
  conf_text | write_file "$CONF"
  ok "engine \"$ENGINE\" -> $DATA_URL (in $CONF)"
  if pgrep -x iter_engine >/dev/null 2>&1 && ! systemctl is-active -q iter-engine 2>/dev/null; then
    warn "an iter_engine runs here outside the service (pid $(pgrep -x iter_engine | head -1)): one engine per machine, stop it"
  fi
}

binary() {
  say "4/9 iter_engine"
  install_binary
}

# iter_data's certificate, before the first sign-in: --fingerprint (or the one
# pasted here) pins it now and stops when the server presents another one;
# --trust-ca checks it against certificate authorities (a TLS proxy with a real
# certificate); else the first sign-in trusts what it sees (trust on first use).
trust() {
  say "4b/9 iter_data's certificate"
  case "$DATA_URL" in https://*) ;; *) ok "$DATA_URL is plain http to this machine: no certificate to trust"; return ;; esac
  if [ $TRUST_CA = 1 ]; then
    run "$BIN" key trust --data-url "$DATA_URL" --ca || die "could not reach $DATA_URL with a certificate-authority check"
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
  run "$BIN" key trust --data-url "$DATA_URL" --fingerprint "$fp" \
    || die "$DATA_URL does not present the certificate $fp: something else answers at that URL (or the fingerprint is mistyped). Nothing was trusted; check the URL and the fingerprint (iter_data installer output, webui Engine keys screen)"
}

# the embedding model (sentence-transformers/all-MiniLM-L6-v2, Apache-2.0,
# ~91 MB), checksum-verified; the engine finds ~/.iter/models beside its
# binary. Without it the engine downloads the model itself on first use.
model() {
  say "5/9 embedding model"
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

key() {
  say "6/9 engine key"
  run mkdir -p "$IHOME"
  [ -f "$ENVF" ] || run touch "$ENVF"
  run chmod 600 "$ENVF"
  if [ $DRY = 1 ]; then
    [ -f "$KEYF" ] && ok "$KEYF exists" || printf '    [dry-run] %s key new --key-file %s --name %s\n' "$BIN" "$KEYF" "$ENGINE"
    printf '    [dry-run] wait until iter_data accepts it (%s key status)\n' "$BIN"; return
  fi
  [ -f "$KEYF" ] || "$BIN" key new --key-file "$KEYF" --name "$ENGINE" | sed 's/^/    /'
  local first=1
  while :; do
    if "$BIN" key status --key-file "$KEYF" --data-url "$DATA_URL" > "$TMPF" 2>&1; then
      sed 's/^/    /' "$TMPF"; ok "iter_data accepts this engine's key"; return
    fi
    if [ $first = 1 ]; then
      sed 's/^/    /' "$TMPF"
      printf '\n    In the webui (%s): Engine keys (the key button in the header) -> Add: paste the iter_... public key above, engine name "%s".\n' "$DATA_URL" "$ENGINE"
      first=0
    fi
    case "$(ask "press Enter once it is added (n = a new key, s = skip):" "")" in
      n|N) mv "$KEYF" "$KEYF.replaced-$(date +%Y%m%d%H%M%S)"; "$BIN" key new --key-file "$KEYF" --name "$ENGINE" | sed 's/^/    /'; first=1 ;;
      s|S) warn "skipped: iter_data refuses this engine until its public key is added"; return ;;
      *) { [ -r /dev/tty ] && { : < /dev/tty; } 2>/dev/null; } || die "iter_data does not accept the key yet (no terminal to wait at): add it, then run this again"
         sleep 2; printf '    not accepted yet\n' ;;
    esac
  done
}

power() {
  say "7/9 power"
  ls /sys/class/power_supply/BAT* >/dev/null 2>&1 || { ok "not a laptop"; return; }
  if [ -f /etc/systemd/logind.conf.d/iter-engine.conf ] && [ "$(systemctl is-enabled sleep.target 2>/dev/null || true)" = masked ]; then
    ok "never suspends (lid and idle)"; return
  fi
  if yes_no "this laptop runs agents unattended: never suspend, on idle or with the lid closed?"; then
    run sudo mkdir -p /etc/systemd/logind.conf.d
    printf '[Login]\nHandleLidSwitch=ignore\nHandleLidSwitchExternalPower=ignore\nHandleLidSwitchDocked=ignore\nIdleAction=ignore\n' \
      | write_file /etc/systemd/logind.conf.d/iter-engine.conf sudo
    # anything asking to suspend (Omarchy's hypridle) is now refused
    run sudo systemctl mask -q sleep.target suspend.target hibernate.target hybrid-sleep.target
    run sudo systemctl kill -s HUP systemd-logind
    ok "lid and idle ignored, suspend masked (undo: sudo systemctl unmask sleep.target suspend.target hibernate.target hybrid-sleep.target)"
  else
    warn "a suspended laptop stops its agents mid-run"
  fi
}

HYPR="$HOME/.config/hypr"
MARK="iter engine (install.sh)"

# Omarchy: no idle lock, no screensaver, no lock on lid close; hypridle turns
# the screen off after 10 minutes. Current Omarchy (Lua Hyprland config) runs
# idle in its own shell (~/.config/omarchy/shell.json) and dropped hypridle;
# older Omarchy runs hypridle with lock and screensaver listeners.
desktop() {
  say "7b/9 Omarchy screen"
  { [ -d "$HOME/.local/share/omarchy" ] || command -v omarchy >/dev/null; } || { ok "not Omarchy: left alone"; return; }
  local done=0
  grep -qs "$MARK" "$HYPR/hypridle.conf" && done=1
  if [ -f "$HYPR/hyprland.lua" ] && ! jq -e '.idle.screensaver == 0 and .idle.lock == 0' "$HOME/.config/omarchy/shell.json" >/dev/null 2>&1; then done=0; fi
  [ $done = 0 ] || { ok "no lock, no screensaver, screen off after 10 min"; return; }
  yes_no "unattended engine: turn off the lock screen and screensaver, and have hypridle turn the screen off after 10 minutes?" \
    || { warn "left as Omarchy set it"; return; }
  if [ $DRY = 1 ]; then
    printf '    [dry-run] Omarchy: idle screensaver + lock off (~/.config/omarchy/shell.json), lid close without lock (%s/bindings.lua),\n' "$HYPR"
    printf '    [dry-run]          hypridle autostart, %s/hypridle.conf = screen off after 10 min (old file kept as hypridle.conf.before-iter)\n' "$HYPR"
    return
  fi
  local off on
  if [ -f "$HYPR/hyprland.lua" ]; then
    local sj="$HOME/.config/omarchy/shell.json"
    mkdir -p "$(dirname "$sj")"; [ -s "$sj" ] || echo '{"version": 1}' > "$sj"
    jq '.idle = ((.idle // {}) + {"screensaver": 0, "lock": 0})' "$sj" > "$sj.tmp" && mv "$sj.tmp" "$sj"
    ok "Omarchy shell: idle screensaver and lock off ($sj)"
    if ! grep -q "$MARK" "$HYPR/bindings.lua" 2>/dev/null; then
      printf '\n-- %s: closing the lid reconciles the displays but does not lock\no.rebind("switch:on:Lid Switch", nil, "omarchy-hyprland-monitor-clamshell", { locked = true })\no.rebind("switch:on:Apple SMC power/lid events", nil, "omarchy-hyprland-monitor-clamshell", { locked = true })\n' "$MARK" >> "$HYPR/bindings.lua"
    fi
    ok "lid close: no lock ($HYPR/bindings.lua)"
    if ! grep -q 'launch_on_start("hypridle")' "$HYPR/autostart.lua" 2>/dev/null; then
      printf '\n-- %s: turns the screen off after 10 minutes (hypridle.conf)\no.launch_on_start("hypridle")\n' "$MARK" >> "$HYPR/autostart.lua"
    fi
    off="hyprctl dispatch 'hl.dsp.dpms({ action = \"disable\" })'"
    on="hyprctl dispatch 'hl.dsp.dpms({ action = \"enable\" })'"
  else
    off="hyprctl dispatch dpms off"; on="hyprctl dispatch dpms on"
  fi
  pacman -Qq hypridle >/dev/null 2>&1 || sudo pacman -S --needed --noconfirm hypridle
  if [ -f "$HYPR/hypridle.conf" ] && ! grep -q "$MARK" "$HYPR/hypridle.conf"; then cp "$HYPR/hypridle.conf" "$HYPR/hypridle.conf.before-iter"; fi
  mkdir -p "$HYPR"
  cat > "$HYPR/hypridle.conf" <<EOF
# $MARK: no lock and no screensaver on idle; the screen turns off after
# 10 minutes and back on at any key or mouse move. Omarchy's own file, if
# there was one, is hypridle.conf.before-iter.
general {
    after_sleep_cmd = $on
}

listener {
    timeout = 600
    on-timeout = $off
    on-resume = $on
}
EOF
  ok "hypridle: screen off after 10 minutes ($HYPR/hypridle.conf)"
  if [ -n "${WAYLAND_DISPLAY:-}" ]; then
    pkill -x hypridle 2>/dev/null || true
    setsid -f hypridle >/dev/null 2>&1 && ok "hypridle running"
  else
    ok "hypridle starts at the next desktop login (this shell has no desktop session)"
  fi
}

unit_text() {
  cat <<EOF
# written by installers/iter_engine/linux/install.sh
[Unit]
Description=iter engine $ENGINE
Wants=network-online.target
After=network-online.target iter-data.service

[Service]
User=$(id -un)
WorkingDirectory=$IHOME
Environment=HOME=$HOME
Environment=PATH=$HOME/.local/bin:/usr/local/bin:/usr/bin:/bin
ExecStart=$BIN --data-url $DATA_URL --env-file $ENVF --key-file $KEYF --name $ENGINE$(secrets_args)
StandardOutput=append:$IHOME/engine.log
StandardError=append:$IHOME/engine.err.log
Restart=on-failure
RestartSec=10
# a stop takes the agent sessions and shells it started with it
KillMode=control-group

[Install]
WantedBy=multi-user.target
EOF
}

need_systemd() {
  [ "$(ps -p 1 -o comm= 2>/dev/null)" = systemd ] && return 0
  [ $DRY = 1 ] && { warn "systemd is not PID 1 here (fine for a dry run; the real run stops)"; return 0; }
  die "systemd is not running: the engine runs as a systemd service"
}

service() {
  say "8/9 iter-engine service"
  need_systemd
  if [ -f "$UNIT" ] && [ "$(unit_text)" = "$(cat "$UNIT")" ]; then ok "$UNIT is current"
  else unit_text | write_file "$UNIT" sudo; run sudo systemctl daemon-reload; fi
  run sudo systemctl enable -q iter-engine
  run sudo systemctl restart iter-engine
  [ $DRY = 1 ] && return
  sleep 3
  systemctl is-active -q iter-engine || { tail -20 "$IHOME/engine.err.log" 2>/dev/null; die "iter-engine did not start"; }
  ok "iter-engine is running (log: $IHOME/engine.log)"
}

assignments() { # the engine's assignments into $TMPF; false when it has none yet
  [ "$(api "/api/engines/$ENGINE/assignments")" = 200 ] && jq -e '.projects | length > 0' "$TMPF" >/dev/null 2>&1
}

accounts() {
  say "account tokens"
  load_conf
  if ! assignments; then ok "no projects served yet: claude agents use this machine's own login (run \`claude\` once and log in)"; return; fi
  local vars missing=0
  vars="$(jq -r '[.accounts[]?.token_envar, .projects[].accounts[]?.token_envar] | map(select(. != null and . != "")) | unique | .[]' "$TMPF")"
  [ -n "$vars" ] || { ok "the served projects name no account tokens"; return; }
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
  say "project checkouts"
  load_conf
  if ! assignments; then
    ok "no projects served yet: in the webui's Settings tab draw a serves edge from \"$ENGINE\" to each project, topdir ~/dev/<project>; then: install.sh projects"
    return
  fi
  local list; list="$(jq -r '.projects[] | "\(.project)\t\(.topdir)"' "$TMPF")"
  while IFS=$'\t' read -r p top; do
    [ -n "$p" ] || continue
    if [ -z "$top" ]; then warn "$p: its serves edge has no topdir (set ~/dev/$p in the Settings tab)"; continue; fi
    local dir; dir="$(expand "$top")"
    if [ -d "$dir/.git" ]; then
      if git -C "$dir" ls-remote -q >/dev/null 2>&1; then ok "$p: $dir"; else warn "$p: $dir cannot reach its remote (credentials? try: gh auth login && gh auth setup-git)"; fi
      continue
    fi
    if [ -e "$dir" ] && [ -n "$(ls -A "$dir" 2>/dev/null)" ]; then warn "$p: $dir exists but is not a git checkout; left alone"; continue; fi
    local url=""
    [ "$(api "/api/projects/$p")" = 200 ] && url="$(jq -r '.gitrepo // ""' "$TMPF")"
    url="$(ask "$p: git URL to clone into $dir [${url:-none}]:" "$url")"
    [ -n "$url" ] || { warn "$p: no checkout (the engine skips it until $dir exists)"; continue; }
    if run git clone -q "$url" "$dir"; then ok "$p: cloned into $dir"
    else warn "$p: clone failed (credentials? try: gh auth login && gh auth setup-git)"; fi
  done <<< "$list"
}

status() {
  load_conf
  [ -n "${ENGINE:-}" ] || die "not installed yet: run install.sh"
  printf 'engine   %s -> %s\nservice  %s\n' "$ENGINE" "$DATA_URL" "$(systemctl is-active iter-engine 2>/dev/null || true)"
  if [ -x "$BIN" ]; then printf 'binary   %s (sha256 %s)\n' "$BIN" "$(sha "$BIN" | cut -c1-12)"; "$BIN" key status --key-file "$KEYF" --data-url "$DATA_URL" || true
  else printf 'binary   %s missing\n' "$BIN"; fi
  if assignments; then jq -r '.projects[] | "project  \(.project)  \(.topdir)  \(.state)"' "$TMPF"; else echo "project  (none served)"; fi
  tail -3 "$IHOME/engine.log" 2>/dev/null || true
}

update() {
  load_conf
  [ -n "${ENGINE:-}" ] || die "not installed yet: run install.sh"
  say "update iter_engine"
  if [ $YES = 0 ] && [ "$(api "/api/engines/$ENGINE")" = 200 ]; then
    local n; n="$(jq -r '.running // 0' "$TMPF" 2>/dev/null || echo 0)"
    [ "${n:-0}" -eq 0 ] 2>/dev/null || die "engine \"$ENGINE\" runs $n item(s) and a restart stops them: set its projects Draining and wait, or rerun with --yes"
  fi
  install_binary
  if [ $CHANGED = 1 ] && [ -f "$UNIT" ]; then
    need_systemd
    [ "$(unit_text)" = "$(cat "$UNIT")" ] || { unit_text | write_file "$UNIT" sudo; run sudo systemctl daemon-reload; }
    run sudo systemctl restart iter-engine
    ok "iter-engine restarted"
  fi
}

uninstall() {
  say "uninstall the iter engine"
  if [ -f "$UNIT" ]; then
    run sudo systemctl disable --now iter-engine
    run sudo rm -f "$UNIT"
    run sudo systemctl daemon-reload
    ok "service removed"
  else ok "no iter-engine service"; fi
  [ -e "$BIN" ] && run rm -f "$BIN" && ok "removed $BIN"
  [ -f /etc/systemd/logind.conf.d/iter-engine.conf ] && warn "suspend stays off; undo: sudo rm /etc/systemd/logind.conf.d/iter-engine.conf && sudo systemctl unmask sleep.target suspend.target hibernate.target hybrid-sleep.target"
  ok "kept $IHOME (key, tokens, model, logs) and the project checkouts; delete them by hand when this machine is done with iter"
  ok "in the webui, remove the engine's key (Engine keys) and its serves edges"
}

migrate_home
case "$VERB" in
  install)
    packages; claude_cli; settings; binary; trust; model; key; power; desktop; service
    say "9/9 accounts and projects"
    accounts; projects
    [ $DRY = 0 ] || { printf '\nDry run done: nothing was changed.\n'; exit 0; }
    cat <<EOF

Done. Engine "$ENGINE" runs as the iter-engine service and checks in with $DATA_URL.
Next, in the webui's Settings tab: draw a serves edge from $ENGINE to each project
(topdir ~/dev/<project>), then run \`install.sh projects\` here to clone them.
  status:  install.sh status        log:  install.sh logs  ($IHOME/engine.log)
  update:  install.sh update        a new key:  $BIN key rotate   (its public key: $BIN key show)
EOF
    ;;
  projects) projects ;;
  accounts) accounts ;;
  status) status ;;
  update) update ;;
  logs) run tail -n 50 -F "$IHOME/engine.log" "$IHOME/engine.err.log" ;;
  uninstall) uninstall ;;
esac
