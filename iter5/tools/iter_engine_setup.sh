#!/usr/bin/env bash
# iter_engine_setup.sh - set up (and start) the iter5 engine on this machine.
#
# iter5 runs ONE engine per machine; it serves every project the settings graph
# connects to it (a `serves` edge: engine -> project, with the checkout folder
# as the edge's topdir). This script needs no project: run it once per machine,
# straight from the server, so no copy of it lands anywhere:
#
#   curl -fsSL http://127.0.0.1:8400/iter_engine_setup.sh | bash -s -- \
#     --data-url http://127.0.0.1:8400 --engine mbp --token <engine token> --start
#
# What it does, in order (each step says what it found; nothing is overwritten):
#   1. checks the tools: git, curl, and the claude CLI the agents run in
#   2. finds iter_engine (--bin, $ITER_ENGINE_BIN, ~/.iter5/bin), else builds it
#      from GitHub with cargo into ~/.iter5/bin
#   3. checks the server answers and the engine token signs in
#   4. writes the env file (default ~/.iter5/.env, mode 600): the engine token
#      plus one token per account this engine holds in the settings graph
#   5. with --start: starts the engine in the background
#      (`iter_engine --data-url URL --env-file FILE --name NAME`) and waits for
#      it to check in with the server
# Then, in the webui's Settings tab: draw a `serves` edge from the engine to each
# project (set its topdir), and `holds` edges to the accounts it has tokens for.
#
# Other verbs:
#   ... | bash -s -- --status     is the engine process running?
#   ... | bash -s -- --stop       stop the engine this script started
set -euo pipefail

DATA_URL="" ENGINE="" TOKEN="${ITER_ENGINE_TOKEN:-}" ACCOUNTS="" ENV_FROM=""
BIN="${ITER_ENGINE_BIN:-}" START=0 VERB="setup" ASSUME_YES=0
SRC_REPO="${ITER_SRC_REPO:-https://github.com/Stephen-Hilton/iter.git}"
HOME_DIR="${ITER5_HOME:-$HOME/.iter5}"
ENV_FILE=""

usage() { # piped into bash there is no file to read the header from
  if [ -f "$0" ]; then sed -n '2,/^set -euo/p' "$0" | sed '$d' | sed 's/^# \{0,1\}//'
  else echo "iter_engine_setup.sh --data-url URL [--engine NAME] [--token T] [--env-file FILE] [--accounts VAR,VAR] [--env-from FILE] [--bin PATH] [--start] [--yes] | --status | --stop"; fi
  exit "${1:-0}"; }
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
    --engine)   ENGINE="${2:-}"; shift 2 ;;
    --token)    TOKEN="${2:-}"; shift 2 ;;
    --env-file) ENV_FILE="${2:-}"; shift 2 ;;
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
ENV_FILE="$(expand "${ENV_FILE:-$HOME_DIR/.env}")"
[ -n "$ENV_FROM" ] && ENV_FROM="$(expand "$ENV_FROM")"
[ -n "$BIN" ] && BIN="$(expand "$BIN")"
PIDF="$HOME_DIR/engine.pid" LOG="$HOME_DIR/engine.log"

# ------------------------------------------------------------ --status / --stop
if [ "$VERB" != "setup" ]; then
  pid="$(cat "$PIDF" 2>/dev/null || true)"
  if [ -n "$pid" ] && kill -0 "$pid" 2>/dev/null; then
    if [ "$VERB" = "stop" ]; then kill "$pid"; rm -f "$PIDF"; ok "engine stopped (pid $pid)"; else ok "engine running (pid $pid), log: $LOG"; fi
  else
    rm -f "$PIDF" 2>/dev/null || true
    say "no engine started by this script is running on this machine"
  fi
  exit 0
fi

