//! Prompt assembly: spin-up = agent body + shared rules + capability index +
//! the project's global requirements (+ the philosophy sentence) + source
//! instructions + the work item block + previous attempt + the item's node
//! context (iter5 spec §8, `load_context`); then the turn sequence
//! prework(prose) → mainwork → postwork(prose) → self-check, all in ONE
//! provider session.  Nothing here is inlined file content: the agent is told
//! which files to read.

use iter_core::nodefile::{self, NodeDoc, NodeType};
use iter_core::WorkItem;
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// Told in every work-item block (CR 2026-09-25 6.6): an orphaned loop an
/// agent started to renew its own locks held four P0 items for 5 h 43 min.
pub const LOCKS_SENTENCE: &str = "Locks: the engine keeps every lock this run holds alive while the run lasts, and releases all of them when it ends, including any you took yourself with `locks/acquire`. Do not renew locks, and do not leave any process running after your turn ends; a lock request made after this run has ended is refused.";

pub const CRITREVIEW_MAX_ROUNDS: u32 = 3;
const PREV_OUTPUT_TAIL_CHARS: usize = 4000;

/// The tooling rows the engine needs, by kind.
#[derive(Default, Clone)]
pub struct Tooling {
    pub shared: String,
    /// name -> desc
    pub capabilities: BTreeMap<String, String>,
    /// name -> body (user | agent | error)
    pub sources: BTreeMap<String, String>,
    /// name -> body (prose pre/postwork steps)
    pub prepost: BTreeMap<String, String>,
}

impl Tooling {
    pub fn from_rows(rows: &[Value]) -> Self {
        let mut t = Tooling::default();
        let s = |v: &Value, k: &str| v.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string();
        for r in rows {
            match s(r, "kind").as_str() {
                "shared" => t.shared = s(r, "body"),
                "capability" => {
                    t.capabilities.insert(iter_core::settings::record_id(r), s(r, "desc"));
                }
                "source" => {
                    t.sources.insert(iter_core::settings::record_id(r), s(r, "body"));
                }
                "prepost" => {
                    t.prepost.insert(iter_core::settings::record_id(r), s(r, "body"));
                }
                _ => {}
            }
        }
        t
    }
}

pub fn expand_topdir_token(pattern: &str, topdir: &Path) -> String {
    let top = iter_core::platform::slash(topdir).trim_end_matches('/').to_string();
    let mut p = pattern.replace("{topdir}/", &format!("{top}/")).replace("{topdir}", &top);
    if let Some(rest) = p.strip_prefix("~/") {
        if let Some(home) = iter_core::platform::home_dir() {
            p = format!("{}/{rest}", iter_core::platform::slash(&home));
        }
    }
    while p.contains("//") {
        p = p.replace("//", "/");
    }
    p
}

/// Resolve one pattern (absolute, {topdir}-relative, relative, glob) to files.
fn resolve_files(pattern: &str, topdir: &Path) -> Vec<PathBuf> {
    let p = expand_topdir_token(pattern, topdir);
    let abs = if Path::new(&p).is_absolute() { p } else { iter_core::platform::slash(&topdir.join(&p)) };
    let mut out = Vec::new();
    if abs.contains('*') || abs.contains('?') || abs.contains('[') {
        if let Ok(paths) = glob::glob(&abs) {
            for e in paths.flatten() {
                if e.is_file() {
                    out.push(e);
                }
            }
        }
    } else {
        let path = PathBuf::from(&abs);
        if path.is_file() {
            out.push(path);
        }
    }
    out
}

/// `*.agentmem.iter.md` (iter5) or the legacy `*.agentmemory.iter.md`: the
/// briefing the last agent on a codepath left for the next one.  Never a
/// graph node.
pub fn is_agentmemory(p: &Path) -> bool {
    p.file_name()
        .map(|n| {
            let n = n.to_string_lossy();
            n == "agentmem.iter.md" || n.ends_with(".agentmem.iter.md") || n == "agentmemory.iter.md" || n.ends_with(".agentmemory.iter.md")
        })
        .unwrap_or(false)
}

/// The one memory file a codepath keeps: `<codepath>/<basename>.agentmem.iter.md`.
pub fn agentmemory_path(codepath: &Path) -> PathBuf {
    let base = codepath.file_name().map(|n| n.to_string_lossy().into_owned()).filter(|b| !b.is_empty()).unwrap_or_else(|| "project".into());
    codepath.join(format!("{base}.agentmem.iter.md"))
}

/// Existing memory files in the directory itself (not ancestors).
pub fn agentmemory_files(dir: &Path) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = std::fs::read_dir(dir)
        .map(|rd| rd.flatten().map(|e| e.path()).filter(|p| p.is_file() && is_agentmemory(p)).collect())
        .unwrap_or_default();
    v.sort();
    v
}

/// Hard cap on a memory file — bigger than this and it costs more context
/// than the re-orientation it saves.
pub const AGENTMEMORY_MAX_BYTES: usize = 2048;

/// The "agentmemory" turn (decided 2026-09-08): after the mainwork and before
/// the self-check, the agent refreshes the codepath's ONE memory file.
pub fn agentmemory_prompt(codepath: &Path) -> String {
    let file = agentmemory_path(codepath);
    format!(
        "Refresh the agent memory file for this codepath: `{}`.\n\n\
         This file is the briefing the NEXT agent working in `{}` reads before anything else, so it \
         pays orientation once instead of every item. Overwrite the whole file (never append a log); \
         keep it under {} bytes — shorter is better. Plain sentences, no headings deeper than `##`. Contents, in this order:\n\
         1. `# Agent memory: <codepath>` — one line on what this codepath is and does.\n\
         2. `## Where things live` — the 3–8 files or directories a newcomer actually needs, one line each.\n\
         3. `## Build and test` — the exact commands that work here (build, the test labels to run) and how long they take.\n\
         4. `## Gotchas` — traps you hit or know of: env words, ordering, flaky dependencies, files that must move together.\n\
         5. `## Recent changes` — at most FIVE entries, newest first, one line each: `<date> <item id short> — <what changed and why>`; drop the oldest when adding.\n\n\
         If the file exists, read it, correct anything now wrong, and rewrite it. Write nothing sensitive (no tokens, no credentials). \
         This is the only file this step touches; do not change code or tests here. If the codepath directory does not exist or you cannot write there, say so and skip.",
        file.display(),
        codepath.display(),
        AGENTMEMORY_MAX_BYTES
    )
}

// ---- agent context (iter5 spec §8) ---------------------------------------

/// The philosophy guidance sentence every agent gets with its requirements.
pub const PHILOSOPHY_SENTENCE: &str = "Use the philosophy file(s) to infer missing requirements and settle conflicts.";

/// One node (or plain requirement document) named to the agent: what it is
/// and where to read it.  `path` is absolute.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct CtxEntry {
    pub id: String,
    pub name: String,
    pub desc: String,
    pub path: String,
    /// node type ("code", "test", "bizreq", …; "doc" for a non-node file)
    pub nodetype: String,
    /// how it relates to the item's node: an edge kind for children
    /// (codenodes, tests, reqs, uses, supplies, connects, drives, touches),
    /// "global" / "local" for requirements
    pub kind: String,
}

/// What an agent is told to read for one work item (spec §8): the item's
/// node file, its children as `[id, name, desc, path]`, the global
/// requirements (project node `children.reqs`, philosophy included) and the
/// local ones (this node's and its ancestors'), and the codepath's agent memory.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct NodeContext {
    pub node: Option<CtxEntry>,
    pub children: Vec<CtxEntry>,
    pub global_reqs: Vec<CtxEntry>,
    pub local_reqs: Vec<CtxEntry>,
    /// requirements of the node's descendants (code nodes below it, and any
    /// reqs/ folder under its directory): listed, read only when the change
    /// affects that part
    pub descendant_reqs: Vec<CtxEntry>,
    pub agentmem: Vec<PathBuf>,
    /// where the nodes came from: "graph" (iter_data) or "files" (the checkout)
    pub source: String,
    pub warnings: Vec<String>,
}

