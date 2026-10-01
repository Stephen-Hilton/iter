//! Graph edits (iter4 Phase 2, R14; datasync 2026-09-29): the file operations
//! behind the Project graph's "build it here" controls. iter_data has no repo
//! access, so the webui's edit is stored as a pending datasync row; the first
//! engine serving the project claims it (iter_engine/src/datasync.rs), calls
//! `apply` here in its checkout, commits exactly the files written, pushes and
//! syncs the map. `iter graph-apply --file op.json` runs the same `apply` by hand.
//!
//! Operations (JSON `op`):
//! - `new_node`   {kind: context|container|component, name, slug?, description?, parent?}
//!   → `<dir>/<slug>.code.iter.md` + `.bizreq` + `.techreq` + `test/<slug>.tests.iter.md`,
//!   linked from the parent's `children.codenodes` (main.iter.md for a context
//!   with no parent). `parent` is the parent code node's file (exact) or folder.
//! - `connect`    {from, to, interface: {name, kind?, description?} | interface_path}
//!   → the interface file (created when new) listed in `from`'s `inputs` and
//!   `to`'s `outputs`: `from` uses what `to` provides.
//! - `new_global` {kind: bizreq|techreq|usecase, name, slug?, description?, body?, codenodes?}
//! - `edit_body`  {path, body} → the markdown under the frontmatter replaced.
//! - `link_child` {parent, child} → an existing code node owned by another
//!   (any level may own any level: a context may own a context).
//! - `unlink_child` {parent, child, reason} → that ownership link removed.
//! - `disconnect` {from, to, interface_path | interface.name, reason} → the
//!   interface dropped from `from`'s inputs and `to`'s outputs.
//! - `define_tests` {node, tests: [{name, desc}], queue_agent?} → the node's
//!   `test/<slug>.tests.iter.md` gets a "Planned tests" list, simplest first
//!   (created and linked when missing); `queue_agent` asks the engine to file a
//!   `test` agent item that writes and runs them (red is expected: TDD).
//! - `new_actor`  {name, id?, description?, actors_file?} → a person or program
//!   at the edge of the map, appended to the actors file (created when missing).
//! - `edit_actor` {actor, name?, description?} → its name / description changed.
//! - `actor_uses` {actor, to, interface: {name, kind?, description?} | interface_path}
//!   → the interface (created when new) listed in `to`'s `outputs`, and its
//!   name added to the actor's `uses` patterns: the actor uses what `to` provides.
//! - `actor_unuse` {actor, interface, reason} → that pattern removed.
//! - `usecase_needs` {usecase, node} → the code node listed in the use case's
//!   `children.codenodes` (the map tags it and its owners: usecase_map).
//! - `usecase_unneed` {usecase, node, reason} → that listing removed.
//! - `name_parts` {usecase} → writes nothing; the engine queues the `usecase`
//!   agent to name the parts the use case needs.
//! `new_node` also takes `simple_description`, `long_description`,
//! `bizreq`, `techreq` (body texts) and `tests` (planned tests, as above).

use crate::project::Project;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

fn s<'a>(v: &'a Value, k: &str) -> &'a str {
    v.get(k).and_then(|x| x.as_str()).unwrap_or("")
}

/// `My New Thing` → `my_new_thing` (letters, digits, `_`, `-`).
pub fn slugify(name: &str) -> String {
    let mut out = String::new();
    for c in name.trim().chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
        } else if (c == '-' || c == '_' || c.is_whitespace()) && !out.ends_with('_') {
            out.push(if c == '-' { '-' } else { '_' });
        }
    }
    out.trim_matches('_').to_string()
}

fn yq(s: &str) -> String {
    serde_json::to_string(s).unwrap_or_else(|_| "\"\"".into())
}

/// A `{topdir}/…` or topdir-relative path → absolute, refusing anything that
/// leaves the topdir.
fn resolve(topdir: &Path, p: &str) -> Result<PathBuf, String> {
    let rel = p.trim().trim_start_matches("{topdir}").trim_start_matches('/');
    if rel.split('/').any(|seg| seg == "..") {
        return Err(format!("path leaves the checkout: {p}"));
    }
    Ok(topdir.join(rel))
}

fn topdir_rel(topdir: &Path, abs: &Path) -> String {
    format!("{{topdir}}/{}", abs.strip_prefix(topdir).unwrap_or(abs).to_string_lossy())
}

/// A code node named by its file (`…/x.code.iter.md`, exact — several code
/// nodes may share a folder) or by its folder (the one code file in it).
fn code_node(top: &Path, p: &str) -> Result<PathBuf, String> {
    let abs = resolve(top, p)?;
    if p.ends_with(".code.iter.md") {
        return if abs.is_file() { Ok(abs) } else { Err(format!("no code node file {p}")) };
    }
    code_file_in(&abs).ok_or_else(|| format!("{p} is not a code node (no *.code.iter.md there)"))
}

/// The code node file in a folder (`<name>.code.iter.md` or `code.iter.md`).
fn code_file_in(dir: &Path) -> Option<PathBuf> {
    std::fs::read_dir(dir).ok()?.filter_map(|e| e.ok()).map(|e| e.path()).find(|p| {
        p.file_name().and_then(|n| n.to_str()).map(|n| n == "code.iter.md" || n.ends_with(".code.iter.md")).unwrap_or(false)
    })
}

/// Add `value` to `children.<key>` in a node file's frontmatter, keeping the
/// file's own style: an inline `[..]` list gets the value appended, a block
/// list gets a `- ` line, a missing key is added under `children:` (and a
/// missing `children:` is added at the end of the frontmatter). No duplicate.
pub fn add_child(content: &str, key: &str, value: &str) -> Result<String, String> {
    let mut lines: Vec<String> = content.split('\n').map(String::from).collect();
    if lines.first().map(|l| l.trim_end() != "---").unwrap_or(true) {
        return Err("the file has no frontmatter".into());
    }
    let end = lines.iter().skip(1).position(|l| l.trim_end() == "---").map(|i| i + 1).ok_or("unterminated frontmatter")?;
    let ch = (1..end).find(|&i| lines[i].trim_end() == "children:");
    let Some(ch) = ch else {
        lines.insert(end, format!("children:\n  {key}: [{}]", yq(value)));
        return Ok(lines.join("\n"));
    };
    // the children block: indented lines after `children:`
    let mut i = ch + 1;
    while i < end && (lines[i].starts_with(' ') || lines[i].trim().is_empty()) {
        let t = lines[i].trim_start();
        if let Some(rest) = t.strip_prefix(&format!("{key}:")) {
            let indent = lines[i].len() - t.len();
            let rest = rest.trim();
            if rest.starts_with('[') {
                let mut list: Vec<String> = serde_json::from_str(rest).unwrap_or_else(|_| {
                    rest.trim_matches(|c| c == '[' || c == ']').split(',').map(|x| x.trim().trim_matches('"').to_string()).filter(|x| !x.is_empty()).collect()
                });
                if !list.iter().any(|x| x == value) {
                    list.push(value.to_string());
                }
                let items: Vec<String> = list.iter().map(|x| yq(x)).collect();
                lines[i] = format!("{}{key}: [{}]", " ".repeat(indent), items.join(", "));
                return Ok(lines.join("\n"));
            }
            // block list: find its last `- ` line
            let mut j = i + 1;
            let mut last = i;
            while j < end && lines[j].trim_start().starts_with("- ") {
                if lines[j].trim_start()[2..].trim().trim_matches('"') == value {
                    return Ok(lines.join("\n"));
                }
                last = j;
                j += 1;
            }
            lines.insert(last + 1, format!("{}  - {}", " ".repeat(indent), yq(value)));
            return Ok(lines.join("\n"));
        }
        i += 1;
    }
    lines.insert(ch + 1, format!("  {key}: [{}]", yq(value)));
    Ok(lines.join("\n"))
}

