#!/usr/bin/env bash
# install.sh - make this Linux machine an iter engine (written for Omarchy /
# Arch Linux; Debian and Ubuntu work too). iter_data runs elsewhere (AWS); this
# machine only makes outbound connections: to iter_data, to Anthropic, to the
# projects' git remote and, once, to Hugging Face for the embedding model.
#
# From the folder that holds your code projects:
#
#   cd ~/dev && git clone https://github.com/Stephen-Hilton/iter.git && ./iter/install.sh
#
# Every step says what it found and changes nothing that is already right, so
# the script can be run again at any time. The steps:
#   1. packages: git, base-devel, rustup, curl, jq, poppler, github-cli (asks first)
#   2. the claude CLI (Anthropic's installer, into ~/.local/bin; asks first)
#   3. iter_data URL and this engine's name (kept in ~/.iter/engine.conf)
#   4. builds iter_engine from this checkout into ~/.iter/bin (the first
#      build takes a while on an old laptop)
#   5. the embedding model into ~/.iter/models (~91 MB, checksum-verified)
#   6. the engine key: `iter_engine key new` makes this machine's key pair
#      (the private key stays in ~/.iter/engine.key); you paste its public key
#      into the webui's Engine keys screen (🔑 in the header) and the script
#      waits until iter_data accepts it
#   7. a laptop: never suspend, lid closed or not (asks first); on Omarchy:
#      no lock screen or screensaver, hypridle turns the screen off after
#      10 minutes (asks first)
#   8. the iter-engine systemd service (linux/install.sh engine)
#   9. account tokens and project checkouts (below), when the engine already
#      serves projects
#
# Projects live beside this repo: give an engine's `serves` edge (webui,
# Settings tab) the topdir ~/dev/<project>, e.g. ~/dev/pdy-dev; then
#   ./iter/install.sh projects   clones each served project that is missing
#   ./iter/install.sh accounts   asks for each missing account token
#   ./iter/install.sh update     git pull + rebuild + restart (--yes while work runs)
#   ./iter/install.sh status     the service, the key, the served projects
#
# Everything this machine keeps for iter is in ~/.iter: the engine (bin/), its
# key (engine.key), the env file with account tokens (.env), the embedding
# model (models/) and the logs. A machine set up when that folder was
# ~/.iter5 is moved over on the next run (~/.iter5 stays as a link to it).
set -euo pipefail

REPO="$(cd "$(dirname "$0")" && pwd)"
DEV="$(dirname "$REPO")"
IHOME="$HOME/.iter"
CONF="$IHOME/engine.conf"
ENVF="$IHOME/.env"
KEYF="$IHOME/engine.key"
BIN="$IHOME/bin/iter_engine"
MODEL="$IHOME/models/all-MiniLM-L6-v2"
export PATH="$HOME/.local/bin:$HOME/.cargo/bin:$PATH"

say()  { printf '\n\033[1m==>\033[0m %s\n' "$*"; }
ok()   { printf '    \033[32mok\033[0m  %s\n' "$*"; }
warn() { printf '    \033[33m!!\033[0m  %s\n' "$*" >&2; }
die()  { printf '\n\033[31mstopped:\033[0m %s\n' "$*" >&2; exit 1; }
ask() { # ask "question" default -> the answer (the default without a terminal)
  local a=""
  if [ -r /dev/tty ]; then printf '    %s ' "$1" > /dev/tty; IFS= read -r a < /dev/tty || true; fi
  printf '%s' "${a:-$2}"
}
yes_no() { case "$(ask "$1 [Y/n]" y)" in [nN]*) return 1 ;; *) return 0 ;; esac; }
ask_secret() {
  local a=""
  if [ -r /dev/tty ]; then printf '    %s ' "$1" > /dev/tty; IFS= read -rs a < /dev/tty || true; printf '\n' > /dev/tty; fi
  printf '%s' "$a"
}
confval() { [ -f "$CONF" ] && grep -E "^$1=" "$CONF" | tail -1 | cut -d= -f2- || true; }
envval() { [ -f "$2" ] && grep -E "^[[:space:]]*$1=" "$2" | tail -1 | cut -d= -f2- | sed -e 's/^["'\'']//' -e 's/["'\'']$//' | tr -d '\r' || true; }
expand() { case "$1" in "~") printf '%s' "$HOME" ;; "~/"*) printf '%s/%s' "$HOME" "${1#\~/}" ;; *) printf '%s' "$1" ;; esac; }

