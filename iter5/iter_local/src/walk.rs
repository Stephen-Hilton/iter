//! Node-file walking (iter5 spec §3.2): every `*.iter.md` whose filename names
//! a node type (`iter_core::nodefile::type_of`), under the project's
//! `scandirs`, git-ignore aware, never entering `.git`, `target`,
//! `node_modules`, `.iter` or `.claude` ([`crate::SKIP_DIRS`]). Symlinks are
//! never followed.
//!
//! Two shapes: [`node_files`] (one full walk, for validate / migrate / the
//! test sweep) and the pieces the engine's incremental filescan is built from
//! ([`walk`], [`list_dir`], [`stat`]): a walk also returns every directory it
//! entered with its mtime, so a later tick only re-lists directories whose
//! mtime moved (an entry was added or removed) and only re-reads files whose
//! `(mtime, size)` moved.

use crate::GitIgnored;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// What a cheap `stat` tells about a file or directory.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct Stat {
    /// modification time, nanoseconds since the epoch
    pub mtime_ns: u64,
    pub size: u64,
}

/// `lstat` (symlinks are not followed); None when the path is gone.
pub fn stat(p: &Path) -> Option<Stat> {
    let m = std::fs::symlink_metadata(p).ok()?;
    let mtime_ns = m
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0);
    Some(Stat { mtime_ns, size: m.len() })
}

/// Is `name` (a bare file name) a node file name?
pub fn is_node_name(name: &str) -> bool {
    iter_core::nodefile::type_of(name).is_some()
}

/// One directory, one level: its sub-directories to descend into (skip dirs
/// and symlinks left out) and its node files.
pub fn list_dir(dir: &Path) -> (Vec<PathBuf>, Vec<PathBuf>) {
    let mut dirs = Vec::new();
    let mut files = Vec::new();
    let Ok(rd) = std::fs::read_dir(dir) else { return (dirs, files) };
    for e in rd.flatten() {
        let Ok(ft) = e.file_type() else { continue };
        let name = e.file_name().to_string_lossy().into_owned();
        if ft.is_dir() {
            if !crate::is_skip_dir(&name) {
                dirs.push(e.path());
            }
        } else if ft.is_file() && is_node_name(&name) {
            files.push(e.path());
        }
    }
    dirs.sort();
    files.sort();
    (dirs, files)
}

/// A full walk below `root`.
#[derive(Debug, Default)]
pub struct Walked {
    /// node files (absolute), sorted
    pub files: Vec<PathBuf>,
    /// every directory entered, with its stat
    pub dirs: Vec<(PathBuf, Stat)>,
}

/// Walk `root` recursively; directories git ignores (per `ignored`, one
/// listing loaded up front) are not entered. Files are NOT checked against
/// git here — pass new files through [`crate::drop_git_ignored`] (one git call).
pub fn walk(root: &Path, ignored: &GitIgnored) -> Walked {
    let mut out = Walked::default();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Some(st) = stat(&dir) else { continue };
        out.dirs.push((dir.clone(), st));
        let (subdirs, files) = list_dir(&dir);
        for f in files {
            if !ignored.contains(&f) {
                out.files.push(f);
            }
        }
        for d in subdirs.into_iter().rev() {
            if !ignored.contains(&d) {
                stack.push(d);
            }
        }
    }
    out.files.sort();
    out.dirs.sort();
    out
}

/// The scan roots of a project: its `scandirs` entries (`{topdir}/…`,
/// relative = relative to topdir) that stay inside `topdir`; the whole
/// topdir when none are given or none are usable.
pub fn scan_roots(topdir: &Path, scandirs: &[String]) -> Vec<PathBuf> {
    let mut roots: Vec<PathBuf> = Vec::new();
    for s in scandirs {
        let s = s.trim();
        if s.is_empty() {
            continue;
        }
        let rel = s.strip_prefix("{topdir}").unwrap_or(s).trim_start_matches('/');
        if rel.split('/').any(|c| c == "..") || (s.starts_with('/') && !s.starts_with("{topdir}")) {
            continue;
        }
        let p = if rel.is_empty() { topdir.to_path_buf() } else { topdir.join(rel.trim_end_matches('/')) };
        roots.push(p);
    }
    if roots.is_empty() {
        roots.push(topdir.to_path_buf());
    }
    // a root under another root is walked once
    roots.sort();
    let mut kept: Vec<PathBuf> = Vec::new();
    for r in roots {
        if !kept.iter().any(|k| r.starts_with(k)) {
            kept.push(r);
        }
    }
    kept
}

