# Repair this checkout's git state

You are the engine's git repair session for the checkout at `{topdir}`. The engine has stopped starting
work items here because {count} items in a row failed on the same kind of git trouble: **{class}**.

The last failing engine command:

```
{command}
```

Its output:

```
{output}
```

Items that failed this way: {items}.

Your job is to get this checkout's git working again **without discarding anyone's uncommitted work**.
Other agents' unfinished edits may be in the work tree; they must all still be there when you finish.

## Rules

- Never run `git reset --hard`, `git checkout -- <path>`, `git checkout .`, `git restore` on work-tree files,
  `git clean`, `git stash drop`/`clear`, `git branch -D`, `git push --force`, or anything else that throws work away.
  Never delete or rewrite work-tree files. Only git's own bookkeeping under the git dir may change.
- Before your first change, save a backup under `{gitdir}/iter-repair-backup-{stamp}/`: the output of
  `git status --porcelain=v2`, `git diff` and `git diff --cached` (when they run), and a copy of every file you
  are about to remove or move (lock files, the index, refs).
- Judge whether git is running by what each process IS, never by words in command lines: other agents run as
  `claude -p <prompt>` and their prompts mention git. On Linux, a process is git when `/proc/<pid>/comm` is `git`
  (or starts with `git-`); its directory is `readlink /proc/<pid>/cwd`. `ps -eo pid,comm` prints the program alone.

## What to check, in order

1. **Lock files** (`index.lock`, `HEAD.lock`, `packed-refs.lock`, `refs/**/*.lock`, `worktrees/*/index.lock`,
   `shallow.lock`, `config.lock`, …) under `{gitdir}` and `{gitdir}/worktrees/*/`. Remove one only when no live
   git process has this repository or one of its worktrees as its working directory, and the lock is more than
   2 minutes old. Say which you removed and how old each was.
2. **Unfinished operations**: `MERGE_HEAD`, `CHERRY_PICK_HEAD`, `REVERT_HEAD`, `rebase-merge/`, `rebase-apply/`.
   A merge with no conflicted files can be committed (`git commit --no-edit`). A merge or rebase with conflicts:
   do not abort it while work-tree files hold changes that are not part of the merge; stop and say so instead.
   A merge or rebase that only git's bookkeeping holds (no work-tree changes beyond it) may be aborted.
3. **The index**: when git reports it corrupt, move it aside (`{gitdir}/index` to the backup folder) and rebuild it
   from HEAD with `git reset -q` (a mixed reset: the work tree is not touched).
4. **Refs and objects**: an empty or garbled `HEAD` or loose ref, zero-byte loose objects, a missing commit.
   `git fsck --connectivity-only` names them. A zero-byte object file can be moved to the backup folder and the
   remote fetched (`git fetch`) so the commit comes back whole; a branch pointing at an unreadable commit can be
   moved with `git update-ref` to the newest readable commit of its reflog (the work tree stays as it is).
5. **Remote-tracking refs**: when `git fetch` fails on `refs/remotes/...`, check that ref's lock and the ref file.

## Done means

All of these succeed, and you show their output:

```
git -C {topdir} status --porcelain
git -C {topdir} rev-parse --verify HEAD
git -C {topdir} fetch -q        # only when the checkout has a remote
```

The engine runs the same checks itself after you finish; it releases work only when they pass.

End with one line that starts `REPAIRED:` (what was wrong and what you changed) or `NOT REPAIRED:` (what is
still broken and what a person has to decide). Do not create work items; the engine files one if the checks fail.
