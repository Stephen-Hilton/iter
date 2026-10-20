#!/usr/bin/env bash
# install.sh - make this Mac an iter engine, run by launchd as a per-user agent
# (~/Library/LaunchAgents/com.iter.engine.plist). Apple silicon or Intel.
# ONE engine per machine; it serves every project the webui assigns to it.
# iter_data runs elsewhere; this Mac only makes outbound connections: to
# iter_data, to Anthropic, to the projects' git remote and, once, to Hugging
# Face for the embedding model.
#
#   curl -fsSL https://raw.githubusercontent.com/Stephen-Hilton/iter/main/installers/iter_engine/macos/install.sh | bash
#   ./install.sh [verb] [options]
#
# Verbs:
#   install     (default) set up or repair; safe to run again
#   status      the launchd agent, the binary, the key, the served projects
#   update      install the newer binary and restart (refuses while work runs, without --yes)
#   accounts    ask for each account token the served projects name
#   projects    clone each served project that has no checkout yet
#   logs        follow the engine log
#   uninstall   unload and remove the launchd agent and the binary (keeps the key, tokens, model)
#
# Options:
#   --iter-dir DIR    the folder that holds everything iter installs [$ITER_DIR, else where the
#                     engine is installed already, else /opt/iter]; it always ends in /iter:
#                     /my/dir/iter/, /my/dir/iter and /my/dir all mean /my/dir/iter (also --iter_dir)
#   --data-url URL    iter_data URL (else asked; kept in <iter dir>/iter_engine/config/engine.conf)
#   --name NAME       this engine's name, unique per machine (default: the Mac's local host name)
#   --version TAG     release to download, e.g. v2026.10.17 (else $ITER_VERSION, else latest)
#   --binary PATH     install this iter_engine file instead of downloading one
#   --fingerprint FP  iter_data's certificate fingerprint (sha256:..., from the iter_data
#                     installer or the webui's Engine keys screen): pinned before sign-in
#   --trust-ca        iter_data is behind a TLS proxy with a real certificate: check it
#                     against certificate authorities instead of pinning
#   --secrets-command CMD  a shell command printing KEY=value lines (the account tokens,
#                     from a secret store: aws ssm / secretsmanager, op, security, vault);
#                     the engine runs it at startup and every --secrets-every minutes and
#                     keeps the values in memory only. Kept in config/engine.conf and put
#                     on the service's command line; "none" removes it. With it, the
#                     accounts step asks for no tokens.
#   --secrets-every N  minutes between secrets-command runs [5]
#   --yes             answer yes to every question; update while work runs
#   --dry-run         print every action, change nothing
#   -h, --help        this text
#   ($ITER_RELEASE_URL: download the binary and SHA256SUMS from this base URL instead)
#
# Runs under macOS's own bash 3.2 (the curl | bash line uses it).
#
# Steps of `install`: the iter dir (sudo once to create /opt/iter; an older
# ~/.iter is moved in) -> tools (git and jq, from Homebrew when macOS has none;
# poppler optional; asks first) -> the claude CLI (Anthropic's installer) ->
# iter_data URL + engine name (config/engine.conf) -> iter_engine into bin/
# (the asset next to this script, else the GitHub Release, sha256-checked;
# linked as /usr/local/bin/iter_engine) -> the embedding model into models/ ->
# the engine key (`iter_engine key new`: paste its public key into the webui's
# Engine keys screen; the script waits until iter_data accepts it) -> iter_data's
# local CA into the login keychain (browsers here trust the webui) -> a laptop:
# keep awake on power while the engine runs (asks) -> the launchd agent ->
# account tokens and project checkouts.
#
# The agent runs while you are logged in (it starts at login); for an
# unattended Mac turn on automatic login. Everything iter keeps is in one
# folder, <iter dir>/iter_engine (/opt/iter/iter_engine by default, owned by
# you): bin/iter_engine, config/ (engine.conf, server_trust.json), secrets/
# (engine.key, .env with the account tokens), models/ and logs/ (engine.log,
# engine.err.log). Outside it only the plist and the /usr/local/bin link.
# An install from before 2026-10-18 kept these in ~/.iter: install (or update)
# moves them in, once; files it does not know go to iter_engine/old-dot-iter/.
#
# HTTPS: iter_data must be https:// (iter_data serves HTTPS by default; plain
# http:// is refused unless iter_data runs on this machine: 127.0.0.1,
# localhost, ::1). The engine pins iter_data's certificate fingerprint in
# config/server_trust.json: --fingerprint (or the fingerprint pasted when
# asked) runs `iter_engine key trust` right after the binary is installed and
# stops if the server presents another certificate; without one, the first
# sign-in trusts the certificate it sees (compare: iter_engine key status).
# Browsers: iter_data signs its certificate with a local CA of its own (name-
# constrained to that server); once the key is accepted the CA is fetched over
# the pinned connection (`iter_engine key ca`) and added to your login keychain
# as trusted for SSL (macOS asks for your password once), so Safari and Chrome
# open the webui as secure.
# Other key commands: `iter_engine key show` (print the public key),
# `iter_engine key rotate` (a new key, signed by the current one, no webui step).
set -euo pipefail

