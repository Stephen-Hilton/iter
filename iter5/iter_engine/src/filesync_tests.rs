use super::*;
use crate::client::fake::{self, FakeServer};
use std::sync::{Arc, Mutex};

/// What the fake iter_data does / saw.
#[derive(Default)]
struct World {
    version: u64,
    locks: Vec<String>,
    pending: Vec<Value>,
    /// path → text the server wants written instead (reply `rewrite`)
    rewrite: Option<(String, String)>,
    fail_sync: bool,
    graph: Value,
}

fn server(world: Arc<Mutex<World>>) -> FakeServer {
    fake::serve(move |m, path, body| {
        let mut w = world.lock().unwrap();
        if m == "POST" && path.ends_with("/files/sync") {
            if w.fail_sync {
                return Some((503, json!({"error": "down"})));
            }
            let mut applied = Vec::new();
            for f in body["files"].as_array().into_iter().flatten() {
                w.version += 1;
                let p = f["path"].as_str().unwrap();
                let id = nodefile::parse_tolerant(p, f["text"].as_str().unwrap()).map(|(d, _)| d.id).unwrap_or_default();
                applied.push(json!({"id": id, "path": p, "node_version": w.version}));
            }
            let rewrite: Vec<Value> = w.rewrite.take().map(|(p, t)| vec![json!({"path": p, "text": t, "node_version": 99})]).unwrap_or_default();
            return Some((200, json!({"applied": applied, "rewrite": rewrite, "removed": [], "conflicts": []})));
        }
        if m == "GET" && path.ends_with("/locks") {
            let rows: Vec<Value> = w.locks.iter().map(|l| json!({"kind": "lock", "path": l, "workid": "w-000000000001", "holder_lease_live": true})).collect();
            return Some((200, json!(rows)));
        }
        if m == "GET" && path.ends_with("/files/pending") {
            return Some((200, json!(w.pending.clone())));
        }
        if m == "POST" && path.ends_with("/files/ack") {
            let acked: Vec<String> = body["acks"].as_array().unwrap().iter().map(|a| a["id"].as_str().unwrap().to_string()).collect();
            w.pending.retain(|p| !acked.contains(&p["id"].as_str().unwrap_or("").to_string()));
            return Some((200, json!({"ok": true})));
        }
        if m == "POST" && path.ends_with("/build/done") {
            return Some((200, json!({"ok": true})));
        }
        if m == "GET" && path.ends_with("/graph") {
            return Some((200, w.graph.clone()));
        }
        Some((404, json!({"error": "no route"})))
    })
}

fn tmp(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("iter5_filesync_{name}_{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&d).unwrap();
    iter_core::platform::canonicalize(&d).unwrap()
}

fn sh(top: &Path, args: &[&str]) -> String {
    git(top, args).unwrap()
}

fn repo(name: &str) -> PathBuf {
    let top = tmp(name);
    sh(&top, &["init", "-q"]);
    sh(&top, &["config", "user.email", "t@t"]);
    sh(&top, &["config", "user.name", "t"]);
    std::fs::write(top.join(".gitignore"), ".iter/\n").unwrap();
    std::fs::write(top.join("README.md"), "hi\n").unwrap();
    sh(&top, &["add", "-A"]);
    sh(&top, &["commit", "-q", "-m", "init"]);
    top
}

fn write(top: &Path, rel: &str, text: &str) {
    let p = top.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, text).unwrap();
}

fn conformed(rel: &str, name: &str) -> String {
    let now = "2026-10-02 10:00:00Z";
    let t = nodefile::type_of(rel).unwrap();
    let mut d = nodefile::NodeDoc::new(t, name, "t", now);
    d.path = format!("{{topdir}}/{rel}");
    nodefile::render(&d)
}

fn syncs(srv: &FakeServer) -> Vec<Value> {
    srv.calls_to("POST", "/files/sync")
}

fn sent_paths(call: &Value) -> Vec<String> {
    call["files"].as_array().unwrap().iter().map(|f| f["path"].as_str().unwrap().to_string()).collect()
}

fn last_commit_files(top: &Path) -> Vec<String> {
    let mut v: Vec<String> = sh(top, &["show", "--no-renames", "--name-only", "--format=", "HEAD"]).lines().map(String::from).collect();
    v.sort();
    v
}

