//! The iter_data connection the checkout-side verbs share (`iter sync`,
//! `iter sweep`, `iter rag sync`), and `iter sync` itself: one immediate
//! file-sync round (crate::filesync::tick — filescan → conform → sync, then
//! the pending graph edits) for the current checkout's project, the same
//! round the engine runs every tick.
//!
//! Connection: --data-url / $ITER_DATA_URL, token $ITER_ENGINE_TOKEN (or
//! $ITER_TOKEN), project --project / $ITER_PROJECT, else the project node's
//! name. Inside an engine-run item the engine sets all of them.

use crate::client::Api;
use iter_core::nodefile::{self, NodeType};
use std::path::{Path, PathBuf};

/// Where to reach iter_data from here, and for which project.
pub struct Conn {
    pub api: Option<Api>,
    pub project: String,
    pub topdir: PathBuf,
    /// how the url was found, for the one-line banner
    pub source: String,
}

/// The project node file directly under `dir/global/` (or a legacy
/// `main.iter.md` in `dir`).
pub fn project_file_in(dir: &Path) -> Option<PathBuf> {
    let g = dir.join("global");
    if let Ok(rd) = std::fs::read_dir(&g) {
        let mut hits: Vec<PathBuf> = rd
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.is_file() && nodefile::type_of(&p.to_string_lossy()) == Some(NodeType::Project) && !nodefile::is_legacy_filename(&p.to_string_lossy()))
            .collect();
        hits.sort();
        if let Some(h) = hits.into_iter().next() {
            return Some(h);
        }
    }
    let legacy = dir.join("main.iter.md");
    legacy.is_file().then_some(legacy)
}

/// The checkout these verbs act on: $ITER_TOPDIR (inside an engine-run
/// item), else a path given as --project, else the nearest directory at or
/// above `cwd` holding a project node (`global/*.project.iter.md`), else
/// `fallback`.
pub fn checkout_root(project_flag: Option<&str>, cwd: &Path, fallback: &Path) -> PathBuf {
    if let Ok(t) = std::env::var("ITER_TOPDIR") {
        if !t.trim().is_empty() {
            return PathBuf::from(t.trim());
        }
    }
    if let Some(p) = project_flag.filter(|p| p.contains('/') || *p == ".") {
        return PathBuf::from(p);
    }
    cwd.ancestors().find(|d| project_file_in(d).is_some()).map(Path::to_path_buf).unwrap_or_else(|| fallback.to_path_buf())
}

/// The project node's name, read from the checkout.
pub fn project_name(topdir: &Path) -> String {
    project_file_in(topdir)
        .and_then(|f| {
            let sp = format!("{{topdir}}/{}", f.strip_prefix(topdir).ok()?.to_string_lossy());
            let text = std::fs::read_to_string(&f).ok()?;
            nodefile::parse_tolerant(&sp, &text).ok().map(|(d, _)| d.name)
        })
        .unwrap_or_default()
}

/// Resolve url, token and project: explicit flag, then the ITER_* environment.
pub fn conn(topdir: &Path, project_flag: Option<&str>, data_url_flag: Option<&str>) -> Conn {
    let nonempty = |s: Option<String>| s.filter(|v| !v.trim().is_empty());
    let (url, source) = if let Some(u) = nonempty(data_url_flag.map(String::from)) {
        (u, "--data-url".to_string())
    } else if let Some(u) = nonempty(std::env::var("ITER_DATA_URL").ok()) {
        (u, "ITER_DATA_URL".to_string())
    } else {
        (String::new(), String::new())
    };
    let token = nonempty(std::env::var("ITER_ENGINE_TOKEN").ok()).or_else(|| nonempty(std::env::var("ITER_TOKEN").ok())).unwrap_or_default();
    let project = nonempty(project_flag.filter(|p| !p.contains('/') && *p != ".").map(String::from))
        .or_else(|| nonempty(std::env::var("ITER_PROJECT").ok()))
        .unwrap_or_else(|| project_name(topdir));
    let api = (!url.is_empty() && !token.is_empty()).then(|| Api::new(&url, &token));
    Conn { api, project, topdir: topdir.to_path_buf(), source }
}

/// The engine name `iter sync` reports as: $ITER_ENGINE_NAME, else the
/// short hostname (the engine's own default).
pub fn engine_name() -> String {
    if let Ok(n) = std::env::var("ITER_ENGINE_NAME") {
        if !n.trim().is_empty() {
            return n.trim().to_string();
        }
    }
    std::process::Command::new("hostname")
        .arg("-s")
        .output()
        .ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "engine".into())
}

/// `iter sync [--read-only]`: one file-sync round now. Exit 0 ok, 1 the
/// round failed, 2 no connection.
pub fn sync_verb(c: &Conn, read_only: bool) -> i32 {
    let Some(api) = &c.api else {
        eprintln!("iter sync: no iter_data connection (set --data-url / ITER_DATA_URL and ITER_ENGINE_TOKEN)");
        return 2;
    };
    if c.project.is_empty() {
        eprintln!("iter sync: no project (set --project / ITER_PROJECT, or run inside a checkout with a global/*.project.iter.md)");
        return 2;
    }
    let mut st = crate::filesync::FileSyncState::default();
    match crate::filesync::tick(api, &engine_name(), &c.project, &c.topdir, read_only, true, &mut st) {
        Ok(()) => {
            let s = &st.last;
            println!(
                "iter sync ({}): project {} — {} file(s) read, {} sent, {} deleted, {} conformed, {} rewritten, {} graph edit(s) written{}",
                c.source, c.project, s.read, s.sent, s.deleted, s.conformed, s.rewritten, s.pending_applied,
                if s.pending_waiting > 0 { format!(", {} waiting on locks", s.pending_waiting) } else { String::new() }
            );
            0
        }
        Err(e) => {
            eprintln!("iter sync: {e}");
            1
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checkout_root_finds_the_project_node() {
        let top = std::env::temp_dir().join(format!("iter5_root_{}", uuid::Uuid::new_v4()));
        let sub = top.join("proj");
        std::fs::create_dir_all(sub.join("global")).unwrap();
        std::fs::create_dir_all(sub.join("src/deep")).unwrap();
        std::fs::write(sub.join("global/demo.project.iter.md"), "---\nname: Demo\n---\n").unwrap();
        // SAFETY: tests in this crate do not read ITER_TOPDIR concurrently
        unsafe { std::env::remove_var("ITER_TOPDIR") };
        assert_eq!(checkout_root(None, &sub.join("src/deep"), &top), sub);
        assert_eq!(checkout_root(Some("demo"), &sub, &top), sub, "a project NAME never picks the directory");
        assert_eq!(checkout_root(Some("./elsewhere/"), &sub, &top), PathBuf::from("./elsewhere/"));
        assert_eq!(checkout_root(None, &top, &top), top, "no project node above: the fallback");
        assert_eq!(project_name(&sub), "Demo");
    }
}
