//! The architecture-map snapshot (iter4, decided 2026-09-28): the structureV2
//! scan rendered as a graph for iter_data. One vertex per node file (keyed by
//! its frontmatter `id`, see `ids`), one edge per `children` link, typed by the
//! sub-key that declared it. The main file points at every root (context-level
//! code nodes, interfaces, use-cases) with `root` edges, so one traversal from
//! main reaches everything that is linked. Bodies stay in git: a vertex carries
//! only a hash of its body.
//!
//! The snapshot hash covers vertices + edges, so the engine pushes only when
//! the map actually changed.

use crate::ids::{read_id, topdir_path};
use crate::markers::{self, Front, role_name, role_of};
use crate::project::Project;
use crate::testgroups;
use serde::Serialize;
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

#[derive(Debug, Serialize, Default)]
pub struct Snapshot {
    pub project: String,
    pub hash: String,
    pub vertices: Vec<Value>,
    pub edges: Vec<Value>,
    /// files the scan left out of the DAG (still vertices, flagged `orphan`)
    pub orphans: Vec<Value>,
    /// links or files that could not become graph elements (no id, missing target)
    pub unresolved: Vec<String>,
    /// the people and outside organisations at the edge of the system (from
    /// the actors file): [{id, name, description, uses: [{pattern, why}]}]
    pub actors: Vec<Value>,
    /// where the checkout lives in its repository: {web, branch, prefix} —
    /// lets the map link a node's code files to the repository's web view
    pub repo: Value,
    /// the actors file, `{topdir}/…` (where the graph editor writes actors)
    pub actors_file: String,
}

/// `children.documents` entries as `{topdir}/…` paths: `{thisfiledir}` is the
/// node file's folder; a bare relative path is from the checkout root.
pub fn document_paths(entries: &[String], node_rel: &str) -> Vec<String> {
    let dir = node_rel.rsplit_once('/').map(|(d, _)| d).unwrap_or("{topdir}");
    let mut out: Vec<String> = entries
        .iter()
        .map(|e| e.trim())
        .filter(|e| !e.is_empty())
        .map(|e| {
            if let Some(r) = e.strip_prefix("{thisfiledir}") {
                format!("{dir}/{}", r.trim_start_matches('/'))
            } else if e.starts_with("{topdir}") {
                e.to_string()
            } else {
                format!("{{topdir}}/{}", e.trim_start_matches("./").trim_start_matches('/'))
            }
        })
        .collect();
    out.sort();
    out.dedup();
    out
}

fn sha(s: &str) -> String {
    let d = Sha256::digest(s.as_bytes());
    d.iter().map(|b| format!("{b:02x}")).collect()
}

fn front_json(f: &Front) -> Value {
    let mut m = Map::new();
    for (k, v) in &f.scalars {
        m.insert(k.clone(), json!(v));
    }
    for (k, v) in &f.lists {
        m.insert(k.clone(), json!(v));
    }
    let ch: BTreeMap<&String, &Vec<String>> = f.children.iter().collect();
    m.insert("children".into(), json!(ch));
    // stable key order for hashing
    let sorted: BTreeMap<String, Value> = m.into_iter().collect();
    json!(sorted)
}

struct FileInfo {
    id: String,
    nodetype: &'static str,
    front: Front,
    /// the whole frontmatter as YAML (nested keys the flat parser drops:
    /// a use case's `flowmap`, multi-line `simple_description`)
    yaml: Value,
    content_hash: String,
}

fn read_file(p: &Path) -> Option<FileInfo> {
    let name = p.file_name()?.to_str()?;
    let role = role_of(name)?;
    let content = std::fs::read_to_string(p).ok()?;
    let front = markers::parse_front(&content);
    Some(FileInfo {
        id: read_id(&content),
        nodetype: role_name(Some(role)),
        yaml: yaml_front(&content),
        content_hash: sha(&front.body),
        front,
    })
}