# ------------------------------------------------------------ inputs
HOST="$(hostname -s 2>/dev/null || echo engine)"
[ -n "$DATA_URL" ] || DATA_URL="$(ask "iter server URL (as this machine reaches it) [http://127.0.0.1:8400]:" "http://127.0.0.1:8400")"
[ -n "$ENGINE" ]   || ENGINE="$(ask "engine name [$HOST]:" "$HOST")"
DATA_URL="${DATA_URL%/}"
[ -n "$ENGINE" ] || die "an engine name is required (--engine)"
mkdir -p "$HOME_DIR"
printf '\n  server    %s\n  engine    %s\n  env file  %s\n\n' "$DATA_URL" "$ENGINE" "$ENV_FILE"

# ------------------------------------------------------------ 1 tools
say "1/5 tools"
command -v git  >/dev/null || die "git is not installed"
command -v curl >/dev/null || die "curl is not installed"
ok "git, curl"
if command -v claude >/dev/null; then ok "claude $(claude --version 2>/dev/null | head -1)"
else warn "the claude CLI is not on PATH: claude-provider agents cannot run until it is. Install: npm install -g @anthropic-ai/claude-code"; fi

# ------------------------------------------------------------ 2 iter_engine
say "2/5 iter_engine"
if [ -z "$BIN" ] && [ -x "$HOME_DIR/bin/iter_engine" ]; then BIN="$HOME_DIR/bin/iter_engine"; fi
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
  (cd "$SRC/iter5" && cargo build --release -q -p iter_engine)
  mkdir -p "$HOME_DIR/bin"
  # rm then cp, never cp over a binary: macOS kills a binary copied over a live one
  rm -f "$HOME_DIR/bin/iter_engine"; cp "$SRC/iter5/target/release/iter_engine" "$HOME_DIR/bin/iter_engine"
  BIN="$HOME_DIR/bin/iter_engine"
fi
[ -x "$BIN" ] || die "$BIN is not an executable file"
ok "$BIN"

# ------------------------------------------------------------ 3 server + token
say "3/5 server and engine token"
curl -fsS --max-time 10 "$DATA_URL/health" >/dev/null 2>&1 || die "no answer from $DATA_URL/health - is the iter server running, and is this the address this machine reaches it by?"
ok "$DATA_URL answers"
envget() { [ -f "$2" ] && grep -E "^[[:space:]]*$1=" "$2" | tail -1 | cut -d= -f2- | sed -e 's/^[[:space:]]*//' -e 's/^["'\'']//' -e 's/["'\'']$//' || true; }
[ -n "$TOKEN" ] || TOKEN="$(envget ITER_ENGINE_TOKEN "$ENV_FILE")"
[ -n "$TOKEN" ] || TOKEN="$(ask_secret "engine token (an admin mints it in the webui's engine setup; input hidden):")"
[ -n "$TOKEN" ] || die "an engine token is required (--token)"
TMP="$(mktemp)"; trap 'rm -f "$TMP"' EXIT
api() { curl -sS --max-time 15 -H "authorization: Bearer $TOKEN" -o "$TMP" -w '%{http_code}' "$DATA_URL$1" 2>/dev/null || echo 000; }
code="$(api "/api/engines/$ENGINE")"
case "$code" in
  200) ok "the token signs in; engine $ENGINE is on record" ;;
  404) ok "the token signs in; engine $ENGINE registers itself on first start" ;;
  401) die "the server refused the engine token (HTTP 401): mint a fresh one" ;;
  403) die "engine $ENGINE belongs to another user (HTTP 403): pick another --engine name, or have an admin reassign it" ;;
  *) die "unexpected answer from the server (HTTP $code) for /api/engines/$ENGINE" ;;
