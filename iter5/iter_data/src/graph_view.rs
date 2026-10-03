//! `GET /api/projects/{p}/graph/view` (iter5): the project graph in the
//! shape the Project graph tab draws. Built on the fly from the stored nodes
//! and derived edges (nodes.rs):
//!
//! - nodes: every live node with `level` (the code level, else the
//!   nodetype), `parent` (the owner: who holds it in `children.codenodes` —
//!   preferring a code owner whose folder contains it — or `children.tests` /
//!   `children.reqs`; root contexts hang under the project node), `owners`
//!   (the chain up to the root), `file_state`, `test`, and `usecases` (the use
//!   cases it takes part in, with its order and flow steps);
//! - edges: every derived edge (`type` = its kind: codenodes, tests, reqs,
//!   supplies, connects, drives, touches, uses), plus `flow` edges from each
//!   use case's numbered process / data steps;
//! - usecases: per use case its parts (`uses`), actors (`drives`), flowmap,
//!   sequence, steps, branches and the references that resolve to no node.
//!
//! The project record's `graph.hide` (folders left out with everything under
//! them) still tunes the picture without touching the repo.

use crate::nodes::{Graph, StoredNode, type_label};
use iter_core::nodefile::{self as nf, EdgeKind, NodeType};
use serde_json::{Map, Value, json};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque};

fn s<'a>(v: &'a Value, k: &str) -> &'a str {
    v.get(k).and_then(|x| x.as_str()).unwrap_or("")
}

fn level_of(n: &StoredNode) -> String {
    match (&n.doc.nodetype, &n.doc.level) {
        (NodeType::Code, Some(l)) => l.clone(),
        (NodeType::Code, None) => "component".into(),
        (t, _) => t.as_str().to_string(),
    }
}

/// A flowmap reference → a node id: a node path (with or without
/// `{topdir}/`), `actor:<name>`, a code folder (`a/b`), a node id or name.
fn resolve_ref(r: &str, nodes: &[&StoredNode]) -> Option<String> {
    let r = r.trim();
    if r.is_empty() {
        return None;
    }
    let rel = r.strip_prefix("{topdir}/").unwrap_or(r).trim_start_matches("./").trim_end_matches('/');
    let full = format!("{{topdir}}/{rel}");
    if let Some(n) = nodes.iter().find(|n| n.doc.path == full || n.doc.id == r) {
        return Some(n.doc.id.clone());
    }
    if let Some(a) = r.strip_prefix("actor:") {
        return nodes
            .iter()
            .find(|n| n.doc.nodetype == NodeType::Actor && (n.doc.name == a || nf::stem_of(&n.doc.path) == a || nf::slug(&n.doc.name) == a))
            .map(|n| n.doc.id.clone());
    }
    if let Some(n) = nodes.iter().find(|n| n.doc.nodetype == NodeType::Code && nf::dir_of(&n.doc.path) == full) {
        return Some(n.doc.id.clone());
    }
    nodes.iter().find(|n| n.doc.name == r).map(|n| n.doc.id.clone())
}

