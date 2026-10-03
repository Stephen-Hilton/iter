//! iter_local — the checkout-side helpers the iter5 engine and the `iter`
//! CLI use, with no server involved: node-file walking (`walk`, git-ignore
//! aware), the deterministic test runner (`runtests`, standard result JSON),
//! `validate` (a wrapper over `iter_core::nodefile::conform`) and the iter4 →
//! iter5 converter (`migrate5`, which also reads iter4 testgroup registries
//! via `testgroups`). iter4's marker scan, graph snapshot, node ids and graph
//! edits are gone: `iter_core::nodefile` is the one node-file library.

pub mod migrate5;
pub mod runtests;
pub mod testgroups;
pub mod validate;
pub mod walk;

/// Directories every tree walk skips (2026-09-28, F14): VCS/build noise, the
/// engine home, and `.claude` — Claude Code keeps whole worktree copies of the
/// checkout under `.claude/worktrees/`, and a walk that enters them grades a
/// stale copy of a testgroup (and writes its stamp into the copy's registry).
pub const SKIP_DIRS: &[&str] = &[".git", "target", "node_modules", ".iter", ".claude"];

/// True when a directory named `name` is one the tree walks never enter.
pub fn is_skip_dir(name: &str) -> bool {
    SKIP_DIRS.contains(&name)
}

/// The files in `files` that git does not ignore (2026-09-30). Build output can
/// copy whole source folders, node files included (AWS CDK's `cdk.out/` holds a
/// copy of every Lambda folder per build), and a file git ignores can never be
/// committed, so it is never part of the map, a test run or a validation. One
/// `git check-ignore` call per list; outside a git repository, or without git,
/// the list comes back unchanged.
pub fn drop_git_ignored(root: &std::path::Path, files: Vec<std::path::PathBuf>) -> Vec<std::path::PathBuf> {
    use std::io::Write;
    use std::process::{Command, Stdio};
    if files.is_empty() {
        return files;
    }
    let Ok(mut child) = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["check-ignore", "--stdin", "-z"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
    else {
        return files;
    };
    let mut input: Vec<u8> = Vec::new();
    for f in &files {
        input.extend_from_slice(f.to_string_lossy().as_bytes());
        input.push(0);
    }
    // feed stdin from a thread: git answers as it reads, and a long answer
    // would fill the pipe while we were still writing
    let writer = child.stdin.take().map(|mut stdin| std::thread::spawn(move || {
        let _ = stdin.write_all(&input);
    }));
    let Ok(out) = child.wait_with_output() else { return files };
    if let Some(w) = writer {
        let _ = w.join();
    }
    // 0 = some ignored, 1 = none ignored, anything else = not a repository / error
    if out.status.code() != Some(0) {
        return files;
    }
    let ignored: std::collections::HashSet<String> = out
        .stdout
        .split(|b| *b == 0)
        .filter(|s| !s.is_empty())
        .map(|s| String::from_utf8_lossy(s).into_owned())
        .collect();
    files.into_iter().filter(|f| !ignored.contains(f.to_string_lossy().as_ref())).collect()
}

/// The untracked paths git ignores under `root`, from ONE `git ls-files`
/// call (2026-09-30): an ignored folder is listed once with a trailing `/`
/// (`cdk.out/`), so a tree walk asks `contains` per entry instead of running
/// git per folder. Paths are relative to `root`. Empty outside a repository.
#[derive(Default, Debug)]
pub struct GitIgnored {
    root: std::path::PathBuf,
    paths: Vec<String>,
}

impl GitIgnored {
    pub fn load(root: &std::path::Path) -> GitIgnored {
        let out = std::process::Command::new("git")
            .arg("-C")
            .arg(root)
            .args(["ls-files", "--others", "--ignored", "--exclude-standard", "--directory", "-z"])
            .stderr(std::process::Stdio::null())
            .output();
        let paths = match out {
            Ok(o) if o.status.success() => o.stdout.split(|b| *b == 0).filter(|s| !s.is_empty()).map(|s| String::from_utf8_lossy(s).into_owned()).collect(),
            _ => Vec::new(),
        };
        GitIgnored { root: root.to_path_buf(), paths }
    }

    /// Whether git ignores `path` (absolute, or relative to the root).
    pub fn contains(&self, path: &std::path::Path) -> bool {
        if self.paths.is_empty() {
            return false;
        }
        let rel = path.strip_prefix(&self.root).unwrap_or(path).to_string_lossy();
        self.paths.iter().any(|p| match p.strip_suffix('/') {
            Some(dir) => rel == dir || rel.starts_with(p.as_str()),
            None => rel == p.as_str(),
        })
    }
}

/// UTC now as RFC 3339 seconds — the timestamp shape testgroup blocks carry.
pub fn now_iso() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("iter_local_lib_{name}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d.canonicalize().unwrap()
    }

    #[test]
    fn git_ignored_files_are_dropped() {
        let top = tmp("gitignored");
        let git = |args: &[&str]| std::process::Command::new("git").args(args).current_dir(&top).output().unwrap();
        git(&["init", "-q"]);
        std::fs::write(top.join(".gitignore"), "cdk.out/\n").unwrap();
        for f in ["lambda/lambda.code.iter.md", "cdk.out/asset.1/lambda.code.iter.md"] {
            std::fs::create_dir_all(top.join(f).parent().unwrap()).unwrap();
            std::fs::write(top.join(f), "x").unwrap();
        }
        let files = vec![top.join("lambda/lambda.code.iter.md"), top.join("cdk.out/asset.1/lambda.code.iter.md")];
        assert_eq!(super::drop_git_ignored(&top, files), vec![top.join("lambda/lambda.code.iter.md")]);
    }

    #[test]
    fn ignored_folders_are_known_from_one_listing() {
        let top = tmp("ignoredset");
        std::process::Command::new("git").args(["init", "-q"]).current_dir(&top).output().unwrap();
        std::fs::write(top.join(".gitignore"), "cdk.out/\n*.log\n").unwrap();
        for f in ["lambda/handler.py", "cdk.out/asset.1/handler.py", "lambda/run.log"] {
            std::fs::create_dir_all(top.join(f).parent().unwrap()).unwrap();
            std::fs::write(top.join(f), "x").unwrap();
        }
        let ign = super::GitIgnored::load(&top);
        assert!(ign.contains(&top.join("cdk.out")));
        assert!(ign.contains(&top.join("cdk.out/asset.1/handler.py")));
        assert!(ign.contains(&top.join("lambda/run.log")));
        assert!(!ign.contains(&top.join("lambda/handler.py")));
        assert!(!ign.contains(&top.join("cdk.outside/x.py")), "a prefix of a name is not the folder");
    }

    #[test]
    fn outside_a_repository_nothing_is_dropped() {
        let top = tmp("norepo");
        let files = vec![top.join("a.code.iter.md")];
        assert_eq!(super::drop_git_ignored(&top, files.clone()), files);
    }
}
