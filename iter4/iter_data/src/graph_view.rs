//! `GET /api/projects/{p}/graph/view` (iter4 Phase 2, P2): the architecture
//! map in the shape the Project graph tab draws — the same shape as
//! pdy-dev's demos/usecase_map/graph.json, so the viewer ported from it reads
//! either. Built on the fly from the stored vertices and edges:
//!
//! - nodes: the project (main file), actors (the snapshot's actors file),
//!   every code node (context / container / component, id = its folder
//!   relative to the topdir, as the reference names them), every use case
//!   (`usecase:<file stem>`);
//! - edges: `contains` (ownership: a code node's parent through
//!   `children.codenodes`, contexts under the project), `interface` (a
//!   connection: a code node that inputs interface I uses the node that
//!   outputs I; an actor uses the producers of the interfaces its patterns
//!   name), `flow` (a use case's numbered process / data steps) and
//!   `usecase_touches`;
//! - usecases: the reference's record per use case (sequence, steps, branches).
//!
//! The project record's `graph` object tunes the picture without touching
//! the checkout: `hide` (folders left out with everything under them — test
//! tooling, demo scaffolding) and `parents` (a node whose folder sits outside
//! its owner's, placed under that owner).

use serde_json::{Map, Value, json};
use std::collections::{BTreeMap, BTreeSet, HashMap};

fn s<'a>(v: &'a Value, k: &str) -> &'a str {
    v.get(k).and_then(|x| x.as_str()).unwrap_or("")
}

fn strip_top(p: &str) -> String {
    let r = p.strip_prefix("{topdir}/").or_else(|| p.strip_prefix("{topdir}")).unwrap_or(p);
    let r = r.trim_end_matches('/');
    if r.is_empty() { ".".into() } else { r.into() }
}

/// A flowmap / codenodes reference → a view id: `actor:x` when that actor
/// exists; a code node's file (`{topdir}/a/b/b.code.iter.md`) by its path;
/// else `a/b` when that node exists.
fn resolve(r: &str, ids: &BTreeSet<String>, by_file: &HashMap<String, String>) -> Option<String> {
    let r = r.trim();
    if r.is_empty() {
        return None;
    }
    if r.starts_with("actor:") {
        return ids.contains(r).then(|| r.to_string());
    }
    if let Some(id) = by_file.get(r.replace("{topdir}/", "").trim_start_matches("./")) {
        return Some(id.clone());
    }
    let mut p = r.replace("{topdir}/", "");
    p = p.trim_start_matches("./").to_string();
    if p.ends_with(".code.iter.md") {
        p = match p.rfind('/') {
            Some(i) => p[..i].to_string(),
            None => ".".into(),
        };
    }
    let p = p.trim_end_matches('/').to_string();
    ids.contains(&p).then_some(p)
}

/// `*` wildcard match (the actors file's interface patterns).
fn glob_match(pat: &str, text: &str) -> bool {
    let parts: Vec<&str> = pat.split('*').collect();
    if parts.len() == 1 {
        return pat == text;
    }
    let mut pos = 0;
    for (i, part) in parts.iter().enumerate() {
        if part.is_empty() {
            continue;
        }
        if i == 0 {
            if !text.starts_with(part) {
                return false;
            }
            pos = part.len();
            continue;
        }
        match text[pos..].find(part) {
            Some(j) => pos += j + part.len(),
            None => return false,
        }
    }
    parts.last().map(|l| l.is_empty() || text.ends_with(l)).unwrap_or(true)
}