/// The frontmatter between the first two `---` lines, parsed as YAML into
/// JSON (Null when absent or not YAML — the flat parser still has the basics).
pub fn yaml_front(content: &str) -> Value {
    let mut lines = content.lines();
    if lines.next().map(|l| l.trim_end() != "---").unwrap_or(true) {
        return Value::Null;
    }
    let mut buf = String::new();
    for l in lines {
        if l.trim_end() == "---" {
            return serde_yaml::from_str::<Value>(&buf).unwrap_or(Value::Null);
        }
        buf.push_str(l);
        buf.push('\n');
    }
    Value::Null
}

/// The text under `# Long Description` up to the next level-1 heading,
/// trimmed and cut near `limit` at a paragraph break (the usecase_map rule).
pub fn long_description(body: &str, limit: usize) -> String {
    let lines: Vec<&str> = body.lines().collect();
    let Some(start) = lines.iter().position(|l| {
        let t = l.trim();
        t.starts_with("# ") && t[2..].trim().eq_ignore_ascii_case("long description")
    }) else {
        return String::new();
    };
    let rest = &lines[start + 1..];
    let end = rest.iter().position(|l| l.starts_with("# ") && l.len() > 2).unwrap_or(rest.len());
    let text = rest[..end].join("\n").trim().to_string();
    if text.chars().count() <= limit {
        return text;
    }
    let cut: String = text.chars().take(limit).collect();
    match cut.rfind("\n\n") {
        Some(i) if i > limit / 2 => format!("{} …", &cut[..i]),
        _ => format!("{} …", cut.trim_end()),
    }
}

/// A stable id for a file with none, derived from project + path (UUIDv5),
/// so a live checkout can be mapped without writing into it (spec R11).
pub fn derived_id(project: &str, rel_path: &str) -> String {
    const NS: uuid::Uuid = uuid::Uuid::from_bytes([0x69, 0x74, 0x65, 0x72, 0x34, 0x2d, 0x4e, 0x53, 0x8d, 0x1f, 0x3a, 0x77, 0x2e, 0x55, 0x90, 0x01]);
    uuid::Uuid::new_v5(&NS, format!("{project}|{rel_path}").as_bytes()).to_string()
}

/// Options for `snapshot_with`.
#[derive(Default, Clone)]
pub struct SnapOpts {
    /// give files without an `id:` a derived one instead of leaving them out
    pub derive_ids: bool,
    /// an actors file (YAML `actors: [{id, name, description, uses: [{pattern}]}]`);
    /// else main.iter.md's `actorsfile:` key
    pub actors_file: Option<PathBuf>,
}

/// Build the snapshot for a checkout.
pub fn snapshot(project: &Project) -> Snapshot {
    snapshot_with(project, &SnapOpts::default())
}

