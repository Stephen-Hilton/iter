use super::*;

fn sh(cwd: &Path, script: &str) -> String {
    run_shell(&cwd.to_string_lossy(), script, 60).unwrap_or_else(|e| panic!("{script}: {e}"))
}

/// a bare remote and a clone `a` with one pushed commit (base.txt)
fn setup(tag: &str) -> (PathBuf, PathBuf) {
    let root = std::env::temp_dir().join(format!("iter-githeal-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    sh(&root, "git init -q --bare -b main remote.git && git clone -q remote.git a 2>/dev/null && cd a && git config user.email t@t && git config user.name t && git checkout -q -b main && echo one > base.txt && git add . && git commit -qm base && git push -q origin main");
    (root.clone(), root.join("a"))
}

fn top(p: &Path) -> String {
    p.to_string_lossy().into_owned()
}

/// truncate the loose object behind `rev` to 0 bytes, as a power cut leaves it
fn empty_object(a: &Path, rev: &str) -> String {
    let oid = sh(a, &format!("git rev-parse {rev}")).trim().to_string();
    let path = a.join(".git/objects").join(&oid[..2]).join(&oid[2..]);
    assert!(path.is_file(), "{rev} is not a loose object");
    let mut perm = std::fs::metadata(&path).unwrap().permissions();
    #[allow(clippy::permissions_set_readonly_false)]
    perm.set_readonly(false);
    std::fs::set_permissions(&path, perm).unwrap();
    std::fs::write(&path, b"").unwrap();
    oid
}

#[test]
fn healthy_checkout_needs_nothing_but_fsync() {
    let (root, a) = setup("healthy");
    let r = heal(&top(&a));
    assert!(r.broken.is_none(), "{r:?}");
    assert!(r.fixed.iter().all(|f| f.contains("fsync")), "only the fsync setting: {r:?}");
    assert!(!sh(&a, "git config --get core.fsync; git config --get core.fsyncObjectFiles; true").trim().is_empty());
    let again = heal(&top(&a));
    assert!(again.fixed.is_empty() && again.broken.is_none(), "{again:?}");
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn pushed_commit_left_empty_comes_back_from_the_remote() {
    let (root, a) = setup("pushed");
    sh(&a, "echo two >> base.txt && git commit -qam two && git push -q origin main");
    let oid = empty_object(&a, "HEAD");
    assert!(run_shell(&top(&a), "git status", 30).is_err(), "the checkout is broken to start with");
    let r = heal(&top(&a));
    assert!(r.broken.is_none(), "{r:?}");
    assert!(r.fixed.iter().any(|f| f.contains("removed the empty object")), "{r:?}");
    assert!(r.fixed.iter().any(|f| f.contains("fetched origin")), "{r:?}");
    assert_eq!(sh(&a, "git rev-parse HEAD").trim(), oid, "the branch keeps its commit");
    assert_eq!(sh(&a, "git status --porcelain"), "");
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn unpushed_commit_left_empty_rewinds_and_keeps_the_work_tree() {
    let (root, a) = setup("unpushed");
    let base = sh(&a, "git rev-parse HEAD").trim().to_string();
    sh(&a, "echo two >> base.txt && git commit -qam two");
    empty_object(&a, "HEAD");
    let r = heal(&top(&a));
    assert!(r.broken.is_none(), "{r:?}");
    assert!(r.fixed.iter().any(|f| f.contains("main pointed at the unreadable commit")), "{r:?}");
    assert_eq!(sh(&a, "git rev-parse HEAD").trim(), base);
    assert_eq!(std::fs::read_to_string(a.join("base.txt")).unwrap(), "one\ntwo\n", "the work tree is untouched");
    assert_eq!(sh(&a, "git status --porcelain").trim(), "M  base.txt", "the lost commit's change shows as uncommitted (still staged)");
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn stale_locks_go_and_a_live_one_stays() {
    let (root, a) = setup("locks");
    sh(&a, "touch -d '1 hour ago' .git/index.lock && mkdir -p .git/refs/heads && touch -d '1 hour ago' .git/refs/heads/main.lock");
    let r = heal(&top(&a));
    assert!(r.broken.is_none(), "{r:?}");
    assert!(!a.join(".git/index.lock").exists() && !a.join(".git/refs/heads/main.lock").exists(), "{r:?}");
    assert_eq!(r.fixed.iter().filter(|f| f.contains("stale lock")).count(), 2, "{r:?}");
    // a git that is running right now holds its lock
    std::fs::write(a.join(".git/index.lock"), b"").unwrap();
    heal(&top(&a));
    assert!(a.join(".git/index.lock").exists());
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn corrupt_index_is_rebuilt_from_head() {
    let (root, a) = setup("index");
    sh(&a, "echo work >> base.txt");
    std::fs::write(a.join(".git/index"), b"DIRC garbage").unwrap();
    let r = heal(&top(&a));
    assert!(r.broken.is_none(), "{r:?}");
    assert!(r.fixed.iter().any(|f| f.contains("the index was corrupt")), "{r:?}");
    assert_eq!(sh(&a, "git status --porcelain").trim(), "M base.txt");
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn empty_head_and_garbled_branch_ref_are_restored() {
    let (root, a) = setup("refs");
    let oid = sh(&a, "git rev-parse HEAD").trim().to_string();
    std::fs::write(a.join(".git/HEAD"), b"").unwrap();
    std::fs::write(a.join(".git/refs/heads/main"), b"").unwrap();
    let r = heal(&top(&a));
    assert!(r.broken.is_none(), "{r:?}");
    assert_eq!(std::fs::read_to_string(a.join(".git/HEAD")).unwrap(), "ref: refs/heads/main\n");
    assert_eq!(sh(&a, "git rev-parse HEAD").trim(), oid);
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn unrecoverable_checkout_is_held_until_repaired() {
    let (root, a) = setup("held");
    // the only commit, never pushed anywhere, and no remote to fetch it from
    sh(&a, "git remote remove origin");
    sh(&a, "echo two >> base.txt && git commit -qam two");
    empty_object(&a, "HEAD~1");
    empty_object(&a, "HEAD");
    let r = heal(&top(&a));
    assert!(r.broken.is_some(), "{r:?}");
    assert!(a.join(".git").join(FLAG_FILE).is_file());
    // a human starts the history over: the next check passes and lifts the hold
    sh(&a, "rm -rf .git && git init -q -b main && git config user.email t@t && git config user.name t && git add . && git commit -qm fresh");
    let ok = heal(&top(&a));
    assert!(ok.broken.is_none(), "{ok:?}");
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn unborn_repository_and_plain_folder_are_fine() {
    let root = std::env::temp_dir().join(format!("iter-githeal-unborn-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("repo")).unwrap();
    std::fs::create_dir_all(root.join("plain")).unwrap();
    sh(&root.join("repo"), "git init -q -b main");
    let r = heal(&top(&root.join("repo")));
    assert!(r.broken.is_none(), "{r:?}");
    let p = heal(&top(&root.join("plain")));
    assert!(p.broken.is_none() && p.fixed.is_empty(), "{p:?}");
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn healer_holds_until_the_check_finishes() {
    let (root, a) = setup("healer");
    let mut h = Healer::default();
    let t = top(&a);
    assert!(!h.ready(&t, "p"), "the first ask starts the check");
    let deadline = Instant::now() + Duration::from_secs(60);
    while !h.ready(&t, "p") {
        assert!(Instant::now() < deadline, "the check never finished");
        std::thread::sleep(Duration::from_millis(20));
    }
    let _ = std::fs::remove_dir_all(root);
}
