//! Keeping a checkout in step with its remote when several clones push to it
//! (built 2026-10-11 for engine fleets: two engines serving one project each
//! have their own clone).
//!
//! Every engine pull goes through `pull`: before a work item runs (the git
//! prework) and when a push is refused because the remote moved on (`push`).
//! A merge git finishes on its own is committed as usual. A merge that
//! conflicts is handed to a resolver: an agent session in the checkout,
//! mid-merge, told which files conflict, what each side's commits were and
//! which work items made them, and asked to combine both sides' intent. It
//! edits the files only; the engine checks that no conflict marker is left,
//! then stages them and commits the merge. A merge nobody could finish is
//! backed out (`git merge --abort`), so the checkout is never left
//! half-merged for the next item.
//!
//! When the resolver cannot finish, the work item asks a human (`question`
//! state) and `.git/iter-merge-conflict` records it. While that item waits on
//! the answer, this engine starts nothing new in that checkout (`hold_reason`,
//! the dispatch hold); other engines carry on in their own clones. Once the
//! human answers, the item queues again and the next merge attempt's
//! resolver is given the answer as a decision. The record goes when a merge
//! succeeds.

use crate::client::Api;
use crate::work::{git_lock, git_shell, run_shell};
use iter_core::{Project, WorkItem};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

/// Pushes tried before a refusal is reported (each refusal merges first).
const PUSH_TRIES: u32 = 3;
const FLAG_FILE: &str = "iter-merge-conflict";

/// Who resolves a conflicted merge: `run(prompt)` runs one agent session in
/// the checkout and returns its final text. The work item it runs for names
/// the context.
pub struct Resolver<'a> {
    pub api: &'a Api,
    pub project: &'a Project,
    pub item: &'a WorkItem,
    /// the item's request text (its `request` detail)
    pub request: &'a str,
    pub run: &'a dyn Fn(&str) -> Result<String, String>,
}

/// A merge that could not be finished (already backed out).
#[derive(Debug)]
pub struct Conflict {
    pub files: Vec<String>,
    pub ours: String,
    pub theirs: String,
    pub why: String,
}

#[derive(Debug)]
pub enum MergeError {
    Conflict(Conflict),
    Other(String),
}

impl std::fmt::Display for MergeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MergeError::Other(e) => write!(f, "{e}"),
            MergeError::Conflict(c) => write!(f, "merge conflict in {} ({}): {}", c.files.join(", "), c.files.len(), c.why),
        }
    }
}

