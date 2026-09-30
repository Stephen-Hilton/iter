#!/usr/bin/env bash
# iter_engine_setup.sh - set up (and start) an iter engine for one project, on
# the machine that holds the project's code.
#
# The webui's "Start a new project" wizard (and its "Engine setup" page) prints
# this command with every value filled in. Get the script from the iter server
# itself, or from GitHub:
#
#   curl -fsSLo iter_engine_setup.sh http://127.0.0.1:8300/iter_engine_setup.sh
#   curl -fsSLo iter_engine_setup.sh https://raw.githubusercontent.com/Stephen-Hilton/iter/main/iter4/tools/iter_engine_setup.sh
#
#   bash iter_engine_setup.sh --data-url http://127.0.0.1:8300 --project shop-api \
#        --engine Engine01 --topdir ~/dev/shop-api --token <engine token> --start
#
# What it does, in order (each step says what it found; nothing is overwritten):
#   1. checks the tools: git, curl, and the claude CLI the agents run in
#   2. finds iter_engine (--bin, $ITER_ENGINE_BIN, PATH, ~/.iter/bin), else
#      builds it from GitHub with cargo into ~/.iter/bin
#   3. creates the checkout folder if missing (offers `git init` when it is not
#      inside a git repository: the engine commits the agents' work)
#   4. checks the server answers and the engine token signs in
#   5. scaffolds the project files (`iter_engine cli init`): main.iter.md,
#      .iter/config.json, reqs/, interfaces/, usecases/ - existing files are kept
#   6. writes .env (the engine token + one Claude token per account the project
#      lists), keeps it out of git
#   7. with --start: starts the engine in the background and waits for it to
#      check in with the server
#
# Other verbs, run in the checkout folder (or with --topdir):
#   bash iter_engine_setup.sh --status     is the engine process running here?
#   bash iter_engine_setup.sh --stop       stop the engine started here
set -euo pipefail

DATA_URL="" PROJECT="" ENGINE="" TOPDIR="" TOKEN="${ITER_ENGINE_TOKEN:-}" DESC=""
ACCOUNTS="" ENV_FROM="" BIN="${ITER_ENGINE_BIN:-}" START=0 VERB="setup" ASSUME_YES=0
SRC_REPO="${ITER_SRC_REPO:-https://github.com/Stephen-Hilton/iter.git}"
HOME_DIR="${ITER_HOME:-$HOME/.iter}"

usage() { sed -n '2,33p' "$0" | sed 's/^# \{0,1\}//'; exit "${1:-0}"; }
say()  { printf '\033[1m==>\033[0m %s\n' "$*"; }
ok()   { printf '    \033[32mok\033[0m  %s\n' "$*"; }
warn() { printf '    \033[33m!!\033[0m  %s\n' "$*" >&2; }
die()  { printf '\n\033[31mstopped:\033[0m %s\n' "$*" >&2; exit 1; }
tty_ok() { [ -r /dev/tty ] && [ -w /dev/tty ] && [ "$ASSUME_YES" = 0 ]; }
ask() { # ask "question" default -> echoes the answer (default when no terminal)
  local a=""
  if tty_ok; then printf '    %s ' "$1" > /dev/tty; IFS= read -r a < /dev/tty || true; fi
  printf '%s' "${a:-$2}"
}
ask_secret() {
  local a=""
  if tty_ok; then printf '    %s ' "$1" > /dev/tty; IFS= read -rs a < /dev/tty || true; printf '\n' > /dev/tty; fi
  printf '%s' "$a"
}

while [ $# -gt 0 ]; do
  case "$1" in
    --data-url) DATA_URL="${2:-}"; shift 2 ;;
    --project)  PROJECT="${2:-}"; shift 2 ;;
    --engine)   ENGINE="${2:-}"; shift 2 ;;
    --topdir)   TOPDIR="${2:-}"; shift 2 ;;
    --token)    TOKEN="${2:-}"; shift 2 ;;
    --desc)     DESC="${2:-}"; shift 2 ;;
    --accounts) ACCOUNTS="${2:-}"; shift 2 ;;
    --env-from) ENV_FROM="${2:-}"; shift 2 ;;
    --bin)      BIN="${2:-}"; shift 2 ;;
    --start)    START=1; shift ;;
    --yes|-y)   ASSUME_YES=1; shift ;;
    --stop)     VERB="stop"; shift ;;
    --status)   VERB="status"; shift ;;
    -h|--help)  usage 0 ;;
    *) warn "unknown option: $1"; usage 2 ;;
  esac
