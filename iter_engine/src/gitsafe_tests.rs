use super::*;
use crate::work::run_shell;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

/// A scratch dir under $HOME/.pdy-tmp (never /tmp: Docker Desktop here
/// shares no /tmp, and the disk sweep knows this folder).
fn scratch(tag: &str) -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
    let d = PathBuf::from(home).join(".pdy-tmp").join("iter-gitsafe-tests").join(format!("{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn sh(cwd: &Path, script: &str) -> String {
    // generous: this disk stalls for tens of seconds under load (bakugo, measured)
    run_shell(&cwd.to_string_lossy(), script, 300).unwrap_or_else(|e| panic!("{script}: {e}"))
}

const ID: &str = "git -c user.email=t@t -c user.name=t";

/// A bare remote and a clone of it with one pushed commit on `main`.
fn repo_with_remote(tag: &str) -> (PathBuf, PathBuf) {
    let d = scratch(tag);
    let (remote, clone) = (d.join("remote.git"), d.join("clone"));
    sh(&d, "git init -q --bare -b main remote.git && git clone -q remote.git clone 2>/dev/null");
    sh(&clone, &format!("git checkout -q -b main 2>/dev/null; echo one > a.txt && git add -A && {ID} commit -qm one && git push -q -u origin main"));
    (remote, clone)
}

fn age(p: &Path, secs: u64) {
    let f = std::fs::OpenOptions::new().write(true).open(p).unwrap();
    f.set_modified(SystemTime::now() - Duration::from_secs(secs)).unwrap();
}

fn no_git(_: &GitDirs) -> Vec<(u32, String)> {
    vec![]
}

/// The bakugo incident, reproduced: a fetch killed mid-update leaves
/// `refs/remotes/origin/main.lock`; a plain pull then fails on it.  The
/// engine's pull removes the stale lock (no git alive, older than 2 min)
/// and succeeds, and the deletion is what made the difference.
#[test]
fn a_stale_ref_lock_with_no_live_git_is_cleared_and_the_pull_succeeds() {
    let (remote, clone) = repo_with_remote("stale");
    // another clone pushes a commit the stale clone lacks
    let other = remote.parent().unwrap().join("other");
    sh(remote.parent().unwrap(), "git clone -q remote.git other");
    sh(&other, &format!("echo two > b.txt && git add -A && {ID} commit -qm two && git push -q origin main"));
    let lock = clone.join(".git/refs/remotes/origin/main.lock");
    std::fs::write(&lock, "").unwrap();
    age(&lock, 600);
    // control: git itself refuses while the lock is there
    let refused = run_shell(&clone.to_string_lossy(), "git pull -q --no-rebase origin main", 60).unwrap_err();
    assert_eq!(classify(&refused), Some(Class::RefLock), "{refused}");
    assert!(lock.exists());
    sh(&clone, "git merge --abort 2>/dev/null; true");

    crate::merge::pull(&clone.to_string_lossy(), None).expect("the engine's pull clears the stale lock and pulls");
    assert!(!lock.exists(), "the stale lock was removed");
    assert_eq!(sh(&clone, "git rev-parse HEAD"), sh(&other, "git rev-parse HEAD"), "the clone has the other clone's commit");
}

/// A lock older than 2 minutes is kept while a live git process works in
/// the repository: here a real `git hash-object --stdin` waiting on its
/// input, whose /proc/<pid>/comm is "git" and whose cwd is the checkout.
/// Once that git ends, the same lock is removed (the control).
#[cfg(target_os = "linux")]
#[test]
fn a_lock_held_by_a_live_git_process_is_not_deleted() {
    let (_remote, clone) = repo_with_remote("live");
    let lock = clone.join(".git/index.lock");
    std::fs::write(&lock, "").unwrap();
    age(&lock, 600);
    let mut git = std::process::Command::new("git")
        .args(["hash-object", "--stdin"])
        .current_dir(&clone)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::null())
        .spawn()
        .unwrap();
    // let it exec (comm becomes "git")
    std::thread::sleep(Duration::from_millis(300));
    let d = dirs(&clone).unwrap();
    let users = live_git(&d);
    assert!(users.iter().any(|(pid, _)| *pid == git.id()), "the live git is seen by what it is: {users:?}");
    assert!(clear_stale_locks(&clone).is_empty());
    assert!(lock.exists(), "a live git uses the repository: its lock stays");

    let _ = git.kill();
    let _ = git.wait();
    let cleared = clear_stale_locks(&clone);
    assert_eq!(cleared.len(), 1, "with the git gone the stale lock goes: {cleared:?}");
    assert!(!lock.exists());
    assert!(cleared[0].age_secs >= 599);
}