esac
ASSIGN=""
if [ "$code" = 200 ] && [ "$(api "/api/engines/$ENGINE/assignments")" = 200 ]; then
  ASSIGN="$(cat "$TMP")"
  n="$(printf '%s' "$ASSIGN" | grep -o '"project":"[^"]*"' | wc -l | tr -d ' ')"
  if [ "$n" = 0 ]; then warn "no project is connected to $ENGINE yet: draw a serves edge in the Settings tab"
  else ok "serves $n project(s): $(printf '%s' "$ASSIGN" | grep -o '"project":"[^"]*"' | cut -d'"' -f4 | sort -u | tr '\n' ' ')"; fi
fi

# ------------------------------------------------------------ 4 env file
say "4/5 env file (secrets: mode 600)"
mkdir -p "$(dirname "$ENV_FILE")"; touch "$ENV_FILE"; chmod 600 "$ENV_FILE"
setenv() { # setenv KEY VALUE: replace or append one line, value never echoed
  local tmp; tmp="$(mktemp)"
  grep -vE "^[[:space:]]*$1=" "$ENV_FILE" > "$tmp" || true
  printf '%s=%s\n' "$1" "$2" >> "$tmp"; cat "$tmp" > "$ENV_FILE"; rm -f "$tmp"
}
[ "$(envget ITER_ENGINE_TOKEN "$ENV_FILE")" = "$TOKEN" ] || setenv ITER_ENGINE_TOKEN "$TOKEN"
ok "ITER_ENGINE_TOKEN"
if [ -z "$ACCOUNTS" ] && [ -n "$ASSIGN" ]; then # the accounts this engine holds
  ACCOUNTS="$(printf '%s' "$ASSIGN" | grep -o '"token_envar":"[A-Za-z0-9_]*"' | cut -d'"' -f4 | sort -u | tr '\n' ',' | sed 's/,$//')"
fi
if [ -z "$ACCOUNTS" ]; then
  ok "no account tokens to set yet (add holds edges in the Settings tab, or pass --accounts VAR,VAR); claude agents use this machine's own login meanwhile"
else
  missing=0
  for var in $(printf '%s' "$ACCOUNTS" | tr ',' ' '); do
    if [ -n "$(envget "$var" "$ENV_FILE")" ]; then ok "$var (already set)"; continue; fi
    val=""
    [ -n "$ENV_FROM" ] && val="$(envget "$var" "$ENV_FROM")"
    [ -n "$val" ] && { setenv "$var" "$val"; ok "$var (copied from $ENV_FROM)"; continue; }
    val="$(ask_secret "$var - paste a token from 'claude setup-token' (run while logged in to that account; blank = later):")"
    if [ -n "$val" ]; then setenv "$var" "$val"; ok "$var"; else warn "$var not set yet: add it to $ENV_FILE (the engine reloads it on its own)"; missing=1; fi
  done
  [ "$missing" = 0 ] || warn "an account without a token is never used"
fi
if [ -n "${ANTHROPIC_API_KEY:-}" ]; then warn "ANTHROPIC_API_KEY is set in this shell: it outranks the account tokens and bills API credits. Unset it before starting the engine."; fi

# ------------------------------------------------------------ 5 start
say "5/5 engine"
RUN=("$BIN" --data-url "$DATA_URL" --env-file "$ENV_FILE" --name "$ENGINE")
last_seen() { curl -sS --max-time 10 -H "authorization: Bearer $TOKEN" "$DATA_URL/api/engines/$ENGINE" 2>/dev/null | grep -o '"last_seen":"[^"]*"' | cut -d'"' -f4 || true; }
pid="$(cat "$PIDF" 2>/dev/null || true)"
if [ -n "$pid" ] && kill -0 "$pid" 2>/dev/null; then
  ok "already running (pid $pid); it re-reads its assignments every tick"
elif [ "$START" = 1 ]; then
  before="$(last_seen)"
  nohup "${RUN[@]}" >> "$LOG" 2>&1 &
  echo $! > "$PIDF"
  ok "started (pid $!), log: $LOG"
  printf '    waiting for it to check in'
  now=""
  for _ in $(seq 1 30); do
    sleep 2; printf '.'
    now="$(last_seen)"
    if [ -n "$now" ] && [ "$now" != "$before" ]; then printf '\n'; ok "engine $ENGINE is online ($now)"; break; fi
    kill -0 "$(cat "$PIDF")" 2>/dev/null || { printf '\n'; tail -20 "$LOG" >&2; die "the engine exited: see the log above"; }
  done
  [ -n "$now" ] && [ "$now" != "$before" ] || { printf '\n'; warn "no check-in after 60 s: see $LOG"; }
else
  printf '\n    Start it:\n\n      %s\n\n    or in the background: rerun this script with --start.\n' "${RUN[*]}"
fi

cat <<EOF

Done. In the webui's Settings tab: connect $ENGINE to a project with a serves
edge (its topdir is the checkout folder; a designed project is built there),
and give it holds edges to the accounts whose tokens are in $ENV_FILE.
  engine status:     curl -fsSL $DATA_URL/iter_engine_setup.sh | bash -s -- --status
  stop the engine:   curl -fsSL $DATA_URL/iter_engine_setup.sh | bash -s -- --stop
  its log:           tail -f $LOG
EOF