impl NodeContext {
    pub fn has_philosophy(&self) -> bool {
        self.global_reqs.iter().chain(self.local_reqs.iter()).any(|r| r.nodetype == "philosophy")
    }

    /// Every requirement file, global then local (ITER_CONTEXT_FILES).
    pub fn req_paths(&self) -> Vec<String> {
        let mut v: Vec<String> = Vec::new();
        for r in self.global_reqs.iter().chain(self.local_reqs.iter()) {
            if !v.contains(&r.path) {
                v.push(r.path.clone());
            }
        }
        v
    }
}

/// "{topdir}/a/b" -> "<topdir>/a/b".
fn abs_of(path: &str, topdir: &Path) -> String {
    expand_topdir_token(path, topdir)
}

/// "<topdir>/a/b" -> "{topdir}/a/b" (paths inside the checkout only).
fn topdir_form(path: &str, topdir: &Path) -> String {
    if path.starts_with("{topdir}") {
        return path.trim_end_matches('/').to_string();
    }
    let top = iter_core::platform::slash(topdir).trim_end_matches('/').to_string();
    match path.strip_prefix(&top) {
        Some(rest) if rest.is_empty() || rest.starts_with('/') => format!("{{topdir}}{}", rest.trim_end_matches('/')),
        _ => path.trim_end_matches('/').to_string(),
    }
}

fn entry_of(d: &NodeDoc, kind: &str, topdir: &Path) -> CtxEntry {
    CtxEntry {
        id: d.id.clone(),
        name: d.name.clone(),
        desc: d.desc.clone(),
        path: abs_of(&d.path, topdir),
        nodetype: if d.is_connection() { "connection".into() } else { d.nodetype.as_str().to_string() },
        kind: kind.to_string(),
    }
}

/// The node owning `lockdir` ("{topdir}/…"): the code node whose
/// `children.codedirs` (default: its own directory) covers it most closely.
fn owner_of(nodes: &[NodeDoc], lockdir: &str) -> Option<usize> {
    let lock = lockdir.trim_end_matches('/');
    let mut best: Option<(usize, usize)> = None; // (index, matched base length)
    for (i, d) in nodes.iter().enumerate() {
        if d.nodetype != NodeType::Code {
            continue;
        }
        let entries: Vec<String> = if d.children.codedirs.is_empty() { vec!["{thisfiledir}".into()] } else { d.children.codedirs.clone() };
        for e in entries {
            let x = nodefile::expand_entry(&e, &d.path, None);
            let base = x.split(['*', '?', '[']).next().unwrap_or("").trim_end_matches('/').to_string();
            if base.is_empty() {
                continue;
            }
            let covers = lock == base || lock.starts_with(&format!("{base}/"));
            if covers && best.map(|(_, l)| base.len() > l).unwrap_or(true) {
                best = Some((i, base.len()));
            }
        }
    }
    best.map(|(i, _)| i)
}

/// Non-node requirement documents a `children.reqs` entry names on disk
/// (a file, a directory's files, or a glob), `.iter.md` files excluded —
/// those are nodes or plain context docs the graph already resolved.
fn req_docs_on_disk(entry: &str, this_path: &str, topdir: &Path) -> Vec<PathBuf> {
    let x = nodefile::expand_entry(entry, this_path, None);
    let abs = abs_of(&x, topdir);
    let mut out: Vec<PathBuf> = Vec::new();
    let p = Path::new(&abs);
    if abs.contains('*') || abs.contains('?') || abs.contains('[') {
        if let Ok(paths) = glob::glob(&abs) {
            out.extend(paths.flatten().filter(|e| e.is_file()));
        }
    } else if p.is_file() {
        out.push(p.to_path_buf());
    } else if p.is_dir() {
        let mut stack = vec![p.to_path_buf()];
        while let Some(d) = stack.pop() {
            for e in std::fs::read_dir(&d).into_iter().flatten().flatten() {
                let path = e.path();
                if path.is_dir() {
                    if !path.file_name().map(|n| iter_local::is_skip_dir(&n.to_string_lossy())).unwrap_or(false) {
                        stack.push(path);
                    }
                } else {
                    out.push(path);
                }
            }
        }
    }
    out.retain(|f| !f.to_string_lossy().ends_with(".iter.md"));
    out.sort();
    out
}

/// The requirement entries one node's `children.reqs` names: req nodes
/// (bizreq / techreq / philosophy) and non-node documents.
fn reqs_of(d: &NodeDoc, nodes: &[NodeDoc], node_paths: &[String], kind: &str, topdir: &Path) -> Vec<CtxEntry> {
    let mut out: Vec<CtxEntry> = Vec::new();
    for e in &d.children.reqs {
        for p in nodefile::resolve(e, &d.path, node_paths) {
            if let Some(n) = nodes.iter().find(|n| n.path == p && n.nodetype.is_req()) {
                out.push(entry_of(n, kind, topdir));
            }
        }
        for f in req_docs_on_disk(e, &d.path, topdir) {
            let name = f.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            out.push(CtxEntry { name, path: f.to_string_lossy().into_owned(), nodetype: "doc".into(), kind: kind.into(), ..Default::default() });
        }
    }
    out
}

/// The folders a directory keeps its requirement files in.
pub const REQ_FOLDERS: &[&str] = &["reqs", "requirements"];

/// Requirement nodes (bizreq / techreq / philosophy) a directory holds by
/// convention, with no `children.reqs` entry needed: the ones in the
/// directory itself and anywhere under its `reqs/` or `requirements/` folder.
/// `dir` is "{topdir}/…" without a trailing slash.
fn folder_reqs(dir: &str, nodes: &[NodeDoc], kind: &str, topdir: &Path) -> Vec<CtxEntry> {
    let dir = dir.trim_end_matches('/');
    let mut out: Vec<CtxEntry> = Vec::new();
    for n in nodes.iter().filter(|n| n.nodetype.is_req()) {
        let Some(rest) = n.path.strip_prefix(dir).and_then(|r| r.strip_prefix('/')) else { continue };
        let in_dir = !rest.contains('/');
        let in_folder = REQ_FOLDERS.iter().any(|f| rest.starts_with(&format!("{f}/")));
        if in_dir || in_folder {
            out.push(entry_of(n, kind, topdir));
        }
    }
    out.sort_by(|a, b| a.path.cmp(&b.path));
    out
}

/// "{topdir}/a/b/x.code.iter.md" -> "{topdir}/a/b".
fn dir_of_path(path: &str) -> String {
    path.rsplit_once('/').map(|(d, _)| d.to_string()).unwrap_or_default()
}

fn push_unique(list: &mut Vec<CtxEntry>, r: CtxEntry, also_not_in: &[&Vec<CtxEntry>]) {
    if list.iter().any(|l| l.path == r.path) || also_not_in.iter().any(|o| o.iter().any(|x| x.path == r.path)) {
        return;
    }
    list.push(r);
}

