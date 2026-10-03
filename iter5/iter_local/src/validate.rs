//! `iter validate [--fix]` (iter5): a thin wrapper over
//! `iter_core::nodefile::conform`. Every node file under the topdir (agent
//! memory excluded) is conformed in memory; a file whose conformed text
//! differs is "not conformed" (with `--fix` it is rewritten), and conform's
//! findings are reported. Duplicate ids across files are reported too (the
//! server would give the second file a new id).

use iter_core::nodefile::{self, Finding, NodeType};
use serde::Serialize;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize)]
pub struct FileReport {
    /// `{topdir}/…`
    pub path: String,
    /// conform would change the file
    pub changed: bool,
    /// it was rewritten (--fix)
    pub fixed: bool,
    pub findings: Vec<Finding>,
}

#[derive(Debug, Default, Serialize)]
pub struct Report {
    pub files_checked: usize,
    pub files: Vec<FileReport>,
    /// id → the files sharing it
    pub duplicate_ids: BTreeMap<String, Vec<String>>,
}

/// Findings that stay true after conform (it cannot fix them by itself).
pub const PERSISTENT: &[&str] = &["level-missing", "teststate-invalid", "connects-invalid", "children-invalid", "timestamps-invalid", "frontmatter-unterminated"];

impl Report {
    /// Files still not conformed (would change).
    pub fn unconformed(&self) -> usize {
        self.files.iter().filter(|f| f.changed && !f.fixed).count()
    }
    /// Findings conform cannot clear on its own.
    pub fn persistent(&self) -> usize {
        self.files.iter().flat_map(|f| &f.findings).filter(|f| PERSISTENT.contains(&f.code.as_str()) || f.code.ends_with("-invalid")).count()
    }
    /// 0 = clean, 1 = something left to do.
    pub fn exit_code(&self) -> i32 {
        if self.unconformed() > 0 || self.persistent() > 0 || !self.duplicate_ids.is_empty() { 1 } else { 0 }
    }
}

/// Validate one file (`single`) or every node file under `topdir`.
pub fn run(topdir: &Path, single: Option<&Path>, fix: bool) -> std::io::Result<Report> {
    let files: Vec<PathBuf> = match single {
        Some(f) => vec![f.to_path_buf()],
        None => crate::walk::node_files(topdir, &[topdir.to_path_buf()]),
    };
    let now = nodefile::now_ts();
    let mut rep = Report::default();
    let mut ids: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for f in files {
        let Some(stored) = crate::walk::topdir_path(topdir, &f) else { continue };
        match nodefile::type_of(&stored) {
            None | Some(NodeType::Agentmem) => continue,
            _ => {}
        }
        let text = std::fs::read_to_string(&f)?;
        let c = nodefile::conform(&stored, &text, &now, "");
        rep.files_checked += 1;
        let mut fixed = false;
        if c.changed && fix {
            std::fs::write(&f, &c.text)?;
            fixed = true;
        }
        if let Some(d) = &c.doc {
            ids.entry(d.id.clone()).or_default().push(stored.clone());
        }
        if c.changed || !c.findings.is_empty() {
            rep.files.push(FileReport { path: stored, changed: c.changed, fixed, findings: c.findings });
        }
    }
    rep.duplicate_ids = ids.into_iter().filter(|(_, v)| v.len() > 1).collect();
    Ok(rep)
}

/// The node-text checks the test sweep files `ingest` items from (adapted
/// from iter4's docs/node_text_standard.md rules to v5 `desc` + body).
pub fn node_text_findings(desc: &str, body: &str) -> Vec<(&'static str, String)> {
    let mut out = Vec::new();
    let d = desc.trim().trim_matches('"');
    if d.is_empty() {
        out.push(("missing-desc", "no `desc:` — about 100 words an agent can use to decide whether to read the whole file".to_string()));
    } else if !description_is_action(d) {
        out.push(("desc-not-action", format!(
            "`desc` reads as a label, not an action: {:?} — say what it DOES (\"Reads …, checks … and hands … so that …\")",
            d.chars().take(80).collect::<String>())));
    }
    let words = body.split_whitespace().count();
    if words < 60 {
        out.push(("thin-body", format!("the body is {words} words — say what it does, how (key functions and files), what it takes and hands out, why it matters, one example")));
    }
    out
}

/// Starts with an action, not a label: not "The/A/An/This …", and not
/// "<label>: x, y, z".
pub fn description_is_action(d: &str) -> bool {
    let first = d.split_whitespace().next().unwrap_or("").trim_matches(|c: char| !c.is_alphanumeric()).to_lowercase();
    if ["the", "a", "an", "this", "these"].contains(&first.as_str()) {
        return false;
    }
    if let Some((head, tail)) = d.split_once(':') {
        let words = head.split_whitespace().count();
        let verb_first = first.len() > 3 && first.ends_with('s') && words > 1;
        if words <= 8 && tail.matches(',').count() >= 2 && !verb_first {
            return false;
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("iter_local_check5_{name}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d.canonicalize().unwrap()
    }

    #[test]
    fn validate_reports_then_fixes_idempotently() {
        let top = tmp("fix");
        std::fs::create_dir_all(top.join("src/a")).unwrap();
        std::fs::write(top.join("src/a/a.code.iter.md"), "---\nname: A\ndescription: \"does a\"\n---\n# A\n").unwrap();
        std::fs::write(top.join("src/a/x.agentmem.iter.md"), "anything").unwrap();
        std::fs::write(top.join("src/a/plain.iter.md"), "plain doc").unwrap();
        let r = run(&top, None, false).unwrap();
        assert_eq!(r.files_checked, 1);
        assert_eq!(r.unconformed(), 1);
        assert_eq!(r.exit_code(), 1);
        assert!(r.files[0].findings.iter().any(|f| f.code == "level-missing"));
        let r = run(&top, None, true).unwrap();
        assert!(r.files[0].fixed);
        let after = std::fs::read_to_string(top.join("src/a/a.code.iter.md")).unwrap();
        assert!(after.contains("desc: ") && after.contains("level: component"));
        let r = run(&top, None, false).unwrap();
        assert_eq!(r.unconformed(), 0, "conform is idempotent");
        assert_eq!(r.exit_code(), 0);
        assert_eq!(std::fs::read_to_string(top.join("src/a/x.agentmem.iter.md")).unwrap(), "anything");
    }

    #[test]
    fn duplicate_ids_are_reported() {
        let top = tmp("dups");
        let id = "3f0c1a2b-1111-4222-8333-444455556666";
        for f in ["a.bizreq.iter.md", "b.bizreq.iter.md"] {
            std::fs::write(top.join(f), format!("---\nid: {id}\nname: x\n---\nbody\n")).unwrap();
        }
        let r = run(&top, None, true).unwrap();
        assert_eq!(r.duplicate_ids[id].len(), 2);
        assert_eq!(r.exit_code(), 1);
    }

    #[test]
    fn node_text_rules() {
        assert!(node_text_findings("", "").iter().any(|f| f.0 == "missing-desc"));
        assert!(node_text_findings("The parser", &"word ".repeat(70)).iter().any(|f| f.0 == "desc-not-action"));
        assert!(node_text_findings("Reads files and hands them on", &"word ".repeat(70)).is_empty());
    }
}