/// Every node file under the scan roots (absolute, sorted, deduped), with
/// git-ignored files dropped. Agent memory files are included (callers that
/// sync filter them with `is_synced`).
pub fn node_files(topdir: &Path, roots: &[PathBuf]) -> Vec<PathBuf> {
    let ignored = GitIgnored::load(topdir);
    let mut files: Vec<PathBuf> = Vec::new();
    for r in roots {
        files.extend(walk(r, &ignored).files);
    }
    files.sort();
    files.dedup();
    crate::drop_git_ignored(topdir, files)
}

/// `{topdir}/rel` for a path under `topdir` (None outside it).
pub fn topdir_path(topdir: &Path, abs: &Path) -> Option<String> {
    let rel = abs.strip_prefix(topdir).ok()?;
    let rel = rel.to_string_lossy().replace('\\', "/");
    Some(if rel.is_empty() { "{topdir}".to_string() } else { format!("{{topdir}}/{rel}") })
}

/// The real path of a stored `{topdir}/rel` path, refusing anything that
/// would leave `topdir` (`..` components, absolute paths, other anchors).
pub fn real_path(topdir: &Path, stored: &str) -> Result<PathBuf, String> {
    let rel = stored
        .strip_prefix("{topdir}/")
        .ok_or_else(|| format!("{stored:?} is not a {{topdir}}/ path"))?;
    if rel.is_empty() || rel.starts_with('/') || rel.contains('\\') || rel.split('/').any(|c| c == ".." || c == "." || c.is_empty()) {
        return Err(format!("{stored:?} leaves the project directory"));
    }
    if rel.split('/').any(|c| c == ".git") {
        return Err(format!("{stored:?} is inside .git"));
    }
    Ok(topdir.join(rel))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("iter_local_walk_{name}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        iter_core::platform::canonicalize(&d).unwrap()
    }

    #[test]
    fn walks_node_files_only_skipping_noise_and_ignored() {
        let top = tmp("nodes");
        std::process::Command::new("git").args(["init", "-q"]).current_dir(&top).output().unwrap();
        std::fs::write(top.join(".gitignore"), "cdk.out/\n").unwrap();
        for f in [
            "global/p.project.iter.md",
            "src/a/a.code.iter.md",
            "src/a/notes.iter.md",
            "src/a/readme.md",
            "src/a/x.agentmem.iter.md",
            "target/b.code.iter.md",
            "node_modules/c.code.iter.md",
            ".iter/d.code.iter.md",
            "cdk.out/asset/e.code.iter.md",
        ] {
            std::fs::create_dir_all(top.join(f).parent().unwrap()).unwrap();
            std::fs::write(top.join(f), "x").unwrap();
        }
        let got: Vec<String> = node_files(&top, &[top.clone()]).iter().map(|p| topdir_path(&top, p).unwrap()).collect();
        assert_eq!(got, vec!["{topdir}/global/p.project.iter.md", "{topdir}/src/a/a.code.iter.md", "{topdir}/src/a/x.agentmem.iter.md"]);
        let w = walk(&top, &GitIgnored::load(&top));
        assert!(w.dirs.iter().any(|(d, _)| d == &top.join("src/a")));
        assert!(!w.dirs.iter().any(|(d, _)| d.starts_with(top.join("target"))));
    }

    #[test]
    fn scan_roots_stay_inside() {
        let top = PathBuf::from("/x/top");
        assert_eq!(scan_roots(&top, &[]), vec![top.clone()]);
        assert_eq!(scan_roots(&top, &["{topdir}/".into()]), vec![top.clone()]);
        assert_eq!(scan_roots(&top, &["{topdir}/src/".into(), "{topdir}/src/a".into(), "global".into()]), vec![top.join("global"), top.join("src")]);
        assert_eq!(scan_roots(&top, &["{topdir}/../etc".into(), "/etc".into()]), vec![top.clone()]);
    }

    #[test]
    fn real_path_refuses_escapes() {
        let top = PathBuf::from("/x/top");
        assert_eq!(real_path(&top, "{topdir}/a/b.code.iter.md").unwrap(), top.join("a/b.code.iter.md"));
        for bad in ["{topdir}/../etc/passwd", "{topdir}/a/../../x", "/etc/passwd", "a/b", "{topdir}/", "{topdir}//etc", "{topdir}/.git/config", "{topdir}/./a"] {
            assert!(real_path(&top, bad).is_err(), "{bad} must be refused");
        }
    }
}
