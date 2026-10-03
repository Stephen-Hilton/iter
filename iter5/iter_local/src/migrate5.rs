//! `iter migrate5 --from <iter4 checkout> --to <dir> [--dry-run]` (iter5 spec
//! §10): convert an iter4 checkout into iter5 node files. Never in place:
//! the tree (`.git` included, so history is kept) is copied to `--to` and only
//! the copy is changed; `--from` is only read. With `--dry-run` nothing is
//! written anywhere — the conversion runs in memory and the report says what
//! it would do.
//!
//! Conversions, in order:
//! 1. **interfaces** — every `*.interface.iter.md` is removed; one connection
//!    node (`level: connection`) per interface KIND that has any
//!    (request-reply → "API call", event → "Event", stream → "Stream",
//!    dataset → "Shared dataset") at `global/connections/<slug>.code.iter.md`,
//!    `connects.from` = the code nodes that produced an interface of that kind
//!    (their `outputs`), `connects.to` = the ones that consumed one (`inputs`);
//!    the body lists the old contracts by name.
//! 2. **main** — `main.iter.md` → `global/<slug(projectname)>.project.iter.md`
//!    (its `{thisfiledir}` entries made `{topdir}`-absolute; `globalcontextfiles`
//!    → `children.reqs`, `globalscandirs` → `scandirs`; `globalinterfacedir`,
//!    `globalusecasedir`, `actorsfile` dropped).
//! 3. **testgroups** — `*.tests.iter.md` / `*.testgroup.iter.md` →
//!    `*.test.iter.md`; the `iterapp:testgroups` registry block is removed and
//!    every registered script not already matched by `children.tests` is added
//!    (`{thisfiledir}/<shell>`); the last recorded run becomes `last_result`
//!    (standard JSON) + `timestamps.last_tested`; `input_space` / `coverage`
//!    (golden+malformed → normal) / `auto_fix` are kept on the node; a
//!    top-level `testpaths:` tier mapping is kept as `test_tiers`.
//! 4. **requirements** (§2.8) — every bizreq / techreq file becomes part of
//!    ONE file of its type per attachment point, one `## ` section per
//!    requirement (marker with a minted id, status agreed): a file beside a
//!    code node (or in the `reqs/` folder under one) →
//!    `<nodedir>/reqs/<nodeslug>.<type>.iter.md`; one named by `main.iter.md`
//!    (globalcontextfiles / reqs) or at the top →
//!    `global/requirements/<projectslug>.<type>.iter.md`; any other →
//!    `<folder>/reqs/<folderslug>.<type>.iter.md`. A multi-bullet body gives
//!    one section per top-level bullet (only the bold-id ones when two or
//!    more have one; `**KEY**` → the key; the text before the first bullet →
//!    the preamble); a body without bullets gives one section titled with the
//!    file's name; already-sectioned bodies are kept. Several files landing
//!    on one path are merged in path order (the first keeps its file id).
//!    Originals are removed, references follow the move (`{thisfiledir}/reqs/…`
//!    from the owner), and each owner names its file(s) in `children.reqs`.
//! 5. **actors** — the `actorsfile` (else `actors.yaml` at the top or in
//!    `global/`) → one `global/usecases/<slug>.actor.iter.md` per actor;
//!    `touches` = the code nodes that produced the interfaces its `uses`
//!    patterns name. The YAML file itself is left in place.
//! 6. **agent memory** — `*.agentmemory.iter.md` → `*.agentmem.iter.md`
//!    (content untouched).
//! 7. **references** — every children / connects / actors / drives / touches
//!    entry naming a renamed file is rewritten; globs get the new type tags.
//!    Code nodes lose the iter4 runner hints `testgroup:` / `test_dir:`.
//! 8. **conform** — every node file goes through `nodefile::conform`
//!    (creator `iter migrate5`), and the result is checked to be a fixed point.

use crate::testgroups;
use iter_core::nodefile::{self, NodeDoc, NodeType};
use serde::Serialize;
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};

pub const CREATOR: &str = "iter migrate5";

#[derive(Debug, Default, Serialize)]
pub struct Report {
    pub from: String,
    pub to: String,
    pub dry_run: bool,
    /// conversion → count
    pub counts: BTreeMap<String, usize>,
    /// conform findings by code (over the final files)
    pub findings: BTreeMap<String, usize>,
    /// one line per thing a human should look at
    pub notes: Vec<String>,
    /// files whose conformed text is not a fixed point (should be empty)
    pub not_idempotent: Vec<String>,
}

impl Report {
    fn add(&mut self, k: &str, n: usize) {
        *self.counts.entry(k.to_string()).or_default() += n;
    }
    pub fn count(&self, k: &str) -> usize {
        self.counts.get(k).copied().unwrap_or(0)
    }
    pub fn print(&self) {
        println!("iter migrate5 {} → {}{}", self.from, self.to, if self.dry_run { "  (dry run: nothing written)" } else { "" });
        for (k, v) in &self.counts {
            println!("  {k:<34} {v}");
        }
        if !self.findings.is_empty() {
            println!("  conform findings:");
            for (k, v) in &self.findings {
                println!("    {k:<32} {v}");
            }
        }
        for n in &self.notes {
            println!("  note: {n}");
        }
        if !self.not_idempotent.is_empty() {
            println!("  NOT IDEMPOTENT ({}): {}", self.not_idempotent.len(), self.not_idempotent.join(", "));
        }
    }
}

fn stored(rel: &str) -> String {
    format!("{{topdir}}/{rel}")
}

fn rel_of(s: &str) -> &str {
    s.strip_prefix("{topdir}/").unwrap_or(s)
}

fn is_glob(p: &str) -> bool {
    p.contains('*') || p.contains('?') || p.contains('[')
}

/// The frontmatter as JSON (strict YAML only; None when absent / unreadable).
pub fn raw_front(text: &str) -> Option<Value> {
    let t = text.strip_prefix('\u{feff}').unwrap_or(text);
    let rest = t.strip_prefix("---\n").or_else(|| t.strip_prefix("---\r\n"))?;
    let mut front = String::new();
    for line in rest.split_inclusive('\n') {
        let bare = line.trim_end();
        if bare == "---" || bare == "..." {
            let v: serde_yaml::Value = serde_yaml::from_str(&front).ok()?;
            return serde_json::to_value(v).ok();
        }
        front.push_str(line);
    }
    None
}

