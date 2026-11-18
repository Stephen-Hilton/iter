//! The agents' `git` wrapper (built 2026-11-18).  Agents' own `git` commands
//! (Claude Code's Bash tool) used to go straight to git and collide with the
//! engine's in the same checkout: `index.lock` refusals, "cannot lock ref …
//! is at X but expected Y".  The engine writes this POSIX sh wrapper as
//! `{topdir}/.iter/bin/git` beside the `iter` shim (crate::work::write_iter_shim,
//! once per agent session and exec item; that directory is first on their
//! PATH).  It runs the real git:
//!
//! - commands that write (commit, pull, fetch, push, merge, rebase,
//!   checkout/switch/restore, add, rm, mv, reset, stash, cherry-pick, revert,
//!   am, tag and branch when they create/delete/move, worktree but `list`,
//!   gc, maintenance, update-ref) run under `flock` on
//!   `<common git dir>/iter-engine-git.mutex`, the lock the engine's own git
//!   steps hold (crate::gitsafe::guard), so the two never overlap;
//! - every other command, and every command outside a git repository, runs
//!   straight through;
//! - a git started inside a locked one (a hook, git's own child git) sees
//!   `ITER_GIT_LOCKED=1` and runs straight through, so nothing ever waits on
//!   its own parent;
//! - the wait is bounded (`ITER_GIT_LOCK_WAIT` seconds, default 600); after
//!   it the command runs anyway with a warning on stderr;
//! - without `flock(1)` (macOS, Git Bash) everything runs straight through.
//!
//! The real git is the absolute path the engine resolved when it wrote the
//! wrapper (its own PATH, never this directory); when that path is gone the
//! wrapper searches PATH, skipping its own directory and any file carrying
//! the wrapper's signature, so it can never run itself.

use std::path::{Path, PathBuf};

/// The line every copy of the wrapper carries (its signature).
pub const SIGNATURE: &str = "iter-git-wrapper";

/// The wrapper's text; `@REAL_GIT@` is the real git's absolute path.
pub const SCRIPT: &str = r#"#!/bin/sh
# iter-git-wrapper: written by iter_engine (iter_engine/src/gitwrap.rs); do not edit, it is rewritten.
# Runs the real git. Commands that write take the engine's lock
# (<common git dir>/iter-engine-git.mutex) so they never overlap the engine's own git.
REAL_GIT='@REAL_GIT@'
set -f
self_dir=$(cd "$(dirname "$0")" 2>/dev/null && pwd -P)
is_wrapper() { head -c 400 "$1" 2>/dev/null | grep -q 'iter-git-wrapper'; }
if [ ! -x "$REAL_GIT" ] || is_wrapper "$REAL_GIT"; then
  REAL_GIT=
  oldifs=$IFS; IFS=:
  for d in $PATH; do
    [ -n "$d" ] || continue
    dd=$(cd "$d" 2>/dev/null && pwd -P) || continue
    [ "$dd" = "$self_dir" ] && continue
    if [ -f "$d/git" ] && [ -x "$d/git" ] && ! is_wrapper "$d/git"; then REAL_GIT=$d/git; break; fi
  done
  IFS=$oldifs
fi
if [ -z "$REAL_GIT" ]; then
  echo "iter git wrapper: no real git on PATH (only this wrapper, $0)" >&2
  exit 127
fi
# already inside a locked git (a hook, git's own child git): never lock again
if [ "${ITER_GIT_LOCKED:-}" = 1 ]; then exec "$REAL_GIT" "$@"; fi
command -v flock >/dev/null 2>&1 || exec "$REAL_GIT" "$@"

# the subcommand, the -C / --git-dir that place it, and what follows it
nl='
'
sub=; want=; cdirs=; gdir=; first=; pos=0; flags=' '
for a in "$@"; do
  if [ -n "$want" ]; then
    case $want in C) cdirs=$cdirs$a$nl ;; gitdir) gdir=$a ;; esac
    want=; continue
  fi
  if [ -z "$sub" ]; then
    case $a in
      -C) want=C ;;
      --git-dir) want=gitdir ;;
      --git-dir=*) gdir=${a#--git-dir=} ;;
      -c|--namespace|--work-tree|--config-env|--super-prefix|--list-cmds) want=skip ;;
      -*) ;;
      *) sub=$a ;;
    esac
    continue
  fi
  [ -n "$first" ] || first=$a
  case $a in
    --) ;;
    -*) flags="$flags$a " ;;
    *) pos=$((pos + 1)) ;;
  esac
