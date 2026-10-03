#!/usr/bin/env bash
# Start Claude Code on one of the Dev accounts' long-lived tokens.
#
# Reads DEV<n>_TOKEN (a `claude setup-token` value, sk-ant-oat01-...) from the
# .env file next to this script, sets CLAUDE_CODE_OAUTH_TOKEN to it, and runs
#   claude --dangerously-skip-permissions [extra args...]
#
# No secrets live in this file; they stay in .env (git-ignored).
# Usage: ./claude-dev.sh [n] [extra claude args...]
#   n  a single digit picks DEV<n>_TOKEN and skips the prompt (e.g. `./claude-dev.sh 3`)

set -euo pipefail

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
env_file="${ENV_FILE:-$script_dir/.env}"

if [ ! -f "$env_file" ]; then
  echo "error: $env_file not found" >&2
  exit 1
fi

# Read the DEV<n>_TOKEN lines from .env without evaluating it as shell code:
# KEY=VALUE or export KEY=VALUE, blank lines and # comments skipped, one layer
# of matching quotes stripped. Each token lands in a plain, unexported
# dev_token_<n> variable, and every other key is ignored, so nothing from .env
# reaches claude's environment except the one CLAUDE_CODE_OAUTH_TOKEN below.
while IFS= read -r line || [ -n "$line" ]; do
  line="${line%$'\r'}"
  case "$line" in ''|'#'*|[[:space:]]*'#'*) continue ;; esac
  line="${line#"${line%%[![:space:]]*}"}"
  line="${line#export }"
  key="${line%%=*}"
  [ "$key" = "$line" ] && continue
  key="${key%"${key##*[![:space:]]}"}"
  case "$key" in DEV[0-9]_TOKEN) ;; *) continue ;; esac
  val="${line#*=}"
  case "$val" in
    \"*\") val="${val#\"}"; val="${val%\"}" ;;
    \'*\') val="${val#\'}"; val="${val%\'}" ;;
  esac
  n="${key#DEV}"; n="${n%_TOKEN}"
  printf -v "dev_token_$n" '%s' "$val"
done < "$env_file"

# Highest DEV<n>_TOKEN present, for the prompt.
max=0
for n in 0 1 2 3 4 5 6 7 8 9; do
  var="dev_token_$n"
  [ -n "${!var:-}" ] && max=$n
done

# A leading single-digit argument picks the account without prompting; a bad
# pick then exits instead of falling back to the prompt.
preset=""
case "${1:-}" in
  [0-9]) preset="$1"; shift ;;
esac

while :; do
  if [ -n "$preset" ]; then
    choice="$preset"
  else
    printf 'Which Dev account do you want to use (1 - %s)? ' "$max"
    IFS= read -r choice || { echo; exit 1; }
  fi
  case "$choice" in
    [0-9]) ;;
    *) echo "Please enter a single digit, 0 thru 9." >&2; continue ;;
  esac
  var="DEV${choice}_TOKEN"
  tvar="dev_token_$choice"
  token="${!tvar:-}"
  if [ -z "$token" ]; then
    echo "$var is not set in $env_file." >&2
    [ -n "$preset" ] && exit 1
    continue
  fi
  case "$token" in
    sk-ant-oat01-*) break ;;
    *) echo "$var does not look like a long-lived token (sk-ant-oat01-...)." >&2
       [ -n "$preset" ] && exit 1 ;;
  esac
done

export CLAUDE_CODE_OAUTH_TOKEN="$token"
# An API key outranks the OAuth token in Claude Code's auth order, so drop any
# the shell supplied, or the Dev account would be ignored. DEV<n>_TOKEN values
# the shell exported (e.g. from an older run of this script) are dropped too,
# so claude and the commands it runs never see the other accounts' tokens.
unset ANTHROPIC_API_KEY ANTHROPIC_AUTH_TOKEN
unset DEV0_TOKEN DEV1_TOKEN DEV2_TOKEN DEV3_TOKEN DEV4_TOKEN \
      DEV5_TOKEN DEV6_TOKEN DEV7_TOKEN DEV8_TOKEN DEV9_TOKEN
# Claude needs the token to sign in, but the commands it runs don't: every
# Bash-tool command and hook goes through claude-dev-shell.sh, which drops it.
# (CLAUDE_CODE_SUBPROCESS_ENV_SCRUB would also hide it, but it forces the
# permission mode back to default, cancelling --dangerously-skip-permissions.)
export CLAUDE_CODE_SHELL_PREFIX="$script_dir/claude-dev-shell.sh"

echo "Starting Claude Code as Dev account $choice ..."
exec claude --dangerously-skip-permissions "$@"
