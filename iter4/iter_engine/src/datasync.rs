//! Datasync, the engine half (iter4, decided 2026-09-29; server half in
//! iter_data/src/datasync.rs). When the heartbeat reply says a project this
//! engine serves has graph edits waiting, the engine applies them on the
//! same tick, oldest first:
//!
//! 1. an edit whose paths overlap a live lock (a running item's) waits — the
//!    agent's work in progress is not disturbed; the next heartbeat retries;
//! 2. claim it (one engine wins; a claim lapses on its own if this engine dies);
//! 3. `git pull` when there is a remote, apply the edit, commit exactly the
//!    files it wrote (never anyone else's), push;
//! 4. report applied (with the commit) or failed (with the reason), and push
//!    the map straight away so every open Project graph redraws.

use crate::client::Api;
use iter_local::project::Project;
use serde_json::{Value, json};
use std::path::Path;
use std::process::Command;

/// One git command. A locked index (an agent's own git inside its session,
/// outside this engine's GIT_LOCK) is waited out for up to ~10 s, not failed.
fn git(topdir: &str, args: &[&str]) -> Result<String, String> {
    for attempt in 0.. {
        let out = Command::new("git").args(args).current_dir(topdir).output().map_err(|e| format!("git {}: {e}", args.join(" ")))?;
        if out.status.success() {
            return Ok(String::from_utf8_lossy(&out.stdout).trim().to_string());
        }
        let err = String::from_utf8_lossy(&out.stderr).trim().to_string();
        if err.contains("index.lock") && attempt < 20 {
            std::thread::sleep(std::time::Duration::from_millis(500));
            continue;
        }
        return Err(format!("git {} failed: {err}", args.join(" ")));
    }
    unreachable!()
}

/// Live lock rows (a running item's) that overlap any of `paths`: (path, holder).
fn blocking_locks(api: &Api, project: &str, paths: &[String]) -> Vec<(String, String)> {
    let rows = api.get(&format!("/api/projects/{project}/locks")).ok().and_then(|v| v.as_array().cloned()).unwrap_or_default();
    rows.iter()
        .filter(|r| r["kind"] == "lock" && r["holder_lease_live"] == true)
        .filter_map(|r| {
            let p = r["path"].as_str()?;
            paths.iter().any(|d| iter_core::paths_overlap(d, p)).then(|| (p.to_string(), r["workid"].as_str().unwrap_or("").to_string()))
        })
        .collect()
}

