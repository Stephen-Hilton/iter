#!/usr/bin/env bash
# upgrade_iter.sh - upgrade every iter program installed on this machine to one
# release, using that release's own installers, then check the result and say
# what is left for a person. Safe to run again: a program already at the
# release is left alone.
#
#   curl -fsSL https://raw.githubusercontent.com/Stephen-Hilton/iter/main/installers/upgrade_iter.sh | bash
#   installers/upgrade_iter.sh [options]
#
# What it finds and upgrades:
#   iter_data   the systemd services from installers/iter_data/linux (iter-data + arangodb3)
#   iter_engine the systemd service from installers/iter_engine/linux (linux_wsl under WSL),
#               or the launchd agent from installers/iter_engine/macos
# Docker installs (containers iter / iter-engine) are only reported: rerun their
# install.sh. Windows: installers/iter_engine/windows/install.ps1 -Update.
#
# Everything is kept: the database (backed up first), iter_data's config, secrets,
# port, memory cap and TLS certificate (so engines' pinned fingerprint), and the
# engine's key, pinned certificate, config, tokens, model and project checkouts.
# Only the binaries and service files are replaced.
#
# Steps:
#   1. what is installed, and the release: --version, else the latest; its SHA256SUMS
#   2. each installed binary is compared with SHA256SUMS: equal = skipped
#   3. each installer is fetched from the release's tag (so script and binary match)
#   4. sudo, once; then the engine's own update runs as a dry run: it refuses while the
#      engine runs work (asked on a terminal, else --yes), and nothing has changed yet
#   5. iter_data: a backup (install.sh backup), then install.sh, which waits for /health
#   6. iter_engine: install.sh update (--full: install --yes, which also repairs the rest,
#      e.g. makes this machine's browsers trust iter_data's certificate authority)
#   7. checks: binaries match the release, services active, the engine key accepted,
#      the account tokens the served projects name are set (names only), iter_data
#      answers as a trusted https site; then the next steps for a person, if any
# iter_data goes first, then the engine, so the two run the same iter_core again
# within a minute. A failed iter_data step stops before the engine is touched.
#
# Options:
#   --version TAG   upgrade to this release, e.g. v2026.10.20 [latest]
#   --only data|engine   upgrade only this program
#   --full          rerun the engine's whole install (--yes), not only its update
#   --yes           do not ask: restart the engine even while it runs work (its items are retried)
#   --no-backup     skip the iter_data backup before its upgrade
#   --force         reinstall a program even when its binary already matches
#   --log FILE|none append this run's output, colors and passwords stripped
#                   [<iter dir>/iter_engine/logs/upgrade_iter.log when the engine is here]
#   --dry-run       print what would happen; change nothing
#   -h, --help      this text
# Token values, passwords and keys are never read or printed: only token NAMES are checked.
set -euo pipefail

main() {
REPO="Stephen-Hilton/iter"
RAW="https://raw.githubusercontent.com/$REPO"
VERSION="${ITER_VERSION:-}" ONLY="" FULL=0 YES=0 BACKUP=1 FORCE=0 DRY=0 LOG=""

say()  { printf '\n\033[1m==>\033[0m %s\n' "$*"; }
ok()   { printf '    \033[32mok\033[0m  %s\n' "$*"; }
warn() { printf '    \033[33m!!\033[0m  %s\n' "$*" >&2; }
die()  { printf '\n\033[31mstopped:\033[0m %s\n' "$*" >&2; exit 1; }
usage() { sed -n '2,/^set -euo/p' "${BASH_SOURCE[0]:-$0}" 2>/dev/null | sed -e '$d' -e 's/^# \{0,1\}//' || echo "see installers/README.md"; }
sha() { if command -v sha256sum >/dev/null; then sha256sum "$1" | cut -d' ' -f1; else shasum -a 256 "$1" | cut -d' ' -f1; fi; }
mtime() { date -r "$1" '+%Y-%m-%d %H:%M' 2>/dev/null || echo '?'; }
tty_ok() { [ -r /dev/tty ] && { : < /dev/tty; } 2>/dev/null; }

while [ $# -gt 0 ]; do
  case "$1" in
    --version) VERSION="${2:?--version needs a tag}"; shift ;;
    --only) ONLY="${2:?--only needs data or engine}"; shift ;;
    --full) FULL=1 ;;
    --yes|-y) YES=1 ;;
    --no-backup) BACKUP=0 ;;
    --force) FORCE=1 ;;
    --log) LOG="${2:?--log needs a file (or none)}"; shift ;;
    --dry-run) DRY=1 ;;
    -h|--help|help) usage; exit 0 ;;
    *) printf 'unknown argument %s (try --help)\n' "$1" >&2; exit 2 ;;
  esac
  shift
