//! Names and paths: slug, designer folder rules (§2.5), placeholder
//! expansion and glob / directory resolution against a node-path list (§2.2).

use super::{EdgeKind, NodeDoc, NodeType, type_of};
use std::collections::HashSet;

/// How a new file is named on a path collision (project `file_naming`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Naming {
    /// `<slug>01`, `<slug>02`, …
    #[default]
    Sequence,
    /// `<slug>_<last 12 of id>`
    Uuid12,
}

impl Naming {
    /// `"uuid12"` → Uuid12, anything else → Sequence (the default).
    pub fn from_setting(s: &str) -> Naming {
        if s.trim() == "uuid12" { Naming::Uuid12 } else { Naming::Sequence }
    }
}

/// How the planned node hangs off `parent`: `Root` (no parent / global) or
/// the edge kind the parent will hold for it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Attach {
    #[default]
    Root,
    Under(EdgeKind),
}

/// lowercase, `[a-z0-9_-]` kept, runs of anything else → `_`, trimmed of
/// `_`/`-`, max 60 chars. Empty input gives `""` — [`plan_path`] then uses the
/// node type.
pub fn slug(name: &str) -> String {
    let mut out = String::new();
    let mut pending_sep = false;
    for c in name.to_lowercase().chars() {
        if c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-' {
            if pending_sep && !out.is_empty() {
                out.push('_');
            }
            pending_sep = false;
            out.push(c);
        } else {
            pending_sep = true;
        }
    }
    let mut s: String = out.trim_matches(|c| c == '_' || c == '-').chars().take(60).collect();
    while s.ends_with('_') || s.ends_with('-') {
        s.pop();
    }
    s
}

/// The directory part of a stored path (`{topdir}/a/b.code.iter.md` →
/// `{topdir}/a`); `""` when there is none.
pub fn dir_of(path: &str) -> String {
    let p = path.trim_end_matches('/');
    match p.rfind('/') {
        Some(i) => p[..i].to_string(),
        None => String::new(),
    }
}

/// The last path segment.
pub fn file_name_of(path: &str) -> String {
    path.trim_end_matches('/').rsplit('/').next().unwrap_or("").to_string()
}

/// `{thisfilestem}`: the filename minus `.iter.md` and minus the type segment
/// (`mylib.code.iter.md` → `mylib`, `code.iter.md` → `""`). Accepts a path.
pub fn stem_of(filename: &str) -> String {
    let base = file_name_of(filename);
    let Some(stem) = base.strip_suffix(".iter.md") else {
        return base.strip_suffix(".md").unwrap_or(&base).to_string();
    };
    if type_of(&base).is_none() {
        return stem.to_string();
    }
    match stem.rfind('.') {
        Some(i) => stem[..i].to_string(),
        None => String::new(),
    }
}

fn join(dir: &str, rest: &str) -> String {
    if dir.is_empty() { rest.to_string() } else { format!("{}/{}", dir, rest) }
}

/// Collapse `//`, `.` and `..` (never above a leading `{topdir}` or `/`).
/// A trailing `/` (directory marker) is kept.
fn normalize(p: &str) -> String {
    let trailing = p.ends_with('/') && p.len() > 1;
    let absolute = p.starts_with('/');
    let mut parts: Vec<&str> = Vec::new();
    for seg in p.split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                match parts.last() {
                    Some(&last) if last != ".." && last != "{topdir}" => {
                        parts.pop();
                    }
                    Some(&"{topdir}") => {} // never climb out of the project
                    _ if absolute => {}
                    _ => parts.push(".."),
                }
            }
            s => parts.push(s),
        }
    }
    let mut out = parts.join("/");
    if absolute {
        out.insert(0, '/');
    }
    if trailing && !out.ends_with('/') {
        out.push('/');
    }
    out
}

