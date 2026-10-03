//! Datasync, the engine half — iter5 keeps it for GraphRAG documents only
//! (node files travel through crate::filesync). When the heartbeat reply's
//! `datasync_waiting` names a served project, the engine applies its rows:
//!
//! - `store_doc {path, content_b64}` — write an uploaded document's original
//!   into the checkout (never a `*.iter.md` node file);
//! - `remove_doc {path}` — remove it;
//! - `gitignore_path {path, ignore}` — add / drop a `/<path>` line in
//!   `.gitignore` (keeps the docs directory out of git).
//!
//! Per row: a row whose path sits inside a live work-item lock waits; claim
//! (`POST …/datasync/{id}/claim`, one engine wins), apply, commit exactly the
//! files it touched (under the checkout's git lock, via
//! `filesync::commit_paths`), report `POST …/datasync/{id}/done {engine,
//! outcome: applied|failed, commit, files, error}`.

use crate::client::Api;
use serde_json::{Value, json};
use std::path::Path;

fn s<'a>(op: &'a Value, k: &str) -> &'a str {
    op.get(k).and_then(|x| x.as_str()).unwrap_or("")
}

/// The paths a row writes (`{topdir}/…`), for the lock check.
pub fn lock_scope(op: &Value) -> Vec<String> {
    match s(op, "op") {
        "gitignore_path" => vec!["{topdir}/.gitignore".to_string()],
        _ => vec![s(op, "path").to_string()],
    }
}

/// Apply one op; the relative paths it touched.
pub fn apply_op(top: &Path, op: &Value) -> Result<Vec<String>, String> {
    let path = s(op, "path");
    match s(op, "op") {
        "store_doc" | "remove_doc" => {
            if path.ends_with(".iter.md") {
                return Err(format!("{} never touches *.iter.md node files", s(op, "op")));
            }
            let p = iter_local::walk::real_path(top, path)?;
            let rel = path.trim_start_matches("{topdir}/").to_string();
            if s(op, "op") == "store_doc" {
                use base64::Engine as _;
                let bytes = base64::engine::general_purpose::STANDARD.decode(s(op, "content_b64")).map_err(|e| format!("content_b64: {e}"))?;
                if let Some(d) = p.parent() {
                    std::fs::create_dir_all(d).map_err(|e| format!("{}: {e}", d.display()))?;
                }
                std::fs::write(&p, bytes).map_err(|e| format!("{}: {e}", p.display()))?;
                Ok(vec![rel])
            } else if p.is_file() {
                std::fs::remove_file(&p).map_err(|e| format!("{}: {e}", p.display()))?;
                Ok(vec![rel])
            } else {
                Ok(vec![])
            }
        }
        "gitignore_path" => {
            let rel = path.trim().trim_start_matches("{topdir}").trim_start_matches('/').to_string();
            if rel.is_empty() || rel.split('/').any(|x| x == "..") {
                return Err(format!("gitignore_path: bad path {path:?}"));
            }
            let line = format!("/{rel}");
            let gi = top.join(".gitignore");
            let old = std::fs::read_to_string(&gi).unwrap_or_default();
            let has = old.lines().any(|l| l.trim() == line || l.trim() == line.trim_start_matches('/'));
            let ignore = op.get("ignore").and_then(|b| b.as_bool()).unwrap_or(true);
            let new = if ignore && !has {
                format!("{}{}# GraphRAG documents (iter docs_gitignore)\n{line}\n", old, if old.is_empty() || old.ends_with('\n') { "" } else { "\n" })
            } else if !ignore && has {
                old.lines()
                    .filter(|l| l.trim() != line && l.trim() != line.trim_start_matches('/') && l.trim() != "# GraphRAG documents (iter docs_gitignore)")
                    .map(|l| format!("{l}\n"))
                    .collect()
            } else {
                old.clone()
            };
            if new != old {
                std::fs::write(&gi, new).map_err(|e| format!("{}: {e}", gi.display()))?;
                return Ok(vec![".gitignore".into()]);
            }
            Ok(vec![])
        }
        other => Err(format!("unknown datasync op {other:?} (iter5 datasync carries GraphRAG documents only)")),
    }
}

/// Live lock rows that overlap any of `paths`: (path, holder).
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

/// Apply every claimable row of one project; returns how many were applied.
pub fn apply_waiting(api: &Api, engine: &str, project: &str, topdir: &Path) -> usize {
    let rows = api.get(&format!("/api/projects/{project}/datasync?state=claimable")).ok().and_then(|v| v.as_array().cloned()).unwrap_or_default();
    let top = topdir.canonicalize().unwrap_or_else(|_| topdir.to_path_buf());
    let mut applied = 0;
    for row in rows {
        let id = row["id"].as_str().unwrap_or("").to_string();
        let op = &row["op"];
        let summary = row["summary"].as_str().map(String::from).unwrap_or_else(|| format!("{} {}", s(op, "op"), s(op, "path")));
        if let Some((path, holder)) = blocking_locks(api, project, &lock_scope(op)).first() {
            println!("[engine] {project}: document edit '{summary}' waits: {path} is locked by running item {}", &holder[holder.len().saturating_sub(12)..]);
            continue;
        }
        if api.post(&format!("/api/projects/{project}/datasync/{id}/claim"), &json!({"engine": engine})).is_err() {
            continue; // another engine won it
        }
        let finish = |outcome: &str, commit: &str, files: &[String], error: &str| {
            let _ = api.post(
                &format!("/api/projects/{project}/datasync/{id}/done"),
                &json!({"engine": engine, "outcome": outcome, "commit": commit, "files": files, "error": error}),
            );
        };
        match apply_op(&top, op) {
            Ok(files) => {
                let commit = match crate::filesync::commit_paths(&top, &files, &format!("iter: {summary}")) {
                    Ok(c) => c.unwrap_or_default(),
                    Err(e) => {
                        eprintln!("[engine] {project}: document edit commit failed: {e}");
                        String::new()
                    }
                };
                let stored: Vec<String> = files.iter().map(|f| format!("{{topdir}}/{f}")).collect();
                finish("applied", &commit, &stored, "");
                applied += 1;
            }
            Err(e) => {
                println!("[engine] {project}: document edit '{summary}' failed: {e}");
                finish("failed", "", &[], &e);
            }
        }
    }
    applied
}