done
case "$ONLY" in ""|data|engine) ;; *) die "--only data or --only engine" ;; esac
[ "$(id -u)" != 0 ] || die "run this as your own user, not root (the installers use sudo where they must, and the engine's refuses root)"
command -v curl >/dev/null || die "curl is missing"

TMP="$(mktemp -d)"
KEEPALIVE=""
trap '[ -z "$KEEPALIVE" ] || kill "$KEEPALIVE" 2>/dev/null; rm -rf "$TMP"' EXIT

# ------------------------------------------------------------------ this machine
OS="$(uname -s)"
case "$(uname -m)" in x86_64|amd64) ARCH=x86_64 ;; aarch64|arm64) ARCH=aarch64 ;; *) die "no iter build for $(uname -m)" ;; esac
DATA_BIN="" ENGINE_BIN="" ENGINE_DIR="" PORT=8400
case "$OS" in
  Linux)
    # each unit's ExecStart starts with the binary (the engine's may be the PATH link)
    if [ -e /etc/systemd/system/iter-data.service ]; then
      DATA_BIN="$(sed -n 's|^ExecStart=\(/[^ ]*\) .*|\1|p' /etc/systemd/system/iter-data.service | head -1)"
      PORT="$(sed -n 's/.*--listen [0-9.]*:\([0-9]*\).*/\1/p' /etc/systemd/system/iter-data.service | head -1)"; PORT="${PORT:-8400}"
    fi
    if [ -e /etc/systemd/system/iter-engine.service ]; then
      ENGINE_BIN="$(sed -n 's|^ExecStart=\(/[^ ]*\) .*|\1|p' /etc/systemd/system/iter-engine.service | head -1)"
      ENGINE_BIN="$(readlink -f "$ENGINE_BIN" 2>/dev/null || printf '%s' "$ENGINE_BIN")"
      if grep -qi microsoft /proc/version 2>/dev/null; then ENGINE_DIR=linux_wsl; else ENGINE_DIR=linux; fi
    fi
    ;;
  Darwin)
    local plist="$HOME/Library/LaunchAgents/com.iter.engine.plist"
    if [ -f "$plist" ]; then
      ENGINE_BIN="$(/usr/libexec/PlistBuddy -c 'Print :ProgramArguments:0' "$plist" 2>/dev/null || true)"
      ENGINE_DIR=macos
    fi
    ;;
  *) die "this script is for Linux and macOS (Windows: installers/iter_engine/windows/install.ps1 -Update)" ;;
esac
[ "$ONLY" != engine ] || DATA_BIN=""
[ "$ONLY" != data ] || ENGINE_BIN=""
[ -n "$DATA_BIN$ENGINE_BIN" ] || die "no iter_data or iter_engine service found on this machine${ONLY:+ (--only $ONLY)}; a first install: installers/README.md"
[ "$OS" = Linux ] || [ -z "$DATA_BIN" ] || die "iter_data runs only on Linux"
# the engine's folder (<iter dir>/iter_engine) and what its config says
IHOME="" ECONF="" ENVF=""
if [ -n "$ENGINE_BIN" ]; then IHOME="${ENGINE_BIN%/bin/iter_engine}"; ECONF="$IHOME/config/engine.conf"; ENVF="$IHOME/secrets/.env"; fi
confval() { [ -n "$ECONF" ] && [ -f "$ECONF" ] && grep -E "^$1=" "$ECONF" | tail -1 | cut -d= -f2- || true; }
ENGINE="$(confval ENGINE)" DATA_URL="$(confval DATA_URL)"

# the log: this run's output, colors stripped, a printed password masked
if [ -z "$LOG" ] && [ -n "$IHOME" ] && [ -w "$IHOME/logs" ]; then LOG="$IHOME/logs/upgrade_iter.log"; fi
[ "$LOG" != none ] || LOG=""
if [ -n "$LOG" ] && [ $DRY = 0 ]; then
  exec > >(tee >(sed -u -E 's/\x1b\[[0-9;]*[A-Za-z]//g; s/((admin|first-boot) password[^:]*: ).*/\1[shown on screen only]/' >> "$LOG")) 2>&1
  printf '\n================ upgrade_iter.sh %s on %s ================\n' "$(date '+%Y-%m-%dT%H:%M:%S%z')" "$(uname -n)"
fi
[ $DRY = 0 ] || printf '\033[1mdry run:\033[0m every action is printed, nothing is changed\n'
if command -v docker >/dev/null && docker info >/dev/null 2>&1; then
  local c
  for c in $(docker ps -a --format '{{.Names}}' 2>/dev/null | grep -xE 'iter|iter-engine' || true); do
    warn "docker container $c is not upgraded here: rerun its install.sh (installers/iter_*/docker)"
  done