fn list(v: &Value) -> Vec<String> {
    match v {
        Value::Array(a) => a.iter().filter_map(|x| x.as_str().map(|s| s.trim().to_string())).filter(|s| !s.is_empty()).collect(),
        Value::String(s) if !s.trim().is_empty() => vec![s.trim().to_string()],
        _ => Vec::new(),
    }
}

/// Every `*.iter.md` file (and nothing else) under `from`, relative paths,
/// git-ignored ones left out.
fn iter_files(from: &Path) -> Vec<String> {
    let mut out = Vec::new();
    let mut stack = vec![from.to_path_buf()];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d).into_iter().flatten().flatten() {
            let Ok(ft) = e.file_type() else { continue };
            let name = e.file_name().to_string_lossy().into_owned();
            if ft.is_dir() {
                if !crate::is_skip_dir(&name) {
                    stack.push(e.path());
                }
            } else if ft.is_file() && name.ends_with(".iter.md") {
                out.push(e.path());
            }
        }
    }
    let mut rels: Vec<String> = crate::drop_git_ignored(from, out)
        .into_iter()
        .filter_map(|p| p.strip_prefix(from).ok().map(|r| r.to_string_lossy().replace('\\', "/")))
        .collect();
    rels.sort();
    rels
}

fn kind_name(kind: &str) -> String {
    match kind.trim() {
        "request-reply" | "request_reply" | "rpc" | "api" => "API call".into(),
        "event" | "events" => "Event".into(),
        "stream" => "Stream".into(),
        "dataset" | "data" => "Shared dataset".into(),
        "" => "Connection".into(),
        other => {
            let mut c = other.replace(['-', '_'], " ");
            if let Some(f) = c.get(0..1) {
                c = f.to_uppercase() + &c[1..];
            }
            c
        }
    }
}

fn unique_path(dir: &str, slug: &str, tag: &str, taken: &BTreeSet<String>) -> String {
    let base = if slug.is_empty() { tag.to_string() } else { slug.to_string() };
    let mut cand = format!("{dir}/{base}.{tag}.iter.md");
    let mut n = 1;
    while taken.contains(&cand) {
        cand = format!("{dir}/{base}{n:02}.{tag}.iter.md");
        n += 1;
    }
    cand
}

/// The legacy tag → v5 tag of a filename (`x.testgroup.iter.md` →
/// `x.test.iter.md`, `testgroup.iter.md` → `test.iter.md`, …).
pub fn v5_file_name(name: &str) -> String {
    for (old, new) in [("testgroup", "test"), ("tests", "test"), ("agentmemory", "agentmem")] {
        if let Some(stem) = name.strip_suffix(&format!(".{old}.iter.md")) {
            return format!("{stem}.{new}.iter.md");
        }
        if name == format!("{old}.iter.md") {
            return format!("{new}.iter.md");
        }
    }
    name.to_string()
}

/// Glob entries get the v5 tags too.
fn retag_glob(entry: &str) -> String {
    entry
        .replace(".testgroup.iter.md", ".test.iter.md")
        .replace("testgroup.iter.md", "test.iter.md")
        .replace(".tests.iter.md", ".test.iter.md")
        .replace(".agentmemory.iter.md", ".agentmem.iter.md")
}

/// One top-level bullet of a requirement body.
#[derive(Debug, Clone, PartialEq)]
pub struct Bullet {
    pub text: String,
    pub section: String,
}

/// Top-level `- ` / `* ` bullets (outside code fences) with their
/// continuation lines (indented or blank-then-indented), and the heading
/// they sit under.
pub fn top_level_bullets(body: &str) -> Vec<Bullet> {
    let mut out: Vec<Bullet> = Vec::new();
    let mut section = String::new();
    let mut cur: Option<Bullet> = None;
    let mut fence = false;
    let mut pending_blank = 0usize;
    for line in body.lines() {
        let t = line.trim_start();
        if t.starts_with("```") || t.starts_with("~~~") {
            fence = !fence;
            if let Some(b) = cur.as_mut() {
                for _ in 0..pending_blank {
                    b.text.push('\n');
                }
                pending_blank = 0;
                b.text.push_str(line);
                b.text.push('\n');
            }
            continue;
        }
        if fence {
            if let Some(b) = cur.as_mut() {
                b.text.push_str(line);
                b.text.push('\n');
            }
            continue;
        }
        let top_bullet = line.starts_with("- ") || line.starts_with("* ");
        if top_bullet {
            if let Some(b) = cur.take() {
                out.push(b);
            }
            pending_blank = 0;
            cur = Some(Bullet { text: format!("{}\n", &line[2..]), section: section.clone() });
            continue;
        }
        if line.trim().is_empty() {
            pending_blank += 1;
            continue;
        }
        let indented = line.starts_with(' ') || line.starts_with('\t');
        if indented {
            if let Some(b) = cur.as_mut() {
                for _ in 0..pending_blank {
                    b.text.push('\n');
                }
                b.text.push_str(line.trim_start());
                b.text.push('\n');
                pending_blank = 0;
                continue;
            }
        }
        // anything else at column 0 ends the bullet
        if let Some(b) = cur.take() {
            out.push(b);
        }
        pending_blank = 0;
        if let Some(h) = line.strip_prefix('#') {
            section = h.trim_start_matches('#').trim().to_string();
        }
    }
    if let Some(b) = cur.take() {
        out.push(b);
    }
    for b in out.iter_mut() {
        b.text = b.text.trim_end().to_string();
    }
    out
}

/// Does the bullet open with a bold requirement id (`**PDY-BIZ-001**`,
/// `**R12 …**`: the bold text's first word holds a digit)?
pub fn has_bold_id(text: &str) -> bool {
    let Some(rest) = text.trim_start().strip_prefix("**") else { return false };
    let Some(end) = rest.find("**") else { return false };
    rest[..end].split_whitespace().next().is_some_and(|w| w.chars().any(|c| c.is_ascii_digit()))
}

/// The requirements of a body: when two or more top-level bullets carry a
/// bold id, only those start a requirement — an id-less bullet after one is
/// part of it (a list inside the rationale), one before the first is
/// preamble (kept in the full-text doc only). Without ids every top-level
/// bullet is a requirement.
pub fn requirement_bullets(body: &str) -> Vec<Bullet> {
    let all = top_level_bullets(body);
    if all.iter().filter(|b| has_bold_id(&b.text)).count() < 2 {
        return all;
    }
    let mut out: Vec<Bullet> = Vec::new();
    for b in all {
        if has_bold_id(&b.text) {
            out.push(b);
        } else if let Some(last) = out.last_mut() {
            last.text.push_str("\n- ");
            last.text.push_str(&b.text.replace('\n', "\n  "));
        }
    }
    out
}