/// Remove `children.<key>` entries that name `target` (an absolute file):
/// entries written as `{topdir}/…`, `{thisfiledir}/…` or relative paths are
/// resolved against `topdir` / the node's folder. An entry that is a glob
/// matching the target cannot be removed one file at a time — refused, naming it.
pub fn remove_child(content: &str, key: &str, target: &Path, topdir: &Path, filedir: &Path) -> Result<(String, usize), String> {
    let mut lines: Vec<String> = content.split('\n').map(String::from).collect();
    let end = lines.iter().skip(1).position(|l| l.trim_end() == "---").map(|i| i + 1).ok_or("unterminated frontmatter")?;
    let resolve = |v: &str| -> PathBuf {
        let v = v.trim();
        if let Some(r) = v.strip_prefix("{topdir}/") { topdir.join(r) }
        else if let Some(r) = v.strip_prefix("{thisfiledir}/") { filedir.join(r) }
        else if v.starts_with('/') { PathBuf::from(v) }
        else { filedir.join(v) }
    };
    let canon = |p: PathBuf| p.canonicalize().unwrap_or(p);
    let tgt = canon(target.to_path_buf());
    let mut removed = 0;
    for i in 1..end {
        let t = lines[i].trim_start().to_string();
        let Some(rest) = t.strip_prefix(&format!("{key}:")) else { continue };
        let indent = lines[i].len() - t.len();
        let rest = rest.trim();
        if !rest.starts_with('[') {
            return Err(format!("children.{key} is a block list; edit it by hand"));
        }
        let list: Vec<String> = serde_json::from_str(rest).map_err(|e| format!("children.{key}: {e}"))?;
        let mut keep = Vec::new();
        for v in list {
            if v.contains('*') || v.contains('?') {
                let pat = resolve(&v);
                if glob::Pattern::new(&pat.to_string_lossy()).map(|g| g.matches_path(&tgt)).unwrap_or(false) {
                    return Err(format!("children.{key} links it through the pattern {v:?}; narrow that pattern instead"));
                }
                keep.push(v);
            } else if canon(resolve(&v)) == tgt {
                removed += 1;
            } else {
                keep.push(v);
            }
        }
        let items: Vec<String> = keep.iter().map(|x| yq(x)).collect();
        lines[i] = format!("{}{key}: [{}]", " ".repeat(indent), items.join(", "));
    }
    Ok((lines.join("\n"), removed))
}

/// Replace the markdown body under the frontmatter.
pub fn set_body(content: &str, body: &str) -> Result<String, String> {
    let lines: Vec<&str> = content.split('\n').collect();
    if lines.first().map(|l| l.trim_end() != "---").unwrap_or(true) {
        return Ok(body.to_string());
    }
    let end = lines.iter().skip(1).position(|l| l.trim_end() == "---").map(|i| i + 1).ok_or("unterminated frontmatter")?;
    let head = lines[..=end].join("\n");
    Ok(format!("{head}\n{}\n", body.trim_end()))
}

fn new_id() -> String {
    uuid::Uuid::new_v4().to_string()
}

fn write_new(path: &Path, content: &str, written: &mut Vec<PathBuf>) -> Result<(), String> {
    if path.exists() {
        return Err(format!("{} already exists", path.display()));
    }
    if let Some(d) = path.parent() {
        std::fs::create_dir_all(d).map_err(|e| format!("{}: {e}", d.display()))?;
    }
    std::fs::write(path, content).map_err(|e| format!("{}: {e}", path.display()))?;
    written.push(path.to_path_buf());
    Ok(())
}

fn edit(path: &Path, f: impl FnOnce(&str) -> Result<String, String>, written: &mut Vec<PathBuf>) -> Result<(), String> {
    let old = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let new = f(&old)?;
    if new != old {
        std::fs::write(path, new).map_err(|e| format!("{}: {e}", path.display()))?;
        written.push(path.to_path_buf());
    }
    Ok(())
}

/// Every folder an operation may write, for the work item's lockdirs.
pub fn lock_scope(op: &Value) -> Vec<String> {
    let norm = |p: &str| -> String {
        let r = p.trim().trim_start_matches("{topdir}").trim_start_matches('/').trim_end_matches('/');
        if r.is_empty() { "{topdir}".into() } else { format!("{{topdir}}/{r}") }
    };
    let dir_of = |p: &str| -> String {
        let n = norm(p);
        if n.ends_with(".iter.md") { n.rsplit_once('/').map(|(d, _)| d.to_string()).unwrap_or(n) } else { n }
    };
    let mut v: Vec<String> = match s(op, "op") {
        "new_node" => {
            let parent = s(op, "parent");
            let slug = if s(op, "slug").is_empty() { slugify(s(op, "name")) } else { s(op, "slug").to_string() };
            let base = if parent.is_empty() { "{topdir}".to_string() } else { dir_of(parent) };
            let mut v = vec![format!("{}/{slug}", base.trim_end_matches('/'))];
            // the parent file (or main.iter.md) gains the link
            v.push(if parent.is_empty() { "{topdir}/main.iter.md".into() } else if parent.ends_with(".iter.md") { norm(parent) } else { base });
            v
        }
        "connect" => {
            let mut v = vec![dir_of(s(op, "from")), dir_of(s(op, "to"))];
            let iname = op.pointer("/interface/name").and_then(|x| x.as_str()).map(slugify_iface);
            match (s(op, "interface_path"), iname) {
                (p, _) if !p.is_empty() => v.push(dir_of(p)),
                (_, Some(n)) => v.push(format!("{{topdir}}/interfaces/{n}")),
                _ => {}
            }
            v
        }
        "new_global" => {
            let slug = if s(op, "slug").is_empty() { slugify(s(op, "name")) } else { s(op, "slug").to_string() };
            match s(op, "kind") {
                "usecase" => vec![format!("{{topdir}}/usecases/{slug}")],
                _ => vec![format!("{{topdir}}/reqs/{slug}.{}.iter.md", s(op, "kind"))],
            }
        }
        "edit_body" => vec![norm(s(op, "path"))],
        "link_child" | "unlink_child" => vec![dir_of(s(op, "parent"))],
        "disconnect" => vec![dir_of(s(op, "from")), dir_of(s(op, "to"))],
        "define_tests" => vec![dir_of(s(op, "node"))],
        "new_actor" | "edit_actor" | "actor_unuse" => vec![norm(&actors_file_of(op))],
        "actor_uses" => {
            let mut v = vec![norm(&actors_file_of(op)), dir_of(s(op, "to"))];
            match (s(op, "interface_path"), op.pointer("/interface/name").and_then(|n| n.as_str()).map(slugify_iface)) {
                (p, _) if !p.is_empty() => v.push(dir_of(p)),
                (_, Some(n)) => v.push(format!("{{topdir}}/interfaces/{n}")),
                _ => {}
            }
            v
        }
        "usecase_needs" | "usecase_unneed" | "name_parts" => vec![dir_of(s(op, "usecase"))],
        "store_doc" | "remove_doc" => vec![norm(s(op, "path"))],
        "link_document" | "unlink_document" => vec![norm(s(op, "node"))],
        "gitignore_path" => vec!["{topdir}/.gitignore".to_string()],
        _ => vec![],
    };
    v.sort();
    v.dedup();
    v
}