fi

# ------------------------------------------------------------------ the release
say "1. the release"
if [ -z "$VERSION" ]; then
  # github.com/<repo>/releases/latest redirects to .../releases/tag/<tag> (no API rate limit)
  VERSION="$(curl -fsSLI -o /dev/null -w '%{url_effective}' "https://github.com/$REPO/releases/latest" 2>/dev/null | sed -n 's|.*/releases/tag/||p')"
  [ -n "$VERSION" ] || die "could not find the latest release of $REPO (pass --version vYYYY.MM.DD)"
fi
case "$VERSION" in v[0-9]*) ;; *) die "--version $VERSION: a release tag like v2026.10.20" ;; esac
BASE="https://github.com/$REPO/releases/download/$VERSION"
curl -fsSL --retry 3 -o "$TMP/SHA256SUMS" "$BASE/SHA256SUMS" || die "release $VERSION has no SHA256SUMS ($BASE)"
ok "$VERSION"
want() { awk -v a="$1" '$2==a || $2=="*"a {print $1}' "$TMP/SHA256SUMS"; }

# ------------------------------------------------------------------ what needs it
say "2. installed programs"
DO_DATA=0 DO_ENGINE=0 OLD_DATA="" OLD_ENGINE=""
DATA_ASSET="iter_data-linux-$ARCH"
case "$OS" in Linux) ENGINE_ASSET="iter_engine-linux-$ARCH" ;; *) ENGINE_ASSET="iter_engine-macos-$ARCH" ;; esac
check() { # check NAME BIN ASSET -> 0 when it needs the upgrade
  local w; w="$(want "$3")"
  [ -n "$w" ] || die "$3 is not in $VERSION's SHA256SUMS"
  if [ ! -f "$2" ]; then warn "$1: $2 is missing; its installer puts it back"; return 0; fi
  if [ "$(sha "$2")" = "$w" ]; then
    if [ $FORCE = 1 ] || { [ "$1" = iter_engine ] && [ $FULL = 1 ]; }; then ok "$1 ($2) is already $VERSION; reinstalling"; return 0; fi
    ok "$1 ($2) is already $VERSION"; return 1
  fi
  ok "$1 ($2, installed $(mtime "$2")) -> $VERSION"
}
if [ -n "$DATA_BIN" ]; then OLD_DATA="$(mtime "$DATA_BIN")"; if check iter_data "$DATA_BIN" "$DATA_ASSET"; then DO_DATA=1; fi; fi
if [ -n "$ENGINE_BIN" ]; then
  OLD_ENGINE="$(mtime "$ENGINE_BIN")"
  if check iter_engine "$ENGINE_BIN" "$ENGINE_ASSET"; then DO_ENGINE=1; fi
  [ -z "$ENGINE" ] || ok "engine \"$ENGINE\" -> $DATA_URL"
fi

if [ $DO_DATA = 1 ] || [ $DO_ENGINE = 1 ]; then
# ------------------------------------------------------------------ the installers
say "3. the installers, from tag $VERSION"
fetch() { # fetch FOLDER -> $TMP/FOLDER/install.sh
  mkdir -p "$TMP/$1"
  curl -fsSL --retry 3 -o "$TMP/$1/install.sh" "$RAW/$VERSION/installers/$1/install.sh" \
    || die "could not fetch installers/$1/install.sh at $VERSION"
  bash -n "$TMP/$1/install.sh" || die "installers/$1/install.sh at $VERSION does not parse"
  ok "installers/$1/install.sh"
}
[ $DO_DATA = 0 ] || fetch iter_data/linux
[ $DO_ENGINE = 0 ] || fetch "iter_engine/$ENGINE_DIR"
# piped from curl, our stdin is this script: the installers read the terminal, never it
ENGINE_SH="$TMP/iter_engine/$ENGINE_DIR/install.sh" DATA_SH="$TMP/iter_data/linux/install.sh"

# ------------------------------------------------------------------ sudo + may the engine restart?
say "4. before anything changes"
if [ $DRY = 0 ] && { [ $DO_DATA = 1 ] || [ "$OS" = Linux ]; }; then
  sudo -v || die "sudo did not accept the password"
  # keep sudo's timestamp fresh while the installers run, so it asks only this once
  ( while sleep 50; do kill -0 $$ 2>/dev/null || exit 0; sudo -n true 2>/dev/null || exit 0; done ) &
  KEEPALIVE=$!
  ok "sudo"