done

expand() { case "$1" in "~") printf '%s' "$HOME" ;; "~/"*) printf '%s/%s' "$HOME" "${1#\~/}" ;; *) printf '%s' "$1" ;; esac; }
[ -n "$TOPDIR" ] && TOPDIR="$(expand "$TOPDIR")"
[ -n "$ENV_FROM" ] && ENV_FROM="$(expand "$ENV_FROM")"
[ -n "$BIN" ] && BIN="$(expand "$BIN")"

# ------------------------------------------------------------ --status / --stop
if [ "$VERB" != "setup" ]; then
  cd "${TOPDIR:-.}"
  PIDF=".iter/engine.pid"
  pid="$(cat "$PIDF" 2>/dev/null || true)"
  if [ -n "$pid" ] && kill -0 "$pid" 2>/dev/null; then
    if [ "$VERB" = "stop" ]; then kill "$pid"; rm -f "$PIDF"; ok "engine stopped (pid $pid)"; else ok "engine running (pid $pid), log: $(pwd)/.iter/engine.log"; fi
  else
    rm -f "$PIDF" 2>/dev/null || true
    say "no engine started by this script is running in $(pwd)"
  fi
  exit 0
fi

# ------------------------------------------------------------ inputs
[ -n "$DATA_URL" ] || DATA_URL="$(ask "iter server URL (as this machine reaches it) [http://127.0.0.1:8300]:" "http://127.0.0.1:8300")"
[ -n "$PROJECT" ]  || PROJECT="$(ask "project name:" "")"
[ -n "$ENGINE" ]   || ENGINE="$(ask "engine name [$(hostname -s 2>/dev/null || echo Engine01)]:" "$(hostname -s 2>/dev/null || echo Engine01)")"
[ -n "$TOPDIR" ]   || TOPDIR="$(expand "$(ask "checkout folder [$(pwd)]:" "$(pwd)")")"
DATA_URL="${DATA_URL%/}"
[ -n "$PROJECT" ] || die "a project name is required (--project)"
[ -n "$ENGINE" ]  || die "an engine name is required (--engine)"
printf '\n  server   %s\n  project  %s\n  engine   %s\n  folder   %s\n\n' "$DATA_URL" "$PROJECT" "$ENGINE" "$TOPDIR"

# ------------------------------------------------------------ 1 tools
say "1/7 tools"
command -v git  >/dev/null || die "git is not installed"
command -v curl >/dev/null || die "curl is not installed"
ok "git, curl"
if command -v claude >/dev/null; then ok "claude $(claude --version 2>/dev/null | head -1)"
else warn "the claude CLI is not on PATH: agents cannot run until it is. Install: npm install -g @anthropic-ai/claude-code"; fi

# ------------------------------------------------------------ 2 iter_engine
say "2/7 iter_engine"
if [ -z "$BIN" ]; then
  if command -v iter_engine >/dev/null; then BIN="$(command -v iter_engine)"
  elif [ -x "$HOME_DIR/bin/iter_engine" ]; then BIN="$HOME_DIR/bin/iter_engine"; fi
fi
if [ -z "$BIN" ]; then
  # rustup installs cargo here; a non-login shell often lacks it on PATH
  command -v cargo >/dev/null || { [ -x "$HOME/.cargo/bin/cargo" ] && PATH="$HOME/.cargo/bin:$PATH"; }
  command -v cargo >/dev/null || die "iter_engine was not found and cargo is not installed to build it.
  Either pass --bin /path/to/iter_engine, or install Rust (https://rustup.rs:
  curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh) and run this again."
  SRC="$HOME_DIR/src/iter"
  if [ -d "$SRC/.git" ]; then say "    updating $SRC"; git -C "$SRC" pull --ff-only -q
  else say "    cloning $SRC_REPO into $SRC"; mkdir -p "$(dirname "$SRC")"; git clone -q --depth 1 "$SRC_REPO" "$SRC"; fi
  say "    building iter_engine (release; the first build takes a few minutes)"
  (cd "$SRC/iter4" && cargo build --release -q -p iter_engine)
  mkdir -p "$HOME_DIR/bin"
  # rm then cp, never cp over a binary: macOS kills a binary copied over a live one
  rm -f "$HOME_DIR/bin/iter_engine"; cp "$SRC/iter4/target/release/iter_engine" "$HOME_DIR/bin/iter_engine"
  BIN="$HOME_DIR/bin/iter_engine"
fi
[ -x "$BIN" ] || die "$BIN is not an executable file"
ok "$BIN"

# ------------------------------------------------------------ 3 folder
say "3/7 checkout folder"
if [ ! -d "$TOPDIR" ]; then mkdir -p "$TOPDIR"; ok "created $TOPDIR"; else ok "$TOPDIR exists"; fi
cd "$TOPDIR"
if git rev-parse --is-inside-work-tree >/dev/null 2>&1; then
  root="$(git rev-parse --show-toplevel)"
  if [ "$root" = "$(pwd -P)" ]; then ok "git repository"; else ok "inside the git repository $root (the engine commits there)"; fi
else
  warn "not a git repository: the engine commits each finished item, so it needs one"
  case "$(ask "run 'git init' here? [Y/n]:" "y")" in n*|N*) warn "left as is: run 'git init' (or clone) before starting work" ;; *) git init -q; ok "git init" ;; esac
