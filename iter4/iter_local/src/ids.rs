//! Stable node-file ids (iter4, decided 2026-09-28): every `*.nodetype.iter.md`
//! frontmatter carries `id: <uuid>` as its first key, so iter_data can follow
//! a node across renames and moves. `check` reports missing, duplicate and
//! malformed ids; `fix` repairs them, asking a lookup (iter_data's stored
//! graph, when connected) before minting: a file whose id line was lost gets
//! its old id back and reconnects to its history.

use crate::markers::{self, role_of};
use crate::project::{ITERGLOB, Project};
use serde::Serialize;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

#[derive(Debug, Default, Serialize)]
pub struct IdReport {
    pub files: usize,
    /// `{topdir}/…` paths with no id
    pub missing: Vec<String>,
    /// id → the files sharing it (2+)
    pub duplicates: BTreeMap<String, Vec<String>>,
    /// (path, value) where the id is not a UUID
    pub invalid: Vec<(String, String)>,
}

impl IdReport {
    pub fn clean(&self) -> bool {
        self.missing.is_empty() && self.duplicates.is_empty() && self.invalid.is_empty()
    }
}

/// One repair `fix` made (or would make, on a dry run).
#[derive(Debug, Serialize, Clone)]
pub struct IdChange {
    pub path: String,
    pub id: String,
    /// "reused (path)" | "reused (name)" | "minted" | "minted (duplicate of <id>)" | "minted (invalid)"
    pub how: String,
}

/// What the stored graph knows, asked by `fix`. The engine answers from
/// iter_data; with no server, `NoLookup` makes every repair a fresh mint.
pub trait IdLookup {
    /// ids of stored vertices at exactly this `{topdir}/…` path
    fn by_path(&self, path: &str) -> Vec<String>;
    /// ids of stored vertices with this nodetype + name
    fn by_name(&self, nodetype: &str, name: &str) -> Vec<String>;
    /// the stored path of a vertex id, if the graph has it
    fn path_of(&self, id: &str) -> Option<String>;
}

pub struct NoLookup;

impl IdLookup for NoLookup {
    fn by_path(&self, _: &str) -> Vec<String> {
        vec![]
    }
    fn by_name(&self, _: &str, _: &str) -> Vec<String> {
        vec![]
    }
    fn path_of(&self, _: &str) -> Option<String> {
        None
    }
}

pub fn is_uuid(s: &str) -> bool {
    uuid::Uuid::parse_str(s).is_ok() && s.len() == 36
}

/// Every node file the scan would read: `{iterglob}` under the scan dirs plus
/// the main file, canonical, sorted, nodetype files only — never one git
/// ignores (2026-09-30: build output such as AWS CDK's cdk.out/ copies node
/// files, and those copies became orphan vertices and got ids written in).
pub fn node_files(project: &Project) -> Vec<PathBuf> {
    let vars = project.vars();
    let mut files: Vec<PathBuf> = Vec::new();
    for dir in &project.scandirs {
        let pattern = format!("{}/{}", dir.to_string_lossy(), ITERGLOB);
        files.extend(vars.expand_files(&pattern, dir));
    }
    if project.mainfile.is_file() {
        files.push(project.mainfile.clone());
    }
    let mut files: Vec<PathBuf> = files
        .iter()
        .map(|p| p.canonicalize().unwrap_or_else(|_| p.clone()))
        .filter(|p| p.file_name().and_then(|n| n.to_str()).map(|n| role_of(n).is_some()).unwrap_or(false))
        .collect();
    files.sort();
    files.dedup();
    crate::drop_git_ignored(&project.root, files)
}

/// `{topdir}/rel/path` for a file under the topdir (absolute path otherwise).
pub fn topdir_path(project: &Project, file: &Path) -> String {
    let top = project.topdir.canonicalize().unwrap_or_else(|_| project.topdir.clone());
    match file.strip_prefix(&top) {
        Ok(rel) => format!("{{topdir}}/{}", rel.to_string_lossy()),
        Err(_) => file.to_string_lossy().into_owned(),
    }
}

/// The frontmatter `id:` of a node file's content ("" when absent).
pub fn read_id(content: &str) -> String {
    markers::parse_front(content).scalar("id").trim().trim_matches('"').trim_matches('\'').to_string()
}

pub fn check(project: &Project) -> IdReport {
    let mut rep = IdReport::default();
    let mut seen: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for f in node_files(project) {
        rep.files += 1;
        let content = std::fs::read_to_string(&f).unwrap_or_default();
        let id = read_id(&content);
        let p = topdir_path(project, &f);
        if id.is_empty() {
            rep.missing.push(p);
        } else if !is_uuid(&id) {
            rep.invalid.push((p, id));
        } else {
            seen.entry(id).or_default().push(p);
        }
    }
    rep.duplicates = seen.into_iter().filter(|(_, v)| v.len() > 1).collect();
    rep
}