fn q(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// (remote, branch) to pull from and push to; None without a remote.
fn upstream(topdir: &str) -> Result<Option<(String, String)>, String> {
    let remotes = run_shell(topdir, "git remote", 15)?;
    let Some(remote) = remotes.lines().map(str::trim).find(|r| *r == "origin").or_else(|| remotes.lines().map(str::trim).find(|r| !r.is_empty())) else {
        return Ok(None);
    };
    let branch = run_shell(topdir, "git rev-parse --abbrev-ref HEAD", 15)?.trim().to_string();
    Ok(Some((remote.to_string(), branch)))
}

fn git_dir(topdir: &str) -> PathBuf {
    Path::new(topdir).join(".git")
}

fn merging(topdir: &str) -> bool {
    git_dir(topdir).join("MERGE_HEAD").exists()
}

fn unmerged(topdir: &str) -> Vec<String> {
    run_shell(topdir, "git diff --name-only --diff-filter=U", 15)
        .map(|o| o.lines().map(str::trim).filter(|l| !l.is_empty()).map(String::from).collect())
        .unwrap_or_default()
}

/// Files that still hold a conflict marker.
fn with_markers(topdir: &str, files: &[String]) -> Vec<String> {
    files
        .iter()
        .filter(|f| {
            std::fs::read_to_string(Path::new(topdir).join(f))
                .map(|t| t.lines().any(|l| l.starts_with("<<<<<<< ") || l.starts_with(">>>>>>> ") || l == "<<<<<<<" || l == ">>>>>>>"))
                .unwrap_or(false)
        })
        .cloned()
        .collect()
}

/// `git pull --no-rebase` of the branch from its remote, under the
/// checkout's git lock. A conflict goes to `resolver` (when given); a merge
/// left unfinished is backed out before returning.
pub fn pull(topdir: &str, resolver: Option<&Resolver>) -> Result<(), MergeError> {
    let Some((remote, branch)) = upstream(topdir).map_err(MergeError::Other)? else { return Ok(()) };
    if branch == "HEAD" {
        return Err(MergeError::Other("the checkout is on a detached HEAD: nothing to pull into".into()));
    }
    let lock = git_lock(topdir);
    let _g = lock.lock().unwrap_or_else(|p| p.into_inner());
    if merging(topdir) {
        // a merge some earlier run left behind: start from a clean tree
        let _ = run_shell(topdir, "git merge --abort", 30);
    }
    let err = match git_shell(topdir, &format!("git pull -q --no-rebase --no-edit {} {}", q(&remote), q(&branch)), 180) {
        Ok(_) => {
            clear_flag(topdir);
            return Ok(());
        }
        Err(e) => e,
    };
    let files = unmerged(topdir);
    if !merging(topdir) || files.is_empty() {
        // not a content conflict (offline, local edits in the way, …): nothing was merged
        return Err(MergeError::Other(format!("git pull failed: {}", err.trim())));
    }
    let ours = run_shell(topdir, "git log --oneline --no-decorate MERGE_HEAD..HEAD", 15).unwrap_or_default();
    let theirs = run_shell(topdir, "git log --oneline --no-decorate HEAD..MERGE_HEAD", 15).unwrap_or_default();
    let why = match resolver {
        None => "no resolver here (a background push): the next work item's merge resolves it".to_string(),
        Some(r) => match resolve(topdir, r, &files, &ours, &theirs) {
            Ok(()) => {
                clear_flag(topdir);
                println!("[engine] {topdir}: merge conflict in {} resolved by an agent and committed", files.join(", "));
                return Ok(());
            }
            Err(why) => why,
        },
    };
    let _ = run_shell(topdir, "git merge --abort", 30);
    Err(MergeError::Conflict(Conflict { files, ours: ours.trim().into(), theirs: theirs.trim().into(), why }))
}

/// Resolver sessions on a conflicted merge; Ok once the merge is committed.
/// The resolver takes its best guess; a session that fails or leaves markers
/// behind gets one more try. Only its own `CANNOT RESOLVE:` (no basis at all
/// to choose) goes to a person at once.
fn resolve(topdir: &str, r: &Resolver, files: &[String], ours: &str, theirs: &str) -> Result<(), String> {
    let prompt = resolver_prompt(topdir, r, files, ours, theirs);
    println!("[engine] {topdir}: merge conflict in {} — an agent is resolving it", files.join(", "));
    let mut retry_note = String::new();
    for attempt in 1..=2 {
        let why = match (r.run)(&format!("{prompt}{retry_note}")) {
            Err(e) => format!("the resolver session failed: {e}"),
            Ok(out) => {
                if let Some(i) = out.find("CANNOT RESOLVE:") {
                    return Err(format!("the resolver found no basis to choose: {}", out[i + 15..].trim().chars().take(1500).collect::<String>()));
                }
                let left = with_markers(topdir, files);
                if left.is_empty() {
                    break;
                }
                format!("conflict markers are still in {}", left.join(", "))
            }
        };
        if attempt == 2 {
            return Err(why);
        }
        println!("[engine] {topdir}: resolver attempt {attempt}: {why} — trying once more");
        retry_note = format!("\n\n## Second attempt\n\nThe first attempt did not finish: {why}. Finish the merge now: no conflict marker may be left.");
    }
    let paths = files.iter().map(|f| q(f)).collect::<Vec<_>>().join(" ");
    git_shell(topdir, &format!("git add -- {paths}"), 60)?;
    if !unmerged(topdir).is_empty() {
        return Err(format!("still unmerged after the resolver: {}", unmerged(topdir).join(", ")));
    }
    git_shell(topdir, "git commit -q --no-edit --no-verify", 60).map(|_| ())
}

fn resolver_prompt(topdir: &str, r: &Resolver, files: &[String], ours: &str, theirs: &str) -> String {
    let request: String = r.request.chars().take(2000).collect();
    let guidance = read_flag(topdir)
        .and_then(|f| f.get("item").and_then(|i| i.as_str()).map(String::from))
        .and_then(|id| human_answer(r.api, r.project, &id))
        .map(|(q, a)| format!("\n## A human already decided\n\nAn earlier attempt at this merge asked:\n\n{q}\n\nThe answer, which is a decision to follow:\n\n{a}\n"))
        .unwrap_or_default();
    format!(
        "# Resolve a git merge conflict\n\n\
         This checkout ({topdir}) is in the middle of a `git merge`: another clone of this repository \
         (another engine, or a person) pushed commits that change the same lines as this checkout's own commits.\n\n\
         Commits only this checkout has (ours):\n```\n{ours}\n```\n\
         Commits being merged in (theirs):\n```\n{theirs}\n```\n\
         Files in conflict:\n{files}\n\n\
         The work item this engine is running when the conflict appeared: \"{name}\" ({id}).\n\n{request}\n{guidance}\n\
         ## What to do\n\n\
         1. For each conflicted file, read it and work out what each side meant: `git show <commit> -- <file>` and \
         `git log -p -3 <commit> -- <file>` show each side's change; commit messages name the work item that made \
         it, and the iter MCP tools (workitem_get, workitem_details) read that item's request.\n\
         2. Edit the file so it carries BOTH sides' intent, and remove every conflict marker (`<<<<<<<`, `=======`, \
         `>>>>>>>`). Drop one side only when the other clearly replaces it, and say so.\n\
         3. Do not run `git add`, `git commit`, `git merge` or `git push`: the engine checks the files and commits \
         the merge itself.\n\
         4. Make the call yourself. When it is unclear which side is right, take your best guess and say what you \
         guessed: the project's tests run after this merge, and a wrong guess shows up there and gets fixed by the \
         next work item, far sooner than a person could research it.\n\
         5. Only when the two sides truly contradict each other and nothing in the code, the tests or the work items \
         gives you any basis to choose, leave the markers in place and end with a line that starts \
         `CANNOT RESOLVE:` saying what has to be decided. That opens a question to a person, so use it rarely.\n\n\
         End with a short summary of what you kept from each side and anything you guessed.",
        files = files.iter().map(|f| format!("- {f}")).collect::<Vec<_>>().join("\n"),
        name = r.item.name,
        id = r.item.id,
    )
}

/// The latest answered question on `item`: (question, answer).
fn human_answer(api: &Api, project: &Project, item: &str) -> Option<(String, String)> {
    let details = api.get(&format!("/api/projects/{}/workitems/{}/details", project.key(), item)).ok()?;
    details.as_array()?.iter().rev().filter(|d| d.get("key").and_then(|k| k.as_str()) == Some("question")).find_map(|d| {
        let v = d.get("value")?;
        let answers: Vec<String> = v.get("fields")?.as_array()?.iter()
            .filter_map(|f| f.get("value").and_then(|x| x.as_str()).map(str::trim).filter(|x| !x.is_empty()).map(String::from))
            .collect();
        if answers.is_empty() {
            return None;
        }
        let question = format!("{}\n\n{}", v.get("title").and_then(|t| t.as_str()).unwrap_or(""), v.get("detail").and_then(|t| t.as_str()).unwrap_or(""));
        Some((question.trim().to_string(), answers.join("\n")))
    })
}

/// `git push` to the remote; a refusal because the remote moved on is merged
/// (`pull`, with `resolver`) and pushed again, up to PUSH_TRIES times.
pub fn push(topdir: &str, resolver: Option<&Resolver>) -> Result<(), MergeError> {
    let Some((remote, _)) = upstream(topdir).map_err(MergeError::Other)? else { return Ok(()) };
    let cmd = format!("git push -q {} HEAD", q(&remote));
    let mut attempt = 1;
    loop {
        let err = match run_shell(topdir, &cmd, 180) {
            Ok(_) => return Ok(()),
            Err(e) => e,
        };
        if attempt >= PUSH_TRIES || !push_was_behind(&err) {
            return Err(MergeError::Other(format!("git push failed: {}", err.trim())));
        }
        println!("[engine] {topdir}: the remote has commits this checkout lacks — merging them in and pushing again ({attempt}/{PUSH_TRIES})");
        pull(topdir, resolver)?;
        attempt += 1;
    }
}

/// Did the push fail only because the remote has commits we do not?
pub fn push_was_behind(err: &str) -> bool {
    err.contains("[rejected]") && (err.contains("fetch first") || err.contains("non-fast-forward"))
}

// ---------------------------------------------------------------- escalation

pub fn read_flag(topdir: &str) -> Option<Value> {
    serde_json::from_str(&std::fs::read_to_string(git_dir(topdir).join(FLAG_FILE)).ok()?).ok()
}

fn clear_flag(topdir: &str) {
    let _ = std::fs::remove_file(git_dir(topdir).join(FLAG_FILE));
}

/// The resolver could not finish: `item` asks a human (question state, the
/// widget says what conflicts and why), and the checkout records it so this
/// engine holds the project until the answer.
pub fn escalate(api: &Api, project: &Project, item: &WorkItem, topdir: &str, c: &Conflict) -> Result<(), String> {
    let detail = format!(
        "This engine's checkout ({topdir}) could not merge the remote: both sides changed the same lines, and the \
         resolver agent could not combine them.\n\nWhy: {}\n\nFiles: {}\n\nCommits only this checkout has:\n{}\n\n\
         Commits on the remote:\n{}\n\nThis engine starts nothing new in this project until you answer; other \
         engines carry on. Answer with how to combine them (e.g. \"keep both lists, ours first\", \"theirs replaces \
         ours in X\"), and the resolver tries again with that as its instruction. If you merged it by hand and \
         pushed, answer \"resolved by hand\".",
        c.why, c.files.join(", "), if c.ours.is_empty() { "(none)" } else { &c.ours }, if c.theirs.is_empty() { "(none)" } else { &c.theirs },
    );
    let widget = json!({
        "title": format!("Merge conflict: {} file(s) both sides changed — how should they be combined?", c.files.len()),
        "summary": "",
        "detail": detail,
        "fields": [{"key": "answer", "label": "How to combine them", "type": "text", "value": ""}]
    });
    let base = format!("/api/projects/{}/workitems/{}", project.key(), item.id);
    api.post(&format!("{base}/details"), &json!({"key": "question", "valuetype": "json", "value": widget})).map_err(|e| e.to_string())?;
    let mut fresh = api.get(&base).map_err(|e| e.to_string())?;
    let version = fresh.get("version").and_then(|v| v.as_u64()).unwrap_or(item.version);
    fresh["state"] = json!("question");
    fresh["lasterror"] = json!(format!("merge conflict in {}", c.files.join(", ")));
    api.put(&format!("{base}?expect_version={version}"), &fresh).map_err(|e| e.to_string())?;
    let flag = json!({"item": item.id, "project": project.key(), "files": c.files, "since": iter_core::now_utc()});
    std::fs::write(git_dir(topdir).join(FLAG_FILE), flag.to_string()).map_err(|e| e.to_string())?;
    println!("[engine] {topdir}: merge conflict in {} — {} asks a human; this project holds on this engine until the answer", c.files.join(", "), &item.id[..8.min(item.id.len())]);
    Ok(())
}

/// The dispatch hold while a conflict waits on a human: Some(reason) when the
/// recorded item is still in `question`.
pub fn hold_reason(topdir: &str, items: &[WorkItem]) -> Option<String> {
    let flag = read_flag(topdir)?;
    let id = flag.get("item").and_then(|i| i.as_str())?;
    let item = items.iter().find(|i| i.id == id)?;
    (item.state == "question").then(|| format!("git merge conflict in this engine's checkout: {} waits on a human", &id[..8.min(id.len())]))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sh(cwd: &Path, script: &str) {
        run_shell(&cwd.to_string_lossy(), script, 60).unwrap_or_else(|e| panic!("{script}: {e}"));
    }

    /// a bare remote and two clones of it, `a` and `b`, sharing base.txt
    fn setup(tag: &str) -> (PathBuf, PathBuf, PathBuf) {
        let root = std::env::temp_dir().join(format!("iter-merge-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        sh(&root, "git init -q --bare -b main remote.git && git clone -q remote.git a 2>/dev/null && cd a && git config user.email t@t && git config user.name t && git checkout -q -b main && printf 'one\\ntwo\\nthree\\n' > base.txt && git add . && git commit -qm base && git push -q origin main && cd .. && git clone -q remote.git b && cd b && git config user.email t@t && git config user.name t");
        (root.clone(), root.join("a"), root.join("b"))
    }

    fn conflicting(a: &Path, b: &Path) {
        sh(a, "printf 'one\\nTWO from a\\nthree\\n' > base.txt && git commit -qam 'a: two' && git push -q");
        sh(b, "printf 'one\\nTWO from b\\nthree\\n' > base.txt && git commit -qam 'b: two'");
    }

    fn resolver<'a>(api: &'a Api, project: &'a Project, item: &'a WorkItem, run: &'a dyn Fn(&str) -> Result<String, String>) -> Resolver<'a> {
        Resolver { api, project, item, request: "", run }
    }

    #[test]
    fn a_clean_merge_needs_no_resolver() {
        let (root, a, b) = setup("clean");
        sh(&a, "echo a > a.txt && git add a.txt && git commit -qm a && git push -q");
        sh(&b, "echo b > b.txt && git add b.txt && git commit -qm b");
        push(&b.to_string_lossy(), None).unwrap();
        sh(&a, "git pull -q && test -f b.txt");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_pull_that_conflicts_is_backed_out_never_left_half_merged() {
        let (root, a, b) = setup("abort");
        conflicting(&a, &b);
        let top = b.to_string_lossy().into_owned();
        let Err(MergeError::Conflict(c)) = pull(&top, None) else { panic!("expected a conflict") };
        assert_eq!(c.files, vec!["base.txt".to_string()]);
        assert!(c.ours.contains("b: two") && c.theirs.contains("a: two"), "{c:?}");
        assert!(!merging(&top));
        assert_eq!(run_shell(&top, "git status --porcelain", 15).unwrap().trim(), "");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn the_resolver_combines_both_sides_and_the_push_goes_through() {
        let (root, a, b) = setup("resolve");
        conflicting(&a, &b);
        let top = b.to_string_lossy().into_owned();
        let (api, project, item) = (Api::new("http://127.0.0.1:9", "t"), Project::default(), WorkItem { name: "b's change".into(), ..Default::default() });
        let seen = std::sync::Mutex::new(String::new());
        let file = b.join("base.txt");
        let run = |prompt: &str| -> Result<String, String> {
            *seen.lock().unwrap() = prompt.to_string();
            std::fs::write(&file, "one\nTWO from a\nTWO from b\nthree\n").unwrap();
            Ok("kept both lines".into())
        };
        push(&top, Some(&resolver(&api, &project, &item, &run))).unwrap();
        let p = seen.lock().unwrap().clone();
        assert!(p.contains("- base.txt") && p.contains("a: two") && p.contains("b: two") && p.contains("b's change"), "{p}");
        sh(&a, "git pull -q && grep -q 'TWO from a' base.txt && grep -q 'TWO from b' base.txt");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn markers_left_or_cannot_resolve_back_the_merge_out() {
        let (root, a, b) = setup("refuse");
        conflicting(&a, &b);
        let top = b.to_string_lossy().into_owned();
        let (api, project, item) = (Api::new("http://127.0.0.1:9", "t"), Project::default(), WorkItem::default());
        let tries = std::sync::atomic::AtomicU32::new(0);
        let lazy = |_: &str| -> Result<String, String> { tries.fetch_add(1, std::sync::atomic::Ordering::SeqCst); Ok("done".into()) };
        let Err(MergeError::Conflict(c)) = push(&top, Some(&resolver(&api, &project, &item, &lazy))) else { panic!("expected a conflict") };
        assert!(c.why.contains("markers are still in base.txt"), "{}", c.why);
        assert_eq!(tries.load(std::sync::atomic::Ordering::SeqCst), 2, "one more try before a person is asked");
        assert!(!merging(&top));
        // a second try that finishes the job merges
        let file = b.join("base.txt");
        let second = std::sync::atomic::AtomicU32::new(0);
        let late = |p: &str| -> Result<String, String> {
            if second.fetch_add(1, std::sync::atomic::Ordering::SeqCst) == 1 {
                assert!(p.contains("## Second attempt"), "{p}");
                std::fs::write(&file, "one\nTWO from b\nthree\n").unwrap();
            }
            Ok("guessed: b's wording".into())
        };
        sh(&b, "git reset -q --hard HEAD");
        pull(&top, Some(&resolver(&api, &project, &item, &late))).unwrap();
        sh(&b, "git reset -q --hard HEAD~1");
        assert!(!merging(&top));
        let unsure = |_: &str| -> Result<String, String> { Ok("looked at it.\nCANNOT RESOLVE: both rewrite line two; which wins?".into()) };
        let Err(MergeError::Conflict(c)) = pull(&top, Some(&resolver(&api, &project, &item, &unsure))) else { panic!("expected a conflict") };
        assert!(c.why.contains("which wins?"), "{}", c.why);
        assert!(!merging(&top));
        assert_eq!(run_shell(&top, "cat base.txt", 15).unwrap().replace('\r', ""), "one\nTWO from b\nthree\n", "ours, untouched");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn the_hold_lasts_while_the_recorded_item_waits_on_a_human() {
        let (root, _a, b) = setup("hold");
        let top = b.to_string_lossy().into_owned();
        let mut item = WorkItem { id: "abcdef1234567890".into(), state: "question".into(), ..Default::default() };
        assert_eq!(hold_reason(&top, &[item.clone()]), None);
        std::fs::write(git_dir(&top).join(FLAG_FILE), json!({"item": item.id}).to_string()).unwrap();
        assert!(hold_reason(&top, &[item.clone()]).unwrap().contains("abcdef12 waits on a human"));
        item.state = "queued".into();
        assert_eq!(hold_reason(&top, &[item]), None, "answered: no hold, the record stays for the resolver");
        assert!(read_flag(&top).is_some());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn escalation_asks_a_human_and_records_the_hold() {
        let (root, _a, b) = setup("escalate");
        let top = b.to_string_lossy().into_owned();
        let srv = crate::client::fake::serve(|m, p, _b| match (m, p) {
            ("GET", p) if p.ends_with("/workitems/w1") => Some((200, json!({"id": "w1", "state": "in-progress", "version": 7}))),
            _ => Some((200, json!({}))),
        });
        let project = Project { name: "p".into(), ..Default::default() };
        let item = WorkItem { id: "w1".into(), version: 7, ..Default::default() };
        let c = Conflict { files: vec!["base.txt".into()], ours: "b1 b: two".into(), theirs: "a1 a: two".into(), why: "CANNOT RESOLVE: which wins?".into() };
        escalate(&srv.api(), &project, &item, &top, &c).unwrap();
        let q = srv.calls_to("POST", "/workitems/w1/details");
        assert_eq!(q[0]["key"], "question");
        let detail = q[0]["value"]["detail"].as_str().unwrap();
        assert!(detail.contains("base.txt") && detail.contains("which wins?") && detail.contains("a1 a: two"), "{detail}");
        let put = srv.calls_to("PUT", "/workitems/w1?expect_version=7");
        assert_eq!(put[0]["state"], "question");
        assert_eq!(read_flag(&top).unwrap()["item"], "w1");
        let waiting = WorkItem { id: "w1".into(), state: "question".into(), ..Default::default() };
        assert!(hold_reason(&top, &[waiting]).is_some());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn only_a_behind_refusal_is_retried() {
        assert!(push_was_behind(" ! [rejected]        main -> main (fetch first)\nerror: failed to push some refs"));
        assert!(push_was_behind(" ! [rejected]        main -> main (non-fast-forward)"));
        assert!(!push_was_behind(" ! [remote rejected] main -> main (pre-receive hook declined)"));
        assert!(!push_was_behind("fatal: could not read Username"));
    }
}
