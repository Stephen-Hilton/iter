//! `iter ids` and `iter sync` (iter4, decided 2026-09-28): give every node
//! file a stable id, then push the checkout's architecture map (the
//! iter_local::graph snapshot) to iter_data. Both work inside an engine-run
//! item (ITER_* env) and from a plain shell in the checkout: the connection
//! falls back to the checkout's `.iter/config.json` and its env file, so
//! `iter sync` in a fresh clone reaches the same iter_data the engine uses.
//! The engine calls `sync_if_changed` on its own, so a pushed map is never
//! more than a minute behind the tree.

use crate::client::Api;
use iter_local::graph;
use iter_local::ids::{self, IdLookup};
use iter_local::project::Project;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Where to reach iter_data from here, and for which project.
pub struct Conn {
    pub api: Option<Api>,
    pub project: String,
    pub topdir: PathBuf,
    /// how the url was found, for the one-line banner
    pub source: String,
}

fn read_env_file(path: &Path) -> HashMap<String, String> {
    let mut out = HashMap::new();
    for line in std::fs::read_to_string(path).unwrap_or_default().lines() {
        let line = line.trim();
        if line.starts_with('#') {
            continue;
        }
        if let Some((k, v)) = line.split_once('=') {
            out.insert(k.trim().to_string(), v.trim().trim_matches('"').trim_matches('\'').to_string());
        }
    }
    out
}

/// The checkout these verbs act on: $ITER_TOPDIR (inside an engine-run
/// item), else a path given as --project, else the nearest directory at or
/// above `cwd` holding a main file (`main.iter.md` or `*.main.iter.md`), else
/// `fallback`. A project NAME never picks the directory: `--project iter4` run
/// in a subfolder of a bigger repo must not map (and write ids into) the whole
/// repo.
pub fn checkout_root(project_flag: Option<&str>, cwd: &Path, fallback: &Path) -> PathBuf {
    if let Ok(t) = std::env::var("ITER_TOPDIR") {
        if !t.trim().is_empty() {
            return PathBuf::from(t.trim());
        }
    }
    if let Some(p) = project_flag.filter(|p| p.contains('/') || *p == ".") {
        return PathBuf::from(p);
    }
    let has_main = |d: &Path| {
        std::fs::read_dir(d)
            .map(|rd| {
                rd.filter_map(|e| e.ok()).any(|e| {
                    let n = e.file_name().to_string_lossy().into_owned();
                    n == "main.iter.md" || n.ends_with(".main.iter.md")
                })
            })
            .unwrap_or(false)
    };
    cwd.ancestors().find(|d| has_main(d)).map(Path::to_path_buf).unwrap_or_else(|| fallback.to_path_buf())
}

/// Resolve url, token and project. Precedence: explicit flag, then the ITER_*
/// environment an engine sets for its agents, then `.iter/config.json` in the
/// topdir (data_url, token_envar, env_file), then main.iter.md's projectname.
pub fn conn(topdir: &Path, project_flag: Option<&str>, data_url_flag: Option<&str>) -> Conn {
    let cfg: Value = std::fs::read_to_string(topdir.join(".iter/config.json"))
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or(Value::Null);
    let nonempty = |s: Option<String>| s.filter(|v| !v.trim().is_empty());
    let (url, source) = if let Some(u) = nonempty(data_url_flag.map(String::from)) {
        (u, "--data-url".to_string())
    } else if let Some(u) = nonempty(std::env::var("ITER_DATA_URL").ok()) {
        (u, "ITER_DATA_URL".to_string())
    } else if let Some(u) = nonempty(cfg["data_url"].as_str().map(String::from)) {
        (u, ".iter/config.json".to_string())
    } else {
        (String::new(), String::new())
    };
    let token = nonempty(std::env::var("ITER_ENGINE_TOKEN").ok())
        .or_else(|| nonempty(std::env::var("ITER_TOKEN").ok()))
        .or_else(|| {
            let envar = cfg["token_envar"].as_str().unwrap_or("ITER_ENGINE_TOKEN");
            nonempty(std::env::var(envar).ok()).or_else(|| {
                let ef = cfg["env_file"].as_str().unwrap_or("./.env");
                let p = if Path::new(ef).is_absolute() { PathBuf::from(ef) } else { topdir.join(ef) };
                nonempty(read_env_file(&p).get(envar).cloned())
            })
        })
        .unwrap_or_default();
    let project = nonempty(project_flag.filter(|p| !p.contains('/') && *p != ".").map(String::from))
        .or_else(|| nonempty(std::env::var("ITER_PROJECT").ok()))
        .unwrap_or_else(|| Project::load(topdir).projectname());
    let api = (!url.is_empty() && !token.is_empty()).then(|| Api::new(&url, &token));
    Conn { api, project, topdir: topdir.to_path_buf(), source }
}

