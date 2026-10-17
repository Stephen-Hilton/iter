#!/usr/bin/env bash
# entrypoint of the iter engine container. With ITER_UID / ITER_GID /
# ITER_HOME_DIR set (compose.yml), it runs the command as that uid/gid with
# HOME = ITER_HOME_DIR, the host user's own home path, so the engine's
# ~/.iter and the ~/dev/<project> checkouts are the same absolute paths inside
# and outside. Without them it runs the command as is (`docker run --rm image
# iter_engine --help`). No command = the engine itself.
set -euo pipefail

if [ $# -eq 0 ]; then
  : "${ITER_DATA_URL:?ITER_DATA_URL is not set}" "${ITER_ENGINE:?ITER_ENGINE is not set}"
  h="${ITER_HOME_DIR:-$HOME}"
  set -- iter_engine --data-url "$ITER_DATA_URL" --name "$ITER_ENGINE" \
    --env-file "$h/.iter/.env" --key-file "$h/.iter/engine.key"
fi

if [ -n "${ITER_UID:-}" ] && [ "$(id -u)" = 0 ] && [ "$ITER_UID" != 0 ]; then
  : "${ITER_GID:?ITER_GID is not set}" "${ITER_HOME_DIR:?ITER_HOME_DIR is not set}"
  getent group "$ITER_GID" >/dev/null || groupadd -g "$ITER_GID" iter
  if ! getent passwd "$ITER_UID" >/dev/null; then
    # (quiet about a macOS uid like 501 being below Debian's UID_MIN; a real failure repeats loudly)
    useradd -u "$ITER_UID" -g "$ITER_GID" -d "$ITER_HOME_DIR" -M -s /bin/bash iter 2>/dev/null \
      || useradd -u "$ITER_UID" -g "$ITER_GID" -d "$ITER_HOME_DIR" -M -s /bin/bash iter
  fi
  # the home folder itself (the mounts below it belong to the host)
  mkdir -p "$ITER_HOME_DIR"
  chown "$ITER_UID:$ITER_GID" "$ITER_HOME_DIR"
  export HOME="$ITER_HOME_DIR" USER
  USER="$(getent passwd "$ITER_UID" | cut -d: -f1)"
  exec setpriv --reuid "$ITER_UID" --regid "$ITER_GID" --init-groups -- "$@"
fi
exec "$@"