/// Expand one children entry for the file at `this_path`: `{thisfiledir}`,
/// `{thisfilename}`, `{thisfilestem}`, `{projectname}` (when given) are
/// substituted, `{topdir}` is kept (stored paths start with it); a relative
/// entry (no leading placeholder, `/` or `~`) is taken relative to the file's
/// directory; `.`/`..`/`//` are normalised.
pub fn expand_entry(entry: &str, this_path: &str, projectname: Option<&str>) -> String {
    let dir = dir_of(this_path);
    let fname = file_name_of(this_path);
    let mut s = entry.trim().to_string();
    s = s.replace("{thisfiledir}", &dir);
    s = s.replace("{thisfilename}", &fname);
    s = s.replace("{thisfilestem}", &stem_of(&fname));
    if let Some(pn) = projectname {
        s = s.replace("{projectname}", pn);
    }
    let anchored = s.starts_with("{topdir}") || s.starts_with('/') || s.starts_with('~') || entry.trim().starts_with('{');
    if !anchored {
        s = join(&dir, &s);
    }
    normalize(&s)
}

fn is_glob(p: &str) -> bool {
    p.contains('*') || p.contains('?') || p.contains('[')
}

/// Resolve one children entry against the project's node paths: a glob
/// matches path-wise (`*` stays inside a segment, `**` spans directories,
/// `a/**/b` also matches `a/b`); a plain path matches itself, or — when it
/// names a directory — every node path under it. Only paths from
/// `node_paths` are returned (sorted, deduped).
pub fn resolve(entry: &str, this_path: &str, node_paths: &[String]) -> Vec<String> {
    resolve_in(entry, this_path, node_paths, None)
}

/// [`resolve`] with `{projectname}` known.
pub fn resolve_in(entry: &str, this_path: &str, node_paths: &[String], projectname: Option<&str>) -> Vec<String> {
    if entry.trim().is_empty() {
        return Vec::new();
    }
    let p = expand_entry(entry, this_path, projectname);
    let mut out: Vec<String> = Vec::new();
    let pattern = if is_glob(&p) { glob::Pattern::new(&p).ok() } else { None };
    if let Some(pat) = pattern {
        let opts = glob::MatchOptions {
            case_sensitive: true,
            require_literal_separator: true,
            require_literal_leading_dot: false,
        };
        for np in node_paths {
            if pat.matches_with(np, opts) {
                out.push(np.clone());
            }
        }
    } else {
        let exact = p.trim_end_matches('/');
        let prefix = format!("{}/", exact);
        for np in node_paths {
            if np == exact || np.starts_with(&prefix) {
                out.push(np.clone());
            }
        }
    }
    out.sort();
    out.dedup();
    out
}

/// Is `entry` a pattern that can match more than one path (glob or
/// directory) rather than naming exactly `target`?
pub(crate) fn names_exactly(entry: &str, this_path: &str, target: &str) -> bool {
    let p = expand_entry(entry, this_path, None);
    !is_glob(&p) && p.trim_end_matches('/') == target
}

/// The first file of type `t` that `parent.children.reqs` names among
/// `existing_paths` (sorted), if any.
fn attached_of_type(parent: &NodeDoc, t: NodeType, existing_paths: &HashSet<String>) -> Option<String> {
    if parent.children.reqs.is_empty() || existing_paths.is_empty() {
        return None;
    }
    let mut paths: Vec<String> = existing_paths.iter().filter(|p| type_of(p) == Some(t)).cloned().collect();
    paths.sort();
    parent
        .children
        .reqs
        .iter()
        .flat_map(|e| resolve(e, &parent.path, &paths))
        .min()
}

