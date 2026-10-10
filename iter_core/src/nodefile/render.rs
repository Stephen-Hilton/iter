//! [`NodeDoc`] → canonical node-file text (§2.2 key order).
//!
//! id, name, desc, creator, teststate, type-specific keys (alphabetical,
//! `level` among them), children (codedirs, codenodes, tests, reqs, extras
//! alphabetical), timestamps (create, last_modified, last_tested, extras), then
//! the body verbatim.

use super::yaml::{emit_entry, flow_strings, quote, scalar_str};
use super::{NodeDoc, NodeType, is_valid_id};
use serde_json::Value;
use std::collections::BTreeMap;

/// Keys owned by NodeDoc fields; a `front` entry with one of these names is
/// never rendered (it would duplicate the real key).
const COMMON: &[&str] = &["id", "name", "desc", "creator", "teststate", "level", "children", "timestamps"];

/// The canonical text of a node. Agent memory (never synced, engine-written
/// plain markdown) renders as its body alone.
pub fn render(doc: &NodeDoc) -> String {
    if doc.nodetype == NodeType::Agentmem {
        return doc.body.clone();
    }
    render_full(doc)
}

pub(crate) fn render_full(doc: &NodeDoc) -> String {
    let mut out = String::from("---\n");
    let id = if is_valid_id(&doc.id) { doc.id.clone() } else { quote(&doc.id) };
    out.push_str(&format!("id: {}\n", id));
    out.push_str(&format!("name: {}\n", quote(&doc.name)));
    out.push_str(&format!("desc: {}\n", quote(&doc.desc)));
    out.push_str(&format!("creator: {}\n", quote(&doc.creator)));
    out.push_str(&format!("teststate: {}\n", scalar_str(&doc.teststate)));

    let mut specific: BTreeMap<&str, Value> = BTreeMap::new();
    for (k, v) in &doc.front {
        if !COMMON.contains(&k.as_str()) && !k.is_empty() {
            specific.insert(k.as_str(), v.clone());
        }
    }
    if let Some(level) = &doc.level {
        specific.insert("level", Value::String(level.clone()));
    }
    for (k, v) in &specific {
        emit_entry(&mut out, 0, k, v);
    }

    out.push_str("children:\n");
    let c = &doc.children;
    let mut rows: Vec<(&str, &Vec<String>)> =
        vec![("codedirs", &c.codedirs), ("codenodes", &c.codenodes), ("tests", &c.tests), ("reqs", &c.reqs)];
    for (k, v) in &c.extra {
        if !matches!(k.as_str(), "codedirs" | "codenodes" | "tests" | "reqs") {
            rows.push((k.as_str(), v));
        }
    }
    for (k, v) in rows {
        let key = format!("{}:", super::yaml::yaml_key(k));
        out.push_str(&format!("  {:<11}{}\n", key, flow_strings(v)));
    }

    let t = &doc.timestamps;
    let mut ts = vec![
        format!("create: {}", quote(&t.create)),
        format!("last_modified: {}", quote(&t.last_modified)),
        format!("last_tested: {}", quote(&t.last_tested)),
    ];
    for (k, v) in &t.extra {
        if !matches!(k.as_str(), "create" | "last_modified" | "last_tested") {
            ts.push(format!("{}: {}", super::yaml::scalar_str(k), quote(v)));
        }
    }
    out.push_str(&format!("timestamps: {{{}}}\n", ts.join(", ")));
    out.push_str("---\n");
    out.push_str(&doc.body);
    out
}
