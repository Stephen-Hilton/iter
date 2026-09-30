//! `usecase_map()` (iter4, decided 2026-09-29): a use case is drawn
//! hierarchically, from tags, not from a graph traversal.
//!
//! 1. The use case names the parts it needs, and nothing else: its file's
//!    `children.codenodes` (what the `usecase` agent writes when a use case is
//!    added) plus every code node its flowmap's steps mention.
//! 2. This function walks each named part up its ownership chain
//!    (`children.codenodes` of the code node above it) to the highest code
//!    node it can reach (a context, or whatever sits directly under main), and
//!    tags every node on the way with the use case's id (`usecases: [...]` on
//!    the vertex).
//! 3. Readers pull a use case's picture with one lookup on that array (an
//!    ArangoDB persistent array index on `node.usecases[*]`), and draw it as
//!    the use case → its top-level parts → plain ownership lines down.
//!
//! It runs wherever a map is stored (iter_data, on every sync), so the tags
//! can never drift from the files: they are recomputed from the snapshot.

use serde_json::{Value, json};
use std::collections::{BTreeSet, HashMap, HashSet};

fn s<'a>(v: &'a Value, k: &str) -> &'a str {
    v.get(k).and_then(|x| x.as_str()).unwrap_or("")
}

fn rel(p: &str) -> String {
    let r = p.strip_prefix("{topdir}/").or_else(|| p.strip_prefix("{topdir}")).unwrap_or(p);
    let r = r.trim_start_matches("./").trim_end_matches('/');
    if r.is_empty() { ".".into() } else { r.into() }
}

/// What `usecase_map` did, for the sync reply and tests.
#[derive(Debug, Default, PartialEq, serde::Serialize)]
pub struct Stats {
    pub usecases: usize,
    pub named: usize,
    pub tagged: usize,
    pub unresolved: Vec<String>,
}

/// Every code-node reference a use case's flowmap makes (sequence and both
/// step lists, `from` and `to`).
fn flowmap_refs(uc: &Value) -> Vec<String> {
    let mut out = Vec::new();
    let Some(fm) = uc.get("flowmap").filter(|f| f.is_object()) else { return out };
    for r in fm.get("sequence").and_then(|x| x.as_array()).into_iter().flatten() {
        if let Some(t) = r.as_str() {
            out.push(t.to_string());
        }
    }
    for key in ["process_flow", "data_flow"] {
        for st in fm.get(key).and_then(|x| x.as_array()).into_iter().flatten() {
            for end in ["from", "to"] {
                if let Some(t) = st.get(end).and_then(|x| x.as_str()) {
                    out.push(t.to_string());
                }
            }
        }
    }
    out
}