REPO_SLUG="Stephen-Hilton/iter"
LABEL="com.iter.engine"
HERE=""
if [ -n "${BASH_SOURCE[0]:-}" ] && [ -f "${BASH_SOURCE[0]}" ]; then HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"; fi
PLIST="$HOME/Library/LaunchAgents/$LABEL.plist"
DOMAIN="gui/$(id -u)"
LINK=/usr/local/bin/iter_engine
export PATH="$HOME/.local/bin:/opt/homebrew/bin:/usr/local/bin:$PATH"

VERB=install DRY=0 YES=0 BINARY="" OPT_URL="" OPT_NAME="" OPT_FP="" TRUST_CA=0 OPT_SC="" SC_GIVEN=0 OPT_SE="" OPT_DIR="" VERSION="${ITER_VERSION:-}"
while [ $# -gt 0 ]; do
  case "$1" in
    install|status|update|accounts|projects|logs|uninstall) VERB="$1" ;;
    --iter-dir|--iter_dir) OPT_DIR="${2:?--iter-dir needs a folder}"; shift ;;
    --iter-dir=*|--iter_dir=*) OPT_DIR="${1#*=}"; [ -n "$OPT_DIR" ] || { echo "--iter-dir needs a folder" >&2; exit 2; } ;;
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
# write stdin to a file, or under --dry-run print it
write_file() {
  if [ $DRY = 1 ]; then printf '    [dry-run] write %s:\n' "$1"; sed 's/^/      | /'; return; fi
  cat > "$1"
}
tty_ok() { [ -r /dev/tty ] && { : < /dev/tty; } 2>/dev/null; }
ask() { # ask "question" default -> the answer (the default without a terminal)
  local a=""
  if tty_ok; then printf '    %s ' "$1" > /dev/tty; IFS= read -r a < /dev/tty || true; fi
  printf '%s' "${a:-$2}"
}
yes_no() { [ $YES = 1 ] && return 0; case "$(ask "$1 [Y/n]" y)" in [nN]*) return 1 ;; *) return 0 ;; esac; }
no_yes() { [ $YES = 1 ] && return 0; case "$(ask "$1 [y/N]" n)" in [yY]*) return 0 ;; *) return 1 ;; esac; }
ask_secret() {
  local a=""
  if tty_ok; then printf '    %s ' "$1" > /dev/tty; IFS= read -rs a < /dev/tty || true; printf '\n' > /dev/tty; fi
  printf '%s' "$a"
}
confval() { [ -f "$CONF_READ" ] && grep -E "^$1=" "$CONF_READ" | tail -1 | cut -d= -f2- || true; }
envval() { [ -f "$2" ] && grep -E "^[[:space:]]*$1=" "$2" | tail -1 | cut -d= -f2- | sed -e 's/^["'\'']//' -e 's/["'\'']$//' | tr -d '\r' || true; }
# shellcheck disable=SC2088  # the topdir arrives as text with a literal ~
expand() { case "$1" in "~") printf '%s' "$HOME" ;; "~/"*) printf '%s/%s' "$HOME" "${1#\~/}" ;; *) printf '%s' "$1" ;; esac; }
sha() { shasum -a 256 "$1" | cut -d' ' -f1; }
xml() { printf '%s' "$1" | sed -e 's/&/\&amp;/g' -e 's/</\&lt;/g' -e 's/>/\&gt;/g'; }
unxml() { sed -e 's/&lt;/</g' -e 's/&gt;/>/g' -e 's/&amp;/\&/g'; }

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
# the iter dir the launchd agent runs the engine from now: the plist's
# <iter dir>/iter_engine/bin/iter_engine ("" when none, or an older ~/.iter one)
installed_dir() {
  local t=""
  [ ! -f "$PLIST" ] || t="$(sed -n 's|^[[:space:]]*<string>\(.*\)/iter_engine/bin/iter_engine</string>[[:space:]]*$|\1|p' "$PLIST" | head -1 | unxml || true)"
  # an install that stopped before its agent: the PATH link
  [ -n "$t" ] || { t="$(readlink /usr/local/bin/iter_engine 2>/dev/null || true)"; case "$t" in /*/iter_engine/bin/iter_engine) t="${t%/iter_engine/bin/iter_engine}" ;; *) t="" ;; esac; }
  printf '%s' "$t"
}

[ "$(id -u)" != 0 ] || die "run this as your own user, not root"
[ "$(uname -s)" = Darwin ] || die "this script is for macOS (Linux: installers/iter_engine/linux, Windows: installers/iter_engine/windows)"
[ $DRY = 0 ] || printf '\033[1mdry run:\033[0m every action is printed, nothing is changed\n'

# the iter dir: --iter-dir, else $ITER_DIR, else where the agent runs it from, else /opt/iter
INSTALLED_DIR="$(installed_dir)"
if [ -n "$OPT_DIR" ]; then ITER_DIR="$(iter_dir_of "$OPT_DIR")"
elif [ -n "${ITER_DIR:-}" ]; then ITER_DIR="$(iter_dir_of "$ITER_DIR")"
elif [ -n "$INSTALLED_DIR" ]; then ITER_DIR="$INSTALLED_DIR"
else ITER_DIR=/opt/iter; fi
IHOME="$ITER_DIR/iter_engine"
# iter_engine finds its folder from $ITER_HOME: the agent sets it, and so does
# this script for its own iter_engine calls
export ITER_HOME="$IHOME"
CONF="$IHOME/config/engine.conf"
TRUSTF="$IHOME/config/server_trust.json"
ENVF="$IHOME/secrets/.env"
KEYF="$IHOME/secrets/engine.key"
BIN="$IHOME/bin/iter_engine"
MODEL="$IHOME/models/all-MiniLM-L6-v2"
LOGS="$IHOME/logs"
# confval reads this one (a dry run of the move reads the old ~/.iter/engine.conf)
CONF_READ="$CONF"

case "$(uname -m)" in
  arm64|aarch64) ARCH=aarch64 ;;
  x86_64) ARCH=x86_64 ;;
  *) die "no iter_engine build for $(uname -m)" ;;
esac
# an Intel shell under Rosetta on Apple silicon still gets the native build
[ "$ARCH" = x86_64 ] && [ "$(sysctl -n sysctl.proc_translated 2>/dev/null || echo 0)" = 1 ] && ARCH=aarch64
ASSET="iter_engine-macos-$ARCH"

DATA_URL="" ENGINE="" KEEPAWAKE=""
load_conf() { # fills what this run has not set yet
  [ -n "$DATA_URL" ] || DATA_URL="$(confval DATA_URL)"
  [ -n "$ENGINE" ] || ENGINE="$(confval ENGINE)"
  [ $SC_GIVEN = 1 ] || [ -n "$SECRETS_COMMAND" ] || SECRETS_COMMAND="$(confval SECRETS_COMMAND)"
  [ -n "$SECRETS_EVERY" ] || SECRETS_EVERY="$(confval SECRETS_EVERY)"
  [ -n "$KEEPAWAKE" ] || KEEPAWAKE="$(confval KEEPAWAKE)"
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
  [ -z "$KEEPAWAKE" ] || printf 'KEEPAWAKE=%s\n' "$KEEPAWAKE"
  [ -z "$SECRETS_COMMAND" ] || printf 'SECRETS_COMMAND=%s\n' "$SECRETS_COMMAND"
  [ -z "$SECRETS_EVERY" ] || printf 'SECRETS_EVERY=%s\n' "$SECRETS_EVERY"
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

WORK="$(mktemp -d)"; trap 'rm -rf "$WORK"' EXIT
TMPF="$WORK/api"
# ------------------------------------------------------------------ iter_data over HTTPS
# iter_data serves HTTPS with its own (self-signed) certificate. The engine pins
# that certificate's fingerprint in config/server_trust.json (one entry per
# scheme://host:port), and plain http:// is only for this machine. The script's
# own API calls check the certificate the same way.
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
# does config/server_trust.json have an entry for this URL yet?
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
agent_loaded() { launchctl print "$DOMAIN/$LABEL" >/dev/null 2>&1; }
agent_pid() { { launchctl print "$DOMAIN/$LABEL" 2>/dev/null || true; } | awk '/^[[:space:]]*pid = / { print $3; exit }'; }

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

# installs $NEWBIN as bin/iter_engine: a temp name in the same folder,
# then mv. Never cp over it: macOS SIGKILLs a binary copied over a live one.
install_binary() {
  CHANGED=0
  get_binary
  if [ -z "$NEWBIN" ] || [ $DRY = 1 ]; then printf '    [dry-run] install it as %s (temp file + mv)\n' "$BIN"; CHANGED=1; return; fi
  if [ -x "$BIN" ] && [ "$(sha "$BIN")" = "$(sha "$NEWBIN")" ]; then ok "$BIN is already this build"; return; fi
  mkdir -p "$(dirname "$BIN")"
  local tmp="$IHOME/bin/.iter_engine.new.$$"
  cp "$NEWBIN" "$tmp"
  chmod 755 "$tmp"
  xattr -d com.apple.quarantine "$tmp" 2>/dev/null || true
  "$tmp" --help >/dev/null 2>&1 || { rm -f "$tmp"; die "$ASSET does not run on this Mac"; }
  mv -f "$tmp" "$BIN"
  CHANGED=1; ok "$BIN"
}

# ------------------------------------------------------------------ the iter dir
# Everything lives in <iter dir>/iter_engine, owned by you. <iter dir> itself
# (/opt/iter by default) is made with sudo, root-owned, when you cannot write
# where it goes; under your own home no sudo is needed.
ME="$(id -un)" MYGROUP="$(id -gn)"
# can this user make PATH (the nearest folder that exists is writable)?
can_make() { local p="$1"; while [ ! -e "$p" ]; do p="$(dirname "$p")"; done; [ -w "$p" ]; }
# mv as this user when both folders are writable, else with sudo
mv_maybe_sudo() {
  if [ -w "$(dirname "$1")" ] && can_make "$(dirname "$2")" && { [ ! -e "$(dirname "$2")" ] || [ -w "$(dirname "$2")" ]; }; then run mv "$1" "$2"
  else run sudo mv "$1" "$2"; fi
}
ensure_iter_dir() {
  if [ -d "$ITER_DIR" ]; then return 0; fi
  if can_make "$ITER_DIR"; then run mkdir -p "$ITER_DIR"
  else ok "creating $ITER_DIR with sudo (root-owned; iter_engine/ in it is yours)"; run sudo install -d -m 755 "$ITER_DIR"; fi
}
ensure_ihome() {
  if [ -d "$IHOME" ]; then
    [ -w "$IHOME" ] || { warn "$IHOME is not yours: taking it back with sudo chown"; run sudo chown -R "$ME:$MYGROUP" "$IHOME"; }
  elif { [ -d "$ITER_DIR" ] && [ -w "$ITER_DIR" ]; } || { [ ! -d "$ITER_DIR" ] && can_make "$ITER_DIR"; }; then run mkdir -p "$IHOME"
  else run sudo install -d -o "$ME" -g "$MYGROUP" -m 755 "$IHOME"; fi
  run mkdir -p "$IHOME/bin" "$IHOME/config" "$IHOME/secrets" "$IHOME/models" "$IHOME/logs"
  run chmod 700 "$IHOME/secrets"
}

# an engine installed in another iter dir (the plist runs it from there): move it here
relocate() {
  [ -n "$INSTALLED_DIR" ] && [ "$INSTALLED_DIR" != "$ITER_DIR" ] && [ -d "$INSTALLED_DIR/iter_engine" ] || return 0
  [ ! -e "$IHOME" ] || die "the engine is installed in $INSTALLED_DIR/iter_engine, and $IHOME exists too: keep one of them (move or remove the other by hand), then run this again"
  ok "the engine is installed in $INSTALLED_DIR/iter_engine: moving it to $IHOME"
  if agent_loaded; then run launchctl bootout "$DOMAIN/$LABEL"; fi
  mv_maybe_sudo "$INSTALLED_DIR/iter_engine" "$IHOME"
  # the old iter dir, when nothing else is left in it
  if [ $DRY = 0 ] && [ -z "$(ls -A "$INSTALLED_DIR" 2>/dev/null)" ]; then
    if [ -w "$(dirname "$INSTALLED_DIR")" ]; then rmdir "$INSTALLED_DIR" 2>/dev/null || true; else sudo rmdir "$INSTALLED_DIR" 2>/dev/null || true; fi
  fi
}

# move SRC to DEST. The older folder is the live install (a finished move leaves
# it empty), so its file takes the place: what is in the way at DEST (left from
# an earlier install) is set aside in replaced-<time>/REL. Nothing is
# overwritten or deleted.
STAMP="$(date +%Y%m%d%H%M%S)"
mv_into() { # SRC DEST REL
  local dest="$2"
  if [ -e "$dest" ] || [ -L "$dest" ]; then
    case "$dest" in
      "$IHOME/old-dot-iter/"*) dest="$dest.$STAMP" ;;
      *) run mkdir -p "$(dirname "$IHOME/replaced-$STAMP/$3")"; run mv "$dest" "$IHOME/replaced-$STAMP/$3"
         warn "$dest was there already (from an earlier install): set aside in $IHOME/replaced-$STAMP" ;;
    esac
  fi
  run mkdir -p "$(dirname "$dest")"
  run mv "$1" "$dest"
}
# the older home folder (~/.iter) into the new layout
move_old_home() { # DIR
  local s="$1" f g n m
  if [ $DRY = 1 ] && [ -f "$s/engine.conf" ] && [ ! -f "$CONF" ]; then CONF_READ="$s/engine.conf"; fi
  for f in "$s"/* "$s"/.[!.]* "$s"/..?*; do
    [ -e "$f" ] || [ -L "$f" ] || continue
    n="${f##*/}"
    case "$n" in
      engine.conf|server_trust.json) mv_into "$f" "$IHOME/config/$n" "$n" ;;
      engine.key|engine.key.*|.env) mv_into "$f" "$IHOME/secrets/$n" "$n" ;;
      engine.log|engine.err.log) mv_into "$f" "$IHOME/logs/$n" "$n" ;;
      # config/ and secrets/: a newer iter_engine already wrote there (pinned certificates)
      bin|logs|models|config|secrets)
        if [ -L "$f" ] || [ ! -d "$f" ]; then mv_into "$f" "$IHOME/old-dot-iter/$n" "$n"; continue; fi
        for g in "$f"/* "$f"/.[!.]* "$f"/..?*; do
          [ -e "$g" ] || [ -L "$g" ] || continue
          m="${g##*/}"
          case "$n/$m" in
            bin/iter_engine|logs/*|models/*|config/*|secrets/*) mv_into "$g" "$IHOME/$n/$m" "$n/$m" ;;
            *) mv_into "$g" "$IHOME/old-dot-iter/$n/$m" "$n/$m" ;;
          esac
        done
        if [ $DRY = 1 ]; then run rmdir "$f"; else rmdir "$f" 2>/dev/null || warn "$f is not empty: left as it is"; fi ;;
      *) mv_into "$f" "$IHOME/old-dot-iter/$n" "$n" ;;
    esac
  done
  if [ $DRY = 1 ]; then run rmdir "$s"; else rmdir "$s" 2>/dev/null || warn "$s is not empty: left as it is"; fi
}
# an install from before 2026-10-18 kept everything in ~/.iter:
# move it into the iter dir, once; never deletes a file
migrate_old() {
  local s found=""
  s="$HOME/.iter"
  if [ -d "$s" ] && [ ! -L "$s" ]; then found=" $s"; fi
  [ -n "$found" ] || return 0
  ok "moving the older install in${found} into $IHOME"
  # the agent runs the engine from there: stop it first (step 8/9 loads the new one)
  if [ -f "$PLIST" ] && grep -q '/\.iter/bin/iter_engine' "$PLIST" && agent_loaded; then run launchctl bootout "$DOMAIN/$LABEL"; fi
  for s in $found; do move_old_home "$s"; done
  [ ! -d "$IHOME/old-dot-iter" ] || ok "files the installer does not know are kept in $IHOME/old-dot-iter"
}

place() {
  say "0/9 the iter dir: $ITER_DIR"
  ensure_iter_dir
  relocate
  ensure_ihome
  migrate_old
  ok "$IHOME"
}

# /usr/local/bin/iter_engine -> bin/iter_engine, so `iter_engine key show` works in any shell
path_link() {
  if [ -L "$LINK" ] && [ "$(readlink "$LINK")" = "$BIN" ]; then ok "$LINK -> $BIN"; return; fi
  if [ -e "$LINK" ] && [ ! -L "$LINK" ]; then warn "$LINK is a file, not a link: left alone (the engine is $BIN)"; return; fi
  if [ -d "$(dirname "$LINK")" ] && [ -w "$(dirname "$LINK")" ]; then run ln -sfn "$BIN" "$LINK"
  elif ! { run sudo mkdir -p "$(dirname "$LINK")" && run sudo ln -sfn "$BIN" "$LINK"; }; then
    warn "could not link $LINK (run $BIN directly)"; return
  fi
  ok "$LINK -> $BIN"
}

# ------------------------------------------------------------------ steps

tools() {
  say "1/9 tools"
  # macOS's /usr/bin/git is only a stub until the Command Line Tools are in
  local n=()
  if ! xcode-select -p >/dev/null 2>&1 && { ! command -v git >/dev/null || [ "$(command -v git)" = /usr/bin/git ]; }; then n+=(git); fi
  command -v jq >/dev/null || n+=(jq)
  if [ ${#n[@]} -gt 0 ]; then
    if ! command -v brew >/dev/null; then
      yes_no "${n[*]} missing: install Homebrew (https://brew.sh; asks for your password)?" || die "these are needed: ${n[*]} (or: xcode-select --install)"
      run /bin/bash -c "$(curl -fsSL https://raw.githubusercontent.com/Homebrew/install/HEAD/install.sh)"
      if [ -x /opt/homebrew/bin/brew ]; then eval "$(/opt/homebrew/bin/brew shellenv)"; fi
    fi
    yes_no "install ${n[*]} with Homebrew?" || die "these are needed: ${n[*]}"
    run brew install "${n[@]}"
  fi
  if ! command -v pdftoppm >/dev/null; then
    if command -v brew >/dev/null && no_yes "poppler (pdftoppm) lets GraphRAG read PDF pages: brew install poppler?"; then run brew install poppler
    else warn "no pdftoppm: GraphRAG skips PDF pages"; fi
  fi
  [ $DRY = 1 ] && [ ${#n[@]} -gt 0 ] && { ok "(dry run: the tools are checked after they are installed)"; return; }
  for t in git curl jq; do command -v "$t" >/dev/null || die "$t is still missing"; done
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
  host="$(scutil --get LocalHostName 2>/dev/null || hostname -s)"
  name="${OPT_NAME:-$(ask "engine name [${ENGINE:-$host}]:" "${ENGINE:-$host}")}"
  case "$name" in localhost|MacBook|MacBook-Pro|MacBook-Air|iMac|Mac-mini|Mac)
    warn "\"$name\" is a default name: two engines with one name are one engine to iter_data"
    name="$(ask "a name unique to this machine:" "")"
    [ -n "$name" ] || die "an engine name unique to this machine is needed (--name NAME)" ;;
  esac
  case "$name" in *[!A-Za-z0-9_.-]*|"") die "engine name \"$name\": letters, digits, - _ . only" ;; esac
  DATA_URL="$url" ENGINE="$name"
  secrets_settings
  save_conf
  ok "engine \"$ENGINE\" -> $DATA_URL (in $CONF)"
  if pgrep -x iter_engine >/dev/null 2>&1 && ! agent_loaded; then
    warn "an iter_engine runs here outside launchd (pid $(pgrep -x iter_engine | head -1)): one engine per machine, stop it before the agent starts"
  fi
}

save_conf() {
  run mkdir -p "$(dirname "$CONF")"
  conf_text | write_file "$CONF"
}

binary() {
  say "4/9 iter_engine ($ASSET)"
  install_binary
  path_link
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
# ~91 MB), checksum-verified; the engine finds models/ beside its bin/
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
  run mkdir -p "$(dirname "$KEYF")"
  run chmod 700 "$(dirname "$KEYF")"
  [ -f "$ENVF" ] || run touch "$ENVF"
  run chmod 600 "$ENVF"
  if [ $DRY = 1 ]; then
    if [ -f "$KEYF" ]; then ok "$KEYF exists"; else printf '    [dry-run] %s key new --key-file %s --name %s\n' "$BIN" "$KEYF" "$ENGINE"; fi
    printf '    [dry-run] wait until iter_data accepts it (%s key status)\n' "$BIN"; return
  fi
  [ -f "$KEYF" ] || "$BIN" key new --key-file "$KEYF" --name "$ENGINE" >/dev/null
  while :; do
    if "$BIN" key status --key-file "$KEYF" --data-url "$DATA_URL" > "$TMPF" 2>&1; then
      sed 's/^/    /' "$TMPF"; ok "iter_data accepts this engine's key"; return
    fi
    key_help
    case "$(ask "Press Enter once that key is added in the webui (n = make a new key instead, s = skip for now):" "")" in
      n|N) mv "$KEYF" "$KEYF.replaced-$(date +%Y%m%d%H%M%S)"; "$BIN" key new --key-file "$KEYF" --name "$ENGINE" >/dev/null
           warn "made a new key: paste the new line below instead (delete the old key in the webui if you added it)" ;;
      s|S) warn "skipped: iter_data refuses this engine until its public key is added"; return ;;
      *) tty_ok || die "iter_data does not accept the key yet (no terminal to wait at): add it, then run this again"
         printf '    checking again...\n' ;;
    esac
  done
}

# --------------------------------------------------------- browsers trust iter_data
# iter_data (--tls auto) signs its certificate with a local certificate
# authority of its own, limited by name constraints to that server's names,
# local DNS suffixes (.local .lan .internal .home.arpa) and loopback / private
# addresses: it cannot vouch for any public site. Trusting that CA in the login
# keychain (trusted root, SSL only) makes Safari, Chrome and Edge on this Mac
# show the webui as secure. Firefox uses it too while its
# security.enterprise_roots.enabled is on (the default in current Firefox).
# The CA comes over the engine's pinned connection (`iter_engine key ca`).
LOGIN_KC="$HOME/Library/Keychains/login.keychain-db"
# ca_trust FILE: add the CA certificate in FILE to the login keychain as an SSL root
ca_trust() {
  local ca="$1" cn
  cn="$(openssl x509 -in "$ca" -noout -subject -nameopt multiline 2>/dev/null | sed -n 's/^ *commonName *= *//p')"
  [ -n "$cn" ] || { warn "$ca is not a certificate"; return 1; }
  if security verify-cert -c "$ca" -p ssl -L -l >/dev/null 2>&1; then ok "\"$cn\" is trusted already (login keychain)"; return 0; fi
  if [ $DRY = 1 ]; then printf '    [dry-run] security add-trusted-cert -r trustRoot -p ssl -k %s %s\n' "$LOGIN_KC" "$ca"; return 0; fi
  printf '    macOS now asks for your password to trust "%s" for websites (login keychain)\n' "$cn"
  security add-trusted-cert -r trustRoot -p ssl -k "$LOGIN_KC" "$ca" \
    || { warn "the login keychain did not take it (password dialog cancelled, or no screen session: run this in Terminal on the Mac)"; return 1; }
  ok "login keychain: \"$cn\" trusted for SSL (restart the browser; Firefox: security.enterprise_roots.enabled, on by default)"
}