/// Apply every claimable edit of one project; returns how many were applied.
pub fn apply_waiting(api: &Api, engine: &str, project_name: &str, topdir: &str) -> usize {
    let rows = api
        .get(&format!("/api/projects/{project_name}/datasync?state=claimable"))
        .ok()
        .and_then(|v| v.as_array().cloned())
        .unwrap_or_default();
    let mut applied = 0;
    for row in rows {
        let id = row["id"].as_str().unwrap_or("").to_string();
        let summary = row["summary"].as_str().unwrap_or("").to_string();
        let lockdirs: Vec<String> = row["lockdirs"].as_array().into_iter().flatten().filter_map(|x| x.as_str().map(String::from)).collect();
        let blocked = blocking_locks(api, project_name, &lockdirs);
        if let Some((path, holder)) = blocked.first() {
            println!("[engine] {project_name}: graph edit '{summary}' waits: {path} is locked by running item {}", &holder[holder.len().saturating_sub(12)..]);
            continue;
        }
        if api.post(&format!("/api/projects/{project_name}/datasync/{id}/claim"), &json!({"engine": engine})).is_err() {
            continue; // another engine won it
        }
        let finish = |outcome: &str, commit: &str, files: &[String], error: &str| {
            let _ = api.post(
                &format!("/api/projects/{project_name}/datasync/{id}/done"),
                &json!({"engine": engine, "outcome": outcome, "commit": commit, "files": files, "error": error}),
            );
        };
        // the checkout's index is shared with this engine's run threads
        let _git = crate::work::GIT_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let is_repo = Path::new(topdir).join(".git").exists();
        let has_remote = is_repo && git(topdir, &["remote"]).map(|r| !r.is_empty()).unwrap_or(false);
        if has_remote {
            if let Err(e) = git(topdir, &["pull", "--no-rebase"]) {
                println!("[engine] {project_name}: graph edit '{summary}' waits: {e}");
                finish("retry", "", &[], &e);
                continue;
            }
        }
        let project = Project::load(Path::new(topdir));
        let files = match iter_local::graph_edit::apply(&project, &row["op"]) {
            Ok(f) => f,
            Err(e) => {
                println!("[engine] {project_name}: graph edit '{summary}' failed: {e}");
                finish("failed", "", &[], &e);
                continue;
            }
        };
        let mut commit = String::new();
        // a git-ignored path (a GraphRAG docs directory kept out of git) is
        // written but never committed — `git add` would refuse it anyway
        let mut ignored: Vec<String> = Vec::new();
        let files: Vec<String> = if is_repo {
            files
                .into_iter()
                .filter(|f| {
                    let rel = f.trim_start_matches("{topdir}/");
                    let ign = Command::new("git").args(["check-ignore", "-q", "--", rel]).current_dir(topdir).status().map(|s| s.success()).unwrap_or(false);
                    if ign {
                        ignored.push(f.clone());
                    }
                    !ign
                })
                .collect()
        } else {
            files
        };
        if is_repo && !files.is_empty() {
            let rel: Vec<String> = files.iter().map(|f| f.trim_start_matches("{topdir}/").to_string()).collect();
            let mut add = vec!["add", "--"];
            add.extend(rel.iter().map(String::as_str));
            let mut ci = vec!["commit", "-m"];
            let reason = row["op"]["reason"].as_str().unwrap_or("");
            let msg = format!("iter: graph edit — {summary}\n\n{}Applied by engine {engine} from the Project graph (datasync {id}).",
                if reason.is_empty() { String::new() } else { format!("Reason: {reason}\n\n") });
            ci.push(&msg);
            ci.push("--");
            ci.extend(rel.iter().map(String::as_str));
            let res = git(topdir, &add).and_then(|_| git(topdir, &ci)).and_then(|_| git(topdir, &["rev-parse", "HEAD"]));
            match res {
                Ok(sha) => commit = sha,
                Err(e) => {
                    println!("[engine] {project_name}: graph edit '{summary}' written but not committed: {e}");
                    finish("failed", "", &files, &format!("files written, commit failed: {e}"));
                    continue;
                }
            }
            if has_remote {
                if let Err(e) = git(topdir, &["push"]) {
                    // the commit exists here; the next push (any item's postwork) carries it
                    println!("[engine] {project_name}: graph edit '{summary}' committed {} but not pushed yet: {e}", &commit[..8.min(commit.len())]);
                }
            }
        }
        // TDD from the map: planned tests were written; the `test` agent writes
        // the scripts and runs them (red is expected — the sweep turns red
        // groups into fix items, so the code follows the tests)
        let op = &row["op"];
        if op.get("queue_agent").and_then(|q| q.as_bool()).unwrap_or(false) {
            let node = op["node"].as_str().or_else(|| op["parent"].as_str()).unwrap_or("");
            let tests_file = files.iter().find(|f| f.ends_with(".tests.iter.md")).cloned().unwrap_or_default();
            let target = if op["op"] == "new_node" {
                format!("{}/{}", node.trim_end_matches('/'), iter_local::graph_edit::slugify(op["name"].as_str().unwrap_or("")))
            } else {
                node.to_string()
            };
            let lock = tests_file.rsplit_once('/').map(|(d, _)| d.to_string()).unwrap_or_else(|| target.clone());
            let request = format!(
                "Write and run the planned tests for {target} (TDD: the tests come first; red is expected until the code exists).\n\n\
                 The planned tests are listed, simplest first, in {tests_file} under \"## Planned tests\". For each, write one deterministic shell script in that folder \
                 (exit 0 = passes, 1 = fails, last line `ITER_RESULT pass=… fail=… total=…`), register it in the file's testgroup block in the same order, \
                 then run the group with `iter runtests --group <label>` and report what is red. Do not write or change the code under test."
            );
            let item = json!({"name": format!("Tests first: build the planned tests for {target}"), "agent": "test", "state": "queued",
                "lockdirs": [lock], "blockedby": [], "context": [], "tags": [{"text": "tdd", "color": ""}],
                "requestedby": format!("graph edit {id}"), "prework": [], "postwork": [], "request": request});
            match api.post(&format!("/api/projects/{project_name}/workitems"), &item) {
                Ok(c) => println!("[engine] {project_name}: queued the test agent (…{}) for {target}", c["id"].as_str().map(|i| &i[i.len().saturating_sub(12)..]).unwrap_or("?")),
                Err(e) => eprintln!("[engine] {project_name}: could not queue the test agent: {e}"),
            }
        }
        // a new use case: the `usecase` agent names the parts it needs (step 1);
        // the stored map then tags them and every owner above them (usecase_map,
        // on the next sync), which is the hierarchy the Project graph draws
        let wants_parts = (op["op"] == "new_global" && op["kind"] == "usecase" && op.get("queue_agent").and_then(|q| q.as_bool()) != Some(false))
            || op["op"] == "name_parts";
        if wants_parts {
            let uc_files: Vec<String> = if op["op"] == "name_parts" {
                let p = Project::load(Path::new(topdir));
                let rel = op["usecase"].as_str().unwrap_or("").trim_start_matches("{topdir}/").trim_start_matches('/').to_string();
                let abs = p.topdir.join(&rel);
                let f = if abs.is_dir() {
                    std::fs::read_dir(&abs).ok().into_iter().flatten().flatten().map(|e| e.path()).find(|f| f.to_string_lossy().ends_with(".usecase.iter.md"))
                } else { Some(abs) };
                f.map(|f| format!("{{topdir}}/{}", f.strip_prefix(&p.topdir).unwrap_or(&f).to_string_lossy())).into_iter().collect()
            } else { files.clone() };
            if let Some(item) = usecase_item(&uc_files, op, &id) {
                match api.post(&format!("/api/projects/{project_name}/workitems"), &item) {
                    Ok(c) => println!("[engine] {project_name}: queued the usecase agent (…{}) for {}", c["id"].as_str().map(|i| &i[i.len().saturating_sub(12)..]).unwrap_or("?"), op["name"].as_str().unwrap_or("")),
                    Err(e) => eprintln!("[engine] {project_name}: could not queue the usecase agent: {e}"),
                }
            }
        }
        println!("[engine] {project_name}: graph edit '{summary}' applied ({} file(s){})", files.len(),
            if commit.is_empty() { String::new() } else { format!(", commit {}", &commit[..8.min(commit.len())]) });
        if ignored.is_empty() {
            finish("applied", &commit, &files, "");
        } else {
            let _ = api.post(
                &format!("/api/projects/{project_name}/datasync/{id}/done"),
                &json!({"engine": engine, "outcome": "applied", "commit": commit, "files": files,
                        "note": format!("written, not committed (git-ignored): {}", ignored.join(", "))}),
            );
        }
        applied += 1;
    }
    if applied > 0 {
        // every open Project graph redraws from this push
        let _ = crate::sync::sync_if_changed(api, project_name, Path::new(topdir), "");
    }
    applied
}

