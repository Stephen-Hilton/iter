//! Requirement files hold many requirements (spec §2.8).
//!
//! A bizreq / techreq body is an optional free-markdown preamble followed by
//! one `## ` section per requirement:
//!
//! ```text
//! <optional preamble>
//!
//! ## PDY-TECH-034 — JWT identifies; only Ed25519 authorizes money
//! <!-- req: id=6f2c4c9e-…-uuid status=agreed -->
//! Every caller presents a JSON Web Token … (### or lower for sub-headings)
//!
//! ## Second requirement title
//! <!-- req: id=… status=draft -->
//! …
//! ```
//!
//! The heading is `KEY — title` when the part before the first `" — "` is a
//! key (`[A-Za-z0-9_.\-]+`), else just `title` (key empty). The marker line
//! carries the requirement's permanent `id` (uuid v4) and its `status`
//! (`draft|agreed|done`); unknown `k=v` pairs on it are kept in
//! [`ReqItem::extra`]. `## ` lines inside code fences are not headings.
//!
//! Canonical rendering ([`render_reqs`]): the preamble (trailing whitespace
//! trimmed) and a blank line, then for every section `## heading`, the marker
//! line, the text (leading blank lines and trailing whitespace trimmed), and a
//! blank line between sections; LF line endings; the body ends with one `\n`.
//! A body without any `## ` section is all preamble and is never changed.

use super::{Finding, NodeDoc, REQ_STATUSES, is_valid_id};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashSet};

/// One requirement (one `## ` section of a bizreq / techreq file).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReqItem {
    /// uuid v4, permanent (survives edits and moves between files); `""`
    /// when the section has no marker / id yet (conform mints one)
    #[serde(default)]
    pub id: String,
    /// `PDY-TECH-034`, or `""`
    #[serde(default)]
    pub key: String,
    #[serde(default)]
    pub title: String,
    /// `draft|agreed|done`; `""` when the marker has none (conform → draft)
    #[serde(default)]
    pub status: String,
    /// the requirement's markdown, verbatim (leading blank lines and trailing
    /// whitespace trimmed); `### ` or lower for sub-headings
    #[serde(default)]
    pub text: String,
    /// other `k=v` pairs found on the marker line, kept and re-rendered
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub extra: BTreeMap<String, String>,
}

impl ReqItem {
    /// A new requirement with a fresh uuid v4; empty `status` → `draft`.
    pub fn new(key: &str, title: &str, text: &str, status: &str) -> ReqItem {
        ReqItem {
            id: uuid::Uuid::new_v4().to_string(),
            key: key.trim().to_string(),
            title: one_line(title),
            status: if status.trim().is_empty() { "draft".to_string() } else { status.trim().to_string() },
            text: trim_text(text),
            extra: BTreeMap::new(),
        }
    }

    /// The heading text after `## ` (`KEY — title` or `title`).
    pub fn heading(&self) -> String {
        let key = self.key.trim();
        let title = one_line(&self.title);
        match (key.is_empty(), title.is_empty()) {
            (true, _) => title,
            (false, true) => format!("{} —", key),
            (false, false) => format!("{} — {}", key, title),
        }
    }

    /// The canonical marker line `<!-- req: id=<uuid> status=<s> [k=v …] -->`.
    pub fn marker(&self) -> String {
        let mut s = format!("<!-- req: id={} status={}", marker_value(&self.id), marker_value(&self.status));
        for (k, v) in &self.extra {
            s.push_str(&format!(" {}={}", k, marker_value(v)));
        }
        s.push_str(" -->");
        s
    }
}

/// `[A-Za-z0-9_.\-]+` — what may stand before `" — "` in a heading as a key.
pub fn is_req_key(s: &str) -> bool {
    !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-'))
}