pub fn build(g: &Graph, project_rec: &Value) -> Value {
    let hide: Vec<String> = project_rec
        .pointer("/graph/hide")
        .and_then(|h| h.as_array())
        .into_iter()
        .flatten()
        .filter_map(|x| x.as_str().map(|p| format!("{{topdir}}/{}", p.trim_start_matches("{topdir}/").trim_end_matches('/'))))
        .collect();
    let hidden = |p: &str| hide.iter().any(|h| p.starts_with(&format!("{h}/")));
    let nodes: Vec<&StoredNode> = g.live().filter(|n| !hidden(&n.doc.path)).collect();
    let ids: HashSet<&str> = nodes.iter().map(|n| n.id()).collect();
    let byid: HashMap<&str, &StoredNode> = nodes.iter().map(|n| (n.id(), *n)).collect();
    let edges: Vec<nf::Edge> = g.edges().into_iter().filter(|e| ids.contains(e.from.as_str()) && ids.contains(e.to.as_str())).collect();
    let project_id = g.project_node().map(|p| p.doc.id.clone()).filter(|p| ids.contains(p.as_str()));

    // ownership: the parent of each node
    let mut parent: HashMap<String, String> = HashMap::new();
    for n in &nodes {
        let me = n.id();
        let holders: Vec<&nf::Edge> = edges
            .iter()
            .filter(|e| e.to == me && matches!(e.kind, EdgeKind::Codenodes | EdgeKind::Tests | EdgeKind::Reqs))
            .collect();
        let dir = nf::dir_of(&n.doc.path);
        let pick = holders
            .iter()
            .filter(|e| e.kind == EdgeKind::Codenodes)
            .find(|e| byid.get(e.from.as_str()).is_some_and(|p| p.doc.nodetype == NodeType::Code && dir.starts_with(&format!("{}/", nf::dir_of(&p.doc.path)))))
            .or_else(|| holders.iter().find(|e| e.kind == EdgeKind::Codenodes && byid.get(e.from.as_str()).is_some_and(|p| p.doc.nodetype == NodeType::Code)))
            .or_else(|| holders.iter().find(|e| e.kind == EdgeKind::Codenodes))
            .or_else(|| holders.first());
        if let Some(e) = pick {
            parent.insert(me.to_string(), e.from.clone());
        }
    }
    let owners_of = |id: &str| -> Vec<String> {
        let mut chain = Vec::new();
        let mut cur = id.to_string();
        while let Some(p) = parent.get(&cur) {
            if chain.contains(p) || chain.len() > 64 || p == id {
                break;
            }
            chain.push(p.clone());
            cur = p.clone();
        }
        chain
    };

    let mut out_edges: Vec<Value> = edges
        .iter()
        .map(|e| json!({"id": format!("{}|{}|{}", e.kind.as_str(), e.from, e.to), "type": e.kind.as_str(), "kind": e.kind.as_str(), "source": e.from, "target": e.to}))
        .collect();

    // use cases
    let mut usecases: Vec<Value> = Vec::new();
    let mut touches: HashMap<String, Vec<Value>> = HashMap::new();
    let mut flow_counts: BTreeMap<&str, usize> = BTreeMap::new();
    let mut warnings: Vec<Value> = Vec::new();
    for uv in nodes.iter().filter(|n| n.doc.nodetype == NodeType::Usecase) {
        let uid = uv.id().to_string();
        let parts: Vec<String> = edges.iter().filter(|e| e.from == uid && e.kind == EdgeKind::Uses).map(|e| e.to.clone()).collect();
        let actors: Vec<String> = edges.iter().filter(|e| e.to == uid && e.kind == EdgeKind::Drives).map(|e| e.from.clone()).collect();
        // members: the parts and everything they own
        let mut members: Vec<String> = Vec::new();
        let mut q: VecDeque<String> = parts.iter().cloned().collect();
        while let Some(x) = q.pop_front() {
            if members.contains(&x) {
                continue;
            }
            members.push(x.clone());
            for e in edges.iter().filter(|e| e.from == x && e.kind == EdgeKind::Codenodes) {
                q.push_back(e.to.clone());
            }
        }
        let fm = uv.doc.front.get("flowmap").cloned().unwrap_or(Value::Null);
        let has = fm.is_object();
        let mut seq: Vec<String> = Vec::new();
        let mut unresolved: Vec<String> = Vec::new();
        let mut branches: Vec<String> = Vec::new();
        let mut steps_of: HashMap<String, BTreeSet<i64>> = HashMap::new();
        let mut uc = json!({"id": uid, "name": uv.doc.name, "desc": uv.doc.desc, "path": uv.doc.path, "has_flowmap": has,
            "summary": if has { s(&fm, "summary") } else { "" }, "flowmap": fm, "parts": parts, "actors": actors,
            "process_steps": [], "data_steps": []});
        if has {
            for r in fm.get("sequence").and_then(|x| x.as_array()).into_iter().flatten() {
                let raw = r.as_str().unwrap_or("");
                match resolve_ref(raw, &nodes) {
                    Some(n) if !seq.contains(&n) => seq.push(n),
                    Some(_) => {}
                    None => unresolved.push(raw.to_string()),
                }
            }
            for (kind, key, textkey) in [("process", "process_flow", "what"), ("data", "data_flow", "data")] {
                let mut recs: Vec<Value> = Vec::new();
                for (i, st) in fm.get(key).and_then(|x| x.as_array()).into_iter().flatten().enumerate() {
                    let txt = |k: &str| st.get(k).map(|x| x.as_str().map(String::from).unwrap_or_else(|| x.to_string())).unwrap_or_default();
                    let (fr, to) = (txt("from"), txt("to"));
                    let (a, b) = (resolve_ref(&fr, &nodes), resolve_ref(&to, &nodes));
                    for (raw, n) in [(&fr, &a), (&to, &b)] {
                        if n.is_none() && !raw.is_empty() && !unresolved.contains(raw) {
                            unresolved.push(raw.clone());
                        }
                    }
                    let step = st.get("step").and_then(|x| x.as_i64().or_else(|| x.as_str().and_then(|t| t.parse().ok())));
                    let branch = st.get("branch").and_then(|x| x.as_str()).filter(|b| !b.is_empty()).map(String::from);
                    if let Some(b) = &branch {
                        if !branches.contains(b) {
                            branches.push(b.clone());
                        }
                    }
                    let str_of = |k: &str| st.get(k).and_then(|x| x.as_str()).map(|x| x.trim().to_string()).filter(|x| !x.is_empty());
                    let mut rec = Map::new();
                    rec.insert("step".into(), json!(step));
                    rec.insert("from".into(), json!(a));
                    rec.insert("to".into(), json!(b));
                    rec.insert("from_ref".into(), json!(fr));
                    rec.insert("to_ref".into(), json!(to));
                    rec.insert("via".into(), json!(str_of("via")));
                    rec.insert(textkey.into(), json!(str_of(textkey).unwrap_or_default()));
                    rec.insert("evidence".into(), json!(str_of("evidence")));
                    rec.insert("branch".into(), json!(branch));
                    rec.insert("status".into(), json!(str_of("status")));
                    if kind == "data" {
                        rec.insert("stored".into(), json!(st.get("stored").and_then(|x| x.as_bool()).unwrap_or(false)));
                    }
                    if let (Some(a), Some(b)) = (&a, &b) {
                        let mut edge = json!({"id": format!("flow|{uid}|{kind}|{i}"), "type": "flow", "kind": kind, "usecase": uid, "source": a, "target": b});
                        for (k, v) in &rec {
                            if !["from", "to", "from_ref", "to_ref"].contains(&k.as_str()) {
                                edge[k] = v.clone();
                            }
                        }
                        out_edges.push(edge);
                        *flow_counts.entry(kind).or_default() += 1;
                        for n in [a, b] {
                            if !seq.contains(n) {
                                seq.push(n.clone());
                            }
                            if let Some(sn) = step {
                                steps_of.entry(n.clone()).or_default().insert(sn);
                            }
                        }
                    }
                    recs.push(Value::Object(rec));
                }
                uc[format!("{kind}_steps")] = json!(recs);
            }
        } else {
            seq = actors.iter().chain(parts.iter()).cloned().collect();
        }
        for r in &unresolved {
            warnings.push(json!(format!("{}: `{r}` does not resolve to any node", uv.doc.name)));
        }
        for n in members.iter().chain(actors.iter()) {
            let order = seq.iter().position(|x| x == n).map(|i| i + 1);
            touches.entry(n.clone()).or_default().push(json!({"usecase": uid, "order": order, "named": order.is_some(),
                "steps": steps_of.get(n).map(|s| s.iter().cloned().collect::<Vec<_>>()).unwrap_or_default()}));
        }
        uc["members"] = json!(members);
        uc["sequence"] = json!(seq);
        uc["sequence_source"] = json!(if has { "flowmap" } else { "edges" });
        uc["unresolved"] = json!(unresolved);
        uc["branches"] = json!(branches);
        usecases.push(uc);
    }

    let order = ["project", "actor", "usecase", "context", "container", "component", "connection", "test", "philosophy", "bizreq", "techreq", "req"];
    let mut node_list: Vec<Value> = nodes
        .iter()
        .map(|n| {
            let id = n.id();
            let p = parent.get(id).cloned().or_else(|| {
                // a root context hangs under the project node
                (n.doc.nodetype == NodeType::Code && n.doc.level.as_deref() == Some("context"))
                    .then(|| project_id.clone())
                    .flatten()
                    .filter(|p| p != id)
            });
            json!({"id": id, "nodetype": n.doc.nodetype, "level": level_of(n), "type_label": type_label(&n.doc),
                   "name": n.doc.name, "desc": n.doc.desc, "path": n.doc.path, "creator": n.doc.creator,
                   "teststate": n.doc.teststate, "parent": p, "owners": owners_of(id),
                   "file_state": n.file_state, "node_version": n.node_version, "file_version": n.file_version,
                   "pending": matches!(n.file_state.as_str(), "pending_write" | "pending_delete" | "designed"),
                   "test": n.test, "timestamps": n.doc.timestamps,
                   "usecases": touches.get(id).cloned().unwrap_or_default()})
        })
        .collect();
    // requirement files carry their sections (the UI's table); each section
    // is a req node hanging off its file by a contains edge (§2.8)
    let reqs: Vec<&crate::reqs::ReqNode> = g.reqs.iter().filter(|r| ids.contains(r.file.as_str())).collect();
    for n in node_list.iter_mut() {
        if matches!(s(n, "nodetype"), "bizreq" | "techreq") {
            let mine: Vec<Value> = reqs
                .iter()
                .filter(|r| r.file == s(n, "id"))
                .map(|r| json!({"id": r.id, "key": r.key, "title": r.title, "status": r.status}))
                .collect();
            n["req_count"] = json!(mine.len());
            n["reqs"] = json!(mine);
        }
    }
    for r in &reqs {
        let f = byid[r.file.as_str()];
        let mut owners = vec![r.file.clone()];
        owners.extend(owners_of(&r.file));
        node_list.push(json!({"id": r.id, "nodetype": crate::reqs::REQ, "level": crate::reqs::REQ, "type_label": "Requirement",
            "name": r.name, "desc": r.desc, "path": r.path, "key": r.key, "title": r.title, "status": r.status, "text": r.text,
            "order": r.order, "file": r.file, "file_type": r.file_type, "creator": f.doc.creator, "teststate": "",
            "parent": r.file, "owners": owners, "file_state": f.file_state, "node_version": f.node_version, "file_version": f.file_version,
            "pending": matches!(f.file_state.as_str(), "pending_write" | "pending_delete" | "designed"),
            "test": Value::Null, "timestamps": f.doc.timestamps, "usecases": []}));
        out_edges.push(json!({"id": format!("{}|{}|{}", crate::reqs::CONTAINS, r.file, r.id), "type": crate::reqs::CONTAINS,
            "kind": crate::reqs::CONTAINS, "source": r.file, "target": r.id}));
    }
    node_list.sort_by_key(|n| {
        (order.iter().position(|l| *l == s(n, "level")).unwrap_or(99), s(n, "file").to_string(), n["order"].as_u64().unwrap_or(0), s(n, "path").to_string())
    });
    let mut per_level: BTreeMap<String, usize> = BTreeMap::new();
    for n in &node_list {
        *per_level.entry(s(n, "level").to_string()).or_default() += 1;
    }
    let mut per_type: BTreeMap<String, usize> = BTreeMap::new();
    for e in &out_edges {
        *per_type.entry(s(e, "type").to_string()).or_default() += 1;
    }
    let pending = g.nodes.iter().filter(|n| !n.deleted && matches!(n.file_state.as_str(), "pending_write" | "pending_delete")).count();
    let designed = g.live().filter(|n| n.file_state == "designed").count();
    json!({
        "summary": {
            "project": g.project, "generator": "iter5 iter_data graph/view", "project_node": project_id,
            "served": g.served, "build": project_rec.get("build").cloned().unwrap_or(Value::Null),
            "file_sync": {"pending": pending, "designed": designed, "synced": pending == 0 && designed == 0, "states": crate::nodes::state_counts(g)},
            "counts": {
                "nodes": node_list.len(), "nodes_per_level": per_level, "edges": out_edges.len(), "edges_per_type": per_type,
                "flow_edges_per_kind": flow_counts, "usecases": usecases.len(),
                "usecases_with_flowmap": usecases.iter().filter(|u| u["has_flowmap"] == true).count(),
            },
            "warnings": warnings,
        },
        "nodes": node_list,
        "edges": out_edges,
        "usecases": usecases,
    })
}