pub fn snapshot_with(project: &Project, opts: &SnapOpts) -> Snapshot {
    let scan = markers::scan(project);
    let mut snap = Snapshot { project: project.projectname(), ..Default::default() };
    // build output git ignores (cdk.out/, dist/ …) is never a node's code
    let ignored = crate::GitIgnored::load(&project.topdir.canonicalize().unwrap_or_else(|_| project.topdir.clone()));
    let tp = |p: &str| topdir_path(project, Path::new(p));

    // every file that becomes a vertex, with the scan's extra facts keyed by path
    let mut facts: BTreeMap<PathBuf, Map<String, Value>> = BTreeMap::new();
    fn add(facts: &mut BTreeMap<PathBuf, Map<String, Value>>, p: &str, extra: Value) {
        let e = facts.entry(PathBuf::from(p)).or_default();
        if let Some(o) = extra.as_object() {
            for (k, v) in o {
                e.insert(k.clone(), v.clone());
            }
        }
    }
    add(&mut facts, &project.mainfile.canonicalize().unwrap_or(project.mainfile.clone()).to_string_lossy(), json!({}));
    for n in &scan.nodes {
        add(&mut facts, &n.path, json!({
            "key": n.key, "level": n.level, "owner": n.owner, "depth": n.depth,
            "teststate_effective": n.teststate_effective,
            "codedirs": n.codedirs.iter().map(|d| tp(d)).collect::<Vec<_>>(),
            "missing_testgroups": n.missing_testgroups,
        }));
        for p in n.bizreqs.iter().chain(&n.techreqs).chain(&n.testgroups) {
            add(&mut facts, p, json!({}));
        }
    }
    for i in &scan.interfaces {
        add(&mut facts, &i.file, json!({"interface_id": i.id, "kind": i.kind, "endpoint": i.endpoint, "owner": i.owner,
                             "missing_testgroups": i.missing_testgroups}));
        for p in i.bizreqs.iter().chain(&i.techreqs).chain(&i.testgroups) {
            add(&mut facts, p, json!({}));
        }
    }
    for u in &scan.usecases {
        add(&mut facts, &u.file, json!({"missing_testgroups": u.missing_testgroups}));
        for p in &u.testgroups {
            add(&mut facts, p, json!({}));
        }
    }
    for o in &scan.orphans {
        add(&mut facts, &o.path, json!({"orphan": true, "orphan_reason": o.reason}));
    }
    // main's globalcontextfiles that are node files (the project-wide reqs)
    let context: Vec<PathBuf> = project
        .context_files()
        .iter()
        .map(|p| p.canonicalize().unwrap_or_else(|_| p.clone()))
        .filter(|p| p.file_name().and_then(|n| n.to_str()).map(|n| role_of(n).is_some()).unwrap_or(false))
        .filter(|p| *p != project.mainfile.canonicalize().unwrap_or(project.mainfile.clone()))
        .collect();
    for p in &context {
        add(&mut facts, &p.to_string_lossy(), json!({"context": true}));
    }
    // every node file is a vertex: anything the scan neither linked nor
    // orphaned (it only orphans what it tried to place) is flagged here
    for f in crate::ids::node_files(project) {
        if !facts.contains_key(&f) {
            add(&mut facts, &f.to_string_lossy(), json!({"orphan": true, "orphan_reason": "no node links to it"}));
        }
    }

    // vertices
    let mut id_of: HashMap<PathBuf, String> = HashMap::new();
    for (path, extra) in &facts {
        let Some(fi) = read_file(path) else {
            snap.unresolved.push(format!("unreadable node file {}", tp(&path.to_string_lossy())));
            continue;
        };
        let rel = tp(&path.to_string_lossy());
        let mut id = fi.id.clone();
        let mut derived = false;
        if id.is_empty() {
            if !opts.derive_ids {
                snap.unresolved.push(format!("no id: {rel} (run `iter ids --fix`)"));
                continue;
            }
            id = derived_id(&snap.project, &rel);
            derived = true;
        }
        let ys = |k: &str| fi.yaml.get(k).and_then(|x| x.as_str()).map(|x| x.trim().to_string()).unwrap_or_default();
        let mut v = json!({
            "id": id,
            "nodetype": fi.nodetype,
            "name": fi.front.scalar("name"),
            "description": fi.front.scalar("description"),
            "teststate": fi.front.scalar("teststate"),
            "path": rel,
            "dir": tp(&path.parent().map(|d| d.to_string_lossy().into_owned()).unwrap_or_default()),
            "bodyhash": fi.content_hash,
            "front": front_json(&fi.front),
            "orphan": false,
        });
        if derived {
            v["id_derived"] = json!(true);
        }
        let simple = ys("simple_description");
        if !simple.is_empty() {
            v["simple_description"] = json!(simple);
        }
        let long = long_description(&fi.front.body, 1800);
        if !long.is_empty() {
            v["long_description"] = json!(long);
        }
        // the global objects' text rides along (capped), so the webui can edit it
        if matches!(fi.nodetype, "bizreq" | "techreq" | "usecase" | "interface") {
            let b: String = fi.front.body.chars().take(32 * 1024).collect();
            v["body"] = json!(b.trim());
        }
        if fi.nodetype == "usecase" {
            let stem = path.file_name().and_then(|n| n.to_str()).unwrap_or("").trim_end_matches(".usecase.iter.md").to_string();
            v["ucid"] = json!(format!("usecase:{stem}"));
            if let Some(fm) = fi.yaml.get("flowmap").filter(|f| f.is_object()) {
                v["flowmap"] = fm.clone();
            }
        }
        // GraphRAG (2026-09-29): documents a node is linked to — uploaded
        // files (specs, decks, PDFs) that describe it — under
        // `children.documents`; not vertices, so no edges: the path list rides
        // on the vertex and GraphRAG attaches the node to those documents' hits
        let docs = document_paths(&fi.front.child("documents").unwrap_or_default(), &rel);
        if !docs.is_empty() {
            v["documents"] = json!(docs);
        }
        if fi.nodetype == "code" {
            // the source files this node owns (its codedirs), so a reader can go
            // from the map to the code; capped — a context owning a whole tree
            // lists its first files only
            if let Some(dirs) = extra.get("codedirs").and_then(|d| d.as_array()) {
                let top = project.topdir.canonicalize().unwrap_or_else(|_| project.topdir.clone());
                let mut files: Vec<String> = Vec::new();
                for d in dirs.iter().filter_map(|x| x.as_str()) {
                    let abs = top.join(d.trim_start_matches("{topdir}").trim_start_matches('/'));
                    list_code_files(&abs, &top, &ignored, &mut files, 60);
                }
                files.sort();
                files.dedup();
                v["code_files"] = json!(files);
            }
        }
        if fi.nodetype == "interface" {
            for k in ["status", "transport", "label"] {
                let val = ys(k);
                if !val.is_empty() {
                    v[k] = json!(val);
                }
            }
        }
        if fi.nodetype == "main" {
            v["name"] = json!(fi.front.scalar("projectname"));
            v["description"] = json!(fi.front.scalar("projectdescription"));
        }
        if matches!(fi.nodetype, "testgroup" | "tests") {
            let content = std::fs::read_to_string(path).unwrap_or_default();
            let groups: Vec<Value> = testgroups::parse(&content)
                .iter()
                .map(|g| json!({"label": g.label, "tests": g.testlist.len(), "result": g.result, "lastrun": g.lastrun,
                    "input_space": g.input_space, "coverage": g.coverage, "coverage_gaps": g.coverage_gaps()}))
                .collect();
            v["groups"] = json!(groups);
            v["testpaths"] = json!(fi.front.child("testpaths").unwrap_or_default());
        }
        for (k, val) in extra {
            v[k] = val.clone();
        }
        id_of.insert(path.clone(), id.clone());
        snap.vertices.push(v);
    }

    // edges
    let main = project.mainfile.canonicalize().unwrap_or(project.mainfile.clone());
    let iface_file: HashMap<&str, &str> = scan.interfaces.iter().map(|i| (i.id.as_str(), i.file.as_str())).collect();
    let mut edges: Vec<(String, String, String)> = Vec::new();
    let mut link = |from: &Path, kind: &str, to: &Path, snap: &mut Snapshot| {
        match (id_of.get(from), id_of.get(to)) {
            (Some(f), Some(t)) if f != t => edges.push((f.clone(), kind.to_string(), t.clone())),
            (Some(_), Some(_)) => {}
            _ => snap.unresolved.push(format!(
                "{kind} link {} -> {} has an end without an id",
                tp(&from.to_string_lossy()),
                tp(&to.to_string_lossy())
            )),
        }
    };
    for n in &scan.nodes {
        let me = PathBuf::from(&n.path);
        if n.parent.is_empty() {
            link(&main, "root", &me, &mut snap);
        }
        for c in &n.codenodes {
            link(&me, "codenodes", Path::new(c), &mut snap);
        }
        for (kind, list) in [("inputs", &n.inputs), ("outputs", &n.outputs)] {
            for iid in list {
                match iface_file.get(iid.as_str()) {
                    Some(f) => link(&me, kind, Path::new(f), &mut snap),
                    None => snap.unresolved.push(format!("{kind} of {}: no interface '{iid}'", tp(&n.path))),
                }
            }
        }
        for (kind, list) in [("bizreqs", &n.bizreqs), ("techreqs", &n.techreqs), ("tests", &n.testgroups)] {
            for p in list {
                link(&me, kind, Path::new(p), &mut snap);
            }
        }
    }
    for i in &scan.interfaces {
        let me = PathBuf::from(&i.file);
        link(&main, "root", &me, &mut snap);
        for (kind, list) in [("bizreqs", &i.bizreqs), ("techreqs", &i.techreqs), ("tests", &i.testgroups)] {
            for p in list {
                link(&me, kind, Path::new(p), &mut snap);
            }
        }
    }
    for p in &context {
        link(&main, "context", p, &mut snap);
    }
    for u in &scan.usecases {
        let me = PathBuf::from(&u.file);
        link(&main, "root", &me, &mut snap);
        for c in &u.codenodes {
            link(&me, "codenodes", Path::new(c), &mut snap);
        }
        for p in &u.testgroups {
            link(&me, "tests", Path::new(p), &mut snap);
        }
    }
    edges.sort();
    edges.dedup();
    snap.edges = edges.into_iter().map(|(f, k, t)| json!({"from": f, "kind": k, "to": t})).collect();
    mark_uncovered(&mut snap);
    for v in &mut snap.vertices {
        let mut h = v.clone();
        h.as_object_mut().map(|o| o.remove("vhash"));
        v["vhash"] = json!(sha(&h.to_string()));
    }
    snap.orphans = scan
        .orphans
        .iter()
        .map(|o| json!({"path": tp(&o.path), "role": o.role, "reason": o.reason}))
        .collect();
    snap.vertices.sort_by(|a, b| a["path"].as_str().cmp(&b["path"].as_str()));
    snap.unresolved.sort();
    snap.unresolved.dedup();
    snap.actors = load_actors(project, opts);
    snap.actors_file = {
        let p = opts.actors_file.clone().unwrap_or_else(|| actors_path(project));
        let top = project.topdir.to_string_lossy().trim_end_matches('/').to_string();
        let abs = if p.is_absolute() { p } else { project.topdir.join(p) };
        abs.to_string_lossy().replacen(&top, "{topdir}", 1)
    };
    snap.repo = repo_info(&project.topdir);
    let body = json!({"v": snap.vertices, "e": snap.edges, "a": snap.actors});
    snap.hash = sha(&body.to_string());
    snap
}