/// Write `id` into a node file's content: replace an existing `id:` line in
/// the frontmatter, else insert it as the first key, else (no frontmatter at
/// all) open a frontmatter block holding just the id.
pub fn set_id(content: &str, id: &str) -> String {
    let line = format!("id: {id}");
    let mut lines: Vec<String> = content.split('\n').map(String::from).collect();
    if lines.first().map(|l| l.trim_end() == "---").unwrap_or(false) {
        if let Some(end) = lines.iter().skip(1).position(|l| l.trim_end() == "---").map(|i| i + 1) {
            match lines[1..end].iter_mut().find(|l| l.starts_with("id:")) {
                Some(l) => *l = line,
                None => lines.insert(1, line),
            }
            return lines.join("\n");
        }
    }
    format!("---\n{line}\n---\n{content}")
}

/// Repair every missing, malformed and duplicate id. Reuse needs exactly one
/// stored match (by path, then by nodetype + name) whose id no other file
/// already holds; anything ambiguous gets a fresh UUID. A duplicated id stays
/// with the file at the stored vertex's path (else the first path in sort
/// order); the other copies are re-minted.
pub fn fix(project: &Project, lookup: &dyn IdLookup, dry_run: bool) -> Result<Vec<IdChange>, String> {
    let files = node_files(project);
    let mut contents: Vec<(PathBuf, String, String, String)> = Vec::new(); // (file, path, content, id)
    for f in files {
        let content = std::fs::read_to_string(&f).map_err(|e| format!("{}: {e}", f.display()))?;
        let id = read_id(&content);
        let p = topdir_path(project, &f);
        contents.push((f, p, content, id));
    }
    // ids that are validly held (after duplicate resolution) — reuse must not collide
    let mut holders: BTreeMap<String, Vec<usize>> = BTreeMap::new();
    for (i, c) in contents.iter().enumerate() {
        if is_uuid(&c.3) {
            holders.entry(c.3.clone()).or_default().push(i);
        }
    }
    let mut reassign: Vec<(usize, String)> = Vec::new(); // (index, why)
    for (i, c) in contents.iter().enumerate() {
        if c.3.is_empty() {
            reassign.push((i, String::new()));
        } else if !is_uuid(&c.3) {
            reassign.push((i, "invalid".into()));
        }
    }
    for (id, idxs) in &holders {
        if idxs.len() < 2 {
            continue;
        }
        let stored = lookup.path_of(id);
        let keep = idxs
            .iter()
            .copied()
            .find(|i| Some(&contents[*i].1) == stored.as_ref())
            .unwrap_or(idxs[0]);
        for i in idxs.iter().copied().filter(|i| *i != keep) {
            reassign.push((i, format!("duplicate of {id}")));
        }
    }
    reassign.sort();
    let mut taken: std::collections::HashSet<String> =
        holders.iter().map(|(id, _)| id.clone()).collect();
    let mut changes = Vec::new();
    for (i, why) in reassign {
        let (file, path, content, _) = &contents[i];
        let front = markers::parse_front(content);
        let name = front.scalar("name");
        let nodetype = file
            .file_name()
            .and_then(|n| n.to_str())
            .map(|n| markers::role_name(role_of(n)))
            .unwrap_or("");
        let pick = |cands: Vec<String>, taken: &std::collections::HashSet<String>| -> Option<String> {
            let free: Vec<String> = cands.into_iter().filter(|c| is_uuid(c) && !taken.contains(c)).collect();
            if free.len() == 1 { free.into_iter().next() } else { None }
        };
        let (id, how) = if why.starts_with("duplicate") {
            (uuid::Uuid::new_v4().to_string(), format!("minted ({why})"))
        } else if let Some(id) = pick(lookup.by_path(path), &taken) {
            (id, "reused (path)".to_string())
        } else if let Some(id) = (!name.is_empty()).then(|| pick(lookup.by_name(nodetype, &name), &taken)).flatten() {
            (id, "reused (name)".to_string())
        } else if why == "invalid" {
            (uuid::Uuid::new_v4().to_string(), "minted (invalid)".to_string())
        } else {
            (uuid::Uuid::new_v4().to_string(), "minted".to_string())
        };
        taken.insert(id.clone());
        if !dry_run {
            std::fs::write(file, set_id(content, &id)).map_err(|e| format!("{}: {e}", file.display()))?;
        }
        changes.push(IdChange { path: path.clone(), id, how });
    }
    Ok(changes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn set_id_inserts_replaces_and_opens() {
        let a = set_id("---\nname: x\n---\nbody\n", "11111111-1111-4111-8111-111111111111");
        assert!(a.starts_with("---\nid: 11111111-1111-4111-8111-111111111111\nname: x\n---\nbody"));
        let b = set_id(&a, "22222222-2222-4222-8222-222222222222");
        assert_eq!(b.matches("id:").count(), 1);
        assert!(b.contains("id: 22222222-2222-4222-8222-222222222222"));
        let c = set_id("just a body\n", "33333333-3333-4333-8333-333333333333");
        assert!(c.starts_with("---\nid: 33333333"));
        // an `id:` in the body is never touched
        let d = set_id("---\nname: y\n---\nid: not-frontmatter\n", "44444444-4444-4444-8444-444444444444");
        assert!(d.contains("id: not-frontmatter") && d.starts_with("---\nid: 4444"));
    }

    struct Fake;
    impl IdLookup for Fake {
        fn by_path(&self, p: &str) -> Vec<String> {
            if p.ends_with("lib.code.iter.md") { vec!["aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa".into()] } else { vec![] }
        }
        fn by_name(&self, t: &str, n: &str) -> Vec<String> {
            if t == "tests" && n == "Moved" { vec!["bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb".into()] } else { vec![] }
        }
        fn path_of(&self, id: &str) -> Option<String> {
            (id == "cccccccc-cccc-4ccc-8ccc-cccccccccccc").then(|| "{topdir}/b/two.code.iter.md".to_string())
        }
    }

    #[test]
    fn fix_reuses_mints_and_splits_duplicates() {
        let top = std::env::temp_dir().join(format!("iter4_ids_{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(top.join("a")).unwrap();
        std::fs::create_dir_all(top.join("b")).unwrap();
        std::fs::write(top.join("main.iter.md"), "---\nprojectname: t\nglobalscandirs: [\"{topdir}/\"]\n---\n").unwrap();
        std::fs::write(top.join("a/lib.code.iter.md"), "---\nname: Lib\n---\n").unwrap();
        std::fs::write(top.join("a/new.testgroup.iter.md"), "---\nname: Moved\n---\n").unwrap();
        let dup = "---\nid: cccccccc-cccc-4ccc-8ccc-cccccccccccc\nname: Two\n---\n";
        std::fs::write(top.join("a/one.code.iter.md"), dup).unwrap();
        std::fs::write(top.join("b/two.code.iter.md"), dup).unwrap();
        std::fs::write(top.join("a/notes.iter.md"), "plain doc, no nodetype\n").unwrap();
        let project = Project::load(&top);
        let before = check(&project);
        assert_eq!(before.files, 5);
        assert_eq!(before.missing.len(), 3); // main, lib, the testgroup
        assert_eq!(before.duplicates.len(), 1);

        let ch = fix(&project, &Fake, false).unwrap();
        let how: BTreeMap<String, String> = ch.iter().map(|c| (c.path.clone(), c.how.clone())).collect();
        assert_eq!(how["{topdir}/a/lib.code.iter.md"], "reused (path)");
        assert_eq!(how["{topdir}/a/new.testgroup.iter.md"], "reused (name)");
        assert!(how["{topdir}/a/one.code.iter.md"].starts_with("minted (duplicate"));
        assert!(!how.contains_key("{topdir}/b/two.code.iter.md"), "the stored path keeps the id");
        assert!(how["{topdir}/main.iter.md"] == "minted");
        assert!(check(&project).clean());
        // idempotent
        assert!(fix(&project, &Fake, false).unwrap().is_empty());
    }

    /// cdk.out (2026-09-30): node files git ignores are not node files — no
    /// orphan vertex, no id written into a build copy.
    #[test]
    fn git_ignored_copies_are_not_node_files() {
        let top = std::env::temp_dir().join(format!("iter4_ids_ign_{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(top.join("lambda")).unwrap();
        std::fs::create_dir_all(top.join("infra/cdk.out/asset.1")).unwrap();
        std::process::Command::new("git").args(["init", "-q"]).current_dir(&top).output().unwrap();
        std::fs::write(top.join(".gitignore"), "cdk.out/\n").unwrap();
        std::fs::write(top.join("main.iter.md"), "---\nprojectname: t\nglobalscandirs: [\"{topdir}/\"]\n---\n").unwrap();
        std::fs::write(top.join("lambda/lambda.code.iter.md"), "---\nname: Lambda\n---\n").unwrap();
        std::fs::write(top.join("infra/cdk.out/asset.1/lambda.code.iter.md"), "---\nname: Lambda\n---\n").unwrap();
        let files = node_files(&Project::load(&top));
        assert!(files.iter().any(|f| f.ends_with("lambda/lambda.code.iter.md")));
        assert!(!files.iter().any(|f| f.to_string_lossy().contains("cdk.out")), "{files:?}");
        assert!(fix(&Project::load(&top), &Fake, false).unwrap().iter().all(|c| !c.path.contains("cdk.out")));
        assert!(!std::fs::read_to_string(top.join("infra/cdk.out/asset.1/lambda.code.iter.md")).unwrap().contains("id:"));
    }
}