/// Answers `ids::fix` from the stored graph (one GET, then memory).
pub struct GraphLookup {
    vertices: Vec<Value>,
}

impl GraphLookup {
    pub fn fetch(api: &Api, project: &str) -> Result<Self, String> {
        let g = api.get(&format!("/api/projects/{project}/graph")).map_err(|e| e.to_string())?;
        Ok(Self { vertices: g["vertices"].as_array().cloned().unwrap_or_default() })
    }
}

impl IdLookup for GraphLookup {
    fn by_path(&self, path: &str) -> Vec<String> {
        self.vertices.iter().filter(|v| v["path"] == path).filter_map(|v| v["id"].as_str().map(String::from)).collect()
    }
    fn by_name(&self, nodetype: &str, name: &str) -> Vec<String> {
        self.vertices
            .iter()
            .filter(|v| v["nodetype"] == nodetype && v["name"] == name)
            .filter_map(|v| v["id"].as_str().map(String::from))
            .collect()
    }
    fn path_of(&self, id: &str) -> Option<String> {
        self.vertices.iter().find(|v| v["id"] == id).and_then(|v| v["path"].as_str().map(String::from))
    }
}

/// Repair ids, asking the stored graph first when a server is reachable.
pub fn fix_ids(c: &Conn, project: &Project, dry_run: bool) -> Result<Vec<ids::IdChange>, String> {
    let lookup: Box<dyn IdLookup> = match &c.api {
        Some(api) => match GraphLookup::fetch(api, &c.project) {
            Ok(l) => Box::new(l),
            Err(e) => {
                eprintln!("iter ids: stored graph unavailable ({e}); minting fresh ids only");
                Box::new(ids::NoLookup)
            }
        },
        None => Box::new(ids::NoLookup),
    };
    ids::fix(project, lookup.as_ref(), dry_run)
}

/// `iter ids [--fix] [--dry-run]`: report missing / duplicate / malformed ids;
/// with --fix, repair them. Exit 1 when problems remain.
pub fn ids_verb(c: &Conn, fix: bool, dry_run: bool) -> i32 {
    let project = Project::load(&c.topdir);
    let rep = ids::check(&project);
    println!("node files: {}  missing id: {}  duplicate ids: {}  malformed: {}", rep.files, rep.missing.len(), rep.duplicates.len(), rep.invalid.len());
    if !fix {
        for p in &rep.missing {
            println!("  missing   {p}");
        }
        for (id, ps) in &rep.duplicates {
            println!("  duplicate {id}: {}", ps.join(", "));
        }
        for (p, v) in &rep.invalid {
            println!("  malformed {p}: {v:?}");
        }
        return if rep.clean() { 0 } else { 1 };
    }
    match fix_ids(c, &project, dry_run) {
        Ok(changes) => {
            for ch in &changes {
                println!("  {}{} {} ({})", if dry_run { "would set " } else { "" }, ch.path, ch.id, ch.how);
            }
            println!("{} id(s) {}", changes.len(), if dry_run { "would change (dry run)" } else { "written" });
            if dry_run || ids::check(&project).clean() { 0 } else { 1 }
        }
        Err(e) => {
            eprintln!("iter ids: {e}");
            2
        }
    }
}