/// A process that merely mentions git in its arguments (as every agent's
/// `claude -p <prompt>` does) is not a git process.
#[cfg(target_os = "linux")]
#[test]
fn a_process_whose_arguments_mention_git_is_not_taken_for_git() {
    let (_remote, clone) = repo_with_remote("words");
    let mut words = std::process::Command::new("sleep")
        .args(["30"])
        .env("NOTE", "git fetch git pull index.lock")
        .current_dir(&clone)
        .spawn()
        .unwrap();
    std::thread::sleep(Duration::from_millis(200));
    let users = live_git(&dirs(&clone).unwrap());
    let _ = words.kill();
    let _ = words.wait();
    assert!(users.iter().all(|(pid, _)| *pid != words.id()), "{users:?}");
}

/// A lock younger than 2 minutes is never deleted, even with no git alive;
/// aged past the limit, the same file goes (the control).
#[test]
fn a_lock_younger_than_two_minutes_is_not_deleted() {
    let (_remote, clone) = repo_with_remote("young");
    let lock = clone.join(".git/HEAD.lock");
    std::fs::write(&lock, "").unwrap();
    age(&lock, 100);
    assert!(clear_stale_locks_with(&clone, LOCK_STALE, &no_git).is_empty());
    assert!(lock.exists(), "100 s old: kept");
    age(&lock, 130);
    assert_eq!(clear_stale_locks_with(&clone, LOCK_STALE, &no_git).len(), 1);
    assert!(!lock.exists(), "130 s old and no git alive: removed");
}

/// Linked worktrees' own git dirs (`.git/worktrees/<name>/`) are searched,
/// and a live git working in a linked worktree keeps the repository's locks.
#[cfg(target_os = "linux")]
#[test]
fn worktree_locks_are_found_and_a_git_in_a_linked_worktree_counts() {
    let (_remote, clone) = repo_with_remote("wt");
    sh(&clone, "git worktree add -q ../wt -b side");
    let wt = clone.parent().unwrap().join("wt");
    let wlock = clone.join(".git/worktrees/wt/index.lock");
    std::fs::write(&wlock, "").unwrap();
    age(&wlock, 600);
    let d = dirs(&wt).unwrap();
    assert_eq!(d.common, canon(&clone.join(".git")));
    assert!(lock_files(&d).contains(&canon(&wlock)) || lock_files(&d).contains(&wlock), "{:?}", lock_files(&d));
    assert_eq!(key(&wt), key(&clone), "one repository, one lock");
    let mut git = std::process::Command::new("git").args(["hash-object", "--stdin"]).current_dir(&wt).stdin(std::process::Stdio::piped()).spawn().unwrap();
    std::thread::sleep(Duration::from_millis(300));
    assert!(clear_stale_locks(&clone).is_empty(), "a git in the linked worktree keeps the locks");
    let _ = git.kill();
    let _ = git.wait();
    assert_eq!(clear_stale_locks(&clone).len(), 1);
    assert!(!wlock.exists());
}

/// Engine git steps in one repository never overlap: eight threads (two of
/// them through the other worktree's path) each hold the step for 40 ms,
/// and at no moment are two inside.  The cross-process file lock is held
/// meanwhile: a second open of it cannot lock it.
#[test]
fn concurrent_engine_git_steps_are_serialized() {
    let (_remote, clone) = repo_with_remote("serial");
    let top = clone.to_string_lossy().into_owned();
    let inside = Arc::new(AtomicUsize::new(0));
    let most = Arc::new(AtomicUsize::new(0));
    let mut hs = Vec::new();
    for i in 0..8 {
        let (top, inside, most) = (top.clone(), inside.clone(), most.clone());
        hs.push(std::thread::spawn(move || {
            retrying(&top, &format!("step {i}"), || {
                let n = inside.fetch_add(1, Ordering::SeqCst) + 1;
                most.fetch_max(n, Ordering::SeqCst);
                std::thread::sleep(Duration::from_millis(40));
                inside.fetch_sub(1, Ordering::SeqCst);
                Ok::<_, String>(())
            })
            .unwrap();
        }));
    }
    for h in hs {
        h.join().unwrap();
    }
    assert_eq!(most.load(Ordering::SeqCst), 1, "two engine git steps overlapped");

    let g = guard(&top);
    let other = std::fs::OpenOptions::new().write(true).open(clone.join(".git").join(MUTEX_FILE)).unwrap();
    assert!(matches!(other.try_lock(), Err(std::fs::TryLockError::WouldBlock)), "the file lock is held while the guard is");
    // re-entrant: the same thread takes it again without waiting
    let inner = guard(&top);
    assert!(held(&top));
    drop(inner);
    drop(g);
    assert!(!held(&top));
    other.try_lock().expect("released with the guard");
}