fn actors_file_of(op: &Value) -> String {
    if s(op, "actors_file").is_empty() { "{topdir}/actors.yaml".into() } else { s(op, "actors_file").to_string() }
}

/// The actors file's leading comment block and its `actors` list.
fn read_actors(path: &Path) -> Result<(String, Vec<Value>), String> {
    let text = std::fs::read_to_string(path).unwrap_or_default();
    let head: String = text.lines().take_while(|l| l.starts_with('#') || l.trim().is_empty()).map(|l| format!("{l}\n")).collect();
    let y: Value = if text.trim().is_empty() { Value::Null } else { serde_yaml::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))? };
    Ok((head, y.get("actors").and_then(|a| a.as_array()).cloned().unwrap_or_default()))
}

fn write_actors(path: &Path, head: &str, actors: &[Value], written: &mut Vec<PathBuf>) -> Result<(), String> {
    let head = if head.trim().is_empty() {
        "# The people and programs at the edge of the map. Each `uses` pattern\n# names interfaces (by id) the actor calls; the map draws an edge from the\n# actor to the part that provides each.\n".to_string()
    } else {
        head.to_string()
    };
    let body = serde_yaml::to_string(&json!({"actors": actors})).map_err(|e| e.to_string())?;
    if let Some(d) = path.parent() {
        std::fs::create_dir_all(d).map_err(|e| format!("{}: {e}", d.display()))?;
    }
    std::fs::write(path, format!("{head}{body}")).map_err(|e| format!("{}: {e}", path.display()))?;
    written.push(path.to_path_buf());
    Ok(())
}

fn actor_index(actors: &[Value], id: &str) -> Result<usize, String> {
    let id = id.trim_start_matches("actor:");
    actors.iter().position(|a| s(a, "id") == id).ok_or_else(|| format!("no actor {id:?} in the actors file"))
}

/// The interface an edge names: `interface_path` (must exist), or
/// `interface.name` (created from the template when new).
fn ensure_interface(project: &Project, top: &Path, op: &Value, written: &mut Vec<PathBuf>) -> Result<PathBuf, String> {
    if !s(op, "interface_path").is_empty() {
        let p = resolve(top, s(op, "interface_path"))?;
        if !p.is_file() {
            return Err(format!("no interface file {}", p.display()));
        }
        return Ok(p);
    }
    let spec = op.get("interface").cloned().unwrap_or(Value::Null);
    let name = slugify_iface(s(&spec, "name"));
    if name.is_empty() {
        return Err("a connection needs an interface name or interface_path".into());
    }
    let kind = if s(&spec, "kind").is_empty() { "request-reply" } else { s(&spec, "kind") };
    let desc = if s(&spec, "description").is_empty() { format!("What {name} carries.") } else { s(&spec, "description").to_string() };
    let p = project.interfacedir.join(&name).join(format!("{name}.interface.iter.md"));
    if !p.exists() {
        let example = json!([{"request": {}, "reply": {}}, {"request": {}, "reply": {"refusal": {"code": "REFUSED", "detail": "why"}}}]);
        write_new(&p, &format!(
            "---\nid: {}\nname: {}\nlabel: {}\nkind: {kind}\ndescription: {}\nowner: bespoke\nteststate: inherit\nchildren:\n  bizreqs:    []\n  techreqs:   []\n  tests:      []\n---\n\n# {name}\n\n{desc}\n\n## Request\n\n```json\n{{}}\n```\n\n## Reply, success shape\n\n```json\n{{}}\n```\n\n## Reply, failure shape\n\n```json\n{{\"refusal\": {{\"code\": \"REFUSED\", \"detail\": \"why\"}}}}\n```\n\n## Worked examples\n\nNormative — each pair must hold on every implementation (strict JSON):\n\n```json\n{}\n```\n",
            new_id(), yq(&name), yq(if s(&spec, "label").is_empty() { &name } else { s(&spec, "label") }), yq(&desc), serde_json::to_string_pretty(&example).unwrap()), written)?;
    }
    Ok(p)
}

/// A use-case file (exact path, or its folder).
fn usecase_file(top: &Path, p: &str) -> Result<PathBuf, String> {
    let r = resolve(top, p)?;
    if r.is_file() && r.to_string_lossy().ends_with(".usecase.iter.md") {
        return Ok(r);
    }
    if r.is_dir() {
        if let Some(f) = std::fs::read_dir(&r).ok().into_iter().flatten().flatten().map(|e| e.path()).find(|f| f.to_string_lossy().ends_with(".usecase.iter.md")) {
            return Ok(f);
        }
    }
    Err(format!("no use case at {p}"))
}

fn slugify_iface(name: &str) -> String {
    slugify(name).replace('_', "-")
}