browsers() {
  say "6b/9 browsers here trust iter_data"
  case "$DATA_URL" in https://*) ;; *) ok "$DATA_URL is plain http: nothing to trust"; return ;; esac
  [ $TRUST_CA = 0 ] || { ok "iter_data has a certificate from a public authority: browsers trust it already"; return; }
  if [ $DRY = 1 ]; then printf '    [dry-run] %s key ca --data-url %s, then add it to the login keychain as trusted for SSL (macOS asks for your password)\n' "$BIN" "$DATA_URL"; return; fi
  if ! "$BIN" key ca --data-url "$DATA_URL" --out "$WORK/iter_data-ca.crt" > "$TMPF" 2>&1; then
    warn "$(tail -1 "$TMPF")"; warn "browsers here warn about iter_data's certificate until its CA is trusted (rerun this installer)"; return
  fi
  ca_trust "$WORK/iter_data-ca.crt" || warn "browsers here may still warn about iter_data's certificate"
}

# what to paste and where, and why iter_data refused the last check; printed at
# every try, so the screen always shows the one line to paste and the reason
key_help() {
  local pub id why
  pub="$("$BIN" key show --key-file "$KEYF" 2>/dev/null | head -1)"
  id="$("$BIN" key show --key-file "$KEYF" 2>/dev/null | sed -n 's/^(id \([0-9a-f]*\).*/\1/p')"
  why="$(sed -n 's/^key  *id [0-9a-f]*  *//p' "$TMPF" | head -1)"
  [ -n "$why" ] || why="$(tail -1 "$TMPF")"
  printf '\n    iter_data does not accept this engine yet: %s\n' "$why"
  case "$why" in
    *"unknown engine key"*) printf '    (iter_data has no key with id %s: it was not added yet, or a different line was pasted)\n' "$id" ;;
    *"not reachable"*) printf '    (this machine cannot reach %s: check the URL and that iter_data is running)\n' "$DATA_URL" ;;
  esac
  printf "\n    Paste this one line, this engine's public key (nothing else on this screen):\n\n      %s\n\n" "$pub"
  printf '    Where: open %s, sign in as an admin, click 🔑 Engine keys in the header, then Add.\n' "$DATA_URL"
  printf '    Paste the line above as the public key and type %s as the engine name. The list then shows key id %s.\n\n' "$ENGINE" "$id"
}