/// Build the snapshot and push it. `force` pushes even when the stored hash matches.
pub fn push(c: &Conn, force: bool, opts: &graph::SnapOpts) -> Result<Value, String> {
    let api = c.api.as_ref().ok_or("no iter_data connection (set --data-url / ITER_DATA_URL and a token)")?;
    let project = Project::load(&c.topdir);
    let snap = graph::snapshot_with(&project, opts);
    let mut body = graph::sync_body(&snap);
    body["force"] = json!(force);
    api.put(&format!("/api/projects/{}/graph", c.project), &body).map_err(|e| e.to_string())
}

/// `iter sync [--dry-run] [--force] [--no-fix-ids] [--read-only] [--actors FILE]`.
/// `--read-only` writes nothing into the checkout: files without an id get a
/// derived one (spec R11) — for mapping a live checkout another team owns.
pub fn sync_verb(c: &Conn, dry_run: bool, force: bool, fix: bool, read_only: bool, actors: Option<&str>) -> i32 {
    let project = Project::load(&c.topdir);
    let opts = graph::SnapOpts {
        derive_ids: read_only,
        actors_file: actors.map(|a| if Path::new(a).is_absolute() { PathBuf::from(a) } else { c.topdir.join(a) }),
    };
    let fix = fix && !read_only;
    if fix {
        match fix_ids(c, &project, dry_run) {
            Ok(ch) if !ch.is_empty() => println!("ids: {} {}", ch.len(), if dry_run { "would be written" } else { "written" }),
            Ok(_) => {}
            Err(e) => {
                eprintln!("iter sync: id repair failed: {e}");
                return 2;
            }
        }
    }
    let snap = graph::snapshot_with(&project, &opts);
    println!(
        "snapshot: project {} — {} vertices, {} edges, {} orphans, hash {}",
        c.project,
        snap.vertices.len(),
        snap.edges.len(),
        snap.orphans.len(),
        &snap.hash[..12.min(snap.hash.len())]
    );
    for u in &snap.unresolved {
        println!("  unresolved: {u}");
    }
    if dry_run {
        let mut by: std::collections::BTreeMap<String, usize> = Default::default();
        for v in &snap.vertices {
            *by.entry(v["nodetype"].as_str().unwrap_or("").to_string()).or_default() += 1;
        }
        println!("  by nodetype: {by:?}");
        return 0;
    }
    if read_only {
        println!("  read-only: {} id(s) derived, nothing written to the checkout", snap.vertices.iter().filter(|v| v["id_derived"] == true).count());
    }
    if !snap.actors.is_empty() {
        println!("  actors: {}", snap.actors.len());
    }
    match push(c, force, &opts) {
        Ok(r) if r["unchanged"] == true => {
            println!("iter_data ({}): unchanged", c.source);
            0
        }
        Ok(r) => {
            println!("iter_data ({}): synced {}", c.source, r["counts"]);
            0
        }
        Err(e) => {
            eprintln!("iter sync: {e}");
            1
        }
    }
}