#[test]
fn first_tick_is_full_conforms_writes_back_and_commits() {
    let top = repo("first");
    write(&top, "global/p.project.iter.md", &conformed("global/p.project.iter.md", "P"));
    write(&top, "src/a/a.code.iter.md", "---\nname: A\nlevel: container\n---\n# A\n");
    write(&top, "src/a/x.agentmem.iter.md", "memory, never synced\n");
    write(&top, "src/a/plain.iter.md", "a plain doc\n");
    write(&top, "src/a/code.rs", "fn main() {}\n");
    write(&top, "unrelated.txt", "someone else's work\n");
    let world = Arc::new(Mutex::new(World::default()));
    let srv = server(world.clone());
    let mut st = FileSyncState::default();
    tick(&srv.api(), "e1", "p", &top, false, false, &mut st).unwrap();
    let calls = syncs(&srv);
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0]["full"], true, "no saved state: the first sync is full");
    assert_eq!(calls[0]["engine"], "e1");
    assert_eq!(sent_paths(&calls[0]), vec!["{topdir}/global/p.project.iter.md", "{topdir}/src/a/a.code.iter.md"]);
    for f in calls[0]["files"].as_array().unwrap() {
        assert_eq!(f["base_version"], 0);
        assert_eq!(f["hash"], nodefile::content_hash(f["text"].as_str().unwrap()));
    }
    // the code file got an id etc. on disk, identical to what was sent
    let disk = std::fs::read_to_string(top.join("src/a/a.code.iter.md")).unwrap();
    assert!(disk.contains("\nid: ") && disk.contains("creator: \"engine.e1\"") || disk.contains("creator: engine.e1"), "{disk}");
    assert_eq!(calls[0]["files"][1]["text"], disk.as_str());
    assert_eq!(std::fs::read_to_string(top.join("src/a/x.agentmem.iter.md")).unwrap(), "memory, never synced\n");
    assert_eq!(st.last.conformed, 1);
    assert_eq!(sh(&top, &["log", "-1", "--format=%s"]), "iter: conform 1 node file");
    assert_eq!(last_commit_files(&top), vec!["src/a/a.code.iter.md"], "only the conformed file is committed");
    assert!(sh(&top, &["status", "--porcelain"]).contains("unrelated.txt"));
    // state persisted
    let state: Value = serde_json::from_str(&std::fs::read_to_string(top.join(STATE_FILE)).unwrap()).unwrap();
    assert_eq!(state["project"], "p");
    assert_eq!(state["files"]["src/a/a.code.iter.md"]["version"], 2);
}