# a laptop: the agent can run under `caffeinate -s` (no system sleep while on
# power, only while the engine runs, no sudo). Closing the lid still sleeps
# the Mac unless an external display is attached. Asked once; KEEPAWAKE in
# engine.conf remembers the answer.
power() {
  say "7/9 power"
  if ! pmset -g batt 2>/dev/null | grep -q InternalBattery; then ok "not a laptop"; return; fi
  if [ -n "$KEEPAWAKE" ]; then ok "keep awake on power: $KEEPAWAKE (KEEPAWAKE in $CONF)"; return; fi
  if no_yes "this MacBook runs agents unattended: keep it awake on power while the engine runs (caffeinate -s)?"; then KEEPAWAKE=yes
  else KEEPAWAKE=no; warn "a sleeping Mac stops its agents mid-run (system-wide instead: sudo pmset -c sleep 0)"; fi
  save_conf
  ok "keep awake on power: $KEEPAWAKE"
}

plist_text() {
  local args="" a
  [ "$KEEPAWAKE" != yes ] || args="    <string>/usr/bin/caffeinate</string>
    <string>-s</string>
"
  set -- "$BIN" --data-url "$DATA_URL" --env-file "$ENVF" --key-file "$KEYF" --name "$ENGINE"
  [ -z "$SECRETS_COMMAND" ] || set -- "$@" --secrets-command "$SECRETS_COMMAND"
  [ -z "$SECRETS_EVERY" ] || set -- "$@" --secrets-every "$SECRETS_EVERY"
  for a in "$@"; do
    args="$args    <string>$(xml "$a")</string>
"
  done
  cat <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<!-- written by installers/iter_engine/macos/install.sh -->
<plist version="1.0">
<dict>
  <key>Label</key>
  <string>$LABEL</string>
  <key>ProgramArguments</key>
  <array>
${args}  </array>
  <key>WorkingDirectory</key>
  <string>$(xml "$IHOME")</string>
  <key>EnvironmentVariables</key>
  <dict>
    <key>HOME</key>
    <string>$(xml "$HOME")</string>
    <key>ITER_HOME</key>
    <string>$(xml "$IHOME")</string>
    <key>PATH</key>
    <string>$(xml "$HOME/.local/bin:/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin")</string>
  </dict>
  <key>RunAtLoad</key>
  <true/>
  <key>KeepAlive</key>
  <true/>
  <key>ThrottleInterval</key>
  <integer>10</integer>
  <key>ProcessType</key>
  <string>Standard</string>
  <key>StandardOutPath</key>
  <string>$(xml "$LOGS/engine.log")</string>
  <key>StandardErrorPath</key>
  <string>$(xml "$LOGS/engine.err.log")</string>
</dict>
</plist>
EOF
}

