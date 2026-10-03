#!/bin/sh
# Shell wrapper claude-dev.sh hands to Claude Code as CLAUDE_CODE_SHELL_PREFIX.
#
# Claude Code runs every Bash-tool command (and hook) as
#   <this script> '<full command string>'
# so this drops the Dev account's token from the environment and runs the
# command string in the user's shell. Claude Code itself keeps the token; the
# commands it runs never see it.

unset CLAUDE_CODE_OAUTH_TOKEN
exec "${SHELL:-/bin/sh}" -c "$1"