/// A bullet's name: its leading `**ID**` (trailing punctuation trimmed), else
/// its first eight words.
pub fn bullet_name(text: &str) -> String {
    let t = text.trim();
    if let Some(rest) = t.strip_prefix("**") {
        if let Some(end) = rest.find("**") {
            let id = rest[..end].trim().trim_end_matches([':', '.', '—', '-', ' ']).trim();
            if !id.is_empty() && id.len() <= 80 {
                return id.to_string();
            }
        }
    }
    let plain: String = t.replace("**", "").replace('`', "");
    plain.split_whitespace().take(8).collect::<Vec<_>>().join(" ").trim_end_matches([',', '.', ':', ';', '—']).to_string()
}

/// Top-level bullet lines (`- ` / `* ` at column 0, outside code fences):
/// (line index, opens with a bold requirement id).
fn bullet_lines(lines: &[&str]) -> Vec<(usize, bool)> {
    let mut out = Vec::new();
    let mut fence = false;
    for (i, l) in lines.iter().enumerate() {
        let t = l.trim_start();
        if t.starts_with("```") || t.starts_with("~~~") {
            fence = !fence;
            continue;
        }
        if !fence && (l.starts_with("- ") || l.starts_with("* ")) {
            out.push((i, has_bold_id(&l[2..])));
        }
    }
    out
}

/// `## ` headings (and `# ` ones too when `h1`; column 0, outside fences)
/// become `### ` so they cannot start a requirement section.
fn demote_headings(lines: &[&str], h1: bool) -> String {
    let mut fence = false;
    let mut out: Vec<String> = Vec::with_capacity(lines.len());
    for l in lines {
        let t = l.trim_start();
        if t.starts_with("```") || t.starts_with("~~~") {
            fence = !fence;
        }
        if !fence {
            if let Some(h) = l.strip_prefix("## ").or_else(|| if h1 { l.strip_prefix("# ") } else { None }) {
                out.push(format!("### {h}"));
                continue;
            }
        }
        out.push(l.to_string());
    }
    out.join("\n")
}

/// A short heading title from a requirement's text: its first sentence,
/// trailing full stop dropped, at most ~100 chars (cut at a word).
fn short_title(text: &str) -> String {
    let s = nodefile::first_sentence(text);
    let s = s.trim_end_matches('.').trim().to_string();
    if s.chars().count() <= 100 {
        return s;
    }
    let mut out = String::new();
    for w in s.split_whitespace() {
        if out.chars().count() + w.chars().count() + 1 > 97 {
            break;
        }
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(w);
    }
    out + "…"
}

/// An iter4 requirement body → (preamble, requirements) in the §2.8 shape.
/// - already sectioned (`## ` headings; with or without markers): kept as is;
///   a body with top-level bullets is split first, though:
/// - top-level bullets (only the bold-id ones when two or more carry an id):
///   one requirement per bullet, everything up to the next one included
///   (continuations de-indented, headings demoted to `###`); a bold
///   `**KEY**` becomes the key; text before the first bullet is the preamble;
///   status agreed;
/// - no bullets and no sections: one requirement titled `name`, the body
///   (minus a leading `# ` title, which stays as the preamble) as its text;
/// - an empty body: nothing.
pub fn req_sections(body: &str, name: &str) -> (String, Vec<nodefile::ReqItem>) {
    let body = body.replace("\r\n", "\n");
    let lines: Vec<&str> = body.split('\n').collect();
    let bullets = bullet_lines(&lines);
    let (pre0, existing) = nodefile::parse_reqs(&body);
    if !existing.is_empty() && (bullets.is_empty() || body.contains("<!-- req:")) {
        return (pre0, existing);
    }
    if bullets.is_empty() {
        let mut start = 0;
        while start < lines.len() && lines[start].trim().is_empty() {
            start += 1;
        }
        let (pre, rest) = match lines.get(start) {
            Some(l) if l.starts_with("# ") => (l.to_string(), &lines[start + 1..]),
            _ => (String::new(), &lines[start..]),
        };
        let text = demote_headings(rest, true);
        if text.trim().is_empty() {
            return (pre, Vec::new());
        }
        let mut it = nodefile::ReqItem::new("", name, &text, "agreed");
        if it.title.is_empty() {
            it.title = short_title(&text);
        }
        return (pre, vec![it]);
    }
    let bold = bullets.iter().filter(|(_, b)| *b).count();
    let starts: Vec<usize> = bullets.iter().filter(|(_, b)| bold < 2 || *b).map(|(i, _)| *i).collect();
    let preamble = demote_headings(&lines[..starts[0]], false);
    let mut items = Vec::new();
    for (n, &s) in starts.iter().enumerate() {
        let end = starts.get(n + 1).copied().unwrap_or(lines.len());
        let mut seg: Vec<String> = Vec::new();
        for (j, l) in lines[s..end].iter().enumerate() {
            if j == 0 {
                seg.push(l[2..].to_string());
            } else {
                seg.push(l.strip_prefix("  ").unwrap_or(l).to_string());
            }
        }
        let seg_refs: Vec<&str> = seg.iter().map(String::as_str).collect();
        let mut text = demote_headings(&seg_refs, true);
        let mut key = String::new();
        if has_bold_id(&text) {
            let k = bullet_name(&text);
            if nodefile::is_req_key(&k) {
                key = k;
                // drop the bold id and the separator after it
                let rest = text.trim_start().strip_prefix("**").unwrap_or(&text);
                let rest = rest.find("**").map(|e| &rest[e + 2..]).unwrap_or(rest);
                text = rest.trim_start_matches([' ', ':', '—', '–', '-', '.', '\t']).to_string();
            }
        }
        let title = short_title(&text);
        items.push(nodefile::ReqItem::new(&key, &title, &text, "agreed"));
    }
    (preamble, items)
}

fn ts_from_iso(s: &str) -> String {
    chrono::DateTime::parse_from_rfc3339(s.trim())
        .map(|d| d.with_timezone(&chrono::Utc).format("%Y-%m-%d %H:%M:%SZ").to_string())
        .unwrap_or_default()
}