# (re)load the agent: bootout + bootstrap re-reads the plist; kickstart -k
# restarts a loaded one
load_agent() {
  run mkdir -p "$LOGS" "$(dirname "$PLIST")"
  local fresh=0
  if [ -f "$PLIST" ] && [ "$(plist_text)" = "$(cat "$PLIST")" ]; then ok "$PLIST is current"
  else plist_text | write_file "$PLIST"; fresh=1; fi
  if [ $DRY = 0 ]; then plutil -lint -s "$PLIST" || die "$PLIST does not parse"; fi
  if agent_loaded && [ $fresh = 0 ]; then
    run launchctl kickstart -k "$DOMAIN/$LABEL"
  else
    if agent_loaded; then run launchctl bootout "$DOMAIN/$LABEL"; fi
    run launchctl enable "$DOMAIN/$LABEL"
    run launchctl bootstrap "$DOMAIN" "$PLIST"
  fi
}
agent() {
  say "8/9 launchd agent $LABEL"
  load_agent
  [ $DRY = 1 ] && return
  sleep 3
  local pid; pid="$(agent_pid)"
  [ -n "$pid" ] || { tail -20 "$LOGS/engine.err.log" 2>/dev/null; die "the agent did not start (launchctl print $DOMAIN/$LABEL)"; }
  ok "running (pid $pid; log: $LOGS/engine.log)"
}