#[test]
fn quiet_ticks_cost_nothing_and_changes_carry_base_version() {
    let top = repo("changes");
    write(&top, "global/p.project.iter.md", &conformed("global/p.project.iter.md", "P"));
    write(&top, "src/a/a.code.iter.md", &conformed("src/a/a.code.iter.md", "A"));
    let world = Arc::new(Mutex::new(World::default()));
    let srv = server(world.clone());
    let api = srv.api();
    let mut st = FileSyncState::default();
    tick(&api, "e1", "p", &top, false, false, &mut st).unwrap();
    assert_eq!(st.last.conformed, 0, "already conformed: nothing written");
    let v_a = st.known("src/a/a.code.iter.md").unwrap().version;
    assert!(v_a > 0);
    // no change: no request at all, nothing read
    tick(&api, "e1", "p", &top, false, false, &mut st).unwrap();
    assert_eq!(syncs(&srv).len(), 1);
    assert_eq!((st.last.full, st.last.read), (false, 0));
    // touched only (same bytes, new mtime): read but not sent
    std::thread::sleep(Duration::from_millis(20));
    let t = std::fs::read_to_string(top.join("src/a/a.code.iter.md")).unwrap();
    std::fs::write(top.join("src/a/a.code.iter.md"), &t).unwrap();
    tick(&api, "e1", "p", &top, false, false, &mut st).unwrap();
    assert_eq!(syncs(&srv).len(), 1);
    // a real edit: sent with base_version, last_modified bumped, written back
    std::fs::write(top.join("src/a/a.code.iter.md"), t.replace("desc: \"\"", "desc: \"Edited by hand\"")).unwrap();
    tick(&api, "e1", "p", &top, false, false, &mut st).unwrap();
    let calls = syncs(&srv);
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[1]["full"], false);
    assert_eq!(sent_paths(&calls[1]), vec!["{topdir}/src/a/a.code.iter.md"]);
    assert_eq!(calls[1]["files"][0]["base_version"], v_a);
    assert!(calls[1]["files"][0]["text"].as_str().unwrap().contains("Edited by hand"));
    assert!(!calls[1]["files"][0]["text"].as_str().unwrap().contains("last_modified: \"2026-10-02 10:00:00Z\""), "a hand edit bumps last_modified");
    // new file in a new folder: found by the quick tick (parent dir mtime)
    write(&top, "src/b/deep/b.techreq.iter.md", "---\nname: B\n---\nbody\n");
    tick(&api, "e1", "p", &top, false, false, &mut st).unwrap();
    assert!(!st.last.full);
    assert_eq!(sent_paths(&syncs(&srv)[2]), vec!["{topdir}/src/b/deep/b.techreq.iter.md"]);
    // deletion
    std::fs::remove_file(top.join("src/a/a.code.iter.md")).unwrap();
    tick(&api, "e1", "p", &top, false, false, &mut st).unwrap();
    let c = &syncs(&srv)[3];
    assert_eq!(c["deleted"], json!(["{topdir}/src/a/a.code.iter.md"]));
    assert!(c["files"].as_array().unwrap().is_empty());
    assert!(st.known("src/a/a.code.iter.md").is_none());
    // a whole folder removed
    std::fs::remove_dir_all(top.join("src/b")).unwrap();
    tick(&api, "e1", "p", &top, false, false, &mut st).unwrap();
    assert_eq!(syncs(&srv)[4]["deleted"], json!(["{topdir}/src/b/deep/b.techreq.iter.md"]));
}

#[test]
fn restart_resends_only_what_changed() {
    let top = repo("restart");
    write(&top, "global/p.project.iter.md", &conformed("global/p.project.iter.md", "P"));
    write(&top, "src/a/a.code.iter.md", &conformed("src/a/a.code.iter.md", "A"));
    write(&top, "src/c/c.code.iter.md", &conformed("src/c/c.code.iter.md", "C"));
    let world = Arc::new(Mutex::new(World::default()));
    let srv = server(world.clone());
    let mut st = FileSyncState::default();
    tick(&srv.api(), "e1", "p", &top, false, false, &mut st).unwrap();
    let v_c = st.known("src/c/c.code.iter.md").unwrap().version;
    drop(st);
    // while down: one file edited, one gone, one new
    std::thread::sleep(Duration::from_millis(20));
    let t = std::fs::read_to_string(top.join("src/c/c.code.iter.md")).unwrap();
    std::fs::write(top.join("src/c/c.code.iter.md"), format!("{t}\nmore text\n")).unwrap();
    std::fs::remove_file(top.join("src/a/a.code.iter.md")).unwrap();
    write(&top, "src/d/d.bizreq.iter.md", &conformed("src/d/d.bizreq.iter.md", "D"));
    let mut st = FileSyncState::default();
    tick(&srv.api(), "e1", "p", &top, false, false, &mut st).unwrap();
    let c = &syncs(&srv)[1];
    assert_eq!(c["full"], false, "saved state: not a full sync");
    assert_eq!(sent_paths(c), vec!["{topdir}/src/c/c.code.iter.md", "{topdir}/src/d/d.bizreq.iter.md"]);
    assert_eq!(c["files"][0]["base_version"], v_c);
    assert_eq!(c["deleted"], json!(["{topdir}/src/a/a.code.iter.md"]));
}

