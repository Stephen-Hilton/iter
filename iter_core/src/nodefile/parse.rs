//! Node file → [`NodeDoc`], tolerant of iter4 legacy keys (§2.6 migration
//! happens here, so `parse` and `conform` always agree on what a file says).

use super::yaml;
use super::{Children, Finding, NodeDoc, NodeErr, NodeType, Timestamps, is_valid_id, type_of};
use serde_json::{Map, Value};

/// What the raw file held, for conform's decisions.
#[derive(Debug, Default)]
pub(crate) struct Seen {
    pub front: bool,
    pub id: bool,
    pub name: bool,
    pub desc: bool,
    pub creator: bool,
    pub teststate: bool,
    pub children: bool,
    pub timestamps: bool,
    pub create: bool,
    pub last_modified: bool,
    pub last_tested: bool,
}

pub(crate) struct Loose {
    pub doc: NodeDoc,
    pub findings: Vec<Finding>,
    pub seen: Seen,
    /// a legacy key was renamed / folded / dropped
    pub migrated: bool,
}

/// Parse a node file. Legacy iter4 keys are migrated in the returned doc.
/// Errors: not a node filename, or (synced types) a missing / malformed `id`
/// — run [`super::conform`] first.
pub fn parse(path: &str, text: &str) -> Result<NodeDoc, NodeErr> {
    let l = parse_loose(path, text)?;
    if l.doc.nodetype != NodeType::Agentmem && !is_valid_id(&l.doc.id) {
        return Err(NodeErr::BadId(l.doc.id));
    }
    Ok(l.doc)
}

/// Parse without the id check, returning what was repaired / migrated on the
/// way (used by the converter and diagnostics).
pub fn parse_tolerant(path: &str, text: &str) -> Result<(NodeDoc, Vec<Finding>), NodeErr> {
    let l = parse_loose(path, text)?;
    Ok((l.doc, l.findings))
}

fn legacy(findings: &mut Vec<Finding>, from: &str, to: &str) {
    findings.push(Finding::new("legacy-key", format!("{} → {}", from, to)));
}

fn str_of(v: &Value) -> String {
    match v {
        Value::Null => String::new(),
        Value::String(s) => s.clone(),
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => n.to_string(),
        other => other.to_string(),
    }
}

/// A list key: a list, a single string (forgiven → 1 item), or null.
pub(crate) fn list_of(v: &Value) -> Vec<String> {
    match v {
        Value::Null => Vec::new(),
        Value::Array(a) => a
            .iter()
            .filter(|x| !x.is_null())
            .map(str_of)
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect(),
        other => {
            let s = str_of(other).trim().to_string();
            if s.is_empty() { Vec::new() } else { vec![s] }
        }
    }
}

fn push_unique(dst: &mut Vec<String>, src: Vec<String>) {
    for s in src {
        if !dst.contains(&s) {
            dst.push(s);
        }
    }
}

fn is_interface_key(k: &str) -> bool {
    k == "inputs" || k == "outputs" || k.contains("interface")
}