assignments() { # the engine's assignments into $TMPF; false when it has none yet
  [ "$(api "/api/engines/$ENGINE/assignments")" = 200 ] && jq -e '.projects | length > 0' "$TMPF" >/dev/null 2>&1
}

accounts() {
  say "account tokens"
  load_conf
  if ! assignments; then ok "no projects served yet: claude agents use this Mac's own login (run \`claude\` once and log in)"; return; fi
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
  if [ -z "${ENGINE:-}" ] && [ -f "$HOME/.iter/engine.conf" ]; then die "an older install in ~/.iter: run install.sh (or install.sh update) to move it into $IHOME"; fi
  [ -n "${ENGINE:-}" ] || die "not installed in $IHOME yet: run install.sh (another iter dir: --iter-dir)"
  local pid; pid="$(agent_pid)"
  printf 'engine   %s -> %s\n' "$ENGINE" "$DATA_URL"
  printf 'folder   %s\n' "$IHOME"
  if agent_loaded; then printf 'agent    %s loaded, %s\n' "$LABEL" "${pid:+running (pid $pid)}${pid:-not running}"; else printf 'agent    %s not loaded\n' "$LABEL"; fi
  if [ -x "$BIN" ]; then printf 'binary   %s (sha256 %s)\n' "$BIN" "$(sha "$BIN" | cut -c1-12)"; "$BIN" key status --key-file "$KEYF" --data-url "$DATA_URL" || true
  else printf 'binary   %s missing\n' "$BIN"; fi
  if assignments; then jq -r '.projects[] | "project  \(.project)  \(.topdir)  \(.state)"' "$TMPF"; else echo "project  (none served)"; fi
  tail -3 "$LOGS/engine.log" 2>/dev/null || true
}