[ "$(id -u)" != 0 ] || die "run this as your own user, not root (it uses sudo where it must)"
[ "$(uname -s)" = Linux ] || die "this script is for Linux; on Windows use install.ps1"

load_conf() {
  DATA_URL="$(confval DATA_URL)"
  ENGINE="$(confval ENGINE)"
}

# ~/.iter5 became ~/.iter (2026-10-11): move it once; ~/.iter5 stays as a
# link so a running service or an old path still finds its files
migrate_home() {
  if [ -d "$HOME/.iter5" ] && [ ! -L "$HOME/.iter5" ]; then
    if [ -e "$IHOME" ]; then warn "both ~/.iter5 and ~/.iter exist: using ~/.iter; move what you need from ~/.iter5 by hand"
    else mv "$HOME/.iter5" "$IHOME" && ln -s "$IHOME" "$HOME/.iter5" && ok "moved ~/.iter5 to ~/.iter (~/.iter5 links to it)"; fi
  fi
}

# GET an iter_data path as this engine (a session from its key, else an older
# token): body in $TMPF, prints the status
TMPF="$(mktemp)"; trap 'rm -f "$TMPF"' EXIT
api() {
  local tok=""
  [ -f "$KEYF" ] && tok="$("$BIN" key session --key-file "$KEYF" --data-url "$DATA_URL" 2>/dev/null)"
  [ -n "$tok" ] || tok="$(envval ITER_ENGINE_TOKEN "$ENVF")"
  curl -sS --max-time 15 -H "authorization: Bearer $tok" -o "$TMPF" -w '%{http_code}' "$DATA_URL$1" 2>/dev/null || echo 000
}

# ------------------------------------------------------------------ steps