#[test]
fn failed_sync_is_retried_and_rewrites_are_written_and_committed() {
    let top = repo("rewrite");
    write(&top, "global/p.project.iter.md", &conformed("global/p.project.iter.md", "P"));
    let world = Arc::new(Mutex::new(World { fail_sync: true, ..Default::default() }));
    let srv = server(world.clone());
    let mut st = FileSyncState::default();
    assert!(tick(&srv.api(), "e1", "p", &top, false, false, &mut st).is_err());
    world.lock().unwrap().fail_sync = false;
    let new_text = conformed("global/p.project.iter.md", "P from the server");
    world.lock().unwrap().rewrite = Some(("{topdir}/global/p.project.iter.md".into(), new_text.clone()));
    tick(&srv.api(), "e1", "p", &top, false, false, &mut st).unwrap();
    assert_eq!(sent_paths(&syncs(&srv)[1]), vec!["{topdir}/global/p.project.iter.md"], "the failed file is sent again");
    assert_eq!(std::fs::read_to_string(top.join("global/p.project.iter.md")).unwrap(), new_text);
    assert_eq!(st.last.rewritten, 1);
    assert_eq!(st.known("global/p.project.iter.md").unwrap().version, 99);
    assert_eq!(last_commit_files(&top), vec!["global/p.project.iter.md"]);
    // the rewritten file is not echoed back
    tick(&srv.api(), "e1", "p", &top, false, false, &mut st).unwrap();
    assert_eq!(syncs(&srv).len(), 2);
}

#[test]
fn conform_inside_a_live_lock_waits_for_the_lock() {
    let top = repo("lockconform");
    write(&top, "global/p.project.iter.md", &conformed("global/p.project.iter.md", "P"));
    write(&top, "src/busy/b.code.iter.md", "---\nname: B\nlevel: component\n---\nbody\n");
    let raw = std::fs::read_to_string(top.join("src/busy/b.code.iter.md")).unwrap();
    let world = Arc::new(Mutex::new(World { locks: vec!["{topdir}/src/busy".into()], ..Default::default() }));
    let srv = server(world.clone());
    let mut st = FileSyncState::default();
    tick(&srv.api(), "e1", "p", &top, false, false, &mut st).unwrap();
    let sent = syncs(&srv)[0]["files"].as_array().unwrap().iter().find(|f| f["path"] == "{topdir}/src/busy/b.code.iter.md").unwrap()["text"].as_str().unwrap().to_string();
    assert!(sent.contains("\nid: "), "the node gets the conformed text at once");
    assert_eq!(std::fs::read_to_string(top.join("src/busy/b.code.iter.md")).unwrap(), raw, "the file is left to the running item");
    tick(&srv.api(), "e1", "p", &top, false, false, &mut st).unwrap();
    assert_eq!(syncs(&srv).len(), 1, "waiting does not resend");
    world.lock().unwrap().locks.clear();
    tick(&srv.api(), "e1", "p", &top, false, false, &mut st).unwrap();
    assert_eq!(std::fs::read_to_string(top.join("src/busy/b.code.iter.md")).unwrap(), sent, "written once the lock is gone");
    assert_eq!(sh(&top, &["log", "-1", "--format=%s"]), "iter: conform 1 node file");
    tick(&srv.api(), "e1", "p", &top, false, false, &mut st).unwrap();
    assert_eq!(syncs(&srv).len(), 1, "the write-back is not sent again");
}