fi

# ------------------------------------------------------------ 4 server + token
say "4/7 server and engine token"
curl -fsS --max-time 10 "$DATA_URL/health" >/dev/null 2>&1 || die "no answer from $DATA_URL/health - is the iter server running, and is this the address this machine reaches it by?"
ok "$DATA_URL answers"
envget() { [ -f "$2" ] && grep -E "^[[:space:]]*$1=" "$2" | tail -1 | cut -d= -f2- | sed -e 's/^[[:space:]]*//' -e 's/^["'\'']//' -e 's/["'\'']$//' || true; }
[ -n "$TOKEN" ] || TOKEN="$(envget ITER_ENGINE_TOKEN .env)"
[ -n "$TOKEN" ] || TOKEN="$(ask_secret "engine token (from the webui's Engine setup page; input hidden):")"
[ -n "$TOKEN" ] || die "an engine token is required (--token): an admin mints it on the webui's Engine setup page"
api() { curl -sS --max-time 15 -H "authorization: Bearer $TOKEN" -o /tmp/iter_setup.$$ -w '%{http_code}' "$DATA_URL$1" 2>/dev/null || echo 000; }
code="$(api "/api/engines/$ENGINE")"; ENG_JSON="$(cat /tmp/iter_setup.$$ 2>/dev/null || true)"
case "$code" in
  200) ok "the token signs in; engine $ENGINE is on record" ;;
  404) ok "the token signs in; engine $ENGINE registers itself on first start" ;;
  401|403) die "the server refused the engine token (HTTP $code): mint a fresh one on the Engine setup page" ;;
  *) die "unexpected answer from the server (HTTP $code) for /api/engines/$ENGINE" ;;
esac
code="$(api "/api/projects/$PROJECT")"; PROJ_JSON="$(cat /tmp/iter_setup.$$ 2>/dev/null || true)"; rm -f /tmp/iter_setup.$$
[ "$code" = 200 ] || die "project $PROJECT was not found on the server (HTTP $code): create it in the webui first"
ok "project $PROJECT exists"
case "$ENG_JSON" in *"\"$PROJECT\""*) ok "engine $ENGINE is assigned project $PROJECT" ;;
  *) warn "engine $ENGINE does not list project $PROJECT yet: add it from the engine's gear in the work queue (checkout path $TOPDIR)" ;; esac

# ------------------------------------------------------------ 5 project files
say "5/7 project files"
if [ -f main.iter.md ] && [ -f .iter/config.json ]; then
  ok "main.iter.md and .iter/config.json already exist (left alone)"
else
  args=(cli init --project "$PROJECT" --data-url "$DATA_URL" --engine "$ENGINE")
  [ -n "$DESC" ] && args+=(--desc "$DESC")
  "$BIN" "${args[@]}" | sed 's/^/    /'