/// Build the context from a project's node set (pure; `nodes` carry
/// "{topdir}/…" paths).  The item's node is `item.node` when set, else the
/// code node owning its first lockdir, else the project node.
pub fn build_context(nodes: &[NodeDoc], item: &WorkItem, topdir: &Path, codepath: &Path) -> NodeContext {
    let mut ctx = NodeContext::default();
    let node_paths: Vec<String> = nodes.iter().map(|n| n.path.clone()).collect();
    let by_id: Vec<(String, String)> = nodes.iter().map(|n| (n.id.clone(), n.path.clone())).collect();
    let project_node = nodes.iter().position(|n| n.nodetype == NodeType::Project);

    // 1. the node
    let wanted = item.node.as_deref().map(str::trim).filter(|s| !s.is_empty());
    let idx = match wanted {
        Some(id) => {
            let found = nodes.iter().position(|n| n.id == id);
            if found.is_none() {
                ctx.warnings.push(format!("work item names node {id}, which the project graph does not have"));
            }
            found
        }
        None => None,
    }
    .or_else(|| item.lockdirs.first().and_then(|l| owner_of(nodes, &topdir_form(l, topdir))))
    .or(project_node);
    ctx.node = idx.map(|i| entry_of(&nodes[i], "node", topdir));

    // 2. children: every edge this node's file declares, the other endpoint listed
    if let Some(i) = idx {
        let me = &nodes[i];
        for e in nodefile::edges_of(me, &by_id) {
            let other = if e.from == me.id { &e.to } else { &e.from };
            if let Some(n) = nodes.iter().find(|n| &n.id == other) {
                let entry = entry_of(n, e.kind.as_str(), topdir);
                if !ctx.children.iter().any(|c| c.id == entry.id && c.kind == entry.kind) {
                    ctx.children.push(entry);
                }
            }
        }
    }

    // 3. requirements (spec §8):
    //    global — the project node's `children.reqs`, its own folder's
    //             requirements (global/requirements/), and the checkout
    //             root's reqs/ and requirements/ folders;
    //    local  — this node's folder (its directory and reqs/ / requirements/),
    //             what its file references, then every ancestor directory's
    //             folder up to the root and every code ancestor's references,
    //             nearest first;
    //    descendants — the same for every directory and code node below this
    //             one: listed, read only when the change affects that part.
    let root = "{topdir}".to_string();
    if let Some(p) = project_node {
        let mut g: Vec<CtxEntry> = Vec::new();
        for r in reqs_of(&nodes[p], nodes, &node_paths, "global", topdir)
            .into_iter()
            .chain(folder_reqs(&dir_of_path(&nodes[p].path), nodes, "global", topdir))
            .chain(folder_reqs(&root, nodes, "global", topdir))
        {
            push_unique(&mut g, r, &[]);
        }
        ctx.global_reqs = g;
    } else {
        ctx.global_reqs = folder_reqs(&root, nodes, "global", topdir);
    }
    if let Some(i) = idx.filter(|i| Some(*i) != project_node) {
        let mut local: Vec<CtxEntry> = Vec::new();
        // this node: its folder, then its own references
        let my_dir = dir_of_path(&nodes[i].path);
        for r in folder_reqs(&my_dir, nodes, "local", topdir).into_iter().chain(reqs_of(&nodes[i], nodes, &node_paths, "local", topdir)) {
            push_unique(&mut local, r, &[&ctx.global_reqs]);
        }
        // ancestor directories up to (not including) the root, nearest first
        let mut d = my_dir.clone();
        while let Some((parent, _)) = d.rsplit_once('/') {
            if parent == root || !parent.starts_with(&root) {
                break;
            }
            for r in folder_reqs(parent, nodes, "local", topdir) {
                push_unique(&mut local, r, &[&ctx.global_reqs]);
            }
            d = parent.to_string();
        }
        // code ancestors' references (a parent may live outside the directory chain)
        let parent_of = |child: &NodeDoc| -> Option<usize> {
            nodes.iter().position(|n| {
                n.nodetype != NodeType::Project
                    && n.id != child.id
                    && n.children.codenodes.iter().any(|e| nodefile::resolve(e, &n.path, &node_paths).contains(&child.path))
            })
        };
        let mut seen: Vec<usize> = vec![i];
        let mut cur = parent_of(&nodes[i]);
        while let Some(c) = cur {
            if seen.contains(&c) || Some(c) == project_node {
                break; // a loop in codenodes, or the project (its reqs are global)
            }
            seen.push(c);
            for r in folder_reqs(&dir_of_path(&nodes[c].path), nodes, "local", topdir).into_iter().chain(reqs_of(&nodes[c], nodes, &node_paths, "local", topdir)) {
                push_unique(&mut local, r, &[&ctx.global_reqs]);
            }
            cur = parent_of(&nodes[c]);
        }
        ctx.local_reqs = local;

        // descendants: every requirement under this node's directory that is
        // not its own, and every descendant code node's references
        let mut desc: Vec<CtxEntry> = Vec::new();
        let prefix = format!("{my_dir}/");
        for n in nodes.iter().filter(|n| n.nodetype.is_req() && n.path.starts_with(&prefix)) {
            push_unique(&mut desc, entry_of(n, "descendant", topdir), &[&ctx.global_reqs, &ctx.local_reqs]);
        }
        let mut stack: Vec<usize> = vec![i];
        let mut visited: Vec<usize> = vec![i];
        while let Some(c) = stack.pop() {
            for e in &nodes[c].children.codenodes {
                for p in nodefile::resolve(e, &nodes[c].path, &node_paths) {
                    if let Some(k) = nodes.iter().position(|n| n.path == p && n.nodetype == NodeType::Code) {
                        if !visited.contains(&k) {
                            visited.push(k);
                            stack.push(k);
                            for r in folder_reqs(&dir_of_path(&nodes[k].path), nodes, "descendant", topdir)
                                .into_iter()
                                .chain(reqs_of(&nodes[k], nodes, &node_paths, "descendant", topdir))
                            {
                                push_unique(&mut desc, r, &[&ctx.global_reqs, &ctx.local_reqs]);
                            }
                        }
                    }
                }
            }
        }
        desc.sort_by(|a, b| a.path.cmp(&b.path));
        ctx.descendant_reqs = desc;
    }

    // 5. agent memory: the codepath's (where the agentmemory turn writes),
    //    then the node directory's when that is somewhere else
    ctx.agentmem = agentmemory_files(codepath);
    if let Some(n) = &ctx.node {
        if let Some(dir) = Path::new(&n.path).parent() {
            if dir != codepath {
                for f in agentmemory_files(dir) {
                    if !ctx.agentmem.contains(&f) {
                        ctx.agentmem.push(f);
                    }
                }
            }
        }
    }
    ctx
}

/// The project's nodes as iter_data's graph holds them
/// (`GET /api/projects/{p}/graph`); None when the graph is unavailable or empty.
pub fn nodes_from_graph(api: &crate::client::Api, project: &str) -> Option<Vec<NodeDoc>> {
    let v = api.get(&format!("/api/projects/{project}/graph")).ok()?;
    let arr = v.get("nodes").and_then(|n| n.as_array()).cloned().or_else(|| v.as_array().cloned())?;
    let nodes: Vec<NodeDoc> = arr
        .into_iter()
        .filter(|n| !n.get("deleted").and_then(|d| d.as_bool()).unwrap_or(false))
        .filter_map(|n| serde_json::from_value::<NodeDoc>(n).ok())
        .filter(|n| !n.path.is_empty())
        .collect();
    (!nodes.is_empty()).then_some(nodes)
}

/// The project's nodes read from the checkout: every synced `*.iter.md`
/// under topdir (the walks' skip list, git-ignored files dropped), parsed
/// tolerantly.
pub fn nodes_from_files(topdir: &Path) -> Vec<NodeDoc> {
    let mut files: Vec<PathBuf> = Vec::new();
    let mut stack = vec![topdir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d).into_iter().flatten().flatten() {
            let path = e.path();
            let name = e.file_name().to_string_lossy().into_owned();
            if path.is_dir() {
                if !iter_local::is_skip_dir(&name) && !name.starts_with('.') {
                    stack.push(path);
                }
            } else if nodefile::type_of(&name).map(nodefile::is_synced).unwrap_or(false) {
                files.push(path);
            }
            if files.len() > 50_000 {
                break;
            }
        }
    }
    let files = iter_local::drop_git_ignored(topdir, files);
    let top = iter_core::platform::slash(topdir).trim_end_matches('/').to_string();
    let mut out: Vec<NodeDoc> = Vec::new();
    for f in files {
        let Ok(text) = std::fs::read_to_string(&f) else { continue };
        let fs = iter_core::platform::slash(&f);
        let rel = fs.strip_prefix(&top).unwrap_or(&fs).trim_start_matches('/');
        if let Ok((doc, _)) = nodefile::parse_tolerant(&format!("{{topdir}}/{rel}"), &text) {
            out.push(doc);
        }
    }
    out.sort_by(|a, b| a.path.cmp(&b.path));
    out
}