/// Real commits from many threads at once (file-sync commits and an
/// end-of-run style `git add`/`commit`) all land, none on index.lock.
#[test]
fn concurrent_engine_commits_all_land() {
    let (_remote, clone) = repo_with_remote("commits");
    sh(&clone, "git config user.email t@t && git config user.name t");
    let mut hs = Vec::new();
    for i in 0..6 {
        let c = clone.clone();
        hs.push(std::thread::spawn(move || {
            std::fs::write(c.join(format!("f{i}.txt")), format!("{i}\n")).unwrap();
            if i % 2 == 0 {
                crate::filesync::commit_paths(&c, &[format!("f{i}.txt")], &format!("c{i}")).map(|_| ())
            } else {
                run(&c.to_string_lossy(), &format!("git add -- f{i}.txt && git commit -q -m c{i} -- f{i}.txt"), 60).map(|_| ())
            }
        }));
    }
    for h in hs {
        h.join().unwrap().expect("every commit lands");
    }
    assert_eq!(sh(&clone, "git log --format=%s | grep -c '^c'").trim(), "6");
}

/// A command that meets a lock another git is still holding (young, so not
/// swept) is retried until that git is done, instead of failing.
#[test]
fn a_command_that_meets_a_live_lock_is_retried_until_it_is_gone() {
    let (_remote, clone) = repo_with_remote("retry");
    let lock = clone.join(".git/index.lock");
    std::fs::write(&lock, "").unwrap();
    let l2 = lock.clone();
    let remover = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(1800));
        std::fs::remove_file(l2).unwrap();
    });
    std::fs::write(clone.join("r.txt"), "r\n").unwrap();
    let out = run(&clone.to_string_lossy(), &format!("git add -A && {ID} commit -qm retried"), 60);
    remover.join().unwrap();
    out.expect("retried past the lock");
    assert_eq!(sh(&clone, "git log -1 --format=%s").trim(), "retried");
}

/// A command that still fails records its class on the thread for the
/// repair loop; a success records nothing.
#[test]
fn a_final_failure_records_its_class() {
    let (_remote, clone) = repo_with_remote("record");
    reset_failure();
    std::fs::write(clone.join(".git/index"), "garbage").unwrap();
    let e = run(&clone.to_string_lossy(), "git status --porcelain", 30).unwrap_err();
    let f = last_failure().expect("recorded");
    assert_eq!(f.class, Class::Index, "{e}");
    assert_eq!(f.command, "git status --porcelain");
    reset_failure();
    assert!(last_failure().is_none());
}

#[test]
fn git_errors_are_classified_from_their_output() {
    let cases = [
        ("fatal: Unable to create '/r/.git/index.lock': File exists.\n\nAnother git process seems to be running", Some(Class::Lock)),
        ("error: cannot lock ref 'refs/heads/main': is at 1111 but expected 2222", Some(Class::RefLock)),
        ("error: Unable to create '/r/.git/refs/remotes/origin/main.lock': File exists.", Some(Class::RefLock)),
        ("error: update_ref failed for ref 'refs/remotes/origin/main': cannot lock ref", Some(Class::RefLock)),
        ("error: bad signature 0x00000000\nfatal: index file corrupt", Some(Class::Index)),
        ("error: object file .git/objects/ab/cdef is empty\nfatal: loose object abcdef is corrupt", Some(Class::Objects)),
        ("fatal: could not parse HEAD", Some(Class::Objects)),
        ("error: You have not concluded your merge (MERGE_HEAD exists).", Some(Class::Unfinished)),
        ("fatal: It seems that there is already a rebase-merge directory", Some(Class::Unfinished)),
        ("fatal: unable to access 'https://x/': Could not resolve host: x", None),
        ("! [rejected] main -> main (fetch first)", None),
        ("CONFLICT (content): Merge conflict in a.txt", None),
    ];
    for (text, want) in cases {
        assert_eq!(classify(text), want, "{text}");
    }
}

/// Every engine git command runs with rerere off (a user's global
/// `rerere.autoupdate` would replay an old resolution into a conflicting
/// pull) and with the lock marker set; the filesync path too.
#[test]
fn engine_git_runs_with_rerere_off_and_the_lock_marker() {
    let (_remote, clone) = repo_with_remote("rerere");
    sh(&clone, "git config rerere.enabled true && git config rerere.autoupdate true");
    let top = clone.to_string_lossy().into_owned();
    assert_eq!(run(&top, "git config --get rerere.enabled", 15).unwrap().trim(), "false");
    assert_eq!(run(&top, "git config --get rerere.autoupdate && echo $ITER_GIT_LOCKED", 15).unwrap().trim(), "false\n1");
    assert_eq!(crate::filesync::git(&clone, &["config", "--get", "rerere.autoupdate"]).unwrap(), "false");
}