pub fn build(project: &str, vertices: &[Value], edges: &[Value], meta: &Value, settings: &Value) -> Value {
    let hide: Vec<String> = settings
        .get("hide")
        .and_then(|h| h.as_array())
        .into_iter()
        .flatten()
        .filter_map(|x| x.as_str().map(|p| p.trim_end_matches('/').to_string()))
        .collect();
    let hidden = |id: &str| hide.iter().any(|h| id == h || id.starts_with(&format!("{h}/")));
    let parent_override = |id: &str| settings.get("parents").and_then(|p| p.get(id)).and_then(|x| x.as_str()).map(String::from);
    // hidden code vertices drop out of everything below, as if absent
    let kept: Vec<Value> = vertices
        .iter()
        .filter(|v| s(v, "nodetype") != "code" || !hidden(&strip_top(s(v, "dir"))))
        .cloned()
        .collect();
    let vertices = &kept[..];
    let byid: HashMap<&str, &Value> = vertices.iter().map(|v| (s(v, "id"), v)).collect();
    let edges: Vec<Value> = edges.iter().filter(|e| byid.contains_key(s(e, "from")) && byid.contains_key(s(e, "to"))).cloned().collect();
    let edges = &edges[..];
    let is_code = |v: &Value| s(v, "nodetype") == "code";
    // view ids
    let mut vid: HashMap<&str, String> = HashMap::new();
    // a code node's id is its folder (the reference's scheme); when several
    // code nodes share a folder (components beside the modules they own),
    // each is `folder/stem` instead
    let mut per_dir: HashMap<String, usize> = HashMap::new();
    for v in vertices.iter().filter(|v| s(v, "nodetype") == "code") {
        *per_dir.entry(strip_top(s(v, "dir"))).or_default() += 1;
    }
    let code_id = |v: &Value| -> String {
        let dir = strip_top(s(v, "dir"));
        if per_dir.get(&dir).copied().unwrap_or(0) <= 1 {
            return dir;
        }
        let file = s(v, "path").rsplit('/').next().unwrap_or("");
        let stem = file.trim_end_matches(".code.iter.md");
        let stem = if stem.is_empty() || stem == "code.iter.md" { "code" } else { stem };
        if dir == "." { stem.to_string() } else { format!("{dir}/{stem}") }
    };
    for v in vertices {
        let id = match s(v, "nodetype") {
            "main" => "project".to_string(),
            "code" => code_id(v),
            "usecase" => {
                let u = s(v, "ucid");
                if u.is_empty() { format!("usecase:{}", s(v, "name")) } else { u.to_string() }
            }
            _ => continue,
        };
        vid.insert(s(v, "id"), id);
    }
    let mut nodes: BTreeMap<String, Value> = BTreeMap::new();
    let main = vertices.iter().find(|v| s(v, "nodetype") == "main");
    nodes.insert(
        "project".into(),
        json!({"id": "project", "level": "project",
               "name": main.map(|m| s(m, "name")).filter(|n| !n.is_empty()).unwrap_or(project),
               "description": main.map(|m| s(m, "description")).unwrap_or(""),
               "simple_description": "", "long_description": main.map(|m| s(m, "long_description")).unwrap_or(""),
               "path": main.map(|m| strip_top(s(m, "path"))), "parent": Value::Null, "plain": ""}),
    );
    // actors
    let actors: Vec<Value> = meta.get("actors").and_then(|a| a.as_array()).cloned().unwrap_or_default();
    for a in &actors {
        let id = format!("actor:{}", s(a, "id"));
        nodes.insert(
            id.clone(),
            json!({"id": id, "level": "actor", "name": if s(a, "name").is_empty() { s(a, "id") } else { s(a, "name") },
                   "description": s(a, "description"), "simple_description": s(a, "description"),
                   "long_description": "", "path": Value::Null, "parent": Value::Null, "plain": ""}),
        );
    }
    // ownership: children.codenodes edges between code vertices
    let mut parents_of: HashMap<&str, Vec<&str>> = HashMap::new();
    for e in edges.iter().filter(|e| s(e, "kind") == "codenodes") {
        if let (Some(f), Some(t)) = (byid.get(s(e, "from")), byid.get(s(e, "to"))) {
            if is_code(f) && is_code(t) {
                parents_of.entry(s(t, "id")).or_default().push(s(f, "id"));
            }
        }
    }
    let mut out_edges: Vec<Value> = Vec::new();
    for v in vertices.iter().filter(|v| is_code(v)) {
        let me = &vid[s(v, "id")];
        // primary parent: the one whose DAG key is this node's key minus its last segment
        let key = s(v, "key");
        let parent_key = key.rsplit_once('/').map(|(p, _)| p).unwrap_or("");
        let cands = parents_of.get(s(v, "id")).cloned().unwrap_or_default();
        let parent = cands
            .iter()
            .find(|p| byid.get(*p).map(|pv| s(pv, "key") == parent_key).unwrap_or(false))
            .or_else(|| cands.first())
            .map(|p| vid[*p].clone())
            .unwrap_or_else(|| "project".into());
        let parent = match parent_override(me) {
            Some(o) if vid.values().any(|x| x == &o) => o,
            _ => parent,
        };
        let level = if s(v, "level").is_empty() { "container" } else { s(v, "level") };
        nodes.insert(
            me.clone(),
            json!({"id": me, "level": level, "name": if s(v, "name").is_empty() { me.as_str() } else { s(v, "name") },
                   "description": s(v, "description"), "simple_description": s(v, "simple_description"),
                   "long_description": s(v, "long_description"), "path": strip_top(s(v, "path")),
                   "parent": parent, "plain": "", "vertex": s(v, "id"), "orphan": v.get("orphan").cloned().unwrap_or(json!(false)),
                   "code_files": v.get("code_files").cloned().unwrap_or(json!([])),
                   "uncovered_files": v.get("uncovered_files").cloned().unwrap_or(json!([])),
                   "teststate": s(v, "teststate_effective")}),
        );
        out_edges.push(json!({"id": format!("contains|{parent}|{me}"), "type": "contains", "source": parent, "target": me}));
    }
    // interface connections: consumer (inputs I) uses producer (outputs I)
    let mut producers: HashMap<&str, Vec<&str>> = HashMap::new();
    let mut consumers: HashMap<&str, Vec<&str>> = HashMap::new();
    for e in edges {
        let (k, f, t) = (s(e, "kind"), s(e, "from"), s(e, "to"));
        if !byid.get(f).map(|v| is_code(v)).unwrap_or(false) {
            continue;
        }
        match k {
            "outputs" => producers.entry(t).or_default().push(f),
            "inputs" => consumers.entry(t).or_default().push(f),
            _ => {}
        }
    }
    let mut iface_edges = 0usize;
    let mut library = 0usize;
    let mut status_count: BTreeMap<String, usize> = BTreeMap::new();
    let iface_record = |iv: &Value| -> (String, bool, String, String) {
        let name = if s(iv, "interface_id").is_empty() { s(iv, "name") } else { s(iv, "interface_id") }.to_string();
        let transport = s(iv, "transport").to_string();
        let lib = transport == "build" || (transport.is_empty() && name.contains("-lib-"));
        let status = if s(iv, "status").is_empty() { "live".to_string() } else { s(iv, "status").to_string() };
        let transport = if !transport.is_empty() { transport } else if lib { "build".into() } else { "network".into() };
        (name, lib, status, transport)
    };
    for iv in vertices.iter().filter(|v| s(v, "nodetype") == "interface") {
        let iid = s(iv, "id");
        let (name, lib, status, transport) = iface_record(iv);
        for c in consumers.get(iid).cloned().unwrap_or_default() {
            for p in producers.get(iid).cloned().unwrap_or_default() {
                if c == p {
                    continue;
                }
                let (src, tgt) = (vid[c].clone(), vid[p].clone());
                out_edges.push(json!({"id": format!("if|{src}|{tgt}|{name}"), "type": "interface", "source": src, "target": tgt,
                    "interface": name, "label": s(iv, "label"), "kind": s(iv, "kind"), "is_library": lib, "status": status, "edge_kind": "uses",
                    "description": s(iv, "description"), "transport": transport, "flow": if lib { "build" } else { "source_to_target" },
                    "interface_file": strip_top(s(iv, "path")), "origin": "markers"}));
                iface_edges += 1;
                if lib {
                    library += 1;
                }
                *status_count.entry(status.clone()).or_default() += 1;
            }
        }
    }
    // actors use the producers of the interfaces their patterns name
    for a in &actors {
        let aid = format!("actor:{}", s(a, "id"));
        let pats: Vec<String> = a
            .get("uses")
            .and_then(|u| u.as_array())
            .into_iter()
            .flatten()
            .filter_map(|u| u.get("pattern").and_then(|p| p.as_str()).map(String::from))
            .collect();
        let mut seen: BTreeSet<String> = BTreeSet::new();
        for iv in vertices.iter().filter(|v| s(v, "nodetype") == "interface") {
            let (name, lib, status, transport) = iface_record(iv);
            if !pats.iter().any(|p| glob_match(p, &name)) {
                continue;
            }
            for p in producers.get(s(iv, "id")).cloned().unwrap_or_default() {
                let tgt = vid[p].clone();
                if !seen.insert(format!("{tgt}|{name}")) {
                    continue;
                }
                out_edges.push(json!({"id": format!("if|{aid}|{tgt}|{name}"), "type": "interface", "source": aid, "target": tgt,
                    "interface": name, "label": s(iv, "label"), "kind": s(iv, "kind"), "is_library": lib, "status": status, "edge_kind": "uses",
                    "description": s(iv, "description"), "transport": transport, "flow": "both",
                    "interface_file": strip_top(s(iv, "path")), "origin": "actors"}));
                iface_edges += 1;
                *status_count.entry(status.clone()).or_default() += 1;
            }
        }
    }
    // use cases
    let ids: BTreeSet<String> = nodes.keys().cloned().collect();
    let by_file: HashMap<String, String> = vertices
        .iter()
        .filter(|v| s(v, "nodetype") == "code")
        .map(|v| (strip_top(s(v, "path")), vid[s(v, "id")].clone()))
        .collect();
    let mut usecases: Vec<Value> = Vec::new();
    let mut flow_counts: BTreeMap<&str, usize> = BTreeMap::new();
    let mut touches: HashMap<String, Vec<Value>> = HashMap::new();
    let mut touch_edges = 0usize;
    for uv in vertices.iter().filter(|v| s(v, "nodetype") == "usecase") {
        let uid = vid[s(uv, "id")].clone();
        let codenodes: Vec<String> = edges
            .iter()
            .filter(|e| s(e, "from") == s(uv, "id") && s(e, "kind") == "codenodes")
            .filter_map(|e| byid.get(s(e, "to")).filter(|t| is_code(t)).map(|t| vid[s(t, "id")].clone()))
            .collect();
        let fm = uv.get("flowmap").cloned().unwrap_or(Value::Null);
        let has = fm.is_object();
        let mut uc = json!({"id": uid, "name": s(uv, "name"), "description": s(uv, "description"),
            "path": strip_top(s(uv, "path")), "has_flowmap": has, "summary": if has { s(&fm, "summary") } else { "" },
            "codenodes": codenodes, "sequence": [], "sequence_source": "none",
            "process_steps": [], "data_steps": [], "branches": [], "unresolved": []});
        let mut seq: Vec<String> = Vec::new();
        let mut unresolved: Vec<String> = Vec::new();
        let mut branches: Vec<String> = Vec::new();
        let mut steps_of: HashMap<String, BTreeSet<i64>> = HashMap::new();
        if has {
            for r in fm.get("sequence").and_then(|x| x.as_array()).into_iter().flatten() {
                let raw = r.as_str().unwrap_or("");
                match resolve(raw, &ids, &by_file) {
                    Some(n) if !seq.contains(&n) => seq.push(n),
                    Some(_) => {}
                    None => unresolved.push(raw.to_string()),
                }
            }
            for (kind, key, textkey) in [("process", "process_flow", "what"), ("data", "data_flow", "data")] {
                let mut recs: Vec<Value> = Vec::new();
                for (i, st) in fm.get(key).and_then(|x| x.as_array()).into_iter().flatten().enumerate() {
                    let fr = st.get("from").map(|x| x.as_str().map(String::from).unwrap_or_else(|| x.to_string())).unwrap_or_default();
                    let to = st.get("to").map(|x| x.as_str().map(String::from).unwrap_or_else(|| x.to_string())).unwrap_or_default();
                    let (a, b) = (resolve(&fr, &ids, &by_file), resolve(&to, &ids, &by_file));
                    for (raw, n) in [(&fr, &a), (&to, &b)] {
                        if n.is_none() && !unresolved.contains(raw) {
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
                    if let Some(p) = str_of("plain") {
                        rec.insert("plain".into(), json!(p));
                    }
                    if kind == "data" {
                        rec.insert("stored".into(), json!(st.get("stored").and_then(|x| x.as_bool()).unwrap_or(false)));
                    }
                    if let (Some(a), Some(b)) = (&a, &b) {
                        let mut edge = json!({"id": format!("flow|{uid}|{kind}|{i}"), "type": "flow", "kind": kind,
                            "usecase": uid, "source": a, "target": b});
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
            uc["sequence_source"] = json!("flowmap");
        } else if !uc["codenodes"].as_array().map(|a| a.is_empty()).unwrap_or(true) {
            seq = uc["codenodes"].as_array().unwrap().iter().filter_map(|x| x.as_str().map(String::from)).collect();
            uc["sequence_source"] = json!("codenodes");
        }
        // hierarchical picture (usecase_map.rs): every node tagged with this use
        // case, and the tops — tagged nodes whose owner in this view is not
        // tagged — which are the only nodes the use case links to
        let members: Vec<String> = vertices
            .iter()
            .filter(|v| is_code(v) && v.get("usecases").and_then(|u| u.as_array()).map(|a| a.iter().any(|x| x.as_str() == Some(uid.as_str()))).unwrap_or(false))
            .map(|v| vid[s(v, "id")].clone())
            .collect();
        let member_set: BTreeSet<&String> = members.iter().collect();
        let view_parent = |n: &str| nodes.get(n).and_then(|x| x.get("parent")).and_then(|p| p.as_str()).map(String::from);
        let top_of = |n: &String| -> String {
            let mut cur = n.clone();
            let mut hops = 0;
            while let Some(p) = view_parent(&cur) {
                if !member_set.contains(&p) || hops > 64 {
                    break;
                }
                cur = p;
                hops += 1;
            }
            cur
        };
        let mut tops: Vec<String> = Vec::new();
        for n in seq.iter().filter(|n| member_set.contains(n)).chain(members.iter()) {
            let t = top_of(n);
            if !tops.contains(&t) {
                tops.push(t);
            }
        }
        // the use case links to the highest level present, and only to it
        // (Stephen, 2026-09-29): its actors when any take part; else its
        // top-level parts of the highest level (context before container
        // before component); everything else hangs below by ownership
        let actors: Vec<String> = seq.iter().filter(|n| n.starts_with("actor:") && nodes.contains_key(*n)).cloned().collect();
        let rank = |n: &String| ["context", "container", "component"].iter().position(|l| nodes.get(n).map(|x| s(x, "level") == *l).unwrap_or(false)).unwrap_or(9);
        let links: Vec<String> = if !actors.is_empty() {
            actors
        } else {
            let best = tops.iter().map(rank).min().unwrap_or(9);
            tops.iter().filter(|n| rank(n) == best).cloned().collect()
        };
        for (order, n) in links.iter().enumerate() {
            out_edges.push(json!({"id": format!("touch|{uid}|{n}"), "type": "usecase_touches", "source": uid, "target": n,
                "usecase": uid, "order": order + 1, "from_flowmap": has}));
            touch_edges += 1;
        }
        uc["links"] = json!(links);
        for n in &members {
            let order = seq.iter().position(|x| x == n).map(|i| i + 1);
            touches.entry(n.clone()).or_default().push(json!({"usecase": uid, "order": order, "named": order.is_some(),
                "steps": steps_of.get(n).map(|s| s.iter().cloned().collect::<Vec<_>>()).unwrap_or_default()}));
        }
        uc["members"] = json!(members);
        uc["tops"] = json!(tops);
        uc["sequence"] = json!(seq);
        uc["unresolved"] = json!(unresolved);
        uc["branches"] = json!(branches);
        nodes.insert(
            uid.clone(),
            json!({"id": uid, "level": "usecase", "name": s(uv, "name"), "description": s(uv, "description"),
                   "simple_description": uc["summary"], "long_description": "", "path": strip_top(s(uv, "path")),
                   "parent": Value::Null, "plain": ""}),
        );
        usecases.push(uc);
    }
    for (n, list) in touches {
        if let Some(node) = nodes.get_mut(&n) {
            node["usecases"] = json!(list);
        }
    }
    let order = ["project", "actor", "context", "container", "component", "usecase"];
    let mut node_list: Vec<Value> = nodes.into_values().collect();
    node_list.sort_by_key(|n| (order.iter().position(|l| *l == s(n, "level")).unwrap_or(9), s(n, "id").to_string()));
    let mut per_level: BTreeMap<String, usize> = BTreeMap::new();
    for n in &node_list {
        *per_level.entry(s(n, "level").to_string()).or_default() += 1;
    }
    let mut per_type: BTreeMap<String, usize> = BTreeMap::new();
    for e in &out_edges {
        *per_type.entry(s(e, "type").to_string()).or_default() += 1;
    }
    let code_files = node_list.iter().filter(|n| ["context", "container", "component"].contains(&s(n, "level"))).count();
    let with_simple = node_list
        .iter()
        .filter(|n| ["context", "container", "component"].contains(&s(n, "level")) && !s(n, "simple_description").is_empty())
        .count();
    let _ = touch_edges;
    json!({
        "summary": {
            "generated_at": meta.get("updated").cloned().unwrap_or(Value::Null),
            "hash": meta.get("hash").cloned().unwrap_or(Value::Null),
            "generator": "iter4 iter_data graph/view",
            "repo": meta.get("repo").cloned().unwrap_or(Value::Null),
            "project": project,
            "counts": {
                "nodes": node_list.len(), "nodes_per_level": per_level,
                "edges": out_edges.len(), "edges_per_type": per_type,
                "interface_edges": iface_edges, "interface_edges_library": library,
                "interface_edges_per_status": status_count, "flow_edges_per_kind": flow_counts,
                "usecases": usecases.len(),
                "usecases_with_flowmap": usecases.iter().filter(|u| u["has_flowmap"] == true).count(),
                "code_files": code_files, "code_files_with_simple_description": with_simple,
            },
            "warnings": usecases.iter().flat_map(|u| {
                let id = s(u, "id").to_string();
                u["unresolved"].as_array().cloned().unwrap_or_default().into_iter()
                    .map(move |r| json!(format!("{id}: `{}` does not resolve to any node", r.as_str().unwrap_or(""))))
            }).collect::<Vec<_>>(),
            "errors": [],
        },
        "nodes": node_list,
        "edges": out_edges,
        "usecases": usecases,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(id: &str, t: &str, extra: Value) -> Value {
        let mut b = json!({"id": id, "nodetype": t, "name": id});
        for (k, x) in extra.as_object().unwrap() {
            b[k] = x.clone();
        }
        b
    }
    fn e(f: &str, k: &str, t: &str) -> Value {
        json!({"from": f, "kind": k, "to": t})
    }

    #[test]
    fn without_actors_a_use_case_links_only_to_its_highest_level() {
        let mut vs = vec![
            v("m", "main", json!({"path": "{topdir}/main.iter.md"})),
            v("ctx", "code", json!({"level": "context", "dir": "{topdir}/data", "key": "data", "path": "{topdir}/data/data.code.iter.md"})),
            v("api", "code", json!({"level": "container", "dir": "{topdir}/data/api", "key": "data/api", "path": "{topdir}/data/api/api.code.iter.md"})),
            v("solo", "code", json!({"level": "container", "dir": "{topdir}/solo", "key": "solo", "path": "{topdir}/solo/solo.code.iter.md"})),
            v("u", "usecase", json!({"ucid": "usecase:x"})),
        ];
        let es = vec![e("m", "root", "ctx"), e("m", "root", "solo"), e("ctx", "codenodes", "api"), e("u", "codenodes", "api"), e("u", "codenodes", "solo")];
        iter_local::usecase_map::usecase_map(&mut vs, &es);
        let g = build("demo", &vs, &es, &json!({}), &json!({}));
        let uc = &g["usecases"][0];
        assert_eq!(uc["tops"], json!(["data", "solo"]));
        assert_eq!(uc["links"], json!(["data"]), "a context outranks a top-level container");
    }

    #[test]
    fn view_has_ownership_connections_flows_and_actors() {
        let mut vs = vec![
            v("m", "main", json!({"name": "Demo", "path": "{topdir}/main.iter.md"})),
            v("ctx", "code", json!({"level": "context", "dir": "{topdir}/data", "key": "data", "path": "{topdir}/data/data.code.iter.md"})),
            v("api", "code", json!({"level": "container", "dir": "{topdir}/data/api", "key": "data/api"})),
            v("db", "code", json!({"level": "container", "dir": "{topdir}/data/db", "key": "data/db"})),
            v("lib", "code", json!({"level": "component", "dir": "{topdir}/data/api/lib", "key": "data/api/lib"})),
            v("i1", "interface", json!({"interface_id": "db-read", "kind": "request-reply", "path": "{topdir}/interfaces/db-read.interface.iter.md"})),
            v("i2", "interface", json!({"interface_id": "api-lib-parse", "kind": "request-reply"})),
            v("u", "usecase", json!({"ucid": "usecase:read", "flowmap": {"summary": "s", "sequence": ["actor:worker"],
                "process_flow": [{"step": 1, "from": "actor:worker", "to": "{topdir}/data/api/api.code.iter.md", "what": "asks"},
                                 {"step": 2, "from": "data/api", "to": "data/db", "what": "reads", "status": "not-built"}],
                "data_flow": [{"step": 1, "from": "data/api", "to": "nowhere", "data": "x"}]}})),
        ];
        let es = vec![
            e("m", "root", "ctx"), e("ctx", "codenodes", "api"), e("ctx", "codenodes", "db"), e("api", "codenodes", "lib"),
            e("api", "inputs", "i1"), e("db", "outputs", "i1"), e("api", "inputs", "i2"), e("lib", "outputs", "i2"),
            e("m", "root", "u"), e("u", "codenodes", "api"),
        ];
        let meta = json!({"actors": [{"id": "worker", "name": "Worker", "uses": [{"pattern": "db-*"}]}]});
        iter_local::usecase_map::usecase_map(&mut vs, &es);
        let g = build("demo", &vs, &es, &meta, &json!({}));
        let node = |id: &str| g["nodes"].as_array().unwrap().iter().find(|n| n["id"] == id).cloned().unwrap();
        assert_eq!(node("project")["name"], "Demo");
        assert_eq!((node("data")["parent"].as_str(), node("data/api/lib")["parent"].as_str()), (Some("project"), Some("data/api")));
        let has = |t: &str, a: &str, b: &str| g["edges"].as_array().unwrap().iter().any(|x| x["type"] == t && x["source"] == a && x["target"] == b);
        assert!(has("contains", "project", "data") && has("contains", "data/api", "data/api/lib"));
        assert!(has("interface", "data/api", "data/db"), "consumer uses producer");
        let lib = g["edges"].as_array().unwrap().iter().find(|x| x["interface"] == "api-lib-parse").unwrap();
        assert_eq!(lib["is_library"], true);
        assert!(has("interface", "actor:worker", "data/db"), "actor pattern db-* → producer of db-read");
        assert!(has("flow", "actor:worker", "data/api") && has("flow", "data/api", "data/db"));
        let uc = &g["usecases"][0];
        assert_eq!(uc["sequence"], json!(["actor:worker", "data/api", "data/db"]));
        assert_eq!(uc["unresolved"], json!(["nowhere"]));
        assert_eq!(uc["process_steps"][1]["status"], "not-built");
        assert_eq!(node("data/api")["usecases"][0]["steps"], json!([1, 2]));
        // hierarchical: every tagged part is a member; the use case links to the top only
        assert_eq!(uc["members"], json!(["data", "data/api", "data/db"]));
        assert_eq!(uc["tops"], json!(["data"]));
        // an actor takes part (actor:worker in its steps), so the use case links to it alone
        assert!(has("usecase_touches", "usecase:read", "actor:worker") && !has("usecase_touches", "usecase:read", "data"));
        assert_eq!(uc["links"], json!(["actor:worker"]));
        assert_eq!(node("data")["usecases"][0]["named"], false, "an owner on the chain, not a named part");
        assert_eq!(g["summary"]["counts"]["nodes_per_level"]["component"], 1);
        // settings: a hidden folder drops with its subtree; a parent override moves a node
        let g2 = build("demo", &vs, &es, &meta, &json!({"hide": ["data/api"], "parents": {"data/db": "project"}}));
        let ids: Vec<&str> = g2["nodes"].as_array().unwrap().iter().map(|n| n["id"].as_str().unwrap()).collect();
        assert!(!ids.contains(&"data/api") && !ids.contains(&"data/api/lib"), "{ids:?}");
        let db = g2["nodes"].as_array().unwrap().iter().find(|n| n["id"] == "data/db").unwrap();
        assert_eq!(db["parent"], "project");
        assert!(g2["usecases"][0]["unresolved"].as_array().unwrap().iter().any(|u| u == "data/api"));
        assert!(glob_match("pdy-sdk-*", "pdy-sdk-worker-x") && !glob_match("pdy-sdk-*", "x-pdy-sdk") && glob_match("*-lib-*", "a-lib-b"));
    }
}