/// Code no component owns (2026-09-29): when a node's children own files in a
/// folder, every other file of the node in that same folder is uncovered —
/// on the map, nobody is said to own it. Recorded as `uncovered_files`.
fn mark_uncovered(snap: &mut Snapshot) {
    let files_of: HashMap<String, Vec<String>> = snap
        .vertices
        .iter()
        .filter(|v| v["nodetype"] == "code")
        .map(|v| (v["id"].as_str().unwrap_or("").to_string(), v["code_files"].as_array().into_iter().flatten().filter_map(|f| f.as_str().map(String::from)).collect()))
        .collect();
    let mut kids: HashMap<String, Vec<String>> = HashMap::new();
    for e in &snap.edges {
        if e["kind"] == "codenodes" {
            let (f, t) = (e["from"].as_str().unwrap_or(""), e["to"].as_str().unwrap_or(""));
            if files_of.contains_key(f) && files_of.contains_key(t) {
                kids.entry(f.to_string()).or_default().push(t.to_string());
            }
        }
    }
    let dir = |f: &str| f.rsplit_once('/').map(|(d, _)| d.to_string()).unwrap_or_default();
    for v in &mut snap.vertices {
        let id = v["id"].as_str().unwrap_or("").to_string();
        let Some(children) = kids.get(&id) else { continue };
        let owned: std::collections::HashSet<&String> = children.iter().flat_map(|c| files_of.get(c).into_iter().flatten()).collect();
        let owned_dirs: std::collections::HashSet<String> = owned.iter().map(|f| dir(f)).collect();
        let mine = files_of.get(&id).cloned().unwrap_or_default();
        let uncovered: Vec<String> = mine.into_iter().filter(|f| !owned.contains(f) && owned_dirs.contains(&dir(f))).collect();
        if !uncovered.is_empty() {
            v["uncovered_files"] = json!(uncovered);
        }
    }
}