fi
if [ -f .iter/config.json ] && ! grep -q "\"engine_name\": *\"$ENGINE\"" .iter/config.json; then
  warn ".iter/config.json names another engine: $(grep -o '"engine_name": *"[^"]*"' .iter/config.json). Edit it, or rerun with that --engine."
fi
touch .iter/.gitignore
for f in engine.log engine.pid; do grep -qx "$f" .iter/.gitignore || echo "$f" >> .iter/.gitignore; done

# ------------------------------------------------------------ 6 .env
say "6/7 .env (secrets: kept out of git)"
touch .env; chmod 600 .env
setenv() { # setenv KEY VALUE: replace or append one line, value never echoed
  local tmp; tmp="$(mktemp)"
  grep -vE "^[[:space:]]*$1=" .env > "$tmp" || true
  printf '%s=%s\n' "$1" "$2" >> "$tmp"; cat "$tmp" > .env; rm -f "$tmp"
}
[ "$(envget ITER_ENGINE_TOKEN .env)" = "$TOKEN" ] || setenv ITER_ENGINE_TOKEN "$TOKEN"
ok "ITER_ENGINE_TOKEN"
if [ -z "$ACCOUNTS" ]; then # the project's accounts[].token_envar
  ACCOUNTS="$(printf '%s' "$PROJ_JSON" | grep -o '"token_envar":"[A-Za-z0-9_]*"' | cut -d'"' -f4 | tr '\n' ',' | sed 's/,$//')"
fi
if [ -z "$ACCOUNTS" ]; then
  ok "the project lists no Claude accounts: agents use this machine's own claude login"
else
  missing=0
  for var in $(printf '%s' "$ACCOUNTS" | tr ',' ' '); do
    if [ -n "$(envget "$var" .env)" ]; then ok "$var (already set)"; continue; fi
    val=""
    [ -n "$ENV_FROM" ] && val="$(envget "$var" "$ENV_FROM")"
    [ -n "$val" ] && { setenv "$var" "$val"; ok "$var (copied from $ENV_FROM)"; continue; }
    val="$(ask_secret "$var - paste a token from 'claude setup-token' (run while logged in to that account; blank = later):")"
    if [ -n "$val" ]; then setenv "$var" "$val"; ok "$var"; else warn "$var not set yet: add it to $(pwd)/.env (the engine reloads .env on its own)"; missing=1; fi
  done
  [ "$missing" = 0 ] || warn "an account without a token is never used; with none set, nothing runs"
fi
if git rev-parse --is-inside-work-tree >/dev/null 2>&1 && ! git check-ignore -q .env; then
  echo ".env" >> .gitignore; ok "added .env to $(pwd)/.gitignore"
fi
if [ -n "${ANTHROPIC_API_KEY:-}" ]; then warn "ANTHROPIC_API_KEY is set in this shell: it outranks the account tokens and bills API credits. Unset it before starting the engine."; fi

# ------------------------------------------------------------ 7 start
say "7/7 engine"
"$BIN" --config .iter/config.json --accounts 2>&1 | sed 's/^/    /' || true
last_seen() { curl -sS --max-time 10 -H "authorization: Bearer $TOKEN" "$DATA_URL/api/engines/$ENGINE" 2>/dev/null | grep -o '"last_seen":"[^"]*"' | cut -d'"' -f4 || true; }
pid="$(cat .iter/engine.pid 2>/dev/null || true)"
if [ -n "$pid" ] && kill -0 "$pid" 2>/dev/null; then
  ok "already running here (pid $pid); it re-reads its settings every tick"
elif [ "$START" = 1 ]; then
  before="$(last_seen)"
  nohup "$BIN" --config .iter/config.json >> .iter/engine.log 2>&1 &
  echo $! > .iter/engine.pid
  ok "started (pid $!), log: $(pwd)/.iter/engine.log"
  printf '    waiting for it to check in'
  for _ in $(seq 1 30); do
    sleep 2; printf '.'
    now="$(last_seen)"
    if [ -n "$now" ] && [ "$now" != "$before" ]; then printf '\n'; ok "engine $ENGINE is online ($now)"; break; fi
    kill -0 "$(cat .iter/engine.pid)" 2>/dev/null || { printf '\n'; tail -20 .iter/engine.log >&2; die "the engine exited: see the log above"; }
  done
  [ -n "${now:-}" ] && [ "$now" != "$before" ] || { printf '\n'; warn "no check-in after 60 s: see $(pwd)/.iter/engine.log"; }
else
  printf '\n    Start it (in %s):\n\n      %s --config .iter/config.json\n\n    or in the background: rerun this script with --start.\n' "$(pwd)" "$BIN"
fi

cat <<EOF

Done. In the webui: pick project $PROJECT, press Running if it is Stopped, and
queued work starts within a few seconds.
  stop the engine:   bash $0 --stop --topdir $(pwd)
  its log:           tail -f $(pwd)/.iter/engine.log
Commit the new project files (main.iter.md, .iter/, reqs/, .gitignore); .env stays uncommitted.
EOF