fi
if [ $DO_ENGINE = 1 ] && [ $YES = 0 ]; then
  local out
  if ! out="$(bash "$ENGINE_SH" update --version "$VERSION" --dry-run </dev/null 2>&1)"; then
    if printf '%s' "$out" | grep -q 'runs [0-9]* item' && tty_ok; then
      warn "$(printf '%s\n' "$out" | grep -o 'engine "[^"]*" runs [0-9]* item(s)' | head -1): the upgrade restarts it and stops them (they are retried later)"
      local ans=""
      printf '    Upgrade anyway? [y/N] ' > /dev/tty; IFS= read -r ans < /dev/tty || true
      case "$ans" in [yY]*) YES=1 ;; *) die "nothing changed: rerun when the engine is idle (set its projects Draining), or with --yes" ;; esac
    else
      printf '%s\n' "$out" | tail -5 >&2
      die "the engine's update refused (above); nothing was changed. Set its projects Draining and rerun, or rerun with --yes"
    fi
  else ok "the engine may restart now (no work running)"; fi
fi
YES_ARG=(); [ $YES = 0 ] || YES_ARG=(--yes)
if [ $DRY = 1 ]; then
  if [ $DO_DATA = 1 ]; then
    say "5. iter_data's installer, as a dry run"
    bash "$DATA_SH" install --version "$VERSION" --dry-run </dev/null || die "iter_data's installer refused its dry run (above)"
  fi
  say "dry run done: would upgrade$([ $DO_DATA = 0 ] || echo ' iter_data')$([ $DO_ENGINE = 0 ] || echo ' iter_engine') to $VERSION"
  return 0
fi

# ------------------------------------------------------------------ iter_data
if [ $DO_DATA = 1 ]; then
  if [ $BACKUP = 1 ]; then
    say "5a. iter_data backup"
    bash "$DATA_SH" backup </dev/null || die "the backup failed; nothing was upgraded (--no-backup skips it)"
  fi
  say "5b. iter_data -> $VERSION"
  bash "$DATA_SH" install --version "$VERSION" </dev/null || die "iter_data's installer failed (above); the engine was not touched. Fix it and rerun this script"
fi

# ------------------------------------------------------------------ iter_engine
if [ $DO_ENGINE = 1 ]; then
  if [ $FULL = 1 ]; then
    say "6. iter_engine -> $VERSION (its whole install, --yes)"
    bash "$ENGINE_SH" install --version "$VERSION" --yes </dev/null \
      || die "the engine's install failed (above)$([ $DO_DATA = 0 ] || echo '; iter_data is already upgraded'). Rerun this script: it skips what is done"
  else
    say "6. iter_engine -> $VERSION"
    bash "$ENGINE_SH" update --version "$VERSION" ${YES_ARG[@]+"${YES_ARG[@]}"} </dev/null \
      || die "the engine's update failed (above)$([ $DO_DATA = 0 ] || echo '; iter_data is already upgraded'). Rerun this script: it skips what is done"
  fi
fi
fi  # anything to upgrade

# ------------------------------------------------------------------ check
say "7. check"
local bad=0 s n=0 key_ok=0 key_line="" missing="" set_ok="" tok trusted="" cert=""
next() { n=$((n + 1)); NEXT+=("$n. $*"); }
NEXT=()
if [ -n "$DATA_BIN" ]; then
  if [ "$(sha "$DATA_BIN")" = "$(want "$DATA_ASSET")" ]; then ok "iter_data is $VERSION (binary was $OLD_DATA)"; else warn "iter_data's binary does not match $VERSION"; bad=1; fi
  for s in arangodb3 iter-data; do
    [ -e "/etc/systemd/system/$s.service" ] || continue
    if systemctl is-active --quiet "$s"; then ok "$s active"; else warn "$s is $(systemctl is-active "$s" 2>/dev/null || true): journalctl -u $s"; bad=1; fi
  done
  # without -k: does this machine trust iter_data's certificate (its local CA in the system store)?
  if curl -fsS --max-time 5 -o /dev/null "https://127.0.0.1:$PORT/health" 2>/dev/null; then trusted=1; ok "https://127.0.0.1:$PORT answers as a trusted site"
  elif curl -fsSk --max-time 5 -o /dev/null "https://127.0.0.1:$PORT/health" 2>/dev/null; then
    warn "https://127.0.0.1:$PORT answers, but this machine does not trust its certificate yet"
    next "Browsers here do not trust iter_data's certificate: rerun this script with --full (or iter_data/linux/install.sh), then restart the browser."
  fi
  # an iter_data from before its local CA keeps its self-signed certificate until it is switched
  local st="${DATA_BIN%/bin/iter_data}/state"
  if sudo -n test -f "$st/tls-ca.crt" 2>/dev/null && ! sudo -n openssl verify -CAfile "$st/tls-ca.crt" "$st/tls.crt" >/dev/null 2>&1 \
     && sudo -n test -f "$st/tls.next.crt" 2>/dev/null; then
    cert="iter_data still serves its old self-signed certificate; a CA-signed next one is ready"
    warn "$cert"
    next "Once every engine has signed in since this upgrade, open the webui's 🔑 Engine keys screen and switch to the next certificate."
  fi