/// Tag every code vertex on each use case's ownership chains, in place:
/// code vertices get `usecases` (sorted use-case ids, `[]` when none); each
/// use-case vertex gets `uc_named` (the vertex ids it names) and `uc_tops`
/// (the highest tagged vertex of each chain). A use case's id is its `ucid`
/// (`usecase:<slug>`), else `usecase:<name>`.
pub fn usecase_map(vertices: &mut [Value], edges: &[Value]) -> Stats {
    let is_code: HashSet<String> = vertices.iter().filter(|v| s(v, "nodetype") == "code").map(|v| s(v, "id").to_string()).collect();
    let key_of: HashMap<String, String> = vertices.iter().map(|v| (s(v, "id").to_string(), s(v, "key").to_string())).collect();
    // references → vertex ids: the file itself, its folder (when one code node
    // lives there, or the one named after the folder), or `folder/stem`
    let mut by_path: HashMap<String, String> = HashMap::new();
    let mut by_dir: HashMap<String, Vec<(String, String)>> = HashMap::new();
    for v in vertices.iter().filter(|v| s(v, "nodetype") == "code") {
        let path = rel(s(v, "path"));
        let stem = path.rsplit('/').next().unwrap_or("").trim_end_matches(".code.iter.md").to_string();
        let dir = rel(s(v, "dir"));
        by_path.insert(path.clone(), s(v, "id").to_string());
        by_path.insert(if dir == "." { stem.clone() } else { format!("{dir}/{stem}") }, s(v, "id").to_string());
        by_dir.entry(dir).or_default().push((stem, s(v, "id").to_string()));
    }
    let resolve = |r: &str| -> Option<String> {
        let r = rel(r.trim());
        if r.starts_with("actor:") || r.is_empty() {
            return None;
        }
        if let Some(id) = by_path.get(&r) {
            return Some(id.clone());
        }
        let dir = if r.ends_with(".code.iter.md") { r.rsplit_once('/').map(|(d, _)| d.to_string()).unwrap_or(".".into()) } else { r.clone() };
        let list = by_dir.get(&dir)?;
        let base = dir.rsplit('/').next().unwrap_or("");
        list.iter().find(|(stem, _)| stem == base).or_else(|| list.first()).map(|(_, id)| id.clone())
    };
    // primary owner: the code node above whose DAG key is this key minus its last segment
    let mut owners: HashMap<&str, Vec<&str>> = HashMap::new();
    for e in edges.iter().filter(|e| s(e, "kind") == "codenodes") {
        if is_code.contains(s(e, "from")) && is_code.contains(s(e, "to")) {
            owners.entry(s(e, "to")).or_default().push(s(e, "from"));
        }
    }
    let owner = |id: &str| -> Option<String> {
        let cands = owners.get(id)?;
        let key = key_of.get(id).map(String::as_str).unwrap_or("");
        let pk = key.rsplit_once('/').map(|(p, _)| p).unwrap_or("");
        cands.iter().find(|c| key_of.get(**c).map(|k| k == pk).unwrap_or(false)).or_else(|| cands.first()).map(|c| c.to_string())
    };
    let mut stats = Stats::default();
    let mut tags: HashMap<String, BTreeSet<String>> = HashMap::new();
    let mut per_uc: Vec<(usize, Vec<String>, Vec<String>)> = Vec::new();
    for (i, uc) in vertices.iter().enumerate().filter(|(_, v)| s(v, "nodetype") == "usecase") {
        stats.usecases += 1;
        let ucid = if s(uc, "ucid").is_empty() { format!("usecase:{}", s(uc, "name")) } else { s(uc, "ucid").to_string() };
        let mut named: Vec<String> = Vec::new();
        for e in edges.iter().filter(|e| s(e, "from") == s(uc, "id") && s(e, "kind") == "codenodes") {
            if is_code.contains(s(e, "to")) && !named.iter().any(|n| n == s(e, "to")) {
                named.push(s(e, "to").to_string());
            }
        }
        for r in flowmap_refs(uc) {
            match resolve(&r) {
                Some(id) => {
                    if !named.contains(&id) {
                        named.push(id);
                    }
                }
                None if !r.trim().starts_with("actor:") && !stats.unresolved.contains(&r) => stats.unresolved.push(r),
                None => {}
            }
        }
        stats.named += named.len();
        let mut tops: Vec<String> = Vec::new();
        for n in &named {
            let mut cur = n.clone();
            let mut seen = HashSet::new();
            loop {
                tags.entry(cur.clone()).or_default().insert(ucid.clone());
                if !seen.insert(cur.clone()) {
                    break; // an ownership loop: stop where it closes
                }
                match owner(&cur) {
                    Some(p) if !seen.contains(&p) => cur = p,
                    _ => break,
                }
            }
            if !tops.contains(&cur) {
                tops.push(cur);
            }
        }
        per_uc.push((i, named, tops));
    }
    for v in vertices.iter_mut() {
        if s(v, "nodetype") == "code" {
            let t: Vec<&String> = tags.get(s(v, "id")).map(|t| t.iter().collect()).unwrap_or_default();
            if !t.is_empty() {
                stats.tagged += 1;
            }
            v["usecases"] = json!(t);
        }
    }
    for (i, named, tops) in per_uc {
        vertices[i]["uc_named"] = json!(named);
        vertices[i]["uc_tops"] = json!(tops);
    }
    stats
}