/// Apply one operation to the checkout; returns the files written or changed.
pub fn apply(project: &Project, op: &Value) -> Result<Vec<String>, String> {
    let top = project.topdir.canonicalize().unwrap_or_else(|_| project.topdir.clone());
    let mut written: Vec<PathBuf> = Vec::new();
    match s(op, "op") {
        "new_node" => {
            let kind = s(op, "kind");
            if !["context", "container", "component"].contains(&kind) {
                return Err(format!("kind must be context, container or component (got {kind:?})"));
            }
            let name = s(op, "name").trim();
            if name.is_empty() {
                return Err("a node needs a name".into());
            }
            let slug = if s(op, "slug").is_empty() { slugify(name) } else { slugify(s(op, "slug")) };
            if slug.is_empty() {
                return Err(format!("no file name can be made from {name:?}"));
            }
            let desc = if s(op, "description").is_empty() { format!("{name}.") } else { s(op, "description").to_string() };
            let simple = if s(op, "simple_description").is_empty() { desc.clone() } else { s(op, "simple_description").to_string() };
            let long = if s(op, "long_description").is_empty() { desc.clone() } else { s(op, "long_description").to_string() };
            let parent = s(op, "parent");
            let parent_file = if parent.is_empty() { project.mainfile.clone() } else { code_node(&top, parent)? };
            let parent_dir = if parent.is_empty() { top.clone() } else { parent_file.parent().unwrap_or(&top).to_path_buf() };
            let dir = parent_dir.join(&slug);
            let code = format!(
                "---\nid: {id}\nname: {n}\ndescription: {d}\nsimple_description: {sd}\nlevel: {kind}\nowner: bespoke\nteststate: inherit\nchildren:\n  codedirs:   [\"{{thisfiledir}}/\"]\n  codenodes:  []\n  inputs:     []\n  outputs:    []\n  bizreqs:    [\"{{thisfiledir}}/{slug}.bizreq.iter.md\"]\n  techreqs:   [\"{{thisfiledir}}/{slug}.techreq.iter.md\"]\n  tests:      [\"{{thisfiledir}}/tests/*.tests.iter.md\"]\n---\n\n# Long Description\n\n{long}\n",
                id = new_id(), n = yq(name), d = desc, sd = yq(&simple), kind = kind, slug = slug, long = long
            );
            let d = yq(&desc);
            let code = code.replace(&format!("description: {desc}\n"), &format!("description: {d}\n"));
            write_new(&dir.join(format!("{slug}.code.iter.md")), &code, &mut written)?;
            for (k, title) in [("bizreq", "business requirements"), ("techreq", "technical requirements")] {
                let text = if s(op, k).trim().is_empty() { "- (none yet)".to_string() } else { s(op, k).trim().to_string() };
                write_new(
                    &dir.join(format!("{slug}.{k}.iter.md")),
                    &format!("---\nid: {}\nname: {}\ndescription: {}\nchildren:\n  reqpaths: []\n---\n\n# {name} — {title}\n\n{text}\n", new_id(), yq(&format!("{name} {title}")), yq(&format!("The {title} of {name}."))),
                    &mut written,
                )?;
            }
            let group = json!({"label": format!("{slug}-unit"), "desc": format!("the tests of {name}"), "auto_fix": false,
                               "lastrun": "", "result": "", "counts": "", "testlist": []});
            write_new(
                &dir.join("tests").join(format!("{slug}.tests.iter.md")),
                &format!("---\nid: {}\nname: {}\ndescription: {}\nchildren:\n  testpaths: [\"{{thisfiledir}}/*.sh\"]\n---\n\n# {name} — tests\n\n{}<!-- iterapp:testgroups\n{}\n-->\n",
                    new_id(), yq(&format!("{name} tests")), yq(&format!("The tests of {name}.")), planned_section(op.get("tests")), group),
                &mut written,
            )?;
            let link = topdir_rel(&top, &dir.join(format!("{slug}.code.iter.md")));
            edit(&parent_file, |c| add_child(c, "codenodes", &link), &mut written)?;
        }
        "connect" => {
            let from = code_node(&top, s(op, "from"))?;
            let to = code_node(&top, s(op, "to"))?;
            let iface = ensure_interface(project, &top, op, &mut written)?;
            let link = topdir_rel(&top, &iface);
            edit(&from, |c| add_child(c, "inputs", &link), &mut written)?;
            edit(&to, |c| add_child(c, "outputs", &link), &mut written)?;
        }
        "new_global" => {
            let kind = s(op, "kind");
            let name = s(op, "name").trim();
            if name.is_empty() {
                return Err("a name is required".into());
            }
            let slug = if s(op, "slug").is_empty() { slugify(name) } else { slugify(s(op, "slug")) };
            let desc = if s(op, "description").is_empty() { format!("{name}.") } else { s(op, "description").to_string() };
            let body = if s(op, "body").is_empty() { format!("# {name}\n\n{desc}\n") } else { s(op, "body").to_string() };
            match kind {
                "bizreq" | "techreq" => {
                    let p = top.join("reqs").join(format!("{slug}.{kind}.iter.md"));
                    write_new(&p, &format!("---\nid: {}\nname: {}\ndescription: {}\nchildren:\n  reqpaths: []\n---\n\n{body}\n", new_id(), yq(name), yq(&desc)), &mut written)?;
                }
                "usecase" => {
                    let codenodes: Vec<String> = op
                        .get("codenodes")
                        .and_then(|c| c.as_array())
                        .into_iter()
                        .flatten()
                        .filter_map(|x| x.as_str())
                        .filter_map(|d| code_node(&top, d).ok())
                        .map(|f| yq(&topdir_rel(&top, &f)))
                        .collect();
                    let p = project.usecasedir.join(&slug).join(format!("{slug}.usecase.iter.md"));
                    write_new(&p, &format!("---\nid: {}\nname: {}\ndescription: {}\nteststate: inherit\nchildren:\n  codenodes:  [{}]\n  tests:      []\n---\n\n{body}\n",
                        new_id(), yq(name), yq(&desc), codenodes.join(", ")), &mut written)?;
                }
                other => return Err(format!("kind must be bizreq, techreq or usecase (got {other:?})")),
            }
        }
        "new_actor" | "edit_actor" | "actor_uses" | "actor_unuse" => {
            let path = if s(op, "actors_file").is_empty() { crate::graph::actors_path(project) } else { resolve(&top, s(op, "actors_file"))? };
            let (head, mut actors) = read_actors(&path)?;
            match s(op, "op") {
                "new_actor" => {
                    let name = s(op, "name").trim();
                    if name.is_empty() {
                        return Err("an actor needs a name".into());
                    }
                    let id = slugify_iface(if s(op, "id").is_empty() { name } else { s(op, "id") });
                    if actors.iter().any(|a| s(a, "id") == id) {
                        return Err(format!("actor {id:?} already exists"));
                    }
                    actors.push(json!({"id": id, "name": name, "description": s(op, "description"), "uses": []}));
                }
                "edit_actor" => {
                    let i = actor_index(&actors, s(op, "actor"))?;
                    for k in ["name", "description"] {
                        if !s(op, k).trim().is_empty() {
                            actors[i][k] = json!(s(op, k).trim());
                        }
                    }
                }
                "actor_uses" => {
                    let i = actor_index(&actors, s(op, "actor"))?;
                    let to = code_node(&top, s(op, "to"))?;
                    let iface = ensure_interface(project, &top, op, &mut written)?;
                    let link = topdir_rel(&top, &iface);
                    edit(&to, |c| add_child(c, "outputs", &link), &mut written)?;
                    let name = iface.file_name().and_then(|f| f.to_str()).unwrap_or("").trim_end_matches(".interface.iter.md").to_string();
                    let uses = actors[i].get("uses").and_then(|u| u.as_array()).cloned().unwrap_or_default();
                    if !uses.iter().any(|u| s(u, "pattern") == name) {
                        let mut uses = uses;
                        uses.push(json!({"pattern": name}));
                        actors[i]["uses"] = json!(uses);
                    }
                }
                _ => {
                    if s(op, "reason").trim().is_empty() {
                        return Err("removing an edge needs a reason".into());
                    }
                    let i = actor_index(&actors, s(op, "actor"))?;
                    let name = s(op, "interface").to_string();
                    let uses: Vec<Value> = actors[i].get("uses").and_then(|u| u.as_array()).cloned().unwrap_or_default();
                    let kept: Vec<Value> = uses.iter().filter(|u| s(u, "pattern") != name).cloned().collect();
                    if kept.len() == uses.len() {
                        return Err(format!("actor {} has no uses pattern {name:?} (an edge drawn from a wider pattern is removed by editing the actors file)", s(op, "actor")));
                    }
                    actors[i]["uses"] = json!(kept);
                }
            }
            write_actors(&path, &head, &actors, &mut written)?;
        }
        "usecase_needs" | "usecase_unneed" => {
            let uc = usecase_file(&top, s(op, "usecase"))?;
            let node = code_node(&top, s(op, "node"))?;
            if s(op, "op") == "usecase_needs" {
                let link = topdir_rel(&top, &node);
                edit(&uc, |c| add_child(c, "codenodes", &link), &mut written)?;
            } else {
                if s(op, "reason").trim().is_empty() {
                    return Err("removing an edge needs a reason".into());
                }
                let ucdir = uc.parent().unwrap_or(&top).to_path_buf();
                edit(&uc, |c| remove_child(c, "codenodes", &node, &top, &ucdir).map(|(t, _)| t), &mut written)?;
            }
        }
        "name_parts" => {
            usecase_file(&top, s(op, "usecase"))?;
        }
        "link_child" | "unlink_child" => {
            let parent = code_node(&top, s(op, "parent"))?;
            let child = code_node(&top, s(op, "child"))?;
            if parent == child {
                return Err("a node cannot own itself".into());
            }
            if s(op, "op") == "link_child" {
                let link = topdir_rel(&top, &child);
                edit(&parent, |c| add_child(c, "codenodes", &link), &mut written)?;
            } else {
                if s(op, "reason").trim().is_empty() {
                    return Err("removing an edge needs a reason".into());
                }
                let pdir = parent.parent().unwrap_or(&top).to_path_buf();
                let mut n = 0;
                edit(&parent, |c| remove_child(c, "codenodes", &child, &top, &pdir).map(|(t, k)| { n = k; t }), &mut written)?;
                if n == 0 {
                    return Err(format!("{} does not list {} in children.codenodes", topdir_rel(&top, &parent), topdir_rel(&top, &child)));
                }
            }
        }
        "disconnect" => {
            if s(op, "reason").trim().is_empty() {
                return Err("removing an edge needs a reason".into());
            }
            let from = code_node(&top, s(op, "from"))?;
            let to = code_node(&top, s(op, "to"))?;
            let iface = if !s(op, "interface_path").is_empty() {
                resolve(&top, s(op, "interface_path"))?
            } else {
                let n = slugify_iface(op.pointer("/interface/name").and_then(|x| x.as_str()).unwrap_or(""));
                project.interfacedir.join(&n).join(format!("{n}.interface.iter.md"))
            };
            let mut total = 0;
            for (file, key) in [(&from, "inputs"), (&to, "outputs")] {
                let fdir = file.parent().unwrap_or(&top).to_path_buf();
                edit(file, |c| remove_child(c, key, &iface, &top, &fdir).map(|(t, k)| { total += k; t }), &mut written)?;
            }
            if total == 0 {
                return Err(format!("no {} link between them to remove", topdir_rel(&top, &iface)));
            }
        }
        "define_tests" => {
            let node = code_node(&top, s(op, "node"))?;
            let ndir = node.parent().unwrap_or(&top).to_path_buf();
            let slug = node.file_name().and_then(|n| n.to_str()).unwrap_or("x").trim_end_matches(".code.iter.md").to_string();
            let slug = if slug.is_empty() || slug == "code.iter.md" { "tests".to_string() } else { slug };
            let tests = op.get("tests").and_then(|t| t.as_array()).cloned().unwrap_or_default();
            if tests.is_empty() {
                return Err("define_tests needs at least one test (tests: [{name, desc}])".into());
            }
            // new tests files go in `tests/`; a node that already keeps its file in the
            // older `test/` folder goes on using that one
            let old_tf = ndir.join("test").join(format!("{slug}.tests.iter.md"));
            let tdir = if old_tf.exists() { "test" } else { "tests" };
            let tf = ndir.join(tdir).join(format!("{slug}.tests.iter.md"));
            if tf.exists() {
                let section = planned_section(op.get("tests"));
                edit(&tf, |c| Ok(replace_planned(c, &section)), &mut written)?;
            } else {
                let name = markers_name(&node);
                let group = json!({"label": format!("{slug}-unit"), "desc": format!("the tests of {name}"), "auto_fix": false,
                                   "lastrun": "", "result": "", "counts": "", "testlist": []});
                write_new(&tf, &format!("---\nid: {}\nname: {}\ndescription: {}\nchildren:\n  testpaths: [\"{{thisfiledir}}/*.sh\"]\n---\n\n# {name} — tests\n\n{}<!-- iterapp:testgroups\n{}\n-->\n",
                    new_id(), yq(&format!("{name} tests")), yq(&format!("The tests of {name}.")), planned_section(op.get("tests")), group), &mut written)?;
                // make sure the node links it (a link to the other folder does not reach it)
                let link = topdir_rel(&top, &tf);
                edit(&node, |c| {
                    let already = c.contains(&format!("{tdir}/*.tests.iter.md")) || c.contains(&link);
                    if already { Ok(c.to_string()) } else { add_child(c, "tests", &link) }
                }, &mut written)?;
            }
        }
        "edit_body" => {
            let p = resolve(&top, s(op, "path"))?;
            if !p.file_name().and_then(|n| n.to_str()).map(|n| n.ends_with(".iter.md")).unwrap_or(false) {
                return Err("edit_body edits *.iter.md files only".into());
            }
            let body = s(op, "body").to_string();
            edit(&p, |c| set_body(c, &body), &mut written)?;
        }
        // GraphRAG (2026-09-29): an uploaded document's original lands in the
        // project's docs directory (iter_data already extracted, chunked and
        // embedded its text); re-uploading the same name replaces it
        "store_doc" => {
            let p = resolve(&top, s(op, "path"))?;
            if s(op, "path").ends_with(".iter.md") {
                return Err("store_doc never writes *.iter.md node files".into());
            }
            use base64::Engine as _;
            let bytes = base64::engine::general_purpose::STANDARD
                .decode(s(op, "content_b64"))
                .map_err(|e| format!("content_b64: {e}"))?;
            if let Some(d) = p.parent() {
                std::fs::create_dir_all(d).map_err(|e| format!("{}: {e}", d.display()))?;
            }
            std::fs::write(&p, bytes).map_err(|e| format!("{}: {e}", p.display()))?;
            written.push(p);
        }
        // a node names an uploaded document that describes it (children.documents)
        "link_document" | "unlink_document" => {
            let node = resolve(&top, s(op, "node"))?;
            if !node.file_name().and_then(|n| n.to_str()).map(|n| n.ends_with(".iter.md")).unwrap_or(false) || !node.is_file() {
                return Err(format!("no node file {}", s(op, "node")));
            }
            let doc = s(op, "document").trim();
            if doc.is_empty() || doc.ends_with(".iter.md") {
                return Err("document must be an uploaded file's {topdir}/… path".into());
            }
            let doc_tp = format!("{{topdir}}/{}", doc.trim_start_matches("{topdir}").trim_start_matches('/'));
            let filedir = node.parent().unwrap_or(&top).to_path_buf();
            if s(op, "op") == "link_document" {
                edit(&node, |c| add_child(c, "documents", &doc_tp), &mut written)?;
            } else {
                let target = resolve(&top, &doc_tp)?;
                edit(&node, |c| remove_child(c, "documents", &target, &top, &filedir).map(|(t, _)| t), &mut written)?;
            }
        }
        // keep the GraphRAG docs directory out of git (setting docs_gitignore)
        "gitignore_path" => {
            let rel = s(op, "path").trim().trim_start_matches("{topdir}").trim_start_matches('/').to_string();
            if rel.is_empty() || rel.split('/').any(|x| x == "..") {
                return Err(format!("gitignore_path: bad path {:?}", s(op, "path")));
            }
            let line = format!("/{}", rel);
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
                written.push(gi);
            }
        }
        "remove_doc" => {
            let p = resolve(&top, s(op, "path"))?;
            if s(op, "path").ends_with(".iter.md") {
                return Err("remove_doc never removes *.iter.md node files".into());
            }
            if p.is_file() {
                std::fs::remove_file(&p).map_err(|e| format!("{}: {e}", p.display()))?;
                written.push(p); // `git add` of a removed path stages the removal
            }
        }
        other => return Err(format!("unknown graph edit op {other:?}")),
    }
    Ok(written.iter().map(|p| topdir_rel(&top, p)).collect())
}