#[test]
fn pending_edits_are_written_committed_and_acked() {
    let top = repo("pending");
    write(&top, "global/p.project.iter.md", &conformed("global/p.project.iter.md", "P"));
    write(&top, "src/old/o.code.iter.md", &conformed("src/old/o.code.iter.md", "O"));
    write(&top, "src/gone/g.test.iter.md", &conformed("src/gone/g.test.iter.md", "G"));
    write(&top, "unrelated.txt", "x\n");
    sh(&top, &["add", "-A", "--", "src", "global"]);
    sh(&top, &["commit", "-q", "-m", "nodes"]);
    let new_text = conformed("src/new/n.code.iter.md", "N");
    let moved_text = conformed("src/moved/o.code.iter.md", "O");
    let pending = vec![
        json!({"id": "n1", "op": "write", "path": "{topdir}/src/new/n.code.iter.md", "text": new_text, "node_version": 4}),
        json!({"id": "g1", "op": "delete", "path": "{topdir}/src/gone/g.test.iter.md", "text": "", "node_version": 5}),
        json!({"id": "o1", "op": "move", "path": "{topdir}/src/moved/o.code.iter.md", "old_path": "{topdir}/src/old/o.code.iter.md", "text": moved_text, "node_version": 6}),
        json!({"id": "x1", "op": "write", "path": "{topdir}/../escape.code.iter.md", "text": "evil", "node_version": 7}),
        json!({"id": "l1", "op": "write", "path": "{topdir}/src/busy/l.code.iter.md", "text": "later", "node_version": 8}),
    ];
    let world = Arc::new(Mutex::new(World { pending, locks: vec!["{topdir}/src/busy/".into()], ..Default::default() }));
    let srv = server(world.clone());
    let mut st = FileSyncState::default();
    tick(&srv.api(), "e1", "p", &top, false, true, &mut st).unwrap();
    assert_eq!(std::fs::read_to_string(top.join("src/new/n.code.iter.md")).unwrap(), new_text);
    assert!(!top.join("src/gone/g.test.iter.md").exists());
    assert!(!top.join("src/old/o.code.iter.md").exists());
    assert_eq!(std::fs::read_to_string(top.join("src/moved/o.code.iter.md")).unwrap(), moved_text);
    assert!(!top.parent().unwrap().join("escape.code.iter.md").exists(), "never outside the topdir");
    assert!(!top.join("src/busy").exists(), "a locked path waits");
    assert_eq!((st.last.pending_applied, st.last.pending_waiting), (3, 1));
    let msg = sh(&top, &["log", "-1", "--format=%s"]);
    assert!(msg.starts_with("iter: graph edit — write n.code.iter.md, delete g.test.iter.md, move o.code.iter.md → o.code.iter.md"), "{msg}");
    assert_eq!(last_commit_files(&top), vec!["src/gone/g.test.iter.md", "src/moved/o.code.iter.md", "src/new/n.code.iter.md", "src/old/o.code.iter.md"]);
    assert!(sh(&top, &["status", "--porcelain"]).contains("unrelated.txt"), "nobody else's files committed");
    let head = sh(&top, &["rev-parse", "HEAD"]);
    let acks = srv.calls_to("POST", "/files/ack");
    assert_eq!(acks.len(), 1);
    assert_eq!(acks[0]["engine"], "e1");
    let a = acks[0]["acks"].as_array().unwrap();
    assert_eq!(a.len(), 3);
    assert_eq!(a[0], json!({"id": "n1", "node_version": 4, "path": "{topdir}/src/new/n.code.iter.md", "hash": nodefile::content_hash(&new_text), "commit": head}));
    assert_eq!(a[1]["hash"], "");
    assert_eq!(a[2]["path"], "{topdir}/src/moved/o.code.iter.md");
    // written files are known: the next tick sends nothing for them
    assert_eq!(st.known("src/new/n.code.iter.md").unwrap().version, 4);
    let before = syncs(&srv).len();
    tick(&srv.api(), "e1", "p", &top, false, false, &mut st).unwrap();
    assert_eq!(syncs(&srv).len(), before);
    // the lock goes: the waiting edit is applied on the next files_waiting tick
    world.lock().unwrap().locks.clear();
    tick(&srv.api(), "e1", "p", &top, false, true, &mut st).unwrap();
    assert_eq!(std::fs::read_to_string(top.join("src/busy/l.code.iter.md")).unwrap(), "later");
}

#[test]
fn read_only_checkouts_are_synced_but_never_written() {
    let top = repo("ro");
    write(&top, "src/a/a.code.iter.md", "---\nname: A\n---\n");
    let raw = std::fs::read_to_string(top.join("src/a/a.code.iter.md")).unwrap();
    let world = Arc::new(Mutex::new(World { pending: vec![json!({"id": "n", "op": "write", "path": "{topdir}/n.code.iter.md", "text": "x", "node_version": 1})], ..Default::default() }));
    let srv = server(world.clone());
    let mut st = FileSyncState::default();
    tick(&srv.api(), "e1", "p", &top, true, true, &mut st).unwrap();
    assert_eq!(syncs(&srv).len(), 1);
    assert_eq!(std::fs::read_to_string(top.join("src/a/a.code.iter.md")).unwrap(), raw);
    assert!(!top.join("n.code.iter.md").exists());
    assert!(!top.join(STATE_FILE).exists());
    assert!(srv.calls_to("GET", "/files/pending").is_empty());
}