/// `load_context` (spec §8): the item's node context from iter_data's
/// project graph, or — when the graph has nothing — from the checkout's
/// node files.
pub fn load_context(api: &crate::client::Api, project: &str, topdir: &Path, item: &WorkItem, codepath: &Path) -> NodeContext {
    let (nodes, source) = match nodes_from_graph(api, project) {
        Some(n) => (n, "graph"),
        None => (nodes_from_files(topdir), "files"),
    };
    let mut ctx = build_context(&nodes, item, topdir, codepath);
    ctx.source = source.into();
    ctx
}

fn entry_line(e: &CtxEntry) -> String {
    let id = if e.id.is_empty() { String::new() } else { format!("[{}] ", e.id) };
    let name = if e.name.is_empty() { String::new() } else { format!("{} ", e.name) };
    let desc = if e.desc.trim().is_empty() { String::new() } else { format!(" — {}", e.desc.trim().replace('\n', " ")) };
    format!("- {id}{name}({}) {}{desc}\n", e.nodetype, e.path)
}

/// The stable part (same for every item of the project): global requirements
/// and the philosophy sentence.
pub fn global_context_section(ctx: &NodeContext) -> String {
    if ctx.global_reqs.is_empty() {
        return String::new();
    }
    let mut s = String::from("\n\n# Project requirements (global — they always apply; read what bears on your item)\n");
    for r in &ctx.global_reqs {
        s.push_str(&entry_line(r));
    }
    if ctx.has_philosophy() {
        s.push_str(PHILOSOPHY_SENTENCE);
        s.push('\n');
    }
    s
}

/// The per-item part: the node file, its children, the local requirements.
pub fn node_context_section(ctx: &NodeContext) -> String {
    let mut s = String::new();
    if let Some(n) = &ctx.node {
        s.push_str(&format!(
            "\n# Your node\nRead this whole file first — it is the part of the system this item works on:\n- [{}] {} ({}) {}\n",
            n.id, n.name, n.nodetype, n.path
        ));
        if !n.desc.trim().is_empty() {
            s.push_str(&format!("  {}\n", n.desc.trim().replace('\n', " ")));
        }
    }
    if !ctx.children.is_empty() {
        s.push_str("\n# Its children ([id] name (type) path — desc, by edge kind)\nRead the ones that matter for this item; skip the rest:\n");
        let mut kinds: Vec<&str> = ctx.children.iter().map(|c| c.kind.as_str()).collect();
        kinds.dedup();
        let mut done: Vec<&str> = Vec::new();
        for k in kinds {
            if done.contains(&k) {
                continue;
            }
            done.push(k);
            s.push_str(&format!("{k}:\n"));
            for c in ctx.children.iter().filter(|c| c.kind == k) {
                s.push_str(&entry_line(c));
            }
        }
    }
    if !ctx.local_reqs.is_empty() {
        s.push_str("\n# Local requirements (this node and its ancestors — they always apply)\n");
        for r in &ctx.local_reqs {
            s.push_str(&entry_line(r));
        }
        if ctx.has_philosophy() && ctx.global_reqs.iter().all(|r| r.nodetype != "philosophy") {
            s.push_str(PHILOSOPHY_SENTENCE);
            s.push('\n');
        }
    }
    if !ctx.descendant_reqs.is_empty() {
        s.push_str("\n# Requirements of the parts below this node (read only the ones your change affects)\n");
        for r in &ctx.descendant_reqs {
            s.push_str(&entry_line(r));
        }
    }
    s
}

/// The item's own context patterns (`item.context`), resolved to files.
/// The iter4 placeholders (`{marker}`, `{ancestor_markers}`, `{interfaces}`)
/// are retired: the node context replaces them.
pub fn resolve_context(item: &WorkItem, codepath: &Path, topdir: &Path) -> (Vec<PathBuf>, Vec<String>) {
    let mut files = Vec::new();
    let mut warnings = Vec::new();
    for pat in &item.context {
        match pat.trim() {
            "" | "{marker}" | "{ancestor_markers}" | "{interfaces}" => {}
            p => {
                let p = p.replace("{codepath}", &codepath.to_string_lossy());
                let hits = resolve_files(&p, topdir);
                if hits.is_empty() {
                    warnings.push(format!("context pattern matched nothing: {pat}"));
                }
                files.extend(hits);
            }
        }
    }
    files.sort();
    files.dedup();
    (files, warnings)
}

/// "user" | "agent:<type>" | "error" | anything else (a username) -> user.
pub fn source_instructions(tooling: &Tooling, requestedby: &str, createdby_agent: &str) -> Option<String> {
    let rb = requestedby.trim();
    let (key, agent_type) = if rb == "error" {
        ("error", String::new())
    } else if let Some(rest) = rb.strip_prefix("agent") {
        let t = rest.trim_start_matches(':').trim().to_string();
        ("agent", if t.is_empty() { createdby_agent.to_string() } else { t })
    } else if !createdby_agent.is_empty() && rb.is_empty() {
        ("agent", createdby_agent.to_string())
    } else {
        ("user", String::new())
    };
    let text = tooling.sources.get(key)?;
    Some(if key == "agent" { text.replace("{type}", &agent_type) } else { text.clone() })
}

pub struct SpinupInput<'a> {
    pub agent_body: &'a str,
    pub tooling: &'a Tooling,
    /// the item's node context (`load_context`)
    pub ctx: &'a NodeContext,
    pub item: &'a WorkItem,
    pub codepath: &'a Path,
    pub topdir: &'a Path,
    pub requestedby: &'a str,
    pub createdby_agent: &'a str,
    pub last_response_tail: &'a str,
    pub close_gate_paragraph: &'a str,
    pub engine: &'a EngineEnv,
}

/// The engine this process runs as, as its agents see it: the shared text's
/// `{engine_name}`, `{engine_os}` and `{engine_os_instructions}` (from the
/// engine's settings node), and ITER_ENGINE / ITER_ENGINE_OS.
#[derive(Debug, Clone, Default)]
pub struct EngineEnv {
    pub name: String,
    pub os: String,
    pub os_instructions: String,
}

static ENGINE_ENV: std::sync::RwLock<EngineEnv> =
    std::sync::RwLock::new(EngineEnv { name: String::new(), os: String::new(), os_instructions: String::new() });

/// Set by the engine loop from its record every tick, so an edit on the
/// settings node reaches the next agent run without a restart.
pub fn set_engine_env(env: EngineEnv) {
    if let Ok(mut e) = ENGINE_ENV.write() {
        *e = env;
    }
}

pub fn engine_env() -> EngineEnv {
    ENGINE_ENV.read().map(|e| e.clone()).unwrap_or_default()
}

/// The shared instructions with their placeholders filled in. Constant for
/// an engine, so the prompt-cache prefix stays byte-identical across items.
pub fn shared_text(shared: &str, engine: &EngineEnv) -> String {
    let or = |v: &str, none: &str| if v.trim().is_empty() { none.to_string() } else { v.trim().to_string() };
    shared
        .replace("{critreview_max_rounds}", &CRITREVIEW_MAX_ROUNDS.to_string())
        .replace("{engine_name}", &or(&engine.name, "(unnamed)"))
        .replace("{engine_os}", &or(&engine.os, "not recorded (the engine's settings node has no operating_system)"))
        .replace("{engine_os_instructions}", &or(&engine.os_instructions, "none recorded"))
}

