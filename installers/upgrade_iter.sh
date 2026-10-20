#!/usr/bin/env bash
# upgrade_iter.sh - upgrade every iter program installed on this machine to one
# release, using that release's own installers. Safe to run again: a program
# already at the release is left alone.
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
# Steps:
#   1. the release: --version, else the latest GitHub Release; its SHA256SUMS
#   2. each installed program's binary is compared with SHA256SUMS: equal = skipped
#   3. each installer is fetched from the release's tag (so script and binary match)
#   4. the engine's own update runs as a dry run first: it refuses while the engine
#      runs work, and then nothing has been changed yet
#   5. iter_data: a backup (install.sh backup), then install.sh, which keeps the
#      secrets, the database, the TLS certificate (engines' pinned fingerprint),
#      the port and the memory cap, and waits for /health
#   6. iter_engine: install.sh update (swaps the binary, restarts the service)
#   7. checks each binary now matches the release and each service is active
# iter_data goes first, then the engine, so the two run the same iter_core again
# within a minute.
#
# Options:
#   --version TAG   upgrade to this release, e.g. v2026.10.19 [latest]
#   --only data|engine   upgrade only this program
#   --yes           restart the engine even while it runs work (its items are requeued)
#   --no-backup     skip the iter_data backup before its upgrade
#   --force         reinstall a program even when its binary already matches
#   --dry-run       print what would happen; change nothing
#   -h, --help      this text
set -euo pipefail

main() {
REPO="Stephen-Hilton/iter"
RAW="https://raw.githubusercontent.com/$REPO"
VERSION="${ITER_VERSION:-}" ONLY="" YES=0 BACKUP=1 FORCE=0 DRY=0

say()  { printf '\n\033[1m==>\033[0m %s\n' "$*"; }
ok()   { printf '    \033[32mok\033[0m  %s\n' "$*"; }
warn() { printf '    \033[33m!!\033[0m  %s\n' "$*" >&2; }
die()  { printf '\n\033[31mstopped:\033[0m %s\n' "$*" >&2; exit 1; }
usage() { sed -n '2,/^set -euo/p' "${BASH_SOURCE[0]:-$0}" 2>/dev/null | sed -e '$d' -e 's/^# \{0,1\}//' || echo "see installers/README.md"; }
sha() { if command -v sha256sum >/dev/null; then sha256sum "$1" | cut -d' ' -f1; else shasum -a 256 "$1" | cut -d' ' -f1; fi; }

while [ $# -gt 0 ]; do
  case "$1" in
    --version) VERSION="${2:?--version needs a tag}"; shift ;;
    --only) ONLY="${2:?--only needs data or engine}"; shift ;;
    --yes|-y) YES=1 ;;
    --no-backup) BACKUP=0 ;;
    --force) FORCE=1 ;;
    --dry-run) DRY=1 ;;
    -h|--help|help) usage; exit 0 ;;
    *) printf 'unknown argument %s (try --help)\n' "$1" >&2; exit 2 ;;
  esac
  shift
done
case "$ONLY" in ""|data|engine) ;; *) die "--only data or --only engine" ;; esac
[ "$(id -u)" != 0 ] || die "run this as your own user, not root (the installers use sudo where they must, and the engine's refuses root)"
command -v curl >/dev/null || die "curl is missing"
[ $DRY = 0 ] || printf '\033[1mdry run:\033[0m every action is printed, nothing is changed\n'

TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

# ------------------------------------------------------------------ this machine
OS="$(uname -s)"
case "$(uname -m)" in x86_64|amd64) ARCH=x86_64 ;; aarch64|arm64) ARCH=aarch64 ;; *) die "no iter build for $(uname -m)" ;; esac
DATA_BIN="" ENGINE_BIN="" ENGINE_DIR=""
case "$OS" in
  Linux)
    # each unit's ExecStart starts with the binary (the engine's may be the PATH link)
    if [ -e /etc/systemd/system/iter-data.service ]; then
      DATA_BIN="$(sed -n 's|^ExecStart=\(/[^ ]*\) .*|\1|p' /etc/systemd/system/iter-data.service | head -1)"
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
if command -v docker >/dev/null && docker info >/dev/null 2>&1; then
  local c
  for c in $(docker ps -a --format '{{.Names}}' 2>/dev/null | grep -xE 'iter|iter-engine' || true); do
    warn "docker container $c is not upgraded here: rerun its install.sh (installers/iter_*/docker)"
  done
fi
[ "$ONLY" != engine ] || DATA_BIN=""
[ "$ONLY" != data ] || ENGINE_BIN=""
[ -n "$DATA_BIN$ENGINE_BIN" ] || die "no iter_data or iter_engine service found on this machine${ONLY:+ (--only $ONLY)}"
[ "$OS" = Linux ] || [ -z "$DATA_BIN" ] || die "iter_data runs only on Linux"

# ------------------------------------------------------------------ the release
say "1. the release"
if [ -z "$VERSION" ]; then
  # github.com/<repo>/releases/latest redirects to .../releases/tag/<tag> (no API rate limit)
  VERSION="$(curl -fsSLI -o /dev/null -w '%{url_effective}' "https://github.com/$REPO/releases/latest" 2>/dev/null | sed -n 's|.*/releases/tag/||p')"
  [ -n "$VERSION" ] || die "could not find the latest release of $REPO (pass --version vYYYY.MM.DD)"