#[cfg(test)]
mod tests {
    use super::*;

    fn code(id: &str, dir: &str, key: &str) -> Value {
        json!({"id": id, "nodetype": "code", "dir": format!("{{topdir}}/{dir}"), "key": key,
               "path": format!("{{topdir}}/{dir}/{}.code.iter.md", dir.rsplit('/').next().unwrap())})
    }
    fn e(f: &str, k: &str, t: &str) -> Value {
        json!({"from": f, "kind": k, "to": t})
    }

    #[test]
    fn tags_every_node_up_to_the_top_and_names_the_tops() {
        let mut vs = vec![
            json!({"id": "m", "nodetype": "main"}),
            code("ctx", "data", "data"),
            code("api", "data/api", "data/api"),
            code("lib", "data/api/lib", "data/api/lib"),
            code("db", "data/db", "data/db"),
            code("web", "web", "web"),
            code("ui", "web/ui", "web/ui"),
            json!({"id": "u", "nodetype": "usecase", "ucid": "usecase:read",
                   "flowmap": {"process_flow": [{"from": "actor:worker", "to": "{topdir}/web/ui/ui.code.iter.md"}, {"from": "web/ui", "to": "nowhere"}]}}),
            json!({"id": "u2", "nodetype": "usecase", "ucid": "usecase:other"}),
        ];
        let es = vec![
            e("m", "root", "ctx"), e("m", "root", "web"), e("ctx", "codenodes", "api"), e("ctx", "codenodes", "db"),
            e("api", "codenodes", "lib"), e("web", "codenodes", "ui"), e("u", "codenodes", "lib"), e("u2", "codenodes", "db"),
        ];
        let st = usecase_map(&mut vs, &es);
        let tags = |id: &str| vs.iter().find(|v| v["id"] == id).unwrap()["usecases"].clone();
        assert_eq!(tags("lib"), json!(["usecase:read"]));
        assert_eq!(tags("api"), json!(["usecase:read"]), "every owner on the way up is tagged");
        assert_eq!(tags("ctx"), json!(["usecase:other", "usecase:read"]));
        assert_eq!(tags("ui"), json!(["usecase:read"]), "flowmap steps name parts too");
        assert_eq!(tags("db"), json!(["usecase:other"]), "a sibling no step names is left out of read");
        let u = vs.iter().find(|v| v["id"] == "u").unwrap();
        assert_eq!(u["uc_named"], json!(["lib", "ui"]));
        assert_eq!(u["uc_tops"], json!(["ctx", "web"]), "the use case links to the top of each chain only");
        assert_eq!(st.unresolved, vec!["nowhere".to_string()]);
        assert_eq!((st.usecases, st.tagged), (2, 6));
        // recomputed, never accumulated: dropping a part drops its tags
        let es2: Vec<Value> = es.iter().filter(|x| x["from"] != "u2").cloned().collect();
        usecase_map(&mut vs, &es2);
        assert_eq!(vs.iter().find(|v| v["id"] == "db").unwrap()["usecases"], json!([]));
    }

    #[test]
    fn an_ownership_loop_ends_the_walk() {
        let mut vs = vec![code("a", "a", "a"), code("b", "b", "b"), json!({"id": "u", "nodetype": "usecase", "ucid": "usecase:x"})];
        let es = vec![e("a", "codenodes", "b"), e("b", "codenodes", "a"), e("u", "codenodes", "a")];
        usecase_map(&mut vs, &es);
        assert_eq!(vs[0]["usecases"], json!(["usecase:x"]));
        assert_eq!(vs[1]["usecases"], json!(["usecase:x"]));
    }
}