fi
if [ -n "$ENGINE_BIN" ]; then
  if [ "$(sha "$ENGINE_BIN")" = "$(want "$ENGINE_ASSET")" ]; then ok "iter_engine is $VERSION (binary was $OLD_ENGINE)"; else warn "iter_engine's binary does not match $VERSION"; bad=1; fi
  if [ "$OS" = Linux ]; then
    sleep 3
    if systemctl is-active --quiet iter-engine; then ok "iter-engine active"; else warn "iter-engine is $(systemctl is-active iter-engine 2>/dev/null || true): journalctl -u iter-engine"; bad=1; fi
  elif launchctl print "gui/$(id -u)/com.iter.engine" >/dev/null 2>&1; then ok "com.iter.engine loaded"
  else warn "com.iter.engine is not loaded"; bad=1; fi
  if [ -n "$DATA_URL" ]; then
    if ITER_HOME="$IHOME" "$ENGINE_BIN" key status --data-url "$DATA_URL" >/dev/null 2>&1; then key_ok=1; ok "iter_data accepts engine \"$ENGINE\"'s key"
    else
      key_line="$(ITER_HOME="$IHOME" "$ENGINE_BIN" key show 2>/dev/null | grep -o 'iter_[0-9a-f]*' | head -1 || true)"
      warn "iter_data does not accept engine \"$ENGINE\"'s key"
      next "Add this engine's key: open $DATA_URL as an admin, 🔑 Engine keys -> Add, paste ${key_line:-the line \`$ENGINE_BIN key show\` prints} and type $ENGINE as the engine name."
    fi
  fi
  # the token NAMES the served projects' accounts use, and which the env file lacks (no values read);
  # the session token goes only over a verified connection: iter_data's CA (fetched over the
  # engine's pinned connection), else the system's trust store, never curl -k
  local cacert=()
  if ITER_HOME="$IHOME" "$ENGINE_BIN" key ca --data-url "$DATA_URL" --out "$TMP/ca.pem" >/dev/null 2>&1; then cacert=(--cacert "$TMP/ca.pem"); fi
  if [ $key_ok = 1 ] && [ -z "$(confval SECRETS_COMMAND)" ] && command -v jq >/dev/null \
     && tok="$(ITER_HOME="$IHOME" "$ENGINE_BIN" key session --data-url "$DATA_URL" 2>/dev/null)" \
     && curl -fsS --max-time 15 ${cacert[@]+"${cacert[@]}"} -H "authorization: Bearer $tok" -o "$TMP/assign.json" "$DATA_URL/api/engines/$ENGINE/assignments" 2>/dev/null; then
    local v
    for v in $(jq -r '[.accounts[]?.token_envar, .projects[]?.accounts[]?.token_envar] | map(select(. != null and . != "")) | unique | .[]' "$TMP/assign.json" 2>/dev/null); do
      if grep -qE "^[[:space:]]*(export[[:space:]]+)?$v=." "$ENVF" 2>/dev/null; then set_ok="$set_ok $v"; else missing="$missing $v"; fi
    done
    if [ -n "$set_ok$missing" ]; then ok "account tokens set:${set_ok:- none}; missing:${missing:- none}"; else ok "no account tokens named yet (no project served?)"; fi
    for v in $missing; do next "Account token $v is not set: add $v=<token> to $ENVF (\`claude setup-token\` while signed in to that account); the engine rereads it without a restart."; done
  fi
  unset tok
fi
{ [ -n "$trusted" ] && [ $((DO_DATA + FULL)) -gt 0 ]; } && next "Restart the browser (quit it fully), then open https://127.0.0.1:$PORT: it shows as secure, with no warning."
[ $bad = 0 ] || die "the checks above failed${LOG:+ (log: $LOG)}"
say "done: this machine runs $VERSION"
if [ ${#NEXT[@]} -gt 0 ]; then say "next steps"; printf '    %s\n' "${NEXT[@]}"; fi
[ -z "$LOG" ] || printf '\n    log: %s\n' "$LOG"
}

main "$@"