fi
case "$VERSION" in v[0-9]*) ;; *) die "--version $VERSION: a release tag like v2026.10.19" ;; esac
BASE="https://github.com/$REPO/releases/download/$VERSION"
curl -fsSL --retry 3 -o "$TMP/SHA256SUMS" "$BASE/SHA256SUMS" || die "release $VERSION has no SHA256SUMS ($BASE)"
ok "$VERSION"
want() { awk -v a="$1" '$2==a || $2=="*"a {print $1}' "$TMP/SHA256SUMS"; }

# ------------------------------------------------------------------ what needs it
say "2. installed programs"
DO_DATA=0 DO_ENGINE=0
DATA_ASSET="iter_data-linux-$ARCH"
case "$OS" in Linux) ENGINE_ASSET="iter_engine-linux-$ARCH" ;; *) ENGINE_ASSET="iter_engine-macos-$ARCH" ;; esac
check() { # check NAME BIN ASSET -> 0 when it needs the upgrade
  local w; w="$(want "$3")"
  [ -n "$w" ] || die "$3 is not in $VERSION's SHA256SUMS"
  if [ ! -f "$2" ]; then warn "$1: $2 is missing; its installer puts it back"; return 0; fi
  if [ "$(sha "$2")" = "$w" ]; then
    if [ $FORCE = 1 ]; then ok "$1 ($2) is already $VERSION; reinstalling (--force)"; return 0; fi
    ok "$1 ($2) is already $VERSION"; return 1
  fi
  ok "$1 ($2, installed $(date -r "$2" '+%Y-%m-%d %H:%M' 2>/dev/null || echo '?')) -> $VERSION"
}
if [ -n "$DATA_BIN" ] && check iter_data "$DATA_BIN" "$DATA_ASSET"; then DO_DATA=1; fi
if [ -n "$ENGINE_BIN" ] && check iter_engine "$ENGINE_BIN" "$ENGINE_ASSET"; then DO_ENGINE=1; fi
if [ $DO_DATA = 0 ] && [ $DO_ENGINE = 0 ]; then say "nothing to do: this machine already runs $VERSION"; return 0; fi

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
YES_ARG=(); [ $YES = 0 ] || YES_ARG=(--yes)

# ------------------------------------------------------------------ may the engine restart?
if [ $DO_ENGINE = 1 ]; then
  say "4. may the engine restart now? (its installer's update, as a dry run)"
  if ! out="$(bash "$ENGINE_SH" update --version "$VERSION" --dry-run ${YES_ARG[@]+"${YES_ARG[@]}"} </dev/null 2>&1)"; then
    printf '%s\n' "$out" | tail -5 >&2
    die "the engine's update refused (above); nothing was changed. Set its projects Draining and rerun, or rerun with --yes"
  fi
  ok "yes"
fi
if [ $DRY = 1 ] && [ $DO_DATA = 1 ]; then
  say "5. iter_data's installer, as a dry run"
  bash "$DATA_SH" install --version "$VERSION" --dry-run </dev/null || die "iter_data's installer refused its dry run (above)"
fi
[ $DRY = 0 ] || { say "dry run done: would upgrade$([ $DO_DATA = 0 ] || echo ' iter_data')$([ $DO_ENGINE = 0 ] || echo ' iter_engine') to $VERSION"; return 0; }

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
  say "6. iter_engine -> $VERSION"
  bash "$ENGINE_SH" update --version "$VERSION" ${YES_ARG[@]+"${YES_ARG[@]}"} </dev/null \
    || die "the engine's update failed (above)$([ $DO_DATA = 0 ] || echo '; iter_data is already upgraded'). Rerun this script: it skips what is done"
fi

# ------------------------------------------------------------------ check
say "7. check"
local bad=0 s
if [ $DO_DATA = 1 ]; then
  if [ "$(sha "$DATA_BIN")" = "$(want "$DATA_ASSET")" ]; then ok "iter_data is $VERSION"; else warn "iter_data's binary does not match $VERSION"; bad=1; fi
  for s in arangodb3 iter-data; do
    [ -e "/etc/systemd/system/$s.service" ] || continue
    if systemctl is-active --quiet "$s"; then ok "$s active"; else warn "$s is $(systemctl is-active "$s" 2>/dev/null || true)"; bad=1; fi
  done
fi
if [ $DO_ENGINE = 1 ]; then
  if [ "$(sha "$ENGINE_BIN")" = "$(want "$ENGINE_ASSET")" ]; then ok "iter_engine is $VERSION"; else warn "iter_engine's binary does not match $VERSION"; bad=1; fi
  if [ "$OS" = Linux ]; then
    sleep 3
    if systemctl is-active --quiet iter-engine; then ok "iter-engine active"; else warn "iter-engine is $(systemctl is-active iter-engine 2>/dev/null || true): journalctl -u iter-engine"; bad=1; fi
  elif launchctl print "gui/$(id -u)/com.iter.engine" >/dev/null 2>&1; then ok "com.iter.engine loaded"
  else warn "com.iter.engine is not loaded"; bad=1; fi
fi
[ $bad = 0 ] || die "upgraded, but the checks above failed"
say "done: this machine runs $VERSION"
}

main "$@"