/// "## Planned tests" — the tests a node should pass, simplest first (TDD:
/// written before the code; the `test` agent turns each into a script).
fn planned_section(tests: Option<&Value>) -> String {
    let list: Vec<Value> = tests.and_then(|t| t.as_array()).cloned().unwrap_or_default();
    if list.is_empty() {
        return String::new();
    }
    let mut out = String::from("## Planned tests\n\nSimplest first. Each becomes one script in this group; red until the code exists.\n\n");
    for (i, t) in list.iter().enumerate() {
        let name = t.get("name").and_then(|x| x.as_str()).unwrap_or("").trim();
        let desc = t.get("desc").and_then(|x| x.as_str()).unwrap_or("").trim();
        out.push_str(&format!("{}. **{}**{}\n", i + 1, if name.is_empty() { "(unnamed)" } else { name }, if desc.is_empty() { String::new() } else { format!(" — {desc}") }));
    }
    out.push('\n');
    out
}

/// Put `section` in place of an existing "## Planned tests" section (or before the groups block).
fn replace_planned(content: &str, section: &str) -> String {
    if let Some(i) = content.find("## Planned tests") {
        let rest = &content[i..];
        let end = rest.find("<!-- iterapp:testgroups").or_else(|| rest[3..].find("\n## ").map(|j| j + 4)).unwrap_or(rest.len());
        return format!("{}{}{}", &content[..i], section, &rest[end..]);
    }
    match content.find("<!-- iterapp:testgroups") {
        Some(i) => format!("{}{}{}", &content[..i], section, &content[i..]),
        None => format!("{}\n{}", content.trim_end(), section),
    }
}