/// Files under `path` (or the file itself), topdir-relative, skipping the
/// usual noise, what git ignores, and node files; at most `cap` in all.
fn list_code_files(path: &Path, top: &Path, ignored: &crate::GitIgnored, out: &mut Vec<String>, cap: usize) {
    if out.len() >= cap {
        return;
    }
    let rel = |p: &Path| format!("{{topdir}}/{}", p.strip_prefix(top).unwrap_or(p).to_string_lossy());
    if path.is_file() {
        out.push(rel(path));
        return;
    }
    let Ok(rd) = std::fs::read_dir(path) else { return };
    let mut entries: Vec<PathBuf> = rd.filter_map(|e| e.ok()).map(|e| e.path()).collect();
    entries.sort();
    for e in entries {
        if out.len() >= cap {
            return;
        }
        let name = e.file_name().and_then(|n| n.to_str()).unwrap_or("");
        if name.starts_with('.') || crate::is_skip_dir(name) || ignored.contains(&e) {
            continue;
        }
        if e.is_dir() {
            list_code_files(&e, top, ignored, out, cap);
        } else if !name.ends_with(".iter.md") {
            out.push(rel(&e));
        }
    }
}

/// {web, branch, prefix}: the repository's browsable URL (from `origin`),
/// the checked-out branch, and the checkout's path inside the repository.
fn repo_info(topdir: &Path) -> Value {
    let git = |args: &[&str]| -> String {
        std::process::Command::new("git").args(args).current_dir(topdir).output().ok()
            .filter(|o| o.status.success()).map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string()).unwrap_or_default()
    };
    let url = git(&["remote", "get-url", "origin"]);
    let web = if let Some(rest) = url.strip_prefix("git@") {
        format!("https://{}", rest.replacen(':', "/", 1)).trim_end_matches(".git").to_string()
    } else {
        url.trim_end_matches(".git").to_string()
    };
    json!({"web": web, "branch": git(&["rev-parse", "--abbrev-ref", "HEAD"]), "prefix": git(&["rev-parse", "--show-prefix"])})
}