/// What one input file turns into.
struct Plan {
    /// old relative path → new relative path (renames, incl. unchanged)
    path: HashMap<String, String>,
    removed: BTreeSet<String>,
}

/// Convert, in memory: (final files rel → text, removed rels, report).
pub fn convert(from: &Path, rep: &mut Report) -> Result<(BTreeMap<String, String>, BTreeSet<String>), String> {
    let now = nodefile::now_ts();
    let rels = iter_files(from);
    rep.add("iter.md files read", rels.len());
    let mut text: BTreeMap<String, String> = BTreeMap::new();
    for r in &rels {
        let t = std::fs::read_to_string(from.join(r)).map_err(|e| format!("{r}: {e}"))?;
        text.insert(r.clone(), t);
    }
    let old_paths: Vec<String> = rels.iter().map(|r| stored(r)).collect();
    let type_tag = |r: &str| -> String {
        let base = r.rsplit('/').next().unwrap_or(r);
        let stem = base.trim_end_matches(".iter.md");
        stem.rsplit('.').next().unwrap_or(stem).to_string()
    };

    // ---- 1. interfaces
    struct Iface {
        name: String,
        kind: String,
        desc: String,
    }
    let mut ifaces: BTreeMap<String, Iface> = BTreeMap::new(); // stored → info
    for r in &rels {
        if type_tag(r) == "interface" {
            let f = raw_front(&text[r]).unwrap_or(Value::Null);
            let stem = r.rsplit('/').next().unwrap_or(r).trim_end_matches(".interface.iter.md").to_string();
            ifaces.insert(stored(r), Iface {
                name: f["name"].as_str().map(String::from).unwrap_or(stem),
                kind: f["kind"].as_str().unwrap_or("").to_string(),
                desc: f["description"].as_str().or_else(|| f["desc"].as_str()).unwrap_or("").to_string(),
            });
        }
    }
    let iface_paths: Vec<String> = ifaces.keys().cloned().collect();
    // producers / consumers per interface (code / usecase nodes' outputs / inputs)
    let mut producers: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    let mut consumers: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    let mut unreadable_fronts = 0;
    for r in &rels {
        if nodefile::type_of(r) != Some(NodeType::Code) {
            continue;
        }
        let Some(f) = raw_front(&text[r]) else {
            if text[r].starts_with("---") {
                unreadable_fronts += 1;
            }
            continue;
        };
        let sp = stored(r);
        for (key, map) in [("outputs", &mut producers), ("inputs", &mut consumers)] {
            let mut entries = list(&f["children"][key]);
            entries.extend(list(&f[key]));
            for e in entries {
                for ip in nodefile::resolve(&e, &sp, &iface_paths) {
                    map.entry(ip).or_default().insert(sp.clone());
                }
            }
        }
    }
    if unreadable_fronts > 0 {
        rep.notes.push(format!("{unreadable_fronts} code node(s) had frontmatter strict YAML cannot read; their interface links were not carried into connections"));
    }

    // ---- plan renames / removals
    let mut plan = Plan { path: HashMap::new(), removed: BTreeSet::new() };
    let mut taken: BTreeSet<String> = rels.iter().cloned().collect();
    // the project file: the top-most main
    let mains: Vec<&String> = rels.iter().filter(|r| type_tag(r) == "main").collect();
    let main_rel = mains.iter().min_by_key(|r| (r.matches('/').count(), r.len())).map(|r| r.to_string());
    let projectname = main_rel
        .as_ref()
        .and_then(|m| raw_front(&text[m]))
        .and_then(|f| f["projectname"].as_str().or_else(|| f["name"].as_str()).map(String::from))
        .unwrap_or_else(|| from.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "project".into()));
    for r in &rels {
        let tag = type_tag(r);
        if tag == "interface" {
            plan.removed.insert(r.clone());
            continue;
        }
        let new = if Some(r) == main_rel.as_ref() {
            let p = unique_path("global", &nodefile::slug(&projectname), "project", &taken);
            rep.add("main → project", 1);
            p
        } else if tag == "main" {
            rep.notes.push(format!("{r}: a second main file, left as a project node in place (renamed *.project.iter.md)"));
            let dir = r.rsplit_once('/').map(|(d, _)| d).unwrap_or("");
            let name = r.rsplit('/').next().unwrap_or(r);
            let stem = name.trim_end_matches(".iter.md").trim_end_matches("main").trim_end_matches('.');
            let p = unique_path(if dir.is_empty() { "." } else { dir }, &nodefile::slug(if stem.is_empty() { "project" } else { stem }), "project", &taken);
            p.trim_start_matches("./").to_string()
        } else {
            let name = r.rsplit('/').next().unwrap_or(r);
            let nn = v5_file_name(name);
            if nn == name {
                r.clone()
            } else {
                let dir = r.rsplit_once('/').map(|(d, _)| d.to_string());
                let mut cand = match &dir {
                    Some(d) => format!("{d}/{nn}"),
                    None => nn.clone(),
                };
                let mut n = 1;
                while taken.contains(&cand) {
                    let stem = nn.trim_end_matches(".iter.md");
                    let (s, t) = stem.rsplit_once('.').unwrap_or(("", stem));
                    let f = format!("{}{n:02}.{t}.iter.md", if s.is_empty() { t } else { s });
                    cand = match &dir {
                        Some(d) => format!("{d}/{f}"),
                        None => f,
                    };
                    n += 1;
                }
                rep.add(if tag == "agentmemory" { "agentmemory → agentmem" } else { "testgroup → test" }, 1);
                cand
            }
        };
        if &new != r {
            taken.insert(new.clone());
        }
        plan.path.insert(r.clone(), new);
    }
    rep.add("interfaces removed", plan.removed.len());

    // ---- 4. requirements (§2.8): one bizreq + one techreq file per
    // attachment point, one `## ` section per requirement
    let mut new_files: BTreeMap<String, String> = BTreeMap::new();
    let project_slug = main_rel
        .as_ref()
        .and_then(|m| plan.path.get(m))
        .map(|p| nodefile::stem_of(p))
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| {
            let s = nodefile::slug(&projectname);
            if s.is_empty() { "project".into() } else { s }
        });
    // the first code node of every folder
    let mut code_by_dir: BTreeMap<String, String> = BTreeMap::new();
    for r in &rels {
        if nodefile::type_of(r) == Some(NodeType::Code) {
            code_by_dir.entry(nodefile::dir_of(r)).or_insert_with(|| r.clone());
        }
    }
    // what main.iter.md names as global context
    let mut global_refs: BTreeSet<String> = BTreeSet::new();
    if let Some(m) = &main_rel {
        let f = raw_front(&text[m]).unwrap_or(Value::Null);
        let msp = stored(m);
        let mut entries = list(&f["globalcontextfiles"]);
        for k in ["reqs", "bizreqs", "techreqs", "reqpaths"] {
            entries.extend(list(&f["children"][k]));
        }
        for e in entries {
            global_refs.extend(nodefile::resolve(&e, &msp, &old_paths));
        }
    }
    // target rel → (sources in path order, owning code node rel)
    let mut req_groups: BTreeMap<String, (Vec<String>, Option<String>)> = BTreeMap::new();
    for r in &rels {
        let Some(t) = nodefile::type_of(r).filter(|t| matches!(t, NodeType::Bizreq | NodeType::Techreq)) else { continue };
        let tag = t.as_str();
        let dir = nodefile::dir_of(r);
        let above_reqs = if nodefile::file_name_of(&dir) == "reqs" { Some(nodefile::dir_of(&dir)) } else { None };
        let owner = code_by_dir
            .get(&dir)
            .map(|c| (dir.clone(), c.clone()))
            .or_else(|| above_reqs.as_ref().and_then(|d| code_by_dir.get(d).map(|c| (d.clone(), c.clone()))));
        let target = if let Some((cd, c)) = &owner {
            let s = nodefile::slug(&nodefile::stem_of(c));
            let s = if s.is_empty() {
                let n = raw_front(&text[c]).and_then(|f| f["name"].as_str().map(nodefile::slug)).unwrap_or_default();
                if n.is_empty() { "code".to_string() } else { n }
            } else {
                s
            };
            if cd.is_empty() { format!("reqs/{s}.{tag}.iter.md") } else { format!("{cd}/reqs/{s}.{tag}.iter.md") }
        } else if global_refs.contains(&stored(r)) || dir.is_empty() {
            format!("global/requirements/{project_slug}.{tag}.iter.md")
        } else {
            let folder = above_reqs.clone().unwrap_or_else(|| dir.clone());
            let s = nodefile::slug(&nodefile::file_name_of(&folder));
            let s = if s.is_empty() { project_slug.clone() } else { s };
            if folder.is_empty() {
                format!("global/requirements/{s}.{tag}.iter.md")
            } else {
                format!("{folder}/reqs/{s}.{tag}.iter.md")
            }
        };
        let g = req_groups.entry(target).or_insert_with(|| (Vec::new(), owner.map(|(_, c)| c)));
        g.0.push(r.clone());
    }
    // the converted text of each target's primary source, and where every
    // other source went
    let mut req_override: HashMap<String, String> = HashMap::new();
    let mut merged_into: HashMap<String, String> = HashMap::new(); // stored source → stored target
    let mut req_targets: BTreeSet<String> = BTreeSet::new(); // stored targets
    let mut req_owner: BTreeMap<String, Vec<String>> = BTreeMap::new(); // owner rel (code or main) → stored targets
    for (target, (sources, owner)) in &req_groups {
        let t = nodefile::type_of(target).unwrap();
        let primary = &sources[0];
        let psp = stored(primary);
        let Ok((mut merged, _)) = nodefile::parse_tolerant(&psp, &text[primary]) else { continue };
        let mut preambles: Vec<String> = Vec::new();
        let mut items: Vec<nodefile::ReqItem> = Vec::new();
        for (n, s) in sources.iter().enumerate() {
            let ssp = stored(s);
            let Ok((d, _)) = nodefile::parse_tolerant(&ssp, &text[s]) else {
                rep.notes.push(format!("{s}: unreadable requirement file, left out of {target}"));
                continue;
            };
            let (pre, its) = req_sections(&d.body, &d.name);
            if !pre.trim().is_empty() {
                preambles.push(pre.trim_end().to_string());
            }
            rep.add("requirements (sections) written", its.len());
            items.extend(its);
            if n > 0 {
                // its own references, made absolute, join the primary's
                for e in d.children.reqs.iter().chain(d.children.codedirs.iter()) {
                    let abs = nodefile::expand_entry(e, &ssp, None);
                    let dst = if d.children.reqs.contains(e) { &mut merged.children.reqs } else { &mut merged.children.codedirs };
                    if !dst.contains(&abs) {
                        dst.push(abs);
                    }
                }
                if merged.desc.trim().is_empty() {
                    merged.desc = d.desc.clone();
                }
                merged_into.insert(ssp, stored(target));
                plan.removed.insert(s.clone());
                plan.path.remove(s);
                rep.add("reqs files merged into another", 1);
            }
        }
        merged.body = nodefile::render_reqs(&preambles.join("\n\n"), &items);
        if !merged.body.is_empty() && !merged.body.ends_with('\n') {
            merged.body.push('\n');
        }
        req_override.insert(primary.clone(), nodefile::render(&merged));
        plan.path.insert(primary.clone(), target.clone());
        if primary != target {
            taken.insert(target.clone());
            rep.add("reqs files moved to the §2.8 layout", 1);
        }
        rep.add(&format!("{} files written", t.as_str()), 1);
        req_targets.insert(stored(target));
        let owner_rel = owner.clone().or_else(|| if target.starts_with("global/requirements/") { main_rel.clone() } else { None });
        if let Some(o) = owner_rel {
            req_owner.entry(o).or_default().push(stored(target));
        }
    }

    // ---- 1b. connection nodes
    let mut by_kind: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for (p, i) in &ifaces {
        by_kind.entry(kind_name(&i.kind)).or_default().push(p.clone());
    }
    let mut kind_node: BTreeMap<String, String> = BTreeMap::new();
    for (kname, members) in &by_kind {
        let path = unique_path("global/connections", &nodefile::slug(kname), "code", &taken);
        taken.insert(path.clone());
        let mut from_set: BTreeSet<String> = BTreeSet::new();
        let mut to_set: BTreeSet<String> = BTreeSet::new();
        let mut lines = Vec::new();
        for m in members {
            let i = &ifaces[m];
            let pr = producers.get(m).cloned().unwrap_or_default();
            let co = consumers.get(m).cloned().unwrap_or_default();
            from_set.extend(pr.iter().cloned());
            to_set.extend(co.iter().cloned());
            lines.push(format!("- `{}` — {}{}", i.name, if i.desc.is_empty() { "(no description)".to_string() } else { i.desc.clone() },
                format!(" ({} producer(s), {} consumer(s))", pr.len(), co.len())));
        }
        let mut nd = NodeDoc::new(NodeType::Code, kname, CREATOR, &now);
        nd.level = Some("connection".into());
        nd.path = stored(&path);
        nd.desc = format!(
            "Carries every {} between the parts of this project: the code nodes listed under connects.from supply it, the ones under connects.to are reached through it. Created by iter migrate5 from {} iter4 interface contract(s) of kind {:?}.",
            kname.to_lowercase(), members.len(), ifaces[&members[0]].kind
        );
        nd.front.insert("connects".into(), json!({"from": from_set.iter().collect::<Vec<_>>(), "to": to_set.iter().collect::<Vec<_>>()}));
        nd.body = format!(
            "# {kname}\n\nOne connection TYPE (iter5): it replaces the {} per-call interface contracts iter4 kept for this kind. Who supplies it and who it reaches are the `connects` lists above.\n\n## The iter4 contracts it replaces\n\n{}\n",
            members.len(),
            lines.join("\n")
        );
        rep.add("connection nodes created", 1);
        rep.add("connection supplies (from) edges", from_set.len());
        rep.add("connection connects (to) edges", to_set.len());
        kind_node.insert(kname.clone(), path.clone());
        new_files.insert(path, nodefile::render(&nd));
    }

    // ---- 5. actors
    let main_front = main_rel.as_ref().and_then(|m| raw_front(&text[m])).unwrap_or(Value::Null);
    let actors_file: Option<PathBuf> = main_front["actorsfile"]
        .as_str()
        .map(|a| {
            let rel = a.trim().trim_start_matches("{topdir}").trim_start_matches('/').to_string();
            from.join(rel)
        })
        .filter(|p| p.is_file())
        .or_else(|| [from.join("actors.yaml"), from.join("global/actors.yaml")].into_iter().find(|p| p.is_file()));
    if let Some(af) = &actors_file {
        let y: Value = std::fs::read_to_string(af)
            .ok()
            .and_then(|t| serde_yaml::from_str::<serde_yaml::Value>(&t).ok())
            .and_then(|v| serde_json::to_value(v).ok())
            .unwrap_or(Value::Null);
        let actors = y["actors"].as_array().cloned().unwrap_or_default();
        for a in actors {
            let name = a["name"].as_str().or_else(|| a["id"].as_str()).unwrap_or("actor").to_string();
            let path = unique_path("global/usecases", &nodefile::slug(&name), "actor", &taken);
            taken.insert(path.clone());
            let mut touches: BTreeSet<String> = BTreeSet::new();
            let mut uses_lines = Vec::new();
            for u in a["uses"].as_array().into_iter().flatten() {
                let pat = u["pattern"].as_str().or_else(|| u.as_str()).unwrap_or("");
                let why = u["why"].as_str().unwrap_or("");
                uses_lines.push(format!("- `{pat}` — {why}"));
                if let Ok(g) = glob::Pattern::new(pat) {
                    for (ip, i) in &ifaces {
                        if g.matches(&i.name) {
                            touches.extend(producers.get(ip).cloned().unwrap_or_default());
                        }
                    }
                }
            }
            let mut nd = NodeDoc::new(NodeType::Actor, &name, CREATOR, &now);
            nd.path = stored(&path);
            nd.desc = a["description"].as_str().unwrap_or("").to_string();
            nd.front.insert("drives".into(), json!([]));
            nd.front.insert("touches".into(), json!(touches.iter().collect::<Vec<_>>()));
            nd.body = format!(
                "# {name}\n\n{}\n\n## Where it touches the platform (from {})\n\n{}\n",
                nd.desc,
                af.strip_prefix(from).map(|p| p.to_string_lossy().into_owned()).unwrap_or_default(),
                if uses_lines.is_empty() { "- (no `uses` listed)".to_string() } else { uses_lines.join("\n") }
            );
            rep.add("actors created", 1);
            rep.add("actor touches edges", touches.len());
            new_files.insert(path, nodefile::render(&nd));
        }
    }

    // ---- per-file conversion
    let mut renames: HashMap<String, String> = plan.path.iter().filter(|(a, b)| a != b).map(|(a, b)| (stored(a), stored(b))).collect();
    renames.extend(merged_into.iter().map(|(a, b)| (a.clone(), b.clone())));
    let mut out: BTreeMap<String, String> = BTreeMap::new();
    let mut refs_rewritten = 0usize;
    for r in &rels {
        let Some(new_rel) = plan.path.get(r).cloned() else { continue };
        let old_sp = stored(r);
        let new_sp = stored(&new_rel);
        let Some(t) = nodefile::type_of(&new_rel) else {
            // a plain context doc: kept as it is
            continue;
        };
        if t == NodeType::Agentmem {
            if new_rel != *r {
                out.insert(new_rel, text[r].clone());
            }
            continue;
        }
        let moved = nodefile::dir_of(&old_sp) != nodefile::dir_of(&new_sp);
        let raw = raw_front(&text[r]).unwrap_or(Value::Null);
        // the iter4 testgroup registry block
        let mut src = req_override.get(r).cloned().unwrap_or_else(|| text[r].clone());
        let groups = testgroups::parse(&src);
        if let Some(s) = src.find(testgroups::BLOCK_START) {
            if let Some(e) = src[s..].find(testgroups::BLOCK_END) {
                src.replace_range(s..s + e + testgroups::BLOCK_END.len(), "");
                src = src.trim_end().to_string() + "\n";
            }
        }
        let c1 = nodefile::conform(&new_sp, &src, &now, CREATOR);
        let Some(mut doc) = c1.doc else { continue };
        // a top-level `testpaths:` tier mapping (pdy-dev) — not a list
        if raw["testpaths"].is_object() {
            doc.children.tests.retain(|e| !e.trim_start().starts_with("{\""));
            doc.front.insert("test_tiers".into(), raw["testpaths"].clone());
            rep.add("testpaths tier maps kept as test_tiers", 1);
        }
        for k in ["globalinterfacedir", "globalusecasedir", "actorsfile", "testgroup", "test_dir"] {
            if doc.front.remove(k).is_some() {
                rep.add(&format!("dropped key {k}"), 1);
            }
        }
        // references
        let this_dir = format!("{}/", nodefile::dir_of(&new_sp));
        // a moved requirement file, as this file names it: `{thisfiledir}/…`
        // when it sits below this file, else `{topdir}/…`
        let req_ref = |n: &str| -> String {
            match n.strip_prefix(&this_dir) {
                Some(rest) if req_targets.contains(n) && rest.starts_with("reqs/") => format!("{{thisfiledir}}/{rest}"),
                _ => n.to_string(),
            }
        };
        let mut rewrite = |entries: &mut Vec<String>, is_reqs: bool| {
            let mut outv: Vec<String> = Vec::new();
            for e in entries.iter() {
                let exp = nodefile::expand_entry(e, &old_sp, None);
                let mut produced: Vec<String> = Vec::new();
                if is_glob(&exp) {
                    let base = if moved { exp.clone() } else { e.clone() };
                    produced.push(retag_glob(&base));
                } else {
                    let key = exp.trim_end_matches('/').to_string();
                    if let Some(n) = renames.get(&key) {
                        let same_dir = nodefile::dir_of(n) == nodefile::dir_of(&key);
                        if req_targets.contains(n) {
                            produced.push(req_ref(n));
                        } else if !moved && same_dir && !e.starts_with("{topdir}") && e.contains(&nodefile::file_name_of(&key)) {
                            produced.push(e.replace(&nodefile::file_name_of(&key), &nodefile::file_name_of(n)));
                        } else {
                            produced.push(n.clone());
                        }
                    } else if moved {
                        produced.push(exp.clone());
                    } else {
                        produced.push(e.clone());
                    }
                }
                // a glob / folder entry that matched requirement files which
                // moved out of its reach: name their new files too
                if is_reqs && (is_glob(&exp) || !old_paths.contains(&exp.trim_end_matches('/').to_string())) {
                    for m in nodefile::resolve(e, &old_sp, &old_paths) {
                        if let Some(n) = renames.get(&m).filter(|n| req_targets.contains(*n)) {
                            let still = produced.iter().any(|p| !nodefile::resolve(p, &new_sp, &[n.to_string()]).is_empty());
                            if !still {
                                produced.push(req_ref(n));
                            }
                        }
                    }
                }
                if produced.len() != 1 || produced[0] != *e {
                    refs_rewritten += 1;
                }
                for p in produced {
                    if !outv.contains(&p) {
                        outv.push(p);
                    }
                }
            }
            *entries = outv;
        };
        rewrite(&mut doc.children.codedirs, false);
        rewrite(&mut doc.children.codenodes, false);
        rewrite(&mut doc.children.tests, false);
        rewrite(&mut doc.children.reqs, true);
        // an owner names its requirement files explicitly (§2.8)
        for target in req_owner.get(r).into_iter().flatten() {
            let named = doc.children.reqs.iter().any(|e| !nodefile::resolve(e, &new_sp, &[target.clone()]).is_empty());
            if !named {
                doc.children.reqs.push(req_ref(target));
                rep.add("owners given their requirement files", 1);
            }
        }
        for v in doc.children.extra.values_mut() {
            rewrite(v, false);
        }
        for key in ["actors", "drives", "touches", "scandirs"] {
            if let Some(Value::Array(a)) = doc.front.get(key) {
                let mut l: Vec<String> = a.iter().filter_map(|x| x.as_str().map(String::from)).collect();
                rewrite(&mut l, false);
                doc.front.insert(key.into(), json!(l));
            }
        }
        if let Some(Value::Object(cm)) = doc.front.get("connects").cloned() {
            let mut cm = cm;
            for side in ["from", "to"] {
                if let Some(Value::Array(a)) = cm.get(side) {
                    let mut l: Vec<String> = a.iter().filter_map(|x| x.as_str().map(String::from)).collect();
                    rewrite(&mut l, false);
                    cm.insert(side.into(), json!(l));
                }
            }
            doc.front.insert("connects".into(), Value::Object(cm));
        }
        // testgroup registry → scripts + last result
        if t == NodeType::Test && !groups.is_empty() {
            let from_real = |s: &str| -> PathBuf { from.join(rel_of(s)) };
            let mut covered: BTreeSet<PathBuf> = BTreeSet::new();
            for e in &doc.children.tests {
                let exp = nodefile::expand_entry(e, &new_sp, None);
                let p = from_real(&exp).to_string_lossy().into_owned();
                if is_glob(&p) {
                    covered.extend(glob::glob(&p).into_iter().flatten().flatten());
                } else {
                    covered.insert(PathBuf::from(p.trim_end_matches('/')));
                }
            }
            let dir_real = from.join(rel_of(&nodefile::dir_of(&old_sp)));
            let mut added = 0;
            let mut scripts_total = 0;
            for g in &groups {
                for te in &g.testlist {
                    if te.shell.trim().is_empty() {
                        continue;
                    }
                    scripts_total += 1;
                    let real = dir_real.join(&te.shell);
                    if covered.iter().any(|c| c == &real || real.starts_with(c)) {
                        continue;
                    }
                    let entry = format!("{{thisfiledir}}/{}", te.shell.trim_start_matches("./"));
                    if !doc.children.tests.contains(&entry) {
                        doc.children.tests.push(entry);
                        added += 1;
                    }
                }
            }
            rep.add("testgroup registries converted", 1);
            rep.add("registered scripts", scripts_total);
            rep.add("scripts added to children.tests", added);
            if groups.iter().any(|g| g.testlist.iter().any(|t| !t.gates)) {
                rep.add("non-gating scripts (now gating)", groups.iter().flat_map(|g| &g.testlist).filter(|t| !t.gates).count());
            }
            // last recorded run
            let ran: Vec<&testgroups::TestGroup> = groups.iter().filter(|g| !g.lastrun.is_empty()).collect();
            if !ran.is_empty() {
                let (mut pass, mut total) = (0u64, 0u64);
                for g in &ran {
                    if let Some((p, t)) = g.counts.split_once('/') {
                        pass += p.trim().parse::<u64>().unwrap_or(0);
                        total += t.trim().parse::<u64>().unwrap_or(0);
                    }
                }
                let ok = ran.iter().all(|g| g.result == "passed");
                doc.front.insert("last_result".into(), json!({
                    "name": doc.name, "id": doc.id, "overall_success": ok,
                    "normal": {"total": total, "pass": pass, "err": total.saturating_sub(pass)},
                    "longtail": {"total": 0, "pass": 0, "err": 0}, "failure": {"total": 0, "pass": 0, "err": 0},
                }));
                let last = ran.iter().map(|g| ts_from_iso(&g.lastrun)).max().unwrap_or_default();
                if !last.is_empty() {
                    doc.timestamps.last_tested = last;
                }
                rep.add("last results carried", 1);
            }
            if groups.len() == 1 {
                let g = &groups[0];
                if !g.input_space.trim().is_empty() {
                    doc.front.insert("input_space".into(), json!(g.input_space));
                }
                if let Some(c) = &g.coverage {
                    doc.front.insert("coverage".into(), json!({"normal": c.golden + c.malformed, "longtail": c.longtail, "failure": c.failure}));
                }
            }
            if groups.iter().any(|g| g.auto_fix) {
                doc.front.insert("auto_fix".into(), json!(true));
            }
            if doc.desc.trim().is_empty() {
                if let Some(d) = groups.iter().map(|g| g.desc.trim()).find(|d| !d.is_empty()) {
                    doc.desc = d.to_string();
                }
            }
            if groups.len() > 1 || groups.iter().any(|g| g.label != doc.name) {
                let mut s = String::from("\n\n## Test groups (from the iter4 registry)\n\n");
                for g in &groups {
                    s.push_str(&format!("- `{}` — {} ({} script(s){})\n", g.label, if g.desc.is_empty() { "(no description)" } else { g.desc.as_str() },
                        g.testlist.len(), if g.result.is_empty() { String::new() } else { format!(", last {} {}", g.result, g.counts) }));
                }
                doc.body = doc.body.trim_end().to_string() + &s;
            }
        }
        doc.path = new_sp.clone();
        let c2 = nodefile::conform(&new_sp, &nodefile::render(&doc), &now, CREATOR);
        if new_rel != *r || c2.text != text[r] {
            out.insert(new_rel, c2.text);
        }
    }
    rep.add("references rewritten", refs_rewritten);
    for (k, v) in new_files {
        out.insert(k, v);
    }
    // conform every written node file once more: a fixed point, and count findings
    let now2 = now.clone();
    for (k, v) in out.iter_mut() {
        let sp = stored(k);
        if !nodefile::type_of(&sp).is_some_and(nodefile::is_synced) {
            continue;
        }
        let c = nodefile::conform(&sp, v, &now2, CREATOR);
        for f in &c.findings {
            *rep.findings.entry(f.code.clone()).or_default() += 1;
        }
        if c.changed {
            *v = c.text;
            let again = nodefile::conform(&sp, v, &now2, CREATOR);
            if again.changed {
                rep.not_idempotent.push(k.clone());
            }
        }
    }
    let mut removed = plan.removed.clone();
    for (old, new) in &plan.path {
        if old != new {
            removed.insert(old.clone());
        }
    }
    rep.add("node files written", out.keys().filter(|k| k.ends_with(".iter.md")).count());
    rep.add("files removed", removed.len());
    if let Some(m) = &main_rel {
        let _ = m;
    } else {
        rep.notes.push("no main.iter.md found: no project node was created (create one with `iter init`)".into());
    }
    Ok((out, removed))
}