done
has() { for f in "$@"; do case $flags in *" $f "*|*" $f="*) return 0 ;; esac; done; return 1; }
writes=0
case $sub in
  commit|pull|fetch|push|merge|rebase|checkout|switch|restore|add|rm|mv|reset|cherry-pick|revert|am|gc|maintenance|update-ref) writes=1 ;;
  stash) case $first in list|show) ;; *) writes=1 ;; esac ;;
  worktree) case $first in list) ;; *) writes=1 ;; esac ;;
  branch)
    if has -d -D -m -M -c -C --delete --move --copy -f --force -u --set-upstream-to --unset-upstream --edit-description -t --track --no-track; then writes=1
    elif has -l --list -a --all -r --remotes --show-current --contains --no-contains --merged --no-merged --points-at -v -vv --verbose --format --sort; then writes=0
    elif [ "$pos" -gt 0 ]; then writes=1; fi ;;
  tag)
    if has -d --delete -a --annotate -s --sign -f --force -m --message -F --file -u --local-user; then writes=1
    elif has -l --list -n --contains --no-contains --merged --no-merged --points-at --sort --format -v --verify; then writes=0
    elif [ "$pos" -gt 0 ]; then writes=1; fi ;;
esac
[ "$writes" = 1 ] || exec "$REAL_GIT" "$@"

# the repository's common git dir; outside a repository: no lock
common=$(
  IFS=$nl
  for d in $cdirs; do cd "$d" 2>/dev/null || exit 1; done
  if [ -n "$gdir" ]; then GIT_DIR=$gdir; export GIT_DIR; fi
  c=$("$REAL_GIT" rev-parse --git-common-dir 2>/dev/null) || exit 1
  cd "$c" 2>/dev/null && pwd -P
) || exec "$REAL_GIT" "$@"
[ -n "$common" ] || exec "$REAL_GIT" "$@"
lock=$common/iter-engine-git.mutex
( : >> "$lock" ) 2>/dev/null || exec "$REAL_GIT" "$@"
wait=${ITER_GIT_LOCK_WAIT:-600}
ITER_GIT_LOCKED=1; export ITER_GIT_LOCKED
flock -w "$wait" -E 75 "$lock" "$REAL_GIT" "$@"
rc=$?
if [ "$rc" -eq 75 ]; then
  echo "iter git wrapper: WARNING: $lock still held after ${wait}s; running 'git $sub' without the engine's git lock" >&2
  exec "$REAL_GIT" "$@"
fi
exit "$rc"
"#;

/// The real git on `path` (a PATH-style list): the first executable `git`
/// that is not in `skip_dir` and not a copy of the wrapper.
pub fn real_git(path: &str, skip_dir: &Path) -> Option<PathBuf> {
    let skip = iter_core::platform::canonicalize(skip_dir).unwrap_or_else(|_| skip_dir.to_path_buf());
    std::env::split_paths(path).find_map(|d| {
        if d.as_os_str().is_empty() {
            return None;
        }
        let canon = iter_core::platform::canonicalize(&d).unwrap_or_else(|_| d.clone());
        if canon == skip {
            return None;
        }
        let g = d.join("git");
        let meta = std::fs::metadata(&g).ok()?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if !meta.is_file() || meta.permissions().mode() & 0o111 == 0 {
                return None;
            }
        }
        #[cfg(not(unix))]
        let _ = meta;
        if is_wrapper(&g) {
            return None;
        }
        Some(g)
    })
}

fn is_wrapper(p: &Path) -> bool {
    use std::io::Read;
    let mut head = [0u8; 400];
    let n = std::fs::File::open(p).and_then(|mut f| f.read(&mut head)).unwrap_or(0);
    String::from_utf8_lossy(&head[..n]).contains(SIGNATURE)
}

/// Write (or refresh) the wrapper as `dir/git`, resolving the real git on
/// the engine's own PATH.  None when no real git was found (the wrapper is
/// then removed, so agents get plain git).  Unix only: on Windows the shim
/// directory gets no wrapper.
pub fn write(dir: &Path) -> Result<Option<PathBuf>, String> {
    let target = dir.join("git");
    if !cfg!(unix) {
        return Ok(None);
    }
    let Some(real) = real_git(&std::env::var("PATH").unwrap_or_default(), dir) else {
        let _ = std::fs::remove_file(&target);
        return Ok(None);
    };
    let body = SCRIPT.replace("@REAL_GIT@", &real.to_string_lossy().replace('\'', r"'\''"));
    if std::fs::read_to_string(&target).ok().as_deref() != Some(body.as_str()) {
        // a new file renamed into place: a session running the old one keeps it
        let tmp = dir.join(format!(".git.{}.tmp", std::process::id()));
        std::fs::write(&tmp, &body).map_err(|e| format!("write {}: {e}", tmp.display()))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o755));
        }
        std::fs::rename(&tmp, &target).map_err(|e| format!("rename {}: {e}", target.display()))?;
    }
    Ok(Some(target))
}

#[cfg(test)]
#[path = "gitwrap_tests.rs"]
mod tests;