/// The work item that has the `usecase` agent name the parts a new use case
/// needs: it writes them into the use-case file's `children.codenodes`, and
/// nothing else — the owners above them are tagged by the map, not listed.
pub fn usecase_item(files: &[String], op: &Value, edit_id: &str) -> Option<Value> {
    let file = files.iter().find(|f| f.ends_with(".usecase.iter.md"))?.clone();
    let dir = file.rsplit_once('/').map(|(d, _)| d.to_string()).unwrap_or_default();
    let slug = dir.rsplit('/').next().unwrap_or("").to_string();
    let name = op["name"].as_str().unwrap_or(&slug).to_string();
    let request = format!(
        "Name the parts the use case \"{name}\" needs ({file}).\n\n\
         Read the use case's description and body, then find every code node (context, container or component `*.code.iter.md`) \
         the journey passes through or depends on, at the most specific level that is true: a component when one component does the work, \
         its container only when the whole container is involved. List each one in the use case's `children.codenodes` as a `{{topdir}}/…/*.code.iter.md` path. \
         Do not list the owners of a part you listed: the map tags every owner up the hierarchy on its own (usecase_map), \
         and the Project graph draws the use case → its top-level parts → ownership lines down to the parts you named. \
         A part the journey needs that does not exist yet: say so in the use case's body under \"## Missing parts\" instead of inventing a path. \
         Then add the `flowmap:` block (your instructions, \"The flowmap\"): a plain summary, the sequence of parts in first-touch order, numbered \
         process_flow and data_flow steps between those node files (and `actor:<id>` entries from the actors file), each step citing the function or route that does it. \
         Change only {file}."
    );
    Some(json!({"name": format!("Use case: name the parts \"{name}\" needs"), "agent": "usecase", "state": "queued",
        "lockdirs": [dir], "blockedby": [], "context": [file], "tags": [], "usecase": slug,
        "requestedby": format!("graph edit {edit_id}"), "prework": [], "postwork": [], "request": request}))
}