/// Where a new node's file goes (§2.5), unique against `existing_paths` —
/// except a bizreq / techreq attached to a code node or the project, which
/// has exactly one file per attachment point (§2.8): that path is returned
/// even when it exists (the requirement goes into the existing file).
pub fn plan_path(
    doc: &NodeDoc,
    parent: Option<&NodeDoc>,
    attach: Attach,
    existing_paths: &HashSet<String>,
    naming: Naming,
) -> String {
    let t = doc.nodetype;
    let base = {
        let s = slug(&doc.name);
        if s.is_empty() { t.as_str().to_string() } else { s }
    };
    let ext = format!("{}.iter.md", t.as_str());
    let parent = match attach {
        Attach::Root => None,
        Attach::Under(_) => parent,
    };
    let parent_dir = parent.map(|p| dir_of(&p.path)).filter(|d| !d.is_empty());

    // bizreq / techreq (§2.8): ONE file of each type per attachment point,
    // named after it — the parent's already-attached file of this type when it
    // has one, else `<nodedir>/reqs/<nodeslug>.<type>.iter.md` (code node) or
    // `{topdir}/global/requirements/<projectslug>.<type>.iter.md` (project).
    if matches!(t, NodeType::Bizreq | NodeType::Techreq) {
        if let Some(p) = parent.filter(|p| matches!(p.nodetype, NodeType::Code | NodeType::Project)) {
            if let Some(found) = attached_of_type(p, t, existing_paths) {
                return found;
            }
            let owner = {
                let s = stem_of(&p.path);
                let s = if s.is_empty() { slug(&p.name) } else { slug(&s) };
                if s.is_empty() { p.nodetype.as_str().to_string() } else { s }
            };
            return match (p.nodetype, &parent_dir) {
                (NodeType::Code, Some(d)) => format!("{}/reqs/{}.{}", d, owner, ext),
                _ => format!("{{topdir}}/global/requirements/{}.{}", owner, ext),
            };
        }
    }

    // Layout: Some(dir) = "<dir>/<slug>.<type>.iter.md";
    //         nested(dir) = "<dir>/<slug>/<slug>.<type>.iter.md"
    enum Layout {
        Flat(String),
        Nested(String),
    }
    let layout = match t {
        NodeType::Project => Layout::Flat("{topdir}/global".into()),
        NodeType::Usecase | NodeType::Actor => Layout::Flat("{topdir}/global/usecases".into()),
        NodeType::Bizreq | NodeType::Techreq | NodeType::Philosophy => match (parent, &parent_dir) {
            (Some(p), Some(d)) if p.nodetype != NodeType::Project => Layout::Flat(format!("{}/reqs", d)),
            _ => Layout::Flat("{topdir}/global/requirements".into()),
        },
        NodeType::Code if doc.is_connection() => Layout::Flat("{topdir}/global/connections".into()),
        NodeType::Code => match (parent, &parent_dir, attach) {
            (Some(p), Some(d), Attach::Under(k))
                if p.nodetype == NodeType::Code
                    && !p.is_connection()
                    && !matches!(k, EdgeKind::Supplies | EdgeKind::Connects) =>
            {
                Layout::Nested(d.clone())
            }
            _ => Layout::Nested("{topdir}/src".into()),
        },
        NodeType::Test | NodeType::Agentmem => match &parent_dir {
            Some(d) => Layout::Flat(d.clone()),
            None if t == NodeType::Test => Layout::Flat("{topdir}/global/tests".into()),
            None => Layout::Flat("{topdir}".into()),
        },
    };
    let candidate = |s: &str| -> (String, Option<String>) {
        match &layout {
            Layout::Flat(d) => (format!("{}/{}.{}", d, s, ext), None),
            Layout::Nested(d) => (format!("{}/{}/{}.{}", d, s, s, ext), Some(format!("{}/{}/", d, s))),
        }
    };
    let taken = |s: &str| -> bool {
        let (path, dir) = candidate(s);
        existing_paths.contains(&path)
            || dir.is_some_and(|d| existing_paths.iter().any(|e| e.starts_with(&d)))
    };
    if !taken(&base) {
        return candidate(&base).0;
    }
    let stem = match naming {
        Naming::Uuid12 => {
            let id: String = doc.id.chars().filter(|c| *c != '-').collect();
            let tail: String = id.chars().rev().take(12).collect::<Vec<_>>().into_iter().rev().collect();
            let s = format!("{}_{}", base, tail);
            if !taken(&s) {
                return candidate(&s).0;
            }
            s
        }
        Naming::Sequence => base,
    };
    for n in 1.. {
        let s = format!("{}{:02}", stem, n);
        if !taken(&s) {
            return candidate(&s).0;
        }
    }
    unreachable!()
}