/// main.iter.md's `actorsfile:` (with `{topdir}`), else `<topdir>/actors.yaml`.
pub fn actors_path(project: &Project) -> PathBuf {
    let top = project.topdir.to_string_lossy().trim_end_matches('/').to_string();
    std::fs::read_to_string(&project.mainfile)
        .ok()
        .and_then(|raw| yaml_front(&raw).get("actorsfile").and_then(|p| p.as_str()).map(|p| PathBuf::from(p.replace("{topdir}", &top))))
        .map(|p| if p.is_absolute() { p } else { project.topdir.join(p) })
        .unwrap_or_else(|| project.topdir.join("actors.yaml"))
}

/// The actors file: `opts.actors_file`, else `actors_path`, read as YAML
/// `actors: [...]`.  Missing = no actors.
fn load_actors(project: &Project, opts: &SnapOpts) -> Vec<Value> {
    let path = opts.actors_file.clone().unwrap_or_else(|| actors_path(project));
    let path = if path.is_absolute() { path } else { project.topdir.join(path) };
    let Ok(text) = std::fs::read_to_string(&path) else { return vec![] };
    let y: Value = serde_yaml::from_str(&text).unwrap_or(Value::Null);
    y.get("actors").and_then(|a| a.as_array()).cloned().unwrap_or_default()
}