pub(crate) fn parse_loose(path: &str, text: &str) -> Result<Loose, NodeErr> {
    let nodetype = type_of(path).ok_or_else(|| NodeErr::NotANode(path.to_string()))?;
    let mut findings = Vec::new();
    let mut seen = Seen::default();
    let (front, mut body, unterminated) = yaml::split(text);
    if unterminated {
        findings.push(Finding::new("frontmatter-unterminated", "the opening `---` has no closing `---`; the whole file is read as body"));
    }
    let mut m: Map<String, Value> = match &front {
        Some(f) => {
            seen.front = true;
            yaml::load_map(f, &mut findings)
        }
        None => Map::new(),
    };


    let id = m.remove("id").map(|v| str_of(&v).trim().to_string());
    seen.id = id.as_ref().is_some_and(|s| !s.is_empty());

    // name / desc (+ iter4 projectname / projectdescription / description)
    let mut name = m.remove("name").map(|v| str_of(&v));
    seen.name = name.is_some();
    if let Some(pn) = m.remove("projectname") {
        legacy(&mut findings, "projectname", "name");
        if name.as_deref().unwrap_or("").trim().is_empty() {
            name = Some(str_of(&pn));
            seen.name = true;
        }
    }
    let mut desc = m.remove("desc").map(|v| str_of(&v));
    seen.desc = desc.is_some();
    for old in ["description", "projectdescription"] {
        if let Some(d) = m.remove(old) {
            legacy(&mut findings, old, "desc");
            if desc.as_deref().unwrap_or("").trim().is_empty() {
                desc = Some(str_of(&d));
                seen.desc = true;
            }
        }
    }
    let creator = m.remove("creator").map(|v| str_of(&v));
    seen.creator = creator.is_some();

    let mut teststate = m.remove("teststate").map(|v| str_of(&v).trim().to_string());
    seen.teststate = teststate.as_ref().is_some_and(|s| !s.is_empty());
    if let Some(tl) = m.remove("test_loop") {
        legacy(&mut findings, "test_loop", "teststate");
        if !seen.teststate {
            let v = str_of(&tl).trim().to_string();
            teststate = Some(if v == "blocked" { "block".to_string() } else { v });
            seen.teststate = true;
        }
    }

    let level = m
        .remove("level")
        .map(|v| str_of(&v).trim().to_string())
        .filter(|s| !s.is_empty());

    // body folding of the iter4 description variants
    let simple = m.remove("simple_description").map(|v| str_of(&v));
    let long = m.remove("long_description").map(|v| str_of(&v));
    if simple.is_some() || long.is_some() {
        if simple.is_some() {
            findings.push(Finding::new("legacy-folded", "simple_description folded into the body (## Summary)"));
        }
        if long.is_some() {
            findings.push(Finding::new("legacy-folded", "long_description folded into the body (## Long description)"));
        }
        body = fold_body(&body, simple.as_deref().unwrap_or(""), long.as_deref().unwrap_or(""));
    }

    // children
    let mut children = Children::default();
    let mut legacy_reqs: Vec<String> = Vec::new();
    let mut legacy_tests: Vec<String> = Vec::new();
    match m.remove("children") {
        Some(Value::Object(cm)) => {
            seen.children = true;
            for (k, v) in cm {
                match k.as_str() {
                    "codedirs" => push_unique(&mut children.codedirs, list_of(&v)),
                    "codenodes" => push_unique(&mut children.codenodes, list_of(&v)),
                    "tests" => push_unique(&mut children.tests, list_of(&v)),
                    "reqs" => push_unique(&mut children.reqs, list_of(&v)),
                    "bizreqs" | "techreqs" | "reqpaths" => {
                        legacy(&mut findings, &format!("children.{}", k), "children.reqs");
                        legacy_reqs.extend(list_of(&v));
                    }
                    "testpaths" | "testgroups" => {
                        legacy(&mut findings, &format!("children.{}", k), "children.tests");
                        legacy_tests.extend(list_of(&v));
                    }
                    k if is_interface_key(k) => {
                        findings.push(Finding::new("dropped-interface", format!("children.{} dropped (interfaces are retired in iter5)", k)));
                    }
                    _ => {
                        let l = list_of(&v);
                        children.extra.insert(k, l);
                    }
                }
            }
        }
        Some(Value::Null) => seen.children = true,
        Some(other) => {
            findings.push(Finding::new("children-invalid", format!("children is not a mapping ({}); ignored", other)));
        }
        None => {}
    }
    // top-level V1/iter4 list keys
    for (old, to) in [
        ("bizreqs", "reqs"),
        ("techreqs", "reqs"),
        ("reqpaths", "reqs"),
        ("globalcontextfiles", "reqs"),
        ("testpaths", "tests"),
        ("testgroups", "tests"),
        ("codedirs", "codedirs"),
        ("codenodes", "codenodes"),
    ] {
        if let Some(v) = m.remove(old) {
            legacy(&mut findings, old, &format!("children.{}", to));
            let l = list_of(&v);
            match to {
                "reqs" => legacy_reqs.extend(l),
                "tests" => legacy_tests.extend(l),
                "codedirs" => push_unique(&mut children.codedirs, l),
                _ => push_unique(&mut children.codenodes, l),
            }
        }
    }
    push_unique(&mut children.reqs, legacy_reqs);
    push_unique(&mut children.tests, legacy_tests);
    if let Some(v) = m.remove("globalscandirs") {
        legacy(&mut findings, "globalscandirs", "scandirs");
        if !m.contains_key("scandirs") {
            m.insert("scandirs".to_string(), Value::Array(list_of(&v).into_iter().map(Value::String).collect()));
        }
    }

    // timestamps
    let mut ts = Timestamps::default();
    match m.remove("timestamps") {
        Some(Value::Object(tm)) => {
            seen.timestamps = true;
            for (k, v) in tm {
                let s = str_of(&v).trim().to_string();
                match k.as_str() {
                    "create" | "created" => {
                        seen.create = true;
                        ts.create = s;
                    }
                    "last_modified" | "modified" | "updated" => {
                        seen.last_modified = true;
                        ts.last_modified = s;
                    }
                    "last_tested" | "tested" => {
                        seen.last_tested = true;
                        ts.last_tested = s;
                    }
                    _ => {
                        ts.extra.insert(k, s);
                    }
                }
            }
        }
        Some(Value::Null) => seen.timestamps = true,
        Some(other) => {
            findings.push(Finding::new("timestamps-invalid", format!("timestamps is not a mapping ({}); reset", other)));
        }
        None => {}
    }

    // interface-era top-level keys
    let drop: Vec<String> = m.keys().filter(|k| is_interface_key(k)).cloned().collect();
    for k in drop {
        m.remove(&k);
        findings.push(Finding::new("dropped-interface", format!("{} dropped (interfaces are retired in iter5)", k)));
    }
    // a key named like a common key but empty ("" key) is noise
    m.remove("");

    let doc = NodeDoc {
        id: id.unwrap_or_default(),
        nodetype,
        name: name.unwrap_or_default(),
        desc: desc.unwrap_or_default(),
        creator: creator.unwrap_or_default(),
        teststate: teststate.unwrap_or_default(),
        level,
        children,
        timestamps: ts,
        front: m,
        body,
        path: path.to_string(),
    };
    let migrated = findings
        .iter()
        .any(|f| matches!(f.code.as_str(), "legacy-key" | "legacy-folded" | "dropped-interface"));
    Ok(Loose { doc, findings, seen, migrated })
}