/// The projects a heartbeat reply says have edits waiting.
pub fn waiting_projects(reply: &Value) -> Vec<String> {
    reply
        .get("datasync_waiting")
        .and_then(|w| w.as_object())
        .map(|o| o.iter().filter(|(_, n)| n.as_u64().unwrap_or(0) > 0).map(|(k, _)| k.clone()).collect())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_new_use_case_queues_the_usecase_agent_on_its_own_file() {
        let files = vec!["{topdir}/usecases/read-a-row/read-a-row.usecase.iter.md".to_string()];
        let it = usecase_item(&files, &json!({"op": "new_global", "kind": "usecase", "name": "Read a row"}), "ds1").unwrap();
        assert_eq!((it["agent"].as_str(), it["usecase"].as_str()), (Some("usecase"), Some("read-a-row")));
        assert_eq!(it["lockdirs"], json!(["{topdir}/usecases/read-a-row"]));
        assert!(it["request"].as_str().unwrap().contains("children.codenodes"));
        assert!(it["request"].as_str().unwrap().contains("flowmap"));
        assert!(usecase_item(&["{topdir}/reqs/x.bizreq.iter.md".into()], &json!({}), "d").is_none());
    }

    #[test]
    fn a_locked_index_is_waited_out() {
        let top = std::env::temp_dir().join(format!("iter4_gitlock_{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&top).unwrap();
        let t = top.to_string_lossy().to_string();
        git(&t, &["init", "-q"]).unwrap();
        std::fs::write(top.join("a.txt"), "a").unwrap();
        let lock = top.join(".git/index.lock");
        std::fs::write(&lock, "").unwrap();
        let l2 = lock.clone();
        let h = std::thread::spawn(move || { std::thread::sleep(std::time::Duration::from_millis(1200)); std::fs::remove_file(l2).unwrap(); });
        assert!(git(&t, &["add", "a.txt"]).is_ok(), "the add waits for the other writer");
        h.join().unwrap();
    }

    #[test]
    fn heartbeat_reply_names_projects_with_waiting_edits() {
        assert_eq!(waiting_projects(&json!({"datasync_waiting": {"a": 2, "b": 0}})), vec!["a".to_string()]);
        assert!(waiting_projects(&json!({"name": "E1"})).is_empty());
    }

    /// The full engine half against a fake iter_data: a waiting edit is
    /// claimed, applied in a git checkout, committed with exactly its files,
    /// and reported applied with the commit; an edit whose paths a running
    /// item has locked is left alone.
    #[test]
    fn claims_applies_commits_and_reports() {
        let top = std::env::temp_dir().join(format!("iter4_dsync_{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&top).unwrap();
        std::fs::write(top.join("main.iter.md"), "---\nid: 11111111-1111-4111-8111-111111111111\nprojectname: demo\nprojectdescription: d\nglobalscandirs: [\"{topdir}/\"]\nchildren:\n  codenodes: []\n---\n").unwrap();
        std::fs::write(top.join("unrelated.txt"), "someone else's work in progress\n").unwrap();
        let t = top.to_string_lossy().to_string();
        for a in [vec!["init", "-q"], vec!["add", "main.iter.md"], vec!["-c", "user.email=t@t", "-c", "user.name=t", "commit", "-qm", "seed"]] {
            git(&t, &a).unwrap();
        }
        let _ = git(&t, &["config", "user.email", "t@t"]);
        let _ = git(&t, &["config", "user.name", "t"]);
        let op_ok = json!({"op": "new_node", "kind": "context", "name": "Data"});
        let op_locked = json!({"op": "new_node", "kind": "context", "name": "Busy"});
        let rows = json!([
            {"id": "a1", "summary": "+ context Data", "op": op_ok, "lockdirs": ["{topdir}/data", "{topdir}/main.iter.md"]},
            {"id": "a2", "summary": "+ context Busy", "op": op_locked, "lockdirs": ["{topdir}/busy"]},
        ]);
        let srv = crate::client::fake::serve(move |m, path, _| {
            if m == "GET" && path.contains("/datasync") { return Some((200, rows.clone())); }
            if m == "GET" && path.ends_with("/locks") {
                return Some((200, json!([{"kind": "lock", "path": "{topdir}/busy", "workid": "run-000000000009", "holder_lease_live": true}])));
            }
            Some((200, json!({})))
        });
        let n = apply_waiting(&srv.api(), "E1", "demo", &t);
        assert_eq!(n, 1);
        assert!(top.join("data/data.code.iter.md").exists());
        assert!(!top.join("busy").exists(), "the locked edit waits");
        let claims = srv.calls_to("POST", "/datasync/");
        assert!(claims.iter().all(|b| b["engine"] == "E1"));
        assert!(srv.calls_to("POST", "/datasync/a2/").is_empty(), "never claimed while locked");
        let done = srv.calls_to("POST", "/datasync/a1/done");
        assert_eq!(done[0]["outcome"], "applied");
        let sha = done[0]["commit"].as_str().unwrap().to_string();
        assert_eq!(sha.len(), 40);
        let committed = git(&t, &["show", "--name-only", "--format=", &sha]).unwrap();
        assert!(committed.contains("data/data.code.iter.md") && committed.contains("main.iter.md"));
        assert!(!committed.contains("unrelated.txt"), "only the edit's files are committed");
        assert!(git(&t, &["status", "--porcelain"]).unwrap().contains("unrelated.txt"), "someone else's file left alone");
    }
}