/// The projects a heartbeat reply says have datasync rows waiting
/// (`{"datasync_waiting": {"<project>": n}}`, or a list of names).
pub fn waiting_projects(reply: &Value) -> Vec<String> {
    match reply.get("datasync_waiting") {
        Some(Value::Object(o)) => o.iter().filter(|(_, n)| n.as_u64().unwrap_or(0) > 0).map(|(k, _)| k.clone()).collect(),
        Some(Value::Array(a)) => a.iter().filter_map(|x| x.as_str().map(String::from)).collect(),
        _ => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn heartbeat_reply_names_projects_with_waiting_rows() {
        assert_eq!(waiting_projects(&json!({"datasync_waiting": {"a": 2, "b": 0}})), vec!["a".to_string()]);
        assert_eq!(waiting_projects(&json!({"datasync_waiting": ["c"]})), vec!["c".to_string()]);
        assert!(waiting_projects(&json!({"name": "E1"})).is_empty());
    }

    #[test]
    fn stores_removes_ignores_commits_and_reports() {
        let top = std::env::temp_dir().join(format!("iter5_dsync_{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&top).unwrap();
        let top = top.canonicalize().unwrap();
        let git = |a: &[&str]| crate::filesync::git(&top, a).unwrap();
        git(&["init", "-q"]);
        git(&["config", "user.email", "t@t"]);
        git(&["config", "user.name", "t"]);
        std::fs::write(top.join("unrelated.txt"), "wip\n").unwrap();
        let rows = json!([
            {"id": "a1", "summary": "store docs/a.txt", "op": {"op": "store_doc", "path": "{topdir}/docs/a.txt", "content_b64": "aGVsbG8="}},
            {"id": "a2", "op": {"op": "store_doc", "path": "{topdir}/busy/b.txt", "content_b64": "eA=="}},
            {"id": "a3", "op": {"op": "store_doc", "path": "{topdir}/x.code.iter.md", "content_b64": "eA=="}},
            {"id": "a4", "op": {"op": "gitignore_path", "path": "{topdir}/docs/private"}},
            {"id": "a5", "op": {"op": "store_doc", "path": "{topdir}/../escape.txt", "content_b64": "eA=="}},
        ]);
        let srv = crate::client::fake::serve(move |m, path, _| {
            if m == "GET" && path.contains("/datasync") {
                return Some((200, rows.clone()));
            }
            if m == "GET" && path.ends_with("/locks") {
                return Some((200, json!([{"kind": "lock", "path": "{topdir}/busy", "workid": "run-000000000009", "holder_lease_live": true}])));
            }
            Some((200, json!({})))
        });
        assert_eq!(apply_waiting(&srv.api(), "E1", "demo", &top), 2);
        assert_eq!(std::fs::read_to_string(top.join("docs/a.txt")).unwrap(), "hello");
        assert!(!top.join("busy").exists(), "a locked row waits");
        assert!(srv.calls_to("POST", "/datasync/a2/").is_empty(), "never claimed while locked");
        assert_eq!(srv.calls_to("POST", "/datasync/a3/done")[0]["outcome"], "failed");
        assert_eq!(srv.calls_to("POST", "/datasync/a5/done")[0]["outcome"], "failed");
        assert!(std::fs::read_to_string(top.join(".gitignore")).unwrap().contains("/docs/private"));
        let done = srv.calls_to("POST", "/datasync/a1/done");
        assert_eq!(done[0]["outcome"], "applied");
        assert_eq!(done[0]["files"], json!(["{topdir}/docs/a.txt"]));
        let sha = done[0]["commit"].as_str().unwrap().to_string();
        assert_eq!(git(&["show", "--name-only", "--format=", &sha]), "docs/a.txt");
        assert!(git(&["status", "--porcelain"]).contains("unrelated.txt"));
        // removal
        let rows2 = json!([{"id": "r1", "op": {"op": "remove_doc", "path": "{topdir}/docs/a.txt"}}]);
        let srv2 = crate::client::fake::serve(move |m, path, _| {
            if m == "GET" && path.contains("/datasync") {
                return Some((200, rows2.clone()));
            }
            Some((200, json!([])))
        });
        assert_eq!(apply_waiting(&srv2.api(), "E1", "demo", &top), 1);
        assert!(!top.join("docs/a.txt").exists());
        assert_eq!(git(&["log", "-1", "--format=%s"]), "iter: remove_doc {topdir}/docs/a.txt");
    }
}