/// The req node's name: `KEY — title`, or `title`, or `KEY` when the title is
/// empty; `"untitled requirement"` when both are.
pub fn req_node_name(item: &ReqItem) -> String {
    let key = item.key.trim();
    let title = one_line(&item.title);
    match (key.is_empty(), title.is_empty()) {
        (true, true) => "untitled requirement".to_string(),
        (true, false) => title,
        (false, true) => key.to_string(),
        (false, false) => format!("{} — {}", key, title),
    }
}

/// The first sentence of a requirement's text, as plain text (markdown
/// emphasis / code ticks / heading marks / list bullets dropped, whitespace
/// collapsed, code fences and html comments skipped), at most 300 chars.
pub fn first_sentence(text: &str) -> String {
    let mut words: Vec<String> = Vec::new();
    let mut fence = false;
    for line in text.lines() {
        let t = line.trim();
        if t.starts_with("```") || t.starts_with("~~~") {
            fence = !fence;
            continue;
        }
        if fence || (t.starts_with("<!--") && t.ends_with("-->")) {
            continue;
        }
        let t = t.trim_start_matches('#').trim_start();
        let t = t
            .strip_prefix("- ")
            .or_else(|| t.strip_prefix("* "))
            .or_else(|| t.strip_prefix("> "))
            .unwrap_or(t);
        let plain = t.replace("**", "").replace("__", "").replace('`', "");
        words.extend(plain.split_whitespace().map(String::from));
    }
    let plain = words.join(" ");
    let mut cut = plain.len();
    for (i, c) in plain.char_indices() {
        if matches!(c, '.' | '!' | '?') && plain[i + c.len_utf8()..].starts_with(' ') {
            cut = i + c.len_utf8();
            break;
        }
    }
    let mut s = plain[..cut].to_string();
    if s.chars().count() > 300 {
        s = s.chars().take(297).collect::<String>() + "...";
    }
    s
}

/// `(file id, req id)` for every requirement of a bizreq / techreq file node
/// with a valid id (the `contains` edges iter_data derives). Empty for any
/// other node type.
pub fn req_edges(file_doc: &NodeDoc) -> Vec<(String, String)> {
    if !matches!(file_doc.nodetype, super::NodeType::Bizreq | super::NodeType::Techreq) {
        return Vec::new();
    }
    parse_reqs(&file_doc.body)
        .1
        .into_iter()
        .filter(|r| is_valid_id(&r.id))
        .map(|r| (file_doc.id.clone(), r.id))
        .collect()
}

/// A parsed section plus what conform needs to know about its raw form.
struct RawSection {
    item: ReqItem,
    /// the marker line as written (None = no marker)
    marker: Option<String>,
    /// marker tokens that were not `k=v`
    junk: Vec<String>,
}

/// Body → (preamble, requirements). A body with no `## ` section (outside
/// code fences) is all preamble, returned verbatim.
pub fn parse_reqs(body: &str) -> (String, Vec<ReqItem>) {
    let (pre, secs) = parse_raw(body);
    (pre, secs.into_iter().map(|s| s.item).collect())
}

