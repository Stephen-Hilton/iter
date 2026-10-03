//! File-side conformance (§2.6): make any `*.iter.md` text a well-formed v5
//! node file. Idempotent: `conform(conform(x)) == conform(x)`.

use super::parse::parse_loose;
use super::render::render;
use super::{
    Finding, LEVELS, NodeDoc, NodeType, OWNERS, REQ_STATUSES, TESTSTATES, is_synced, is_valid_id, semantic_hash,
    stem_of, type_of,
};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Conformed {
    /// the conformed file text (== input when nothing needed fixing)
    pub text: String,
    /// `text != input`
    pub changed: bool,
    pub findings: Vec<Finding>,
    /// the conformed node (None for non-node / agentmem files, which are
    /// returned untouched)
    pub doc: Option<NodeDoc>,
}

/// Conform one file. `now` = the timestamp for anything new or bumped
/// ([`super::now_ts`]); `creator` fills a missing `creator:` key.
///
/// `timestamps.last_modified` is bumped when conform itself changes what the
/// file says (legacy migration, added ids / defaults) — formatting-only
/// rewrites do not bump. To also bump on a hand edit, use
/// [`conform_against`] with the semantic hash last synced for this file.
pub fn conform(path: &str, text: &str, now: &str, creator: &str) -> Conformed {
    conform_against(path, text, now, creator, None)
}

/// [`conform`], plus: when `prev_semantic_hash` is given and the conformed
/// content differs from it, `last_modified` is bumped to `now`.
pub fn conform_against(path: &str, text: &str, now: &str, creator: &str, prev_semantic_hash: Option<&str>) -> Conformed {
    let untouched = |findings: Vec<Finding>| Conformed { text: text.to_string(), changed: false, findings, doc: None };
    let Some(t) = type_of(path) else {
        return untouched(vec![Finding::new("not-a-node", format!("{} is not a node file; left as is", path))]);
    };
    if !is_synced(t) {
        return untouched(Vec::new());
    }
    let Ok(loose) = parse_loose(path, text) else {
        return untouched(vec![Finding::new("not-a-node", path.to_string())]);
    };
    let mut findings = loose.findings;
    let seen = loose.seen;
    let mut doc = loose.doc;
    let before = semantic_hash(&doc);

    if !seen.front {
        findings.push(Finding::new("frontmatter-added", "no frontmatter; added one"));
    }
    // id
    if doc.id.is_empty() {
        doc.id = uuid::Uuid::new_v4().to_string();
        findings.push(Finding::new("id-missing", format!("new id {}", doc.id)));
    } else if !is_valid_id(&doc.id) {
        let old = std::mem::replace(&mut doc.id, uuid::Uuid::new_v4().to_string());
        findings.push(Finding::new("id-malformed", format!("{:?} is not a uuid; new id {}", old, doc.id)));
    }
    // common keys
    let mut added: Vec<&str> = Vec::new();
    if !seen.name {
        doc.name = derive_name(&doc.body, path, t);
        added.push("name");
    }
    if !seen.desc {
        added.push("desc");
    }
    if !seen.creator {
        doc.creator = creator.to_string();
        added.push("creator");
    }
    if doc.teststate.is_empty() {
        doc.teststate = "inherit".to_string();
        if !seen.teststate {
            added.push("teststate");
        }
    } else if !TESTSTATES.contains(&doc.teststate.as_str()) {
        findings.push(Finding::new(
            "teststate-invalid",
            format!("teststate {:?} is not one of {}", doc.teststate, TESTSTATES.join("|")),
        ));
    }
    if !seen.children {
        added.push("children");
    }
    if !seen.timestamps {
        added.push("timestamps");
    }
    if !added.is_empty() && seen.front {
        findings.push(Finding::new("keys-added", format!("added missing keys: {}", added.join(", "))));
    }
    apply_type_defaults(&mut doc, &mut findings);
    // a requirement file's sections (§2.8): markers, ids, statuses
    if matches!(t, NodeType::Bizreq | NodeType::Techreq) {
        let (body, f) = super::reqs::conform_reqs(&doc.body);
        doc.body = body;
        findings.extend(f);
    }

    // timestamps
    if !seen.last_modified || doc.timestamps.last_modified.is_empty() {
        doc.timestamps.last_modified = now.to_string();
    }
    if !seen.create || doc.timestamps.create.is_empty() {
        doc.timestamps.create = if seen.last_modified && !doc.timestamps.last_modified.is_empty() {
            doc.timestamps.last_modified.clone()
        } else {
            now.to_string()
        };
    }
    let after = semantic_hash(&doc);
    let content_changed =
        loose.migrated || before != after || prev_semantic_hash.is_some_and(|p| p != after);
    if content_changed && doc.timestamps.last_modified != now {
        doc.timestamps.last_modified = now.to_string();
    }

    let out = render(&doc);
    Conformed { changed: out != text, text: out, findings, doc: Some(doc) }
}