/// `simple_description` becomes a `## Summary` section at the top (after a
/// leading `# Title` line), `long_description` a `## Long description`
/// section at the end.
fn fold_body(body: &str, simple: &str, long: &str) -> String {
    let mut out = body.to_string();
    let simple = simple.trim();
    let long = long.trim();
    if !simple.is_empty() {
        let section = format!("## Summary\n\n{}\n", simple);
        // first non-blank line
        let mut pos = 0;
        let mut insert_at = 0;
        let mut after_title = false;
        for line in out.split_inclusive('\n') {
            if line.trim().is_empty() {
                pos += line.len();
                continue;
            }
            if line.starts_with("# ") {
                insert_at = pos + line.len();
                after_title = true;
            } else {
                insert_at = pos;
            }
            break;
        }
        if out.trim().is_empty() {
            out = section;
        } else if after_title {
            let head = &out[..insert_at];
            let head = if head.ends_with('\n') { head.to_string() } else { format!("{}\n", head) };
            out = format!("{}\n{}{}", head, section, &out[insert_at..]);
        } else {
            out = format!("{}{}\n{}", &out[..insert_at], section, &out[insert_at..]);
        }
    }
    if !long.is_empty() {
        if out.trim().is_empty() {
            out = format!("## Long description\n\n{}\n", long);
        } else {
            if !out.ends_with('\n') {
                out.push('\n');
            }
            out.push_str(&format!("\n## Long description\n\n{}\n", long));
        }
    }
    out
}