fn markers_name(node: &Path) -> String {
    let c = std::fs::read_to_string(node).unwrap_or_default();
    let n = crate::markers::parse_front(&c).scalar("name");
    if n.trim().is_empty() { "this node".into() } else { n.trim().trim_matches('"').to_string() }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph;

    fn w(top: &Path, rel: &str, body: &str) {
        let p = top.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, body).unwrap();
    }

    #[test]
    fn add_child_keeps_the_files_style() {
        let inline = "---\nname: x\nchildren:\n  codenodes: [\"a\"]\n  inputs: []\n---\nbody";
        let out = add_child(inline, "codenodes", "b").unwrap();
        assert!(out.contains("codenodes: [\"a\", \"b\"]"), "{out}");
        assert_eq!(add_child(&out, "codenodes", "b").unwrap(), out, "no duplicate");
        assert!(add_child(inline, "inputs", "i").unwrap().contains("inputs: [\"i\"]"));
        let block = "---\nname: x\nchildren:\n  codenodes:\n    - \"a\"\n---\n";
        assert!(add_child(block, "codenodes", "b").unwrap().contains("    - \"a\"\n    - \"b\"\n"));
        let nokey = "---\nname: x\nchildren:\n  codedirs: []\n---\n";
        assert!(add_child(nokey, "outputs", "o").unwrap().contains("children:\n  outputs: [\"o\"]\n  codedirs: []"));
        let nochildren = "---\nname: x\n---\nb";
        assert!(add_child(nochildren, "codenodes", "c").unwrap().contains("children:\n  codenodes: [\"c\"]\n---"));
        assert_eq!(set_body("---\na: 1\n---\nold\n", "new").unwrap(), "---\na: 1\n---\nnew\n");
        assert_eq!(slugify("My New Thing!"), "my_new_thing");
    }

    #[test]
    fn graphrag_documents_are_stored_and_removed() {
        use base64::Engine as _;
        let top = std::env::temp_dir().join(format!("iter4_edit_doc_{}", uuid::Uuid::new_v4()));
        w(&top, "main.iter.md", "---\nid: 11111111-1111-4111-8111-111111111111\nprojectname: demo\nchildren:\n  codenodes: []\n---\n");
        let project = Project::load(&top);
        let b64 = base64::engine::general_purpose::STANDARD.encode([0u8, 159, 146, 150, b'x']);
        let op = json!({"op": "store_doc", "path": "{topdir}/docs/ref/a.pdf", "content_b64": b64});
        assert_eq!(lock_scope(&op), vec!["{topdir}/docs/ref/a.pdf".to_string()]);
        assert_eq!(apply(&project, &op).unwrap(), vec!["{topdir}/docs/ref/a.pdf".to_string()]);
        assert_eq!(std::fs::read(top.join("docs/ref/a.pdf")).unwrap(), vec![0u8, 159, 146, 150, b'x']);
        // a re-upload replaces it; node files and escapes are refused
        apply(&project, &json!({"op": "store_doc", "path": "{topdir}/docs/ref/a.pdf", "content_b64": "eQ=="})).unwrap();
        assert_eq!(std::fs::read(top.join("docs/ref/a.pdf")).unwrap(), b"y");
        assert!(apply(&project, &json!({"op": "store_doc", "path": "{topdir}/x.code.iter.md", "content_b64": "eQ=="})).is_err());
        assert!(apply(&project, &json!({"op": "store_doc", "path": "{topdir}/../out.txt", "content_b64": "eQ=="})).is_err());
        let rm = apply(&project, &json!({"op": "remove_doc", "path": "{topdir}/docs/ref/a.pdf"})).unwrap();
        assert_eq!(rm, vec!["{topdir}/docs/ref/a.pdf".to_string()]);
        assert!(!top.join("docs/ref/a.pdf").exists());
        assert!(apply(&project, &json!({"op": "remove_doc", "path": "{topdir}/docs/ref/a.pdf"})).unwrap().is_empty());
        // documents linked from a node, and the docs directory kept out of git
        w(&top, "a/a.code.iter.md", "---\nid: x\nname: A\nchildren:\n  codenodes: []\n---\n");
        apply(&project, &json!({"op": "link_document", "node": "{topdir}/a/a.code.iter.md", "document": "{topdir}/docs/spec.pdf"})).unwrap();
        assert!(std::fs::read_to_string(top.join("a/a.code.iter.md")).unwrap().contains("documents: [\"{topdir}/docs/spec.pdf\"]"));
        apply(&project, &json!({"op": "unlink_document", "node": "{topdir}/a/a.code.iter.md", "document": "{topdir}/docs/spec.pdf"})).unwrap();
        assert!(!std::fs::read_to_string(top.join("a/a.code.iter.md")).unwrap().contains("spec.pdf"));
        assert_eq!(apply(&project, &json!({"op": "gitignore_path", "path": "{topdir}/docs/"})).unwrap(), vec!["{topdir}/.gitignore".to_string()]);
        assert!(apply(&project, &json!({"op": "gitignore_path", "path": "{topdir}/docs/"})).unwrap().is_empty(), "idempotent");
        assert!(std::fs::read_to_string(top.join(".gitignore")).unwrap().contains("/docs/"));
        apply(&project, &json!({"op": "gitignore_path", "path": "{topdir}/docs/", "ignore": false})).unwrap();
        assert!(!std::fs::read_to_string(top.join(".gitignore")).unwrap().contains("docs"));
        let _ = std::fs::remove_dir_all(&top);
    }

    #[test]
    fn actors_and_use_cases_are_edited_from_the_graph() {
        let top = std::env::temp_dir().join(format!("iter4_edit_act_{}", uuid::Uuid::new_v4()));
        w(&top, "main.iter.md", "---\nid: 11111111-1111-4111-8111-111111111111\nprojectname: demo\nprojectdescription: d\nglobalscandirs: [\"{topdir}/\"]\nchildren:\n  codenodes: []\n---\n");
        let project = Project::load(&top);
        apply(&project, &json!({"op": "new_node", "kind": "context", "name": "Web"})).unwrap();
        // no actors file yet: the first actor creates <topdir>/actors.yaml
        let f = apply(&project, &json!({"op": "new_actor", "name": "Developer", "description": "files work"})).unwrap();
        assert_eq!(f, vec!["{topdir}/actors.yaml".to_string()]);
        assert!(apply(&project, &json!({"op": "new_actor", "name": "Developer"})).unwrap_err().contains("already exists"));
        let f = apply(&project, &json!({"op": "actor_uses", "actor": "actor:developer", "to": "web", "interface": {"name": "webui page", "description": "the browser page"}})).unwrap();
        assert!(f.iter().any(|p| p.ends_with("webui-page.interface.iter.md")) && f.iter().any(|p| p.ends_with("web.code.iter.md")), "{f:?}");
        let snap = graph::snapshot(&Project::load(&top));
        assert_eq!(snap.actors[0]["uses"][0]["pattern"], "webui-page");
        assert_eq!(snap.actors_file, "{topdir}/actors.yaml");
        assert!(std::fs::read_to_string(top.join("actors.yaml")).unwrap().starts_with("# The people"), "comment head kept");
        apply(&project, &json!({"op": "edit_actor", "actor": "developer", "name": "Dev"})).unwrap();
        assert!(apply(&project, &json!({"op": "actor_unuse", "actor": "developer", "interface": "webui-page"})).is_err(), "a reason is required");
        apply(&project, &json!({"op": "actor_unuse", "actor": "developer", "interface": "webui-page", "reason": "gone"})).unwrap();
        let snap = graph::snapshot(&Project::load(&top));
        assert_eq!((snap.actors[0]["name"].as_str(), snap.actors[0]["uses"].as_array().map(|u| u.len())), (Some("Dev"), Some(0)));
        // a use case names a part; and drops it again with a reason
        apply(&project, &json!({"op": "new_global", "kind": "usecase", "name": "Browse"})).unwrap();
        apply(&project, &json!({"op": "usecase_needs", "usecase": "{topdir}/usecases/browse", "node": "web"})).unwrap();
        let uc = std::fs::read_to_string(top.join("usecases/browse/browse.usecase.iter.md")).unwrap();
        assert!(uc.contains("web/web.code.iter.md"), "{uc}");
        apply(&project, &json!({"op": "usecase_unneed", "usecase": "usecases/browse/browse.usecase.iter.md", "node": "web", "reason": "not needed"})).unwrap();
        assert!(!std::fs::read_to_string(top.join("usecases/browse/browse.usecase.iter.md")).unwrap().contains("web/web.code.iter.md"));
        assert_eq!(apply(&project, &json!({"op": "name_parts", "usecase": "usecases/browse"})).unwrap(), Vec::<String>::new());
        assert_eq!(lock_scope(&json!({"op": "actor_uses", "actor": "a", "to": "{topdir}/web", "interface": {"name": "x y"}, "actors_file": "{topdir}/map/actors.yaml"})),
            vec!["{topdir}/interfaces/x-y", "{topdir}/map/actors.yaml", "{topdir}/web"]);
    }

    #[test]
    fn build_a_project_from_the_graph() {
        let top = std::env::temp_dir().join(format!("iter4_edit_{}", uuid::Uuid::new_v4()));
        w(&top, "main.iter.md", "---\nid: 11111111-1111-4111-8111-111111111111\nprojectname: demo\nprojectdescription: d\nglobalscandirs: [\"{topdir}/\"]\nchildren:\n  codenodes: []\n---\n");
        let project = Project::load(&top);
        let ctx = apply(&project, &json!({"op": "new_node", "kind": "context", "name": "Data"})).unwrap();
        assert!(ctx.iter().any(|p| p == "{topdir}/data/data.code.iter.md"), "{ctx:?}");
        assert!(ctx.iter().any(|p| p == "{topdir}/main.iter.md"), "main links the context");
        apply(&project, &json!({"op": "new_node", "kind": "container", "name": "Ledger API", "parent": "{topdir}/data"})).unwrap();
        apply(&project, &json!({"op": "new_node", "kind": "container", "name": "Store", "parent": "data"})).unwrap();
        let c = apply(&project, &json!({"op": "connect", "from": "data/ledger_api", "to": "{topdir}/data/store",
            "interface": {"name": "store read", "description": "rows by key"}})).unwrap();
        assert!(c.iter().any(|p| p == "{topdir}/interfaces/store-read/store-read.interface.iter.md"), "{c:?}");
        apply(&project, &json!({"op": "new_global", "kind": "usecase", "name": "Read a row", "codenodes": ["data/ledger_api"]})).unwrap();
        apply(&project, &json!({"op": "new_global", "kind": "bizreq", "name": "Global rules", "body": "- B1. rows are never lost"})).unwrap();
        apply(&project, &json!({"op": "edit_body", "path": "{topdir}/reqs/global_rules.bizreq.iter.md", "body": "- B1. rows are never lost\n- B2. reads are fast"})).unwrap();
        assert!(std::fs::read_to_string(top.join("reqs/global_rules.bizreq.iter.md")).unwrap().contains("B2. reads are fast"));
        // refusals
        assert!(apply(&project, &json!({"op": "new_node", "kind": "context", "name": "Data"})).unwrap_err().contains("already exists"));
        assert!(apply(&project, &json!({"op": "edit_body", "path": "../x.iter.md", "body": "x"})).is_err());
        assert!(apply(&project, &json!({"op": "new_node", "kind": "planet", "name": "x"})).is_err());
        // every file validates, and the map shows the ownership and the connection
        let rep = crate::validate::run(std::slice::from_ref(&top), None, false).unwrap();
        // a new node's text is a stub by design: the node-text warnings are what
        // turn it into an `ingest` work item; anything else would be a defect
        let text = ["thin-long-description", "description-not-action", "missing-simple-description"];
        let bad: Vec<_> = rep.findings.iter().filter(|f| f.severity != crate::validate::Severity::Info && !text.contains(&f.code)).collect();
        assert!(bad.is_empty(), "{bad:?}");
        let snap = graph::snapshot(&Project::load(&top));
        let path_of = |id: &str| snap.vertices.iter().find(|v| v["id"] == id).map(|v| v["path"].as_str().unwrap().to_string()).unwrap_or_default();
        let has = |k: &str, f: &str, t: &str| snap.edges.iter().any(|e| e["kind"] == k && path_of(e["from"].as_str().unwrap()).ends_with(f) && path_of(e["to"].as_str().unwrap()).ends_with(t));
        assert!(has("codenodes", "data/data.code.iter.md", "ledger_api.code.iter.md"));
        assert!(has("inputs", "ledger_api.code.iter.md", "store-read.interface.iter.md"));
        assert!(has("outputs", "store.code.iter.md", "store-read.interface.iter.md"));
        assert!(has("tests", "store.code.iter.md", "store.tests.iter.md"));
        assert!(has("codenodes", "read_a_row.usecase.iter.md", "ledger_api.code.iter.md"));
        assert_eq!(lock_scope(&json!({"op": "new_node", "name": "X Y", "parent": "{topdir}/data"})), vec!["{topdir}/data", "{topdir}/data/x_y"]);
        assert_eq!(lock_scope(&json!({"op": "new_node", "name": "X Y", "parent": "{topdir}/data/data.code.iter.md"})), vec!["{topdir}/data/data.code.iter.md", "{topdir}/data/x_y"]);
        // a node addressed by its file, where two code nodes share a folder
        apply(&project, &json!({"op": "new_node", "kind": "component", "name": "Parser", "parent": "{topdir}/data/store/store.code.iter.md"})).unwrap();
        assert!(std::fs::read_to_string(top.join("data/store/store.code.iter.md")).unwrap().contains("data/store/parser/parser.code.iter.md"));
        // any level owns any level: a context inside a context, then linked and unlinked elsewhere
        apply(&project, &json!({"op": "new_node", "kind": "context", "name": "Auth", "parent": "data", "description": "Checks who is asking.",
            "tests": [{"name": "boots", "desc": "the container starts"}, {"name": "login", "desc": "a known user gets a token"}]})).unwrap();
        let t = std::fs::read_to_string(top.join("data/auth/tests/auth.tests.iter.md")).unwrap();
        assert!(t.contains("## Planned tests") && t.find("boots").unwrap() < t.find("login").unwrap(), "{t}");
        apply(&project, &json!({"op": "link_child", "parent": "data/ledger_api", "child": "data/store"})).unwrap();
        assert!(std::fs::read_to_string(top.join("data/ledger_api/ledger_api.code.iter.md")).unwrap().contains("data/store/store.code.iter.md"));
        assert!(apply(&project, &json!({"op": "unlink_child", "parent": "data/ledger_api", "child": "data/store"})).unwrap_err().contains("reason"));
        apply(&project, &json!({"op": "unlink_child", "parent": "data/ledger_api", "child": "data/store", "reason": "store is shared, not owned"})).unwrap();
        assert!(!std::fs::read_to_string(top.join("data/ledger_api/ledger_api.code.iter.md")).unwrap().contains("data/store/store.code.iter.md"));
        apply(&project, &json!({"op": "disconnect", "from": "data/ledger_api", "to": "data/store", "interface": {"name": "store-read"}, "reason": "reads go through the cache now"})).unwrap();
        assert!(!std::fs::read_to_string(top.join("data/store/store.code.iter.md")).unwrap().contains("store-read"));
        apply(&project, &json!({"op": "define_tests", "node": "data/store", "tests": [{"name": "boots"}, {"name": "reads a row"}]})).unwrap();
        let st = std::fs::read_to_string(top.join("data/store/tests/store.tests.iter.md")).unwrap();
        assert!(st.contains("2. **reads a row**") && st.contains("<!-- iterapp:testgroups"), "{st}");
        apply(&project, &json!({"op": "define_tests", "node": "data/store", "tests": [{"name": "only one now"}]})).unwrap();
        let st = std::fs::read_to_string(top.join("data/store/tests/store.tests.iter.md")).unwrap();
        assert!(st.contains("only one now") && !st.contains("reads a row"), "the planned list is replaced, not appended: {st}");
        // a node that keeps its tests file in the older `test/` folder goes on using it
        std::fs::create_dir_all(top.join("data/ledger_api/test")).unwrap();
        std::fs::rename(top.join("data/ledger_api/tests/ledger_api.tests.iter.md"), top.join("data/ledger_api/test/ledger_api.tests.iter.md")).unwrap();
        apply(&project, &json!({"op": "define_tests", "node": "data/ledger_api", "tests": [{"name": "answers a read"}]})).unwrap();
        assert!(std::fs::read_to_string(top.join("data/ledger_api/test/ledger_api.tests.iter.md")).unwrap().contains("answers a read"));
        assert!(!top.join("data/ledger_api/tests/ledger_api.tests.iter.md").exists(), "no second tests file in tests/");
    }
}