#[test]
fn build_creates_the_repo_from_the_design() {
    let base = tmp("build");
    let top = base.join("newproj");
    let ptext = {
        let mut d = nodefile::NodeDoc::new(NodeType::Project, "New Proj", "stephen", "2026-10-02 10:00:00Z");
        d.path = "{topdir}/global/new_proj.project.iter.md".into();
        d.front.insert("gitrepo".into(), json!("git@example.com:me/newproj.git"));
        nodefile::render(&d)
    };
    let pending = vec![
        json!({"id": "p", "op": "write", "path": "{topdir}/global/new_proj.project.iter.md", "text": ptext, "node_version": 1}),
        json!({"id": "r", "op": "write", "path": "{topdir}/global/requirements/philosophy.philosophy.iter.md", "text": conformed("global/requirements/philosophy.philosophy.iter.md", "Philosophy"), "node_version": 1}),
        json!({"id": "c", "op": "write", "path": "{topdir}/src/data/data.code.iter.md", "text": conformed("src/data/data.code.iter.md", "Data"), "node_version": 2}),
    ];
    let world = Arc::new(Mutex::new(World { pending, ..Default::default() }));
    let srv = server(world.clone());
    build(&srv.api(), "e1", "newproj", &top).unwrap();
    let top = iter_core::platform::canonicalize(&top).unwrap();
    assert!(top.join(".git").exists());
    assert_eq!(sh(&top, &["remote", "get-url", "origin"]), "git@example.com:me/newproj.git");
    assert_eq!(std::fs::read_to_string(top.join(".gitignore")).unwrap(), ".iter/\n");
    assert_eq!(sh(&top, &["log", "--format=%s"]), "iter: build from design");
    assert_eq!(
        last_commit_files(&top),
        vec![".gitignore", "global/new_proj.project.iter.md", "global/requirements/philosophy.philosophy.iter.md", "src/data/data.code.iter.md"]
    );
    let head = sh(&top, &["rev-parse", "HEAD"]);
    let done = srv.calls_to("POST", "/build/done");
    assert_eq!(done, vec![json!({"engine": "e1", "commit": head})]);
    assert_eq!(srv.calls_to("POST", "/files/ack")[0]["acks"].as_array().unwrap().len(), 3);
    // the next tick knows the built files: nothing to send... except a full
    // first sync of a checkout the server already has every node of
    let mut st = FileSyncState::default();
    tick(&srv.api(), "e1", "newproj", &top, false, false, &mut st).unwrap();
    assert!(syncs(&srv).is_empty(), "built files are not echoed back");
}

/// Timing on a real tree (opt-in): `ITER5_PERF_TOPDIR=<copy> cargo test --release perf_on_a_real_tree -- --ignored --nocapture`.
/// Read-only, so the tree is never written.
#[test]
#[ignore]
fn perf_on_a_real_tree() {
    let Ok(top) = std::env::var("ITER5_PERF_TOPDIR") else { return };
    let top = PathBuf::from(top);
    let world = Arc::new(Mutex::new(World::default()));
    let srv = server(world.clone());
    let mut st = FileSyncState::default();
    for i in 0..4 {
        let t = Instant::now();
        tick(&srv.api(), "perf", "p", &top, true, false, &mut st).unwrap();
        println!("tick {i}: {:?} full={} read={} sent={}", t.elapsed(), st.last.full, st.last.read, st.last.sent);
    }
}

#[test]
fn a_hand_edit_is_stamped_with_the_file_mtime_not_the_scan_time() {
    let dir = std::env::temp_dir().join(format!("iter5-mtime-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    let f = dir.join("x.code.iter.md");
    std::fs::write(&f, "x").unwrap();
    // what `touch -t 202601021530.45` did, without needing touch (Windows)
    let local = chrono::NaiveDate::from_ymd_opt(2026, 1, 2).unwrap().and_hms_opt(15, 30, 45).unwrap();
    let at = local.and_local_timezone(chrono::Local).unwrap();
    let secs = std::time::Duration::from_secs(at.timestamp() as u64);
    std::fs::OpenOptions::new().write(true).open(&f).unwrap().set_modified(std::time::UNIX_EPOCH + secs).unwrap();
    let ts = file_mtime_ts(&f).unwrap();
    let want = local.and_local_timezone(chrono::Local).unwrap().with_timezone(&chrono::Utc).format("%Y-%m-%d %H:%M:%SZ").to_string();
    assert_eq!(ts, want);
    std::fs::remove_dir_all(&dir).ok();
}