/// The body `PUT /api/projects/{p}/graph` takes.
pub fn sync_body(s: &Snapshot) -> Value {
    json!({"hash": s.hash, "vertices": s.vertices, "edges": s.edges, "orphans": s.orphans, "unresolved": s.unresolved, "actors": s.actors, "actors_file": s.actors_file, "repo": s.repo})
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids;

    fn w(top: &Path, rel: &str, body: &str) {
        let p = top.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, body).unwrap();
    }

    #[test]
    fn long_description_and_yaml_front() {
        let body = "intro\n# Long Description\n\npara one\n\npara two\n# Next\nx";
        assert_eq!(long_description(body, 1800), "para one\n\npara two");
        assert_eq!(long_description("no heading", 1800), "");
        let long = format!("# Long Description\n{}\n\n{}", "a".repeat(1500), "b".repeat(1000));
        let cut = long_description(&long, 1800);
        assert!(cut.ends_with(" …") && !cut.contains('b'), "cut at the paragraph break");
        let y = yaml_front("---\nname: x\nflowmap:\n  summary: s\n  process_flow:\n    - {step: 1, from: a, to: b}\n---\nbody");
        assert_eq!(y["flowmap"]["process_flow"][0]["step"], 1);
        assert_eq!(yaml_front("no front"), Value::Null);
        // derived ids are stable and distinct
        assert_eq!(derived_id("p", "{topdir}/a.code.iter.md"), derived_id("p", "{topdir}/a.code.iter.md"));
        assert_ne!(derived_id("p", "{topdir}/a.code.iter.md"), derived_id("q", "{topdir}/a.code.iter.md"));
    }

    #[test]
    fn read_only_snapshot_derives_ids_and_reads_actors() {
        let top = std::env::temp_dir().join(format!("iter4_ro_{}", uuid::Uuid::new_v4()));
        w(&top, "main.iter.md", "---\nprojectname: ro\nglobalscandirs: [\"{topdir}/\"]\nactorsfile: \"{topdir}/actors.yaml\"\n---\n");
        w(&top, "a/a.code.iter.md", "---\nname: A\ndescription: d\nsimple_description: plain words\nlevel: context\nchildren:\n  codedirs: []\n---\n# Long Description\nwhat A is\n");
        w(&top, "usecases/u/u.usecase.iter.md", "---\nname: U\ndescription: d\nchildren:\n  codenodes: [\"{topdir}/a/a.code.iter.md\"]\nflowmap:\n  summary: s\n  sequence: [\"actor:worker\", \"{topdir}/a/a.code.iter.md\"]\n---\n");
        w(&top, "actors.yaml", "actors:\n  - id: worker\n    name: Worker\n    uses: [{pattern: \"a-*\"}]\n");
        let project = Project::load(&top);
        let before: Vec<u8> = std::fs::read(top.join("a/a.code.iter.md")).unwrap();
        let s = snapshot_with(&project, &SnapOpts { derive_ids: true, actors_file: None });
        assert_eq!(std::fs::read(top.join("a/a.code.iter.md")).unwrap(), before, "read-only: nothing written");
        assert_eq!(s.vertices.len(), 3, "{:?}", s.unresolved);
        let a = s.vertices.iter().find(|v| v["name"] == "A").unwrap();
        assert_eq!((a["id_derived"].as_bool(), a["simple_description"].as_str(), a["long_description"].as_str()), (Some(true), Some("plain words"), Some("what A is")));
        let u = s.vertices.iter().find(|v| v["nodetype"] == "usecase").unwrap();
        assert_eq!(u["ucid"], "usecase:u");
        assert_eq!(u["flowmap"]["sequence"][0], "actor:worker");
        assert_eq!(s.actors[0]["name"], "Worker");
        // the same tree snapshots to the same ids and hash
        assert_eq!(snapshot_with(&project, &SnapOpts { derive_ids: true, actors_file: None }).hash, s.hash);
    }

    #[test]
    fn code_no_component_owns_is_marked() {
        let mut snap = Snapshot::default();
        snap.vertices = vec![
            json!({"id": "c", "nodetype": "code", "code_files": ["{topdir}/x/Cargo.toml", "{topdir}/x/src/a.rs", "{topdir}/x/src/b.rs", "{topdir}/x/src/orphan.rs"]}),
            json!({"id": "a", "nodetype": "code", "code_files": ["{topdir}/x/src/a.rs"]}),
            json!({"id": "b", "nodetype": "code", "code_files": ["{topdir}/x/src/b.rs"]}),
        ];
        snap.edges = vec![json!({"from": "c", "kind": "codenodes", "to": "a"}), json!({"from": "c", "kind": "codenodes", "to": "b"})];
        mark_uncovered(&mut snap);
        assert_eq!(snap.vertices[0]["uncovered_files"], json!(["{topdir}/x/src/orphan.rs"]), "Cargo.toml is container-level, not uncovered");
        assert!(snap.vertices[1].get("uncovered_files").is_none());
    }

    #[test]
    fn snapshot_links_main_roots_children_and_interfaces() {
        let top = std::env::temp_dir().join(format!("iter4_snap_{}", uuid::Uuid::new_v4()));
        w(&top, "main.iter.md", "---\nprojectname: demo\nprojectdescription: d\nglobalscandirs: [\"{topdir}/\"]\n---\nbody\n");
        w(&top, "app/app.code.iter.md", "---\nname: App\ndescription: the app\nlevel: context\nchildren:\n  codenodes: [\"{thisfiledir}/core/core.code.iter.md\"]\n  testgroups: [\"{thisfiledir}/test/*.testgroup.iter.md\"]\n---\n");
        w(&top, "app/core/core.code.iter.md", "---\nname: Core\ndescription: c\nlevel: container\nchildren:\n  outputs: [\"{topdir}/interfaces/greet.interface.iter.md\"]\n---\n");
        w(&top, "app/test/unit.testgroup.iter.md", "---\nname: Unit\ndescription: u\nchildren:\n  testpaths: [\"{thisfiledir}/*.sh\"]\n---\n");
        w(&top, "interfaces/greet.interface.iter.md", "---\nname: greet\ndescription: g\nchildren:\n  testgroups: []\n---\n");
        let project = Project::load(&top);
        // without ids nothing can be a vertex
        assert!(snapshot(&project).vertices.is_empty());
        ids::fix(&project, &ids::NoLookup, false).unwrap();
        let s = snapshot(&project);
        assert_eq!(s.vertices.len(), 5, "{:?}", s.unresolved);
        let by_path: HashMap<String, String> = s
            .vertices
            .iter()
            .map(|v| (v["path"].as_str().unwrap().to_string(), v["id"].as_str().unwrap().to_string()))
            .collect();
        let has = |f: &str, k: &str, t: &str| {
            s.edges.iter().any(|e| e["from"] == by_path[f] && e["kind"] == k && e["to"] == by_path[t])
        };
        assert!(has("{topdir}/main.iter.md", "root", "{topdir}/app/app.code.iter.md"));
        assert!(has("{topdir}/main.iter.md", "root", "{topdir}/interfaces/greet.interface.iter.md"));
        assert!(has("{topdir}/app/app.code.iter.md", "codenodes", "{topdir}/app/core/core.code.iter.md"));
        assert!(has("{topdir}/app/app.code.iter.md", "tests", "{topdir}/app/test/unit.testgroup.iter.md"));
        assert!(has("{topdir}/app/core/core.code.iter.md", "outputs", "{topdir}/interfaces/greet.interface.iter.md"));
        // stable: same tree, same hash
        assert_eq!(snapshot(&project).hash, s.hash);
        w(&top, "app/core/core.code.iter.md", &std::fs::read_to_string(top.join("app/core/core.code.iter.md")).unwrap().replace("description: c", "description: changed"));
        assert_ne!(snapshot(&project).hash, s.hash);
    }
}