/// The spin-up text prepended to the first turn.  Assembly order is load-
/// bearing (prompt cache): everything up to "# Source instructions" is
/// byte-identical for every item an agent type runs.
pub fn spinup(inp: &SpinupInput) -> (String, Vec<PathBuf>, Vec<String>) {
    let mut s = String::new();
    s.push_str(inp.agent_body.trim_end());
    if !inp.tooling.shared.trim().is_empty() {
        s.push_str("\n\n# Shared instructions (all agents)\n");
        s.push_str(&shared_text(&inp.tooling.shared, inp.engine));
    }
    if !inp.tooling.capabilities.is_empty() {
        s.push_str("\n\n# Capabilities (read the full doc when you need one: `iter capability <name>`)\n");
        for (name, desc) in &inp.tooling.capabilities {
            s.push_str(&format!("- {name}: {desc}\n"));
        }
    }
    // the project's global requirements: the same for every item of the project
    s.push_str(&global_context_section(inp.ctx));
    s.push_str(inp.close_gate_paragraph);
    // ---- everything below varies per work item ----
    if let Some(src) = source_instructions(inp.tooling, inp.requestedby, inp.createdby_agent) {
        s.push_str("\n\n# Source instructions\n");
        s.push_str(&src);
    }
    s.push_str(&format!(
        "\n\n# Work item\nTitle: {}\nWork item id: {}\nCodepath (your working directory and lock scope): {}\n{LOCKS_SENTENCE}\nPriority: P{}\n",
        inp.item.name,
        inp.item.id,
        inp.codepath.display(),
        inp.item.priority
    ));
    if !inp.item.lasterror.is_empty() {
        s.push_str(&format!("\n# Previous attempt\nThis work item ran before and did not complete. Last error: {}\n", inp.item.lasterror));
        let out = inp.last_response_tail.trim();
        if !out.is_empty() {
            let start = out.char_indices().rev().nth(PREV_OUTPUT_TAIL_CHARS.saturating_sub(1)).map(|(i, _)| i).unwrap_or(0);
            s.push_str(&format!("Partial output of the previous attempt{}:\n{}\n", if start > 0 { " (tail)" } else { "" }, &out[start..]));
        }
    }
    let (item_files, mut warnings) = resolve_context(inp.item, inp.codepath, inp.topdir);
    warnings.extend(inp.ctx.warnings.iter().cloned());
    s.push_str(&agentmemory_section(&inp.ctx.agentmem));
    s.push_str(&node_context_section(inp.ctx));
    if !item_files.is_empty() {
        s.push_str("\n# Context files\nRead each of these before starting:\n");
        for f in &item_files {
            s.push_str(&format!("- {}\n", f.display()));
        }
    }
    (s, item_files, warnings)
}

/// "# Agent memory" — listed BEFORE the context files: the last agent's
/// briefing on this codepath is the cheapest orientation there is.
fn agentmemory_section(mem: &[PathBuf]) -> String {
    if mem.is_empty() {
        return String::new();
    }
    let mut s = String::from("\n# Agent memory — read this FIRST\nThe previous agent on this codepath left a briefing (where things live, how to build and test, gotchas, recent changes). Read it before the context files; trust it as a map, verify before relying on details that could have moved:\n");
    for f in mem {
        s.push_str(&format!("- {}\n", f.display()));
    }
    s
}

/// The item that came BEFORE this one in a chained session (decided
/// 2026-09-08: session continuation for queue neighbours).
pub struct ChainPrev {
    pub sid: String,
    pub prev_id: String,
    pub prev_name: String,
    pub position: u32,
    pub max: u32,
}

/// First-turn text for an item run in an EXISTING session: the shared
/// instructions, capabilities, project context and close gate are already in
/// the conversation, so only the per-item part is sent.
pub fn chain_spinup(inp: &SpinupInput, prev: &ChainPrev) -> (String, Vec<PathBuf>, Vec<String>) {
    let mut s = format!(
        "# Next work item — same session, same codepath ({} of at most {} in this session)\n\
         You just closed work item {} ('{}') and its result has been recorded. This is a DIFFERENT work item \
         with its own request, acceptance and close gate. Everything from the spin-up still applies verbatim: the agent \
         definition, the shared instructions, the capabilities, the project requirements and the close gate. \
         The environment now points at the new item (ITER_WORKID has changed; `iter add|ask|reject|doc` act on it). \
         Keep what you learned about this codepath; re-read any file this item changes rather than trusting your memory of it. \
         Do not redo or undo the previous item's work.\n",
        prev.position, prev.max, &prev.prev_id[..8.min(prev.prev_id.len())], prev.prev_name
    );
    if let Some(src) = source_instructions(inp.tooling, inp.requestedby, inp.createdby_agent) {
        s.push_str("\n# Source instructions\n");
        s.push_str(&src);
    }
    s.push_str(&format!(
        "\n\n# Work item\nTitle: {}\nWork item id: {}\nCodepath (your working directory and lock scope): {}\n{LOCKS_SENTENCE}\nPriority: P{}\n",
        inp.item.name,
        inp.item.id,
        inp.codepath.display(),
        inp.item.priority
    ));
    if !inp.item.lasterror.is_empty() {
        s.push_str(&format!("\n# Previous attempt\nThis work item ran before and did not complete. Last error: {}\n", inp.item.lasterror));
        let out = inp.last_response_tail.trim();
        if !out.is_empty() {
            let start = out.char_indices().rev().nth(PREV_OUTPUT_TAIL_CHARS.saturating_sub(1)).map(|(i, _)| i).unwrap_or(0);
            s.push_str(&format!("Partial output of the previous attempt{}:\n{}\n", if start > 0 { " (tail)" } else { "" }, &out[start..]));
        }
    }
    let (item_files, mut warnings) = resolve_context(inp.item, inp.codepath, inp.topdir);
    warnings.extend(inp.ctx.warnings.iter().cloned());
    s.push_str(&node_context_section(inp.ctx));
    if !item_files.is_empty() {
        s.push_str("\n# Context files\nRead each of these (again, if this item changes them) before starting:\n");
        for f in &item_files {
            s.push_str(&format!("- {}\n", f.display()));
        }
    }
    (s, item_files, warnings)
}

/// Mainwork prompt: the request, preceded by an answered question when the
/// item went through the `question` state (the answer outranks the request).
pub fn mainwork_prompt(request: &str, answered: Option<(String, String)>) -> String {
    match answered {
        Some((q, a)) if !q.trim().is_empty() && !a.trim().is_empty() => format!(
            "# A question on this work item was answered\n\n\
             Before this run, work on this item stopped to ask a human a question.\n\
             The question and the answer are below — the answer is a decision, and it \
             outranks any assumption in the request that follows.\n\n\
             ## Asked\n\n{}\n\n## Answer\n\n{}\n\n\
             Proceed on that answer. If it is ambiguous, or acting on it raises a NEW \
             decision only a human can make, ask again with `iter ask` rather than \
             guessing.\n\n---\n\n{}",
            q.trim(),
            a.trim(),
            request
        ),
        _ => request.to_string(),
    }
}

/// The final turn of every run.
pub fn selfcheck_prompt(agent_body: &str, shared: &str) -> String {
    let shared_section = if shared.trim().is_empty() { String::new() } else { format!("\n\n# Shared instructions (all agents)\n{shared}") };
    format!(
        "Final check: re-read your agent definition below and confirm every instruction was \
         completed for this work item. Report anything unfinished or skipped (each on its own \
         line starting with \"NOT DONE:\"), or confirm all done.\n\n---\n{}{}",
        agent_body, shared_section
    )
}