/// The whole verb: refuse in-place / non-empty targets, copy, convert, write.
pub fn run(from: &Path, to: &Path, dry_run: bool) -> Result<Report, String> {
    let from = from.canonicalize().map_err(|e| format!("--from {}: {e}", from.display()))?;
    if !from.is_dir() {
        return Err(format!("--from {} is not a directory", from.display()));
    }
    let to_abs = if to.is_absolute() { to.to_path_buf() } else { std::env::current_dir().map_err(|e| e.to_string())?.join(to) };
    let to_canon = to_abs.canonicalize().unwrap_or_else(|_| to_abs.clone());
    if to_canon == from || to_canon.starts_with(&from) || from.starts_with(&to_canon) {
        return Err(format!("--to {} must be outside --from {} (migrate5 never works in place)", to_abs.display(), from.display()));
    }
    if to_abs.exists() && std::fs::read_dir(&to_abs).map(|mut d| d.next().is_some()).unwrap_or(true) {
        return Err(format!("--to {} exists and is not empty", to_abs.display()));
    }
    let mut rep = Report { from: from.display().to_string(), to: to_abs.display().to_string(), dry_run, ..Default::default() };
    let (files, removed) = convert(&from, &mut rep)?;
    if dry_run {
        return Ok(rep);
    }
    std::fs::create_dir_all(&to_abs).map_err(|e| format!("cannot create {}: {e}", to_abs.display()))?;
    let st = std::process::Command::new("cp")
        .arg("-a")
        .arg(format!("{}/.", from.display()))
        .arg(&to_abs)
        .status()
        .map_err(|e| format!("cp: {e}"))?;
    if !st.success() {
        return Err(format!("copying {} to {} failed", from.display(), to_abs.display()));
    }
    for r in &removed {
        let _ = std::fs::remove_file(to_abs.join(r));
    }
    for (r, t) in &files {
        let p = to_abs.join(r);
        if let Some(d) = p.parent() {
            std::fs::create_dir_all(d).map_err(|e| format!("{}: {e}", d.display()))?;
        }
        std::fs::write(&p, t).map_err(|e| format!("{}: {e}", p.display()))?;
    }
    // final check on disk: every node file is a conform fixed point
    let now = nodefile::now_ts();
    let mut checked = 0;
    for f in crate::walk::node_files(&to_abs, &[to_abs.clone()]) {
        let Some(sp) = crate::walk::topdir_path(&to_abs, &f) else { continue };
        if !nodefile::type_of(&sp).is_some_and(nodefile::is_synced) {
            continue;
        }
        checked += 1;
        let t = std::fs::read_to_string(&f).unwrap_or_default();
        if nodefile::conform(&sp, &t, &now, CREATOR).changed && !rep.not_idempotent.contains(&sp) {
            rep.not_idempotent.push(sp);
        }
    }
    rep.add("node files verified on disk", checked);
    let left: Vec<String> = iter_files(&to_abs).into_iter().filter(|r| r.ends_with(".interface.iter.md")).collect();
    if !left.is_empty() {
        rep.notes.push(format!("{} interface file(s) remain (git-ignored copies?): {}", left.len(), left.iter().take(5).cloned().collect::<Vec<_>>().join(", ")));
    }
    rep.notes.push("the converted tree is left uncommitted in --to: review `git status` there and commit it".into());
    Ok(rep)
}

#[cfg(test)]
#[path = "migrate5_tests.rs"]
mod tests;
