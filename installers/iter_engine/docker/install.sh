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
#   --data-url URL    iter_data URL (else asked; kept in ~/.iter/engine.conf). iter_data on
#                     this same host: http://host.docker.internal:<port>, not 127.0.0.1
#   --name NAME       this engine's name, unique per machine (default: the hostname)
#   --checkouts DIR   the host folder holding the project checkouts [~/dev], mounted at the
#                     same path; every serves edge's topdir must be inside it
#   --version TAG     release to download, e.g. v2026.10.17 (else $ITER_VERSION, else latest)
#   --binary PATH     put this iter_engine-linux-<arch> file in the image instead
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
set -euo pipefail

REPO_SLUG="Stephen-Hilton/iter"
HERE=""
if [ -n "${BASH_SOURCE[0]:-}" ] && [ -f "${BASH_SOURCE[0]}" ]; then HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"; fi
IHOME="$HOME/.iter"
CONF="$IHOME/engine.conf"
ENVF="$IHOME/.env"
KEYF="$IHOME/engine.key"
MODEL="$IHOME/models/all-MiniLM-L6-v2"
CTX="$IHOME/docker"

VERB=install DRY=0 YES=0 BINARY="" OPT_URL="" OPT_NAME="" OPT_CHECKOUTS="" VERSION="${ITER_VERSION:-}"
while [ $# -gt 0 ]; do
  case "$1" in
    install|status|update|accounts|projects|logs|uninstall) VERB="$1" ;;
    --data-url) OPT_URL="${2:?--data-url needs a URL}"; shift ;;
    --name) OPT_NAME="${2:?--name needs a name}"; shift ;;
    --checkouts) OPT_CHECKOUTS="${2:?--checkouts needs a folder}"; shift ;;
    --version) VERSION="${2:?--version needs a tag}"; shift ;;
    --binary) BINARY="${2:?--binary needs a path}"; shift ;;
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
}

WORK="$(mktemp -d)"; trap 'rm -rf "$WORK"' EXIT
TMPF="$WORK/api"
# compose in the build folder: compose.yml + compose.override.yml + .env
dc() { (cd "$CTX" && docker compose --progress quiet "$@"); }
# a one-off command in the engine image, as you, with the same mounts
in_image() { dc run --rm --no-deps -T engine "$@"; }
image_ready() { [ -f "$CTX/compose.yml" ] && docker image inspect iter-engine:local >/dev/null 2>&1; }
# GET an iter_data path as this engine: body in $TMPF, prints the status (read-only)
api() {
  local tok=""
  if image_ready && [ -f "$KEYF" ]; then tok="$(in_image iter_engine key session --key-file "$KEYF" --data-url "$DATA_URL" 2>/dev/null || true)"; fi
  [ -n "$tok" ] || tok="$(envval ITER_ENGINE_TOKEN "$ENVF")"
  curl -sS --max-time 15 -H "authorization: Bearer $tok" -o "$TMPF" -w '%{http_code}' "$DATA_URL$1" 2>/dev/null || echo 000
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
    http://127.0.0.1*|http://localhost*)
      local fixed; fixed="$(printf '%s' "$url" | sed -E 's#^http://(127\.0\.0\.1|localhost)#http://host.docker.internal#')"
      warn "$url is the container itself in there: using $fixed (this host, through host-gateway)"; url="$fixed" ;;
    https://*|http://host.docker.internal*) ;;
    http://*) warn "$url is plain http: the engine's session token crosses the network unencrypted" ;;
  esac
  host="$(hostname -s 2>/dev/null || hostname)"
  name="${OPT_NAME:-$(ask "engine name [${ENGINE:-$host}]:" "${ENGINE:-$host}")}"
  case "$name" in *[!A-Za-z0-9_.-]*|"") die "engine name \"$name\": letters, digits, - _ . only" ;; esac
  local co; co="$(ask "the host folder that holds the project checkouts [${CHECKOUTS:-$HOME/dev}]:" "${CHECKOUTS:-$HOME/dev}")"
  co="$(expand "$co")"; co="${co%/}"
  case "$co" in /*) ;; *) die "the checkouts folder must be an absolute path" ;; esac
  case "$co" in "$HOME"|"$IHOME"|"$IHOME"/*) die "the checkouts folder must be a folder of its own (e.g. ~/dev), not $co" ;; esac
  run mkdir -p "$IHOME" "$co" "$IHOME/claude" "$IHOME/models"
  printf 'DATA_URL=%s\nENGINE=%s\n' "$url" "$name" | write_file "$CONF"
  DATA_URL="$url" ENGINE="$name" CHECKOUTS="$co"
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
EOF
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
    docker_ok; settings; build_files; model; image; key; up
    say "8/8 accounts and projects"
    accounts; projects
    [ $DRY = 0 ] || { printf '\nDry run done: nothing was changed.\n'; exit 0; }
    cat <<EOF

Done. Engine "$ENGINE" runs in the container iter-engine and checks in with $DATA_URL.
Next, in the webui's Settings tab: draw a serves edge from $ENGINE to each project
(topdir inside $CHECKOUTS, e.g. ~/dev/<project>), then run \`install.sh projects\`.
  status:  install.sh status        log:  install.sh logs
  update:  install.sh update        (rebuilds the image: a newer iter_engine and claude CLI)
EOF
    ;;
  projects) projects ;;
  accounts) accounts ;;
  status) status ;;
  update) update ;;
  logs) run docker logs -f --tail 100 iter-engine ;;
  uninstall) uninstall ;;
esac