update() {
  # where the engine is now, when not yet in $IHOME: an older ~/.iter (flat
  # files) or another iter dir; place() below moves it
  local old="" sb="$BIN" sk="$KEYF" st="$TRUSTF" sh="$ITER_HOME" d="$HOME/.iter"
  if [ -d "$d" ] && [ ! -L "$d" ] && [ -f "$d/engine.conf" ]; then
    old="$d"; CONF_READ="$d/engine.conf" BIN="$d/bin/iter_engine" KEYF="$d/engine.key" TRUSTF="$d/server_trust.json" ITER_HOME="$d"
  fi
  if [ -z "$old" ] && [ -n "$INSTALLED_DIR" ] && [ "$INSTALLED_DIR" != "$ITER_DIR" ]; then
    old="$INSTALLED_DIR/iter_engine"; CONF_READ="$old/config/engine.conf" BIN="$old/bin/iter_engine" KEYF="$old/secrets/engine.key" TRUSTF="$old/config/server_trust.json" ITER_HOME="$old"
  fi
  load_conf
  [ -n "${ENGINE:-}" ] || die "not installed yet: run install.sh"
  say "update iter_engine"
  if [ $YES = 0 ] && [ "$(api "/api/engines/$ENGINE")" = 200 ]; then
    local n; n="$(jq -r '.running // 0' "$TMPF" 2>/dev/null || echo 0)"
    [ "${n:-0}" -eq 0 ] 2>/dev/null || die "engine \"$ENGINE\" runs $n item(s) and a restart stops them: set its projects Draining and wait, or rerun with --yes"
  fi
  BIN="$sb" KEYF="$sk" TRUSTF="$st" ITER_HOME="$sh"
  if [ -n "$old" ]; then place; [ $DRY = 1 ] || CONF_READ="$CONF"; fi
  install_binary
  path_link
  # a moved folder or a new layout needs a new plist; else a new binary only a restart
  if [ -n "$old" ] || { [ -f "$PLIST" ] && [ "$(plist_text)" != "$(cat "$PLIST")" ]; }; then
    load_agent; ok "agent reloaded from $PLIST"
  elif [ $CHANGED = 1 ] && agent_loaded; then
    run launchctl kickstart -k "$DOMAIN/$LABEL"
    ok "agent restarted"
  fi
}