/// The latest answered question widget the agent has not yet been shown
/// (`surfaced` is set on the row once a mainwork turn carried the answer):
/// (row order, question text, answer text).
pub fn answered_question(details: &[Value]) -> Option<(i64, String, String)> {
    let order = |d: &Value| d.get("order").and_then(|o| o.as_i64()).unwrap_or(0);
    let q = details
        .iter()
        .filter(|d| d.get("key").and_then(|k| k.as_str()) == Some("question"))
        .filter(|d| !d.get("value").and_then(|v| v.get("surfaced")).and_then(|b| b.as_bool()).unwrap_or(false))
        .max_by_key(|d| order(d))?;
    let w = q.get("value")?;
    let title = w.get("title").and_then(|t| t.as_str()).unwrap_or("");
    let detail = w.get("detail").and_then(|t| t.as_str()).unwrap_or("");
    let question = if detail.trim().is_empty() { title.to_string() } else { detail.to_string() };
    let mut answers = Vec::new();
    for f in w.get("fields").and_then(|f| f.as_array()).cloned().unwrap_or_default() {
        let label = f.get("label").or(f.get("key")).and_then(|x| x.as_str()).unwrap_or("");
        let v = f.get("value").cloned().unwrap_or(Value::Null);
        let text = match v {
            Value::String(s) => s,
            Value::Null => String::new(),
            other => other.to_string(),
        };
        if !text.trim().is_empty() {
            answers.push(if label.is_empty() { text } else { format!("{label}: {text}") });
        }
    }
    if answers.is_empty() {
        return None;
    }
    Some((order(q), question, answers.join("\n")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    /// A small v5 project on disk, rendered with the node-file library.
    fn fixture(tag: &str) -> (PathBuf, HashMap<&'static str, NodeDoc>) {
        let top = std::env::temp_dir().join(format!("iter5-ctx-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&top);
        let now = "2026-10-02 12:00:00Z";
        let mut docs: HashMap<&'static str, NodeDoc> = HashMap::new();
        let mk = |t: NodeType, name: &str, rel: &str| {
            let mut d = NodeDoc::new(t, name, "test", now);
            d.path = format!("{{topdir}}/{rel}");
            d.desc = format!("{name} desc");
            d
        };
        let mut proj = mk(NodeType::Project, "Proj", "global/proj.project.iter.md");
        proj.children.reqs = vec!["{topdir}/global/requirements/".into()];
        proj.children.codenodes = vec!["{topdir}/src/app/app.code.iter.md".into()];
        let phil = mk(NodeType::Philosophy, "Core philosophy", "global/requirements/core.philosophy.iter.md");
        let money = mk(NodeType::Bizreq, "Money", "global/requirements/money.bizreq.iter.md");
        let mut app = mk(NodeType::Code, "App", "src/app/app.code.iter.md");
        app.level = Some("context".into());
        app.children.codenodes = vec!["{thisfiledir}/api/api.code.iter.md".into()];
        app.children.reqs = vec!["{thisfiledir}/reqs/".into()];
        let fast = mk(NodeType::Techreq, "Fast", "src/app/reqs/fast.techreq.iter.md");
        let mut api = mk(NodeType::Code, "API", "src/app/api/api.code.iter.md");
        api.level = Some("container".into());
        api.children.tests = vec!["{thisfiledir}/api.test.iter.md".into()];
        api.children.reqs = vec!["{thisfiledir}/reqs/*.iter.md".into()];
        let test = mk(NodeType::Test, "API tests", "src/app/api/api.test.iter.md");
        let auth = mk(NodeType::Bizreq, "Auth", "src/app/api/reqs/auth.bizreq.iter.md");
        for (k, d) in [("proj", proj), ("phil", phil), ("money", money), ("app", app), ("fast", fast), ("api", api), ("test", test), ("auth", auth)] {
            let abs = expand_topdir_token(&d.path, &top);
            std::fs::create_dir_all(Path::new(&abs).parent().unwrap()).unwrap();
            std::fs::write(&abs, nodefile::render(&d)).unwrap();
            docs.insert(k, d);
        }
        std::fs::write(top.join("global/requirements/notes.md"), "plain notes").unwrap();
        std::fs::write(top.join("src/app/api/api.agentmem.iter.md"), "memory").unwrap();
        std::fs::create_dir_all(top.join("target/junk")).unwrap();
        std::fs::write(top.join("target/junk/copy.code.iter.md"), "---\nid: x\n---\n").unwrap();
        (top, docs)
    }

    /// Stephen's layout (spec §8): requirements live in reqs/ folders and
    /// need no `children.reqs` entry. Work on postgres18 (container) gets the
    /// global ones, its own folder, the ancestor data/ folder, and lists the
    /// encryption_extension component's as optional — never another branch's.
    #[test]
    fn requirement_folders_global_self_ancestors_then_descendants() {
        let now = "2026-10-02 00:00:00Z";
        let mk = |t: NodeType, name: &str, rel: &str| {
            let mut d = NodeDoc::new(t, name, "test", now);
            d.path = format!("{{topdir}}/{rel}");
            d
        };
        let mut proj = mk(NodeType::Project, "Proj", "global/proj.project.iter.md");
        proj.children.codenodes = vec!["{topdir}/data/code.iter.md".into(), "{topdir}/web/code.iter.md".into()];
        let mut data = mk(NodeType::Code, "data", "data/code.iter.md");
        data.level = Some("context".into());
        data.children.codenodes = vec!["{thisfiledir}/postgres18/code.iter.md".into()];
        let mut pg = mk(NodeType::Code, "postgres18", "data/postgres18/code.iter.md");
        pg.level = Some("container".into());
        pg.children.codenodes = vec!["{thisfiledir}/encryption_extension/code.iter.md".into()];
        let mut enc = mk(NodeType::Code, "encryption_extension", "data/postgres18/encryption_extension/code.iter.md");
        enc.level = Some("component".into());
        let mut web = mk(NodeType::Code, "web", "web/code.iter.md");
        web.level = Some("context".into());
        let nodes = vec![
            proj, data, pg, enc, web,
            mk(NodeType::Philosophy, "g-phil", "global/requirements/core.philosophy.iter.md"),
            mk(NodeType::Bizreq, "g-biz", "global/requirements/money.bizreq.iter.md"),
            mk(NodeType::Techreq, "data-1", "data/reqs/rule001.techreq.iter.md"),
            mk(NodeType::Bizreq, "data-2", "data/reqs/rule002.bizreq.iter.md"),
            mk(NodeType::Techreq, "pg-1", "data/postgres18/reqs/rule001.techreq.iter.md"),
            mk(NodeType::Bizreq, "pg-2", "data/postgres18/reqs/rule002.bizreq.iter.md"),
            mk(NodeType::Techreq, "enc-1", "data/postgres18/encryption_extension/reqs/rule001.techreq.iter.md"),
            mk(NodeType::Bizreq, "enc-2", "data/postgres18/encryption_extension/requirements/rule002.bizreq.iter.md"),
            mk(NodeType::Techreq, "web-1", "web/reqs/rule001.techreq.iter.md"),
        ];
        let pg_id = nodes[2].id.clone();
        let item = WorkItem { node: Some(pg_id), ..Default::default() };
        let top = std::env::temp_dir().join("iter5-reqfolders");
        let ctx = build_context(&nodes, &item, &top, &top);
        assert_eq!(names(&ctx.global_reqs), vec!["global:g-phil".to_string(), "global:g-biz".into()]);
        assert_eq!(names(&ctx.local_reqs), vec!["local:pg-1".to_string(), "local:pg-2".into(), "local:data-1".into(), "local:data-2".into()],
            "self first, then the ancestor; nothing from web/ or below");
        assert_eq!(names(&ctx.descendant_reqs), vec!["descendant:enc-1".to_string(), "descendant:enc-2".into()]);
        let section = node_context_section(&ctx);
        assert!(section.contains("# Requirements of the parts below this node (read only the ones your change affects)"));
        assert!(ctx.req_paths().iter().all(|p| !p.contains("encryption_extension")), "descendants are offered, not mandatory");
        assert!(ctx.req_paths().iter().all(|p| !p.contains("/web/")));
    }

    fn names(v: &[CtxEntry]) -> Vec<String> {
        v.iter().map(|e| format!("{}:{}", e.kind, if e.name.is_empty() { "?" } else { &e.name })).collect()
    }

    /// §8 from the checkout's files: the node owning the first lockdir, its
    /// children, the global and local requirements, the agent memory.
    #[test]
    fn context_from_files_names_node_children_and_requirements() {
        let (top, docs) = fixture("files");
        let nodes = nodes_from_files(&top);
        assert_eq!(nodes.len(), 8, "target/ is skipped: {:?}", nodes.iter().map(|n| &n.path).collect::<Vec<_>>());
        let item = WorkItem { id: "w1".into(), lockdirs: vec!["{topdir}/src/app/api/handlers".into()], ..Default::default() };
        let codepath = top.join("src/app/api/handlers");
        let ctx = build_context(&nodes, &item, &top, &codepath);
        let node = ctx.node.clone().unwrap();
        assert_eq!((node.id.as_str(), node.nodetype.as_str()), (docs["api"].id.as_str(), "code"));
        assert_eq!(node.path, iter_core::platform::slash(&top.join("src/app/api/api.code.iter.md")));
        assert_eq!(names(&ctx.children), vec!["tests:API tests".to_string(), "reqs:Auth".into()]);
        assert_eq!(ctx.children[0].id, docs["test"].id);
        assert_eq!(names(&ctx.global_reqs), vec!["global:Core philosophy".to_string(), "global:Money".into(), "global:notes.md".into()]);
        assert_eq!(ctx.global_reqs[2].nodetype, "doc");
        assert_eq!(names(&ctx.local_reqs), vec!["local:Auth".to_string(), "local:Fast".into()], "this node's, then the ancestor's");
        assert!(ctx.has_philosophy());
        assert_eq!(ctx.agentmem, vec![top.join("src/app/api/api.agentmem.iter.md")]);
        assert_eq!(ctx.req_paths().len(), 5);

        // item.node wins over the lockdir; an unknown node is a warning
        let item2 = WorkItem { node: Some(docs["app"].id.clone()), ..item.clone() };
        let ctx2 = build_context(&nodes, &item2, &top, &codepath);
        assert_eq!(ctx2.node.as_ref().unwrap().name, "App");
        assert_eq!(names(&ctx2.children), vec!["codenodes:API".to_string(), "reqs:Fast".into()]);
        assert_eq!(names(&ctx2.local_reqs), vec!["local:Fast".to_string()]);
        let item3 = WorkItem { node: Some("nope".into()), lockdirs: vec![], ..Default::default() };
        let ctx3 = build_context(&nodes, &item3, &top, &top);
        assert_eq!(ctx3.node.as_ref().unwrap().nodetype, "project", "no node, no lockdir: the project node");
        assert!(ctx3.warnings[0].contains("does not have"));
        let _ = std::fs::remove_dir_all(&top);
    }

    /// load_context prefers iter_data's graph and falls back to the files;
    /// the spin-up carries the node, its children, both requirement lists and
    /// the philosophy sentence — and no iter4 main.iter.md section.
    #[test]
    fn load_context_reads_the_graph_then_the_files_and_the_spinup_shows_it() {
        let (top, docs) = fixture("graph");
        let graph_nodes: Vec<NodeDoc> = vec![docs["proj"].clone(), docs["api"].clone(), docs["auth"].clone()];
        let body = serde_json::json!({"nodes": graph_nodes, "edges": []});
        let srv = crate::client::fake::serve(move |m, path, _| {
            if m == "GET" && path == "/api/projects/g/graph" { return Some((200, body.clone())); }
            Some((404, serde_json::json!({"error": "no"})))
        });
        let item = WorkItem { id: "w2".into(), name: "fix auth".into(), node: Some(docs["api"].id.clone()), ..Default::default() };
        let ctx = load_context(&srv.api(), "g", &top, &item, &top);
        assert_eq!(ctx.source, "graph");
        assert_eq!(names(&ctx.children), vec!["reqs:Auth".to_string()], "the graph had no test node");
        let ctx_f = load_context(&srv.api(), "other", &top, &item, &top);
        assert_eq!(ctx_f.source, "files");
        assert_eq!(ctx_f.children.len(), 2);

        let tooling = Tooling::default();
        let inp = SpinupInput {
            agent_body: "# code agent", tooling: &tooling, ctx: &ctx_f, item: &item, codepath: &top, topdir: &top,
            requestedby: "", createdby_agent: "", last_response_tail: "", close_gate_paragraph: "",
            engine: &EngineEnv::default(),
        };
        let (s, _, _) = spinup(&inp);
        assert!(s.contains("# Project requirements (global"), "{s}");
        assert!(s.contains(PHILOSOPHY_SENTENCE));
        assert!(s.contains("# Your node") && s.contains(&format!("[{}] API (code)", docs["api"].id)), "{s}");
        assert!(s.contains("tests:\n- [") && s.contains("API tests (test)"), "{s}");
        assert!(s.contains("# Local requirements") && s.contains("Fast (techreq)"), "{s}");
        assert!(!s.contains("ITER_MAINFILE") && !s.contains("globalcontextfiles"));
        // the global section sits before the per-item part (prompt cache)
        assert!(s.find("# Project requirements").unwrap() < s.find("# Work item").unwrap());
        assert!(s.find("# Your node").unwrap() > s.find("# Work item").unwrap());
        let _ = std::fs::remove_dir_all(&top);
    }

    #[test]
    fn shared_text_fills_the_engine_placeholders() {
        let shared = "Engine {engine_name} runs {engine_os}.\nOS notes: {engine_os_instructions}\nRounds: {critreview_max_rounds}";
        let env = EngineEnv { name: "Midoriya".into(), os: "Windows 11".into(), os_instructions: "Git Bash is slow.".into() };
        let s = shared_text(shared, &env);
        assert_eq!(s, format!("Engine Midoriya runs Windows 11.\nOS notes: Git Bash is slow.\nRounds: {CRITREVIEW_MAX_ROUNDS}"));
        // an engine node without the properties still yields readable text
        let s = shared_text(shared, &EngineEnv::default());
        assert!(s.contains("runs not recorded") && s.contains("OS notes: none recorded") && !s.contains('{'), "{s}");
    }

    #[test]
    fn retired_placeholders_resolve_to_nothing() {
        let top = std::env::temp_dir().join(format!("iter5-ctx-ph-{}", std::process::id()));
        std::fs::create_dir_all(&top).unwrap();
        std::fs::write(top.join("extra.md"), "x").unwrap();
        let item = WorkItem { context: vec!["{marker}".into(), "{ancestor_markers}".into(), "{topdir}/extra.md".into(), "{topdir}/none.md".into()], ..Default::default() };
        let (files, warn) = resolve_context(&item, &top, &top);
        assert_eq!(files, vec![top.join("extra.md")]);
        assert_eq!(warn.len(), 1);
        assert!(is_agentmemory(Path::new("/a/x.agentmem.iter.md")) && is_agentmemory(Path::new("/a/x.agentmemory.iter.md")));
        assert_eq!(agentmemory_path(Path::new("/a/api")), PathBuf::from("/a/api/api.agentmem.iter.md"));
        let _ = std::fs::remove_dir_all(&top);
    }

    #[test]
    fn source_and_answered_question() {
        let mut t = Tooling::default();
        t.sources.insert("user".into(), "U".into());
        t.sources.insert("agent".into(), "A {type}".into());
        assert_eq!(source_instructions(&t, "user", "").as_deref(), Some("U"));
        assert_eq!(source_instructions(&t, "agent:plan", "").as_deref(), Some("A plan"));
        assert_eq!(source_instructions(&t, "stephen", "").as_deref(), Some("U"));
        assert_eq!(source_instructions(&t, "", "code").as_deref(), Some("A code"));
        let details = vec![
            serde_json::json!({"order":0,"key":"request","value":"r"}),
            serde_json::json!({"order":1,"key":"question","value":{"title":"Which?","detail":"","fields":[{"key":"answer","label":"Answer","type":"text","value":"B"}]}}),
        ];
        assert_eq!(answered_question(&details), Some((1, "Which?".into(), "Answer: B".into())));
        let mut d2 = details.clone();
        d2[1]["value"]["surfaced"] = serde_json::json!(true);
        assert!(answered_question(&d2).is_none(), "a surfaced answer is history");
    }
}

/// The built-in `explain` persona (spec: Explain / ELI5), used when the
/// project's `explain` agent record has no body of its own.  Kept in sync
/// with the record seeded into pdy-dev on 2026-09-04.
pub const EXPLAIN_DEFAULT_BODY: &str = r#"# Agent Definition: explain

You are the **explain** agent — the "explain it like I'm five" (ELI5) agent. A human
pressed a button on a work item they could not follow. Your only job is to re-explain
that work item so that person understands it on one reading. You change nothing:
no code, no files, no work items — you read, and you write one explanation.

## Who you are writing for

Someone who has NEVER seen this codebase. They know the business — what the product
does for the people who use it — and nothing about how the repository spells it.
Every internal name (a container, a rule id, a gate, a function, a file, a work item
id) is a word they have never met. A bare internal name is a lookup task you have
handed them; gloss every one the first time it appears, in the same sentence.

## How to explain

1. **Start where the business flow is.** Name the moment in the user's or operator's
   journey where this work item matters, in the words a customer or operator would
   use. Only then introduce the internal names, each glossed as it appears.
2. **Say what the work item is asking for, or asking about.** For a question: the one
   decision, restated plainly, then each option in plain terms with what it buys and
   what it costs, then the agent's recommendation if it gave one. For a parked or
   failed item: why it stopped, in plain terms. For finished work: what changed and
   why it mattered.
3. **Use an analogy when the mechanism is genuinely complex.** One analogy, kept
   close to the real thing; drop it as soon as the plain description carries.
4. **Keep it short.** Context is not length. Aim for what fits on one screen: a few
   short paragraphs, or a short list where the item has several parts. No headings
   deeper than one level, no tables of internal names.
5. **Point at the source.** End with one line naming the file(s) a curious reader
   would open to see for themselves (path only, no quoting of code).

Bad: "based on rule ABC, once _potatochip has traversed the cankor gate, should rule
XYZ apply only twice?"
Good: "When a customer asks for a bag of potato chips, we first confirm they have
paid (rule ABC, enforced by the payment container cntr_ABC) and that they have not
typed obscenities into the console (the 'cankor gate', a check run by the Rulebook
container). If they have typed obscenities, should we refuse the chips after one
offense or after two? The instructions in main.bizreq.iter.md do not say."
"#;

pub struct ExplainInput<'a> {
    pub agent_body: &'a str,
    pub ctx: &'a NodeContext,
    pub item: &'a WorkItem,
    pub details: &'a [Value],
    pub codepath: &'a Path,
    pub topdir: &'a Path,
}

/// The single-turn prompt for an ELI5 run: persona, the work item and every
/// human-relevant detail row inline (the reader's whole record), then the
/// files to read for context — the global requirements, the item's node
/// file, its children and local requirements (`load_context`), and the
/// item's own context files.  Read-only is stated up front and enforced by
/// the spawn's tool allow-list.
pub fn explain_prompt(inp: &ExplainInput) -> String {
    let mut s = String::new();
    s.push_str(inp.agent_body.trim_end());
    s.push_str("\n\n# Explain this work item simply (ELI5)\n\n");
    s.push_str("This session is READ-ONLY: you have Read, Glob and Grep and nothing else. Do not try to \
                edit, run commands, or create work items. Your entire output is the explanation; it is \
                appended to the work item as-is for the human to read, so write it for them directly — \
                no preamble about what you are going to do, no notes to the engine.\n\n");
    s.push_str(&format!(
        "## The work item\nTitle: {}\nId: {}\nState: {}\nAgent type: {}\nPriority: P{}\nRequested by: {}\nCreated by: {}\n",
        inp.item.name, inp.item.id, inp.item.state, inp.item.agent, inp.item.priority,
        if inp.item.requestedby.is_empty() { "—" } else { &inp.item.requestedby },
        if inp.item.createdby.is_empty() { "—" } else { &inp.item.createdby },
    ));
    if !inp.item.lasterror.is_empty() {
        s.push_str(&format!("Last error: {}\n", inp.item.lasterror));
    }
    if !inp.item.lockdirs.is_empty() {
        s.push_str(&format!("Codepath(s): {}\n", inp.item.lockdirs.join(", ")));
    }
    s.push_str("\n## Its record (every detail row, oldest first)\n");
    let mut rows: Vec<&Value> = inp.details.iter().collect();
    rows.sort_by_key(|d| d.get("order").and_then(|o| o.as_i64()).unwrap_or(0));
    for d in rows {
        let key = d.get("key").and_then(|k| k.as_str()).unwrap_or("");
        if matches!(key, "spend" | "v2" | "explained") {
            continue; // cost rows, migration leftovers and earlier explanations are not the story
        }
        let by = d.get("by").and_then(|b| b.as_str()).unwrap_or("");
        let ts = d.get("ts").and_then(|t| t.as_str()).unwrap_or("");
        let body = match d.get("value") {
            Some(Value::String(t)) => t.clone(),
            Some(v) if !v.is_null() => serde_json::to_string_pretty(v).unwrap_or_default(),
            _ => String::new(),
        };
        s.push_str(&format!("\n### {key}{}{}\n{}\n", if by.is_empty() { String::new() } else { format!(" (by {by})") },
            if ts.is_empty() { String::new() } else { format!(" — {ts}") }, body.trim()));
    }
    s.push_str("\n## Read these for context before writing (paths, not quotes)\n");
    if !inp.ctx.global_reqs.is_empty() {
        s.push_str("The project's global requirements:\n");
        for r in &inp.ctx.global_reqs {
            s.push_str(&entry_line(r));
        }
    }
    if let Some(n) = &inp.ctx.node {
        s.push_str(&format!("The item's node file: {}\n", n.path));
    }
    if !inp.ctx.children.is_empty() {
        s.push_str("Its children:\n");
        for c in &inp.ctx.children {
            s.push_str(&entry_line(c));
        }
    }
    if !inp.ctx.local_reqs.is_empty() {
        s.push_str("Its local requirements (the node's and its ancestors'):\n");
        for r in &inp.ctx.local_reqs {
            s.push_str(&entry_line(r));
        }
    }
    let (extra, _warnings) = resolve_context(inp.item, inp.codepath, inp.topdir);
    if !extra.is_empty() {
        s.push_str("The work item's own context files:\n");
        for f in &extra {
            s.push_str(&format!("- {}\n", f.display()));
        }
    }
    s.push_str("\nRead what you need from those (and the code they point at) to be sure you understand \
                the item, then write the explanation. Output the explanation only.\n");
    s
}
