use super::*;
use crate::work::run_shell;
use std::time::{Duration, Instant};

/// A scratch dir under $HOME/.pdy-tmp.
fn scratch(tag: &str) -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
    let d = PathBuf::from(home).join(".pdy-tmp").join("iter-gitwrap-tests").join(format!("{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn sh(cwd: &Path, script: &str) -> String {
    run_shell(&cwd.to_string_lossy(), script, 300).unwrap_or_else(|e| panic!("{script}: {e}"))
}

/// A repository with one commit and the wrapper written in `<scratch>/shim`.
fn setup(tag: &str) -> (PathBuf, PathBuf) {
    let d = scratch(tag);
    let repo = d.join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    sh(&repo, "git init -q && git config user.email t@t && git config user.name t && git config rerere.enabled false && echo a > a.txt && git add -A && git commit -qm seed");
    let shim = d.join("shim");
    std::fs::create_dir_all(&shim).unwrap();
    write(&shim).unwrap().expect("a real git exists here");
    (repo, shim)
}

/// `script` in `repo` with the wrapper first on PATH; its stderr is
/// returned with its output (Err on exit status or `secs` timeout).
fn wrapped(repo: &Path, shim: &Path, env: &str, script: &str, secs: u64) -> Result<(String, String), String> {
    let err = repo.parent().unwrap().join(format!("stderr-{}.txt", uuid::Uuid::new_v4()));
    let full = format!("export PATH='{}':\"$PATH\"; {env} {{ {script} ; }} 2> '{}'", shim.display(), err.display());
    let r = run_shell(&repo.to_string_lossy(), &full, secs);
    let e = std::fs::read_to_string(&err).unwrap_or_default();
    r.map(|o| (o, e.clone())).map_err(|x| format!("{x}\nstderr: {e}"))
}

fn hook(repo: &Path, name: &str, body: &str) {
    let p = repo.join(".git/hooks").join(name);
    std::fs::write(&p, format!("#!/bin/sh\n{body}\n")).unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
}

/// The wrapper is what agents' `git` resolves to, and it runs the real git.
#[test]
fn the_wrapper_is_found_first_and_runs_the_real_git() {
    let (repo, shim) = setup("found");
    let (out, _) = wrapped(&repo, &shim, "", "command -v git; git --version", 30).unwrap();
    assert!(out.starts_with(&format!("{}/git\n", shim.display())), "{out}");
    assert!(out.contains("git version"), "{out}");
    let text = std::fs::read_to_string(shim.join("git")).unwrap();
    assert!(text.contains(SIGNATURE) && !text.contains("@REAL_GIT@"));
}

/// Hooks run git themselves: a pre-commit `git add` and a post-commit
/// `git commit`, both inside the wrapper's locked commit.  They see the
/// marker and run straight through: no wait, no warning, all commits land.
/// (git puts its exec-path first on a hook's PATH, and here that holds a
/// real `git`, which alone would bypass the wrapper; the hooks put the shim
/// first again, as a tool that rebuilds PATH does, so the wrapper is what
/// they run.)
#[test]
fn hooks_that_run_git_add_and_git_commit_do_not_deadlock() {
    let (repo, shim) = setup("hooks");
    let p = format!("PATH='{}':\"$PATH\"; export PATH; [ \"$(command -v git)\" = '{}/git' ] || {{ echo hook-not-wrapped >&2; exit 1; }}", shim.display(), shim.display());
    hook(&repo, "pre-commit", &format!("{p}\necho h > hooked.txt && git add hooked.txt"));
    hook(&repo, "post-commit", &format!("{p}\n[ -n \"$INNER\" ] || INNER=1 git commit -q --no-verify --allow-empty -m inner"));
    std::fs::write(repo.join("b.txt"), "b\n").unwrap();
    let started = Instant::now();
    let (_, err) = wrapped(&repo, &shim, "export ITER_GIT_LOCK_WAIT=5;", "git add b.txt && git commit -qm outer", 60).unwrap();
    let took = started.elapsed();
    assert!(!err.contains("WARNING"), "a nested git waited on its parent's lock: {err}");
    assert!(took < Duration::from_secs(4), "took {took:?}");
    assert_eq!(sh(&repo, "git log --format=%s -3").trim(), "inner\nouter\nseed");
    assert!(sh(&repo, "git show --name-only --format= HEAD~1").contains("hooked.txt"));
}

/// An agent's commit (through the wrapper) and the engine's guarded commit
/// never overlap, in either order: while the engine holds its guard the
/// agent's commit waits, and lands once the guard is dropped; while an
/// agent's commit holds the lock (a slow pre-commit hook) the engine's guard
/// waits for it.
#[test]
fn an_agent_commit_and_an_engine_commit_are_serialized() {
    let (repo, shim) = setup("serial");
    let top = repo.to_string_lossy().into_owned();
    // engine first
    let g = crate::gitsafe::guard(&top);
    std::fs::write(repo.join("agent.txt"), "x\n").unwrap();
    let full = format!("export PATH='{}':\"$PATH\" ITER_GIT_LOCK_WAIT=60; git add agent.txt && git commit -qm agent", shim.display());
    let mut agent = std::process::Command::new("sh").arg("-c").arg(&full).current_dir(&repo).spawn().unwrap();
    std::thread::sleep(Duration::from_millis(1500));
    assert!(agent.try_wait().unwrap().is_none(), "the agent's git add waits while the engine holds the lock");
    assert_eq!(sh(&repo, "git log -1 --format=%s").trim(), "seed");
    drop(g);
    let st = agent.wait().unwrap();
    assert!(st.success());
    assert_eq!(sh(&repo, "git log -1 --format=%s").trim(), "agent");

    // agent first: its pre-commit hook holds the lock for 2 s
    hook(&repo, "pre-commit", "sleep 2");
    std::fs::write(repo.join("agent2.txt"), "y\n").unwrap();
    let full = format!("export PATH='{}':\"$PATH\"; git add agent2.txt && git commit -qm agent2", shim.display());
    let mut agent = std::process::Command::new("sh").arg("-c").arg(&full).current_dir(&repo).spawn().unwrap();
    std::thread::sleep(Duration::from_millis(700));
    let started = Instant::now();
    let g = crate::gitsafe::guard(&top);
    let waited = started.elapsed();
    drop(g);
    assert!(agent.wait().unwrap().success());
    assert!(waited >= Duration::from_millis(800), "the engine's guard waited for the agent's commit ({waited:?})");
}

/// Read-only commands never wait, even while the engine holds the lock.
#[test]
fn read_only_commands_do_not_wait_on_a_held_lock() {
    let (repo, shim) = setup("readonly");
    let g = crate::gitsafe::guard(&repo.to_string_lossy());
    let started = Instant::now();
    let (out, err) = wrapped(
        &repo,
        &shim,
        "export ITER_GIT_LOCK_WAIT=30;",
        "git status --porcelain; git log -1 --format=%s; git diff; git branch; git branch --list; git tag -l; git stash list; git show --stat HEAD >/dev/null; git rev-parse --git-common-dir; git -C . worktree list >/dev/null",
        60,
    )
    .unwrap();
    let took = started.elapsed();
    drop(g);
    assert!(took < Duration::from_secs(5), "read-only commands waited ({took:?}): {err}");
    assert!(out.contains("seed") && !err.contains("WARNING"), "{out} {err}");
}

/// The wrapper never runs itself: its recorded real git points at itself,
/// and a second copy sits in another PATH directory before the real git; it
/// skips both and runs the real one.
#[test]
fn the_wrapper_never_recurses_into_itself() {
    let (repo, shim) = setup("recurse");
    let me = shim.join("git");
    std::fs::write(&me, SCRIPT.replace("@REAL_GIT@", &me.to_string_lossy())).unwrap();
    let shim2 = shim.parent().unwrap().join("shim2");
    std::fs::create_dir_all(&shim2).unwrap();
    std::fs::write(shim2.join("git"), SCRIPT.replace("@REAL_GIT@", &shim2.join("git").to_string_lossy())).unwrap();
    use std::os::unix::fs::PermissionsExt;
    for p in [&me, &shim2.join("git")] {
        std::fs::set_permissions(p, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    let both = format!("{}:{}", shim.display(), shim2.display());
    let (out, _) = wrapped(&repo, Path::new(&both), "", "git --version && git commit -q --allow-empty -m via-search", 20).expect("no recursion");
    assert!(out.contains("git version"), "{out}");
    assert_eq!(sh(&repo, "git log -1 --format=%s").trim(), "via-search");
    // the engine's resolver skips the shim directory and wrapper copies too
    let path = format!("{}:{}:{}", shim.display(), shim2.display(), std::env::var("PATH").unwrap());
    let real = real_git(&path, &shim).unwrap();
    assert!(!is_wrapper(&real) && real != me && real != shim2.join("git"), "{real:?}");
}

/// A lock that is never released (the engine holds it throughout) ends in
/// a timeout and a warning, and the command runs anyway, instead of a hang.
#[test]
fn a_lock_that_never_releases_times_out_with_a_warning() {
    let (repo, shim) = setup("timeout");
    let _g = crate::gitsafe::guard(&repo.to_string_lossy());
    std::fs::write(repo.join("late.txt"), "z\n").unwrap();
    let started = Instant::now();
    let (_, err) = wrapped(&repo, &shim, "export ITER_GIT_LOCK_WAIT=2;", "git add late.txt", 30).expect("ran after the timeout");
    let took = started.elapsed();
    assert!(err.contains("WARNING") && err.contains("still held after 2s"), "{err}");
    assert!(took >= Duration::from_secs(2) && took < Duration::from_secs(10), "{took:?}");
    assert!(sh(&repo, "git diff --cached --name-only").contains("late.txt"), "the command ran anyway");
}

/// Outside a git repository a writing command takes no lock and runs at once.
#[test]
fn outside_a_repository_no_lock_is_taken() {
    let (repo, shim) = setup("norepo");
    let outside = repo.parent().unwrap().join("plain");
    std::fs::create_dir_all(&outside).unwrap();
    let started = Instant::now();
    let r = wrapped(&outside, &shim, "export ITER_GIT_LOCK_WAIT=30 GIT_CEILING_DIRECTORIES=\"$(dirname \"$PWD\")\";", "git add x 2>&1; true", 30).unwrap();
    assert!(started.elapsed() < Duration::from_secs(5));
    assert!(r.0.contains("not a git repository"), "{r:?}");
    assert!(!outside.join(".git").exists());
}