uninstall() {
  say "uninstall the iter engine"
  if agent_loaded; then run launchctl bootout "$DOMAIN/$LABEL"; ok "agent unloaded"; else ok "agent not loaded"; fi
  if [ -f "$PLIST" ]; then run rm -f "$PLIST"; ok "removed $PLIST"; fi
  if [ -e "$BIN" ]; then run rm -f "$BIN"; ok "removed $BIN"; fi
  if [ -L "$LINK" ] && [ "$(readlink "$LINK")" = "$BIN" ]; then
    if [ -w "$(dirname "$LINK")" ]; then run rm -f "$LINK"; else run sudo rm -f "$LINK"; fi
    ok "removed the link $LINK"
  fi
  ok "kept $IHOME (config/, secrets/ with the key and tokens, models/, logs/) and the project checkouts; delete them by hand when this Mac is done with iter"
  ok "in the webui, remove the engine's key (Engine keys) and its serves edges"
}

case "$VERB" in
  install)
    place; tools; claude_cli; settings; binary; trust; model; key; browsers; power; agent
    say "9/9 accounts and projects"
    accounts; projects
    [ $DRY = 0 ] || { printf '\nDry run done: nothing was changed.\n'; exit 0; }
    cat <<EOF

Done. Engine "$ENGINE" runs as the launchd agent $LABEL and checks in with $DATA_URL.
Next, in the webui's Settings tab: draw a serves edge from $ENGINE to each project
(topdir ~/dev/<project>), then run \`install.sh projects\` here to clone them.
  status:  install.sh status        log:  install.sh logs  ($LOGS/engine.log)
  update:  install.sh update        a new key:  iter_engine key rotate   (its public key: iter_engine key show)
  files:   $IHOME   (config/, secrets/, models/, logs/)
EOF
    ;;
  projects) projects ;;
  accounts) accounts ;;
  status) status ;;
  update) update ;;
  logs) run tail -n 50 -F "$LOGS/engine.log" "$LOGS/engine.err.log" ;;
  uninstall) uninstall ;;
esac