fn derive_name(body: &str, path: &str, t: NodeType) -> String {
    for line in body.lines() {
        if let Some(h) = line.strip_prefix("# ") {
            let h = h.trim();
            if !h.is_empty() {
                return h.to_string();
            }
        }
    }
    let base = path.rsplit('/').next().unwrap_or(path);
    let stem = stem_of(base);
    if stem.is_empty() { t.as_str().to_string() } else { stem }
}

fn insert_if_missing(front: &mut Map<String, Value>, key: &str, v: Value) {
    if !front.contains_key(key) || front[key].is_null() {
        front.insert(key.to_string(), v);
    }
}

/// A path-list key written as a single string (or null) becomes a list.
fn coerce_list(m: &mut Map<String, Value>, key: &str, label: &str, findings: &mut Vec<Finding>) {
    let Some(v) = m.get(key) else { return };
    if v.is_array() {
        return;
    }
    let list = super::parse::list_of(v);
    findings.push(Finding::new("list-coerced", format!("{} was not a list; now {:?}", label, list)));
    m.insert(key.to_string(), Value::Array(list.into_iter().map(Value::String).collect()));
}

fn check_enum(front: &Map<String, Value>, key: &str, allowed: &[&str], findings: &mut Vec<Finding>) {
    if let Some(v) = front.get(key) {
        let s = v.as_str().unwrap_or("");
        if !allowed.contains(&s) {
            findings.push(Finding::new(
                &format!("{}-invalid", key.replace('_', "-")),
                format!("{} {} is not one of {}", key, v, allowed.join("|")),
            ));
        }
    }
}

/// Type-specific defaults and checks (§2.3). Used by conform and NodeDoc::new.
pub(crate) fn apply_type_defaults(doc: &mut NodeDoc, findings: &mut Vec<Finding>) {
    let empty_list = || Value::Array(Vec::new());
    for key in ["scandirs", "actors", "drives", "touches"] {
        coerce_list(&mut doc.front, key, key, findings);
    }
    if let Some(Value::Object(cm)) = doc.front.get_mut("connects") {
        coerce_list(cm, "from", "connects.from", findings);
        coerce_list(cm, "to", "connects.to", findings);
    }
    match doc.nodetype {
        NodeType::Project => {
            insert_if_missing(&mut doc.front, "scandirs", Value::Array(vec![Value::String("{topdir}/".into())]));
            insert_if_missing(&mut doc.front, "file_naming", Value::String("sequence".into()));
            insert_if_missing(&mut doc.front, "gitrepo", Value::String(String::new()));
            check_enum(&doc.front, "file_naming", &["sequence", "uuid12"], findings);
        }
        NodeType::Code => {
            match doc.level.as_deref() {
                None => {
                    doc.level = Some("component".to_string());
                    findings.push(Finding::new("level-missing", "code node had no level; set to component"));
                }
                Some(l) if !LEVELS.contains(&l) => findings.push(Finding::new(
                    "level-invalid",
                    format!("level {:?} is not one of {}", l, LEVELS.join("|")),
                )),
                _ => {}
            }
            if doc.is_connection() {
                let c = doc.front.entry("connects").or_insert_with(|| Value::Object(Map::new()));
                if !c.is_object() {
                    findings.push(Finding::new("connects-invalid", "connects is not a {from, to} mapping; reset"));
                    *c = Value::Object(Map::new());
                }
                let cm = c.as_object_mut().unwrap();
                insert_if_missing(cm, "from", empty_list());
                insert_if_missing(cm, "to", empty_list());
            } else if doc.front.contains_key("connects") {
                findings.push(Finding::new(
                    "connects-on-non-connection",
                    "connects is only read on level: connection nodes",
                ));
            }
            check_enum(&doc.front, "owner", &OWNERS, findings);
        }
        NodeType::Bizreq | NodeType::Techreq => {
            insert_if_missing(&mut doc.front, "status", Value::String("draft".into()));
            check_enum(&doc.front, "status", &REQ_STATUSES, findings);
        }
        NodeType::Usecase => insert_if_missing(&mut doc.front, "actors", empty_list()),
        NodeType::Actor => {
            insert_if_missing(&mut doc.front, "drives", empty_list());
            insert_if_missing(&mut doc.front, "touches", empty_list());
        }
        NodeType::Test | NodeType::Philosophy | NodeType::Agentmem => {}
    }
}