fn parse_raw(body: &str) -> (String, Vec<RawSection>) {
    let lines: Vec<&str> = body.split('\n').map(|l| l.strip_suffix('\r').unwrap_or(l)).collect();
    // heading line indices, fence-aware
    let mut heads: Vec<usize> = Vec::new();
    let mut fence: Option<char> = None;
    for (i, l) in lines.iter().enumerate() {
        let t = l.trim_start();
        if let Some(c) = fence_char(t) {
            match fence {
                None => fence = Some(c),
                Some(open) if open == c => fence = None,
                _ => {}
            }
            continue;
        }
        if fence.is_none() && (l.starts_with("## ") || l.trim_end() == "##") {
            heads.push(i);
        }
    }
    if heads.is_empty() {
        return (body.to_string(), Vec::new());
    }
    let preamble = lines[..heads[0]].join("\n");
    let mut out = Vec::new();
    for (n, &h) in heads.iter().enumerate() {
        let end = heads.get(n + 1).copied().unwrap_or(lines.len());
        let (key, title) = split_heading(lines[h].trim_start_matches('#'));
        let mut rest: Vec<&str> = lines[h + 1..end].to_vec();
        // the marker: the first marker line of the section outside a fence
        let mut marker = None;
        let mut fence: Option<char> = None;
        for (j, l) in rest.iter().enumerate() {
            let t = l.trim();
            if let Some(c) = fence_char(t) {
                match fence {
                    None => fence = Some(c),
                    Some(open) if open == c => fence = None,
                    _ => {}
                }
                continue;
            }
            if fence.is_none() && marker_body(t).is_some() {
                marker = Some((j, t.to_string()));
                break;
            }
        }
        let mut item = ReqItem { key, title, ..Default::default() };
        let mut junk = Vec::new();
        let marker_line = marker.map(|(j, m)| {
            rest.remove(j);
            for tok in tokens(marker_body(&m).unwrap_or("")) {
                match tok.split_once('=') {
                    Some((k, v)) if !k.is_empty() => {
                        let v = unquote(v);
                        match k {
                            "id" => item.id = v,
                            "status" => item.status = v,
                            _ => {
                                item.extra.insert(k.to_string(), v);
                            }
                        }
                    }
                    _ => junk.push(tok),
                }
            }
            m
        });
        item.text = trim_text(&rest.join("\n"));
        out.push(RawSection { item, marker: marker_line, junk });
    }
    (preamble, out)
}

/// (preamble, requirements) → body (canonical form, see the module doc). With
/// no requirements the preamble is returned as is.
pub fn render_reqs(preamble: &str, items: &[ReqItem]) -> String {
    if items.is_empty() {
        return preamble.to_string();
    }
    let mut out = String::new();
    let pre = preamble.replace("\r\n", "\n");
    let pre = pre.trim_end();
    if !pre.trim().is_empty() {
        out.push_str(pre);
        out.push_str("\n\n");
    }
    for (i, it) in items.iter().enumerate() {
        if i > 0 {
            out.push('\n');
        }
        let h = it.heading();
        out.push_str(if h.is_empty() { "##" } else { "## " });
        out.push_str(&h);
        out.push('\n');
        out.push_str(&it.marker());
        out.push('\n');
        let text = trim_text(&it.text);
        if !text.is_empty() {
            out.push_str(&text);
            out.push('\n');
        }
    }
    out
}

/// Conform a bizreq / techreq body: every section gets a marker with a valid,
/// file-unique uuid v4 id (missing / malformed / duplicate → new id) and a
/// status (missing → `draft`; an invalid one is kept, with a finding), the
/// marker is normalised, text kept verbatim, order kept. A body with no
/// section is returned untouched. Idempotent.
///
/// Finding codes: `req-marker-missing`, `req-id-missing`, `req-id-malformed`,
/// `req-id-duplicate`, `req-status-missing`, `req-status-invalid`,
/// `req-marker-normalised`, `req-marker-junk`.
pub fn conform_reqs(body: &str) -> (String, Vec<Finding>) {
    let (pre, secs) = parse_raw(body);
    if secs.is_empty() {
        return (body.to_string(), Vec::new());
    }
    let mut findings = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    let mut items = Vec::with_capacity(secs.len());
    for s in secs {
        let mut it = s.item;
        let name = req_node_name(&it);
        let had_marker = s.marker.is_some();
        if !had_marker {
            it.id = uuid::Uuid::new_v4().to_string();
            findings.push(Finding::new("req-marker-missing", format!("{:?}: no req marker; new id {}", name, it.id)));
        } else if it.id.is_empty() {
            it.id = uuid::Uuid::new_v4().to_string();
            findings.push(Finding::new("req-id-missing", format!("{:?}: marker had no id; new id {}", name, it.id)));
        } else if !is_valid_id(&it.id) {
            let old = std::mem::replace(&mut it.id, uuid::Uuid::new_v4().to_string());
            findings.push(Finding::new("req-id-malformed", format!("{:?}: {:?} is not a uuid; new id {}", name, old, it.id)));
        } else if seen.contains(&it.id.to_lowercase()) {
            let old = std::mem::replace(&mut it.id, uuid::Uuid::new_v4().to_string());
            findings.push(Finding::new(
                "req-id-duplicate",
                format!("{:?}: id {} already used earlier in this file; new id {}", name, old, it.id),
            ));
        }
        seen.insert(it.id.to_lowercase());
        let st = it.status.trim().to_lowercase();
        if st.is_empty() {
            it.status = "draft".to_string();
            if had_marker {
                findings.push(Finding::new("req-status-missing", format!("{:?}: no status; set to draft", name)));
            }
        } else if REQ_STATUSES.contains(&st.as_str()) {
            it.status = st;
        } else {
            findings.push(Finding::new(
                "req-status-invalid",
                format!("{:?}: status {:?} is not one of {}", name, it.status, REQ_STATUSES.join("|")),
            ));
        }
        if !s.junk.is_empty() {
            findings.push(Finding::new("req-marker-junk", format!("{:?}: dropped marker words {:?}", name, s.junk)));
        }
        if let Some(m) = &s.marker {
            if *m != it.marker() {
                findings.push(Finding::new("req-marker-normalised", format!("{:?}: marker rewritten as {}", name, it.marker())));
            }
        }
        items.push(it);
    }
    (render_reqs(&pre, &items), findings)
}