packages() {
  say "1/9 packages"
  if command -v pacman >/dev/null; then
    local want=(git base-devel curl jq poppler github-cli) missing=()
    command -v cargo >/dev/null || command -v rustup >/dev/null || want+=(rustup)
    for p in "${want[@]}"; do pacman -Qq "$p" >/dev/null 2>&1 || missing+=("$p"); done
    if [ ${#missing[@]} -gt 0 ]; then
      yes_no "install ${missing[*]} with sudo pacman?" || die "these are needed: ${missing[*]}"
      sudo pacman -S --needed --noconfirm "${missing[@]}"
    fi
  elif command -v apt-get >/dev/null; then
    local missing=()
    for p in git build-essential curl jq poppler-utils gh; do dpkg -s "$p" >/dev/null 2>&1 || missing+=("$p"); done
    if [ ${#missing[@]} -gt 0 ]; then
      yes_no "install ${missing[*]} with sudo apt-get?" || die "these are needed: ${missing[*]}"
      sudo apt-get update -q && sudo apt-get install -y -q "${missing[@]}"
    fi
    if ! command -v cargo >/dev/null && ! command -v rustup >/dev/null; then
      yes_no "install Rust with rustup (https://rustup.rs)?" || die "Rust is needed to build iter_engine"
      curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal
    fi
  else
    warn "neither pacman nor apt-get: install git, a C toolchain, curl, jq, poppler (pdftoppm) and Rust yourself"
  fi
  if command -v rustup >/dev/null && ! rustup show active-toolchain >/dev/null 2>&1; then
    rustup default stable
  fi
  for t in git cargo curl jq pdftoppm; do command -v "$t" >/dev/null || die "$t is still missing"; done
  ok "git, cargo $(cargo --version | cut -d' ' -f2), curl, jq, pdftoppm"
}

claude_cli() {
  say "2/9 claude CLI"
  if command -v claude >/dev/null; then ok "claude $(claude --version 2>/dev/null | head -1)"; return; fi
  if yes_no "install the claude CLI (Anthropic's installer, into ~/.local/bin)?"; then
    curl -fsSL https://claude.ai/install.sh | bash
    command -v claude >/dev/null && ok "claude $(claude --version 2>/dev/null | head -1)" || warn "claude is not on PATH yet: open a new shell"
  else
    warn "agents cannot run until the claude CLI is installed"
  fi
}

settings() {
  say "3/9 iter_data and this engine's name"
  load_conf
  local url name host
  url="$(ask "iter_data URL [${DATA_URL:-https://iter.example.com}]:" "${DATA_URL:-}")"
  url="${url%/}"
  [ -n "$url" ] || die "the iter_data URL is required"
  case "$url" in http://127.0.0.1*|http://localhost*|https://*) ;; http://*) warn "$url is plain http: the engine's session token crosses the network unencrypted" ;; esac
  curl -fsS --max-time 10 -o /dev/null "$url/health" 2>/dev/null && ok "$url answers" \
    || warn "no answer from $url/health (a proxy may hide it; the key check below tells)"
  host="$(hostname -s 2>/dev/null || hostname)"
  name="$(ask "engine name [${ENGINE:-$host}]:" "${ENGINE:-$host}")"
  case "$name" in omarchy|archlinux|arch|localhost|ubuntu|debian|linux)
    warn "\"$name\" is a default hostname: every fresh machine has it, and two engines with one name are one engine to iter_data"
    name="$(ask "a name unique to this machine:" "")"
    [ -n "$name" ] || die "an engine name unique to this machine is needed" ;;
  esac
  case "$name" in *[!A-Za-z0-9_.-]*|"") die "engine name \"$name\": letters, digits, - _ . only" ;; esac
  mkdir -p "$IHOME"
  printf 'DATA_URL=%s\nENGINE=%s\n' "$url" "$name" > "$CONF"
  DATA_URL="$url" ENGINE="$name"
  ok "engine \"$ENGINE\" → $DATA_URL (saved in $CONF)"
}

build() {
  say "4/9 iter_engine"
  if [ -x "$BIN" ] && [ "$BIN" -nt "$REPO/.git/HEAD" ] && [ "${1:-}" != force ]; then ok "$BIN (built after the last pull)"; return; fi
  say "    building from $REPO (release; the first build takes 10+ minutes on an old laptop)"
  (cd "$REPO" && cargo build --release -q -p iter_engine)
  mkdir -p "$IHOME/bin"
  install -m 755 "$REPO/target/release/iter_engine" "$BIN"
  ok "$BIN"
}

model() {
  say "5/9 embedding model"
  bash "$REPO/tools/fetch_model.sh" "$MODEL" | sed 's/^/    /'
}

key() {
  say "6/9 engine key"
  mkdir -p "$IHOME"; touch "$ENVF"; chmod 600 "$ENVF"
  [ -f "$KEYF" ] || "$BIN" key new --key-file "$KEYF" --name "$ENGINE" | sed 's/^/    /'
  local first=1
  while :; do
    if "$BIN" key status --key-file "$KEYF" --data-url "$DATA_URL" > "$TMPF" 2>&1; then
      sed 's/^/    /' "$TMPF"; ok "iter_data accepts this engine's key"; return
    fi
    if [ $first = 1 ]; then
      sed 's/^/    /' "$TMPF"
      printf '\n    In the webui (%s): 🔑 Engine keys (header) → Add: paste the iter_… public key above, engine name "%s".\n' "$DATA_URL" "$ENGINE"
      first=0
    fi
    case "$(ask "press Enter once it is added (n = a new key, s = skip):" "")" in
      n|N) rm -f "$KEYF"; "$BIN" key new --key-file "$KEYF" --name "$ENGINE" | sed 's/^/    /'; first=1 ;;
      s|S) warn "skipped: iter_data refuses this engine until its public key is added"; return ;;
      *) [ -r /dev/tty ] || die "iter_data does not accept the key yet (no terminal to wait at)"
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
    sudo mkdir -p /etc/systemd/logind.conf.d
    printf '[Login]\nHandleLidSwitch=ignore\nHandleLidSwitchExternalPower=ignore\nHandleLidSwitchDocked=ignore\nIdleAction=ignore\n' \
      | sudo tee /etc/systemd/logind.conf.d/iter-engine.conf >/dev/null
    # anything asking to suspend (Omarchy's hypridle) is now refused
    sudo systemctl mask -q sleep.target suspend.target hibernate.target hybrid-sleep.target
    sudo systemctl kill -s HUP systemd-logind
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
  local off on
  if [ -f "$HYPR/hyprland.lua" ]; then
    # current Omarchy: its shell's idle timers off (0 = off), lid close keeps
    # the display handling but not the lock
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
  [ -f "$HYPR/hypridle.conf" ] && ! grep -q "$MARK" "$HYPR/hypridle.conf" && cp "$HYPR/hypridle.conf" "$HYPR/hypridle.conf.before-iter"
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

service() {
  say "8/9 iter-engine service"
  bash "$REPO/linux/install.sh" engine --name "$ENGINE" --data-url "$DATA_URL" 2>&1 | sed 's/^/    /'
  systemctl is-active -q iter-engine && ok "iter-engine is running (log: $IHOME/engine.log)"
}

assignments() { # the engine's assignments into $TMPF; false when it has none yet
  [ "$(api "/api/engines/$ENGINE/assignments")" = 200 ] && jq -e '.projects | length > 0' "$TMPF" >/dev/null
}

accounts() {
  say "account tokens"
  load_conf
  if ! assignments; then ok "no projects served yet: claude agents will use this machine's own login (run \`claude\` once and log in)"; return; fi
  local vars missing=0
  vars="$(jq -r '[.accounts[]?.token_envar, .projects[].accounts[]?.token_envar] | map(select(. != null and . != "")) | unique | .[]' "$TMPF")"
  [ -n "$vars" ] || { ok "the served projects name no account tokens"; return; }
  for v in $vars; do
    if [ -n "$(envval "$v" "$ENVF")" ]; then ok "$v"; continue; fi
    local val; val="$(ask_secret "$v: paste a token from \`claude setup-token\` (run while logged in to that account; blank = later):")"
    if [ -n "$val" ]; then
      grep -v "^$v=" "$ENVF" > "$TMPF.env" || true; printf '%s=%s\n' "$v" "$val" >> "$TMPF.env"; cat "$TMPF.env" > "$ENVF"; rm -f "$TMPF.env"
      ok "$v (the engine picks it up without a restart)"
    else warn "$v not set: that account is never used here"; missing=1; fi
  done
  [ $missing = 0 ] || warn "add them later: ./iter/install.sh accounts"
}

projects() {
  say "project checkouts"
  load_conf
  if ! assignments; then
    ok "no projects served yet: in the webui's Settings tab draw a serves edge from \"$ENGINE\" to each project, topdir ~/dev/<project>; then ./iter/install.sh projects"
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
    if git clone -q "$url" "$dir"; then ok "$p: cloned into $dir"
    else warn "$p: clone failed (credentials? try: gh auth login && gh auth setup-git)"; fi
  done <<< "$list"
}

status() {
  load_conf
  [ -n "${ENGINE:-}" ] || die "not installed yet: ./iter/install.sh"
  printf 'engine   %s → %s\nservice  %s\n' "$ENGINE" "$DATA_URL" "$(systemctl is-active iter-engine 2>/dev/null || true)"
  "$BIN" key status --key-file "$KEYF" --data-url "$DATA_URL" || true
  if assignments; then jq -r '.projects[] | "project  \(.project)  \(.topdir)  \(.state)"' "$TMPF"; else echo "project  (none served)"; fi
  tail -3 "$IHOME/engine.log" 2>/dev/null || true
}

update() {
  load_conf
  [ -n "${ENGINE:-}" ] || die "not installed yet: ./iter/install.sh"
  say "updating $REPO"
  git -C "$REPO" pull --ff-only -q
  # rebuilds iter_engine and restarts the service (refuses while it runs work, without --yes)
  bash "$REPO/linux/install.sh" update "${1:-}"
}

case "${1:-install}" in
  install)
    migrate_home; packages; claude_cli; settings; build force; model; key; power; desktop; service
    say "9/9 accounts and projects"
    accounts; projects
    cat <<EOF

Done. Engine "$ENGINE" runs as the iter-engine service and checks in with $DATA_URL.
Next, in the webui's Settings tab: draw a serves edge from $ENGINE to each project
(topdir ~/dev/<project>), then run ./iter/install.sh projects here to clone them.
  status:   ./iter/install.sh status        log:  tail -f $IHOME/engine.log
  update:   ./iter/install.sh update        new key:  $BIN key new   (then rotate it in Engine keys)
EOF
    ;;
  projects) migrate_home; projects ;;
  accounts) migrate_home; accounts ;;
  status) migrate_home; status ;;
  update) shift; migrate_home; update "${1:-}" ;;
  -h|--help|help) sed -n '2,/^set -euo/p' "$0" | sed '$d' | sed 's/^# \{0,1\}//' ;;
  *) die "unknown verb $1 (install | projects | accounts | status | update)" ;;
esac