/// Engine hook: repair ids and push when the snapshot's hash differs from the
/// last one pushed. Returns the new hash (or the old one when nothing moved).
/// Quiet unless something was written or failed.
pub fn sync_if_changed(api: &Api, project_name: &str, topdir: &Path, last_hash: &str) -> String {
    let c = Conn { api: Some(api.clone()), project: project_name.to_string(), topdir: topdir.to_path_buf(), source: "engine".into() };
    let project = Project::load(topdir);
    if !ids::check(&project).clean() {
        match fix_ids(&c, &project, false) {
            Ok(ch) if !ch.is_empty() => println!("[engine] {project_name}: gave {} node file(s) an id", ch.len()),
            Ok(_) => {}
            Err(e) => eprintln!("[engine] {project_name}: id repair failed: {e}"),
        }
    }
    let snap = graph::snapshot(&project);
    if snap.hash == last_hash {
        return snap.hash;
    }
    let mut body = graph::sync_body(&snap);
    body["force"] = json!(false);
    match api.put(&format!("/api/projects/{project_name}/graph"), &body) {
        Ok(r) => {
            if r["unchanged"] != true {
                println!("[engine] {project_name}: architecture map synced {}", r["counts"]);
            }
            snap.hash
        }
        Err(e) => {
            eprintln!("[engine] {project_name}: map sync failed: {e}");
            last_hash.to_string()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_project_name_never_selects_the_git_root() {
        let top = std::env::temp_dir().join(format!("iter4_root_{}", uuid::Uuid::new_v4()));
        let sub = top.join("iter4");
        std::fs::create_dir_all(sub.join("iter_data/src")).unwrap();
        std::fs::write(sub.join("main.iter.md"), "---\nprojectname: iter4\n---\n").unwrap();
        // SAFETY: tests in this crate do not read ITER_TOPDIR concurrently
        unsafe { std::env::remove_var("ITER_TOPDIR") };
        let deep = sub.join("iter_data/src");
        assert_eq!(checkout_root(Some("iter4"), &deep, &top), sub);
        assert_eq!(checkout_root(None, &sub, &top), sub);
        assert_eq!(checkout_root(Some("./elsewhere/"), &sub, &top), PathBuf::from("./elsewhere/"));
        assert_eq!(checkout_root(Some("iter4"), &top, &top), top, "no main above: the fallback");
    }
}

/// `iter graph-apply [--file op.json]`: apply one graph edit to this checkout
/// by hand — the op comes from --file, else from the calling work item's
/// request. (Edits made in the webui do not come this way: they are datasync
/// rows the engine applies itself, iter_engine/src/datasync.rs.) The map is
/// pushed straight away so the Project graph shows the change.
pub fn graph_apply_verb(c: &Conn, file: Option<&str>) -> i32 {
    let op: Value = match file {
        Some(f) => match std::fs::read_to_string(f).map_err(|e| e.to_string()).and_then(|t| serde_json::from_str(&t).map_err(|e| e.to_string())) {
            Ok(v) => v,
            Err(e) => {
                eprintln!("iter graph-apply: {f}: {e}");
                return 2;
            }
        },
        None => {
            let (Some(api), Ok(workid)) = (&c.api, std::env::var("ITER_WORKID")) else {
                eprintln!("iter graph-apply: no --file and not inside an engine-run item");
                return 2;
            };
            let rows = api.get(&format!("/api/projects/{}/workitems/{workid}/details", c.project)).unwrap_or(Value::Null);
            let req = rows
                .as_array()
                .into_iter()
                .flatten()
                .find(|r| r["key"] == "request")
                .map(|r| r["value"].as_str().map(String::from).unwrap_or_else(|| r["value"].to_string()))
                .unwrap_or_default();
            match serde_json::from_str::<Value>(&req) {
                Ok(v) => v,
                Err(e) => {
                    eprintln!("iter graph-apply: the item's request is not an op JSON: {e}");
                    return 2;
                }
            }
        }
    };
    let project = Project::load(&c.topdir);
    match iter_local::graph_edit::apply(&project, &op) {
        Ok(files) => {
            for f in &files {
                println!("  wrote {f}");
            }
            println!("graph edit {} applied: {} file(s)", op["op"].as_str().unwrap_or("?"), files.len());
            if c.api.is_some() {
                match push(c, false, &graph::SnapOpts::default()) {
                    Ok(r) => println!("map synced {}", r["counts"]),
                    Err(e) => eprintln!("map sync failed (the engine retries within a minute): {e}"),
                }
            }
            0
        }
        Err(e) => {
            eprintln!("iter graph-apply: {e}");
            1
        }
    }
}