/* ------------------------------------------------------------- helpers */

fn fence_char(t: &str) -> Option<char> {
    if t.starts_with("```") {
        Some('`')
    } else if t.starts_with("~~~") {
        Some('~')
    } else {
        None
    }
}

/// `KEY — title` / `KEY —` / `title`.
fn split_heading(raw: &str) -> (String, String) {
    let rest = raw.trim();
    if let Some((k, t)) = rest.split_once(" — ") {
        if is_req_key(k.trim()) {
            return (k.trim().to_string(), t.trim().to_string());
        }
    }
    if let Some(k) = rest.strip_suffix(" —") {
        if is_req_key(k.trim()) {
            return (k.trim().to_string(), String::new());
        }
    }
    (String::new(), rest.to_string())
}

/// The inside of a `<!-- req: … -->` line (after `req:`), or None.
fn marker_body(line: &str) -> Option<&str> {
    let inner = line.strip_prefix("<!--")?.strip_suffix("-->")?.trim();
    let rest = inner.strip_prefix("req:").or_else(|| inner.strip_prefix("req :"))?;
    Some(rest.trim())
}

/// Whitespace-separated tokens; `k="a b"` keeps its quoted spaces.
fn tokens(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut quoted = false;
    for c in s.chars() {
        match c {
            '"' => {
                quoted = !quoted;
                cur.push(c);
            }
            c if c.is_whitespace() && !quoted => {
                if !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                }
            }
            c => cur.push(c),
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

fn unquote(v: &str) -> String {
    let v = v.trim();
    if v.len() >= 2 && v.starts_with('"') && v.ends_with('"') {
        v[1..v.len() - 1].replace("\\\"", "\"")
    } else {
        v.to_string()
    }
}

fn marker_value(v: &str) -> String {
    if v.is_empty() || v.chars().any(|c| c.is_whitespace() || c == '"') || v.contains("--") {
        format!("\"{}\"", v.replace("--", "- -").replace('"', "\\\""))
    } else {
        v.to_string()
    }
}

/// One line: newlines and runs of whitespace collapsed to single spaces.
fn one_line(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// LF endings, leading blank lines and trailing whitespace dropped.
fn trim_text(s: &str) -> String {
    let s = s.replace("\r\n", "\n");
    let mut lines: Vec<&str> = s.split('\n').collect();
    while lines.first().is_some_and(|l| l.trim().is_empty()) {
        lines.remove(0);
    }
    lines.join("\n").trim_end().to_string()
}
