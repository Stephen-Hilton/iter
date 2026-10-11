//! Graph edges derived from a node (§2.4) and the inverse: writing / removing
//! one edge in the owning node's frontmatter.

use super::parse::list_of;
use super::paths::{expand_entry, names_exactly, resolve};
use super::{NodeDoc, NodeErr, NodeType, type_of};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::collections::HashMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EdgeKind {
    Codenodes,
    Tests,
    Reqs,
    Supplies,
    Connects,
    Drives,
    Touches,
    Uses,
    /// code → code: the source calls the target's API (2026-10-10)
    Calls,
    /// code → code: the source pulls data from the target
    Pulls,
    /// code → code: the source pushes data to the target
    Pushes,
}

impl EdgeKind {
    pub const ALL: [EdgeKind; 11] = [
        EdgeKind::Codenodes,
        EdgeKind::Tests,
        EdgeKind::Reqs,
        EdgeKind::Supplies,
        EdgeKind::Connects,
        EdgeKind::Drives,
        EdgeKind::Touches,
        EdgeKind::Uses,
        EdgeKind::Calls,
        EdgeKind::Pulls,
        EdgeKind::Pushes,
    ];

    pub fn as_str(&self) -> &'static str {
        match self {
            EdgeKind::Codenodes => "codenodes",
            EdgeKind::Tests => "tests",
            EdgeKind::Reqs => "reqs",
            EdgeKind::Supplies => "supplies",
            EdgeKind::Connects => "connects",
            EdgeKind::Drives => "drives",
            EdgeKind::Touches => "touches",
            EdgeKind::Uses => "uses",
            EdgeKind::Calls => "calls",
            EdgeKind::Pulls => "pulls",
            EdgeKind::Pushes => "pushes",
        }
    }

    pub fn from_name(s: &str) -> Option<EdgeKind> {
        EdgeKind::ALL.iter().copied().find(|k| k.as_str() == s)
    }
}

/// One derived edge; identity = `(from, kind, to)` (node ids).
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Edge {
    pub from: String,
    pub kind: EdgeKind,
    pub to: String,
}

/// The direct code → code edges a code node's file declares in its front
/// matter (`calls:`, `pulls:`, `pushes:`, lists of node paths): what one part
/// does to another, without drawing a connection node between them.
pub const DATAFLOW: [EdgeKind; 3] = [EdgeKind::Calls, EdgeKind::Pulls, EdgeKind::Pushes];

/// Which endpoint's file holds an edge: `true` when the TARGET (`to`) node
/// owns it — `supplies` (the connection's `connects.from`). Every other kind
/// is owned by `from`. (`drives` may also be written on the usecase side as
/// `actors`; see [`add_child`].)
pub fn owner_is_target(kind: EdgeKind) -> bool {
    kind == EdgeKind::Supplies
}

fn target_ok(kind: EdgeKind, t: NodeType) -> bool {
    match kind {
        EdgeKind::Codenodes
        | EdgeKind::Supplies
        | EdgeKind::Connects
        | EdgeKind::Touches
        | EdgeKind::Uses
        | EdgeKind::Calls
        | EdgeKind::Pulls
        | EdgeKind::Pushes => {
            t == NodeType::Code
        }
        EdgeKind::Tests => t == NodeType::Test,
        EdgeKind::Reqs => t.is_req(),
        EdgeKind::Drives => t == NodeType::Usecase || t == NodeType::Actor,
    }
}

fn front_list(front: &Map<String, Value>, key: &str) -> Vec<String> {
    front.get(key).map(list_of).unwrap_or_default()
}

fn connects_list(doc: &NodeDoc, side: &str) -> Vec<String> {
    match doc.front.get("connects") {
        Some(Value::Object(m)) => m.get(side).map(list_of).unwrap_or_default(),
        _ => Vec::new(),
    }
}

/// Every edge this node's file declares (§2.4), resolved against the
/// project's `(id, path)` list. Includes `supplies` edges (supplier →
/// this connection) and `drives` edges from a usecase's `actors` (actor →
/// this usecase), whose `from` is the other node. Sorted, deduped, no
/// self-edges; targets of the wrong node type are ignored.
pub fn edges_of(doc: &NodeDoc, node_paths_by_id: &[(String, String)]) -> Vec<Edge> {
    let paths: Vec<String> = node_paths_by_id.iter().map(|(_, p)| p.clone()).collect();
    let mut id_of: HashMap<&str, &str> = HashMap::new();
    for (id, p) in node_paths_by_id {
        id_of.entry(p.as_str()).or_insert(id.as_str());
    }
    let me = doc.id.as_str();
    let mut out: Vec<Edge> = Vec::new();
    // (entries, kind, outgoing?)
    let mut add = |entries: &[String], kind: EdgeKind, outgoing: bool| {
        for e in entries {
            for p in resolve(e, &doc.path, &paths) {
                let Some(t) = type_of(&p) else { continue };
                let ok = match (kind, outgoing) {
                    (EdgeKind::Drives, false) => t == NodeType::Actor,
                    (EdgeKind::Drives, true) => t == NodeType::Usecase,
                    _ => target_ok(kind, t),
                };
                if !ok {
                    continue;
                }
                let Some(&other) = id_of.get(p.as_str()) else { continue };
                if other == me {
                    continue;
                }
                let (from, to) = if outgoing { (me, other) } else { (other, me) };
                out.push(Edge { from: from.to_string(), kind, to: to.to_string() });
            }
        }
    };
    let t = doc.nodetype;
    if t == NodeType::Agentmem {
        return Vec::new();
    }
    if t == NodeType::Usecase {
        add(&doc.children.codenodes, EdgeKind::Uses, true);
    } else {
        add(&doc.children.codenodes, EdgeKind::Codenodes, true);
    }
    if t != NodeType::Test {
        add(&doc.children.tests, EdgeKind::Tests, true);
    }
    add(&doc.children.reqs, EdgeKind::Reqs, true);
    if doc.is_connection() {
        add(&connects_list(doc, "from"), EdgeKind::Supplies, false);
        add(&connects_list(doc, "to"), EdgeKind::Connects, true);
    }
    if t == NodeType::Actor {
        add(&front_list(&doc.front, "drives"), EdgeKind::Drives, true);
        add(&front_list(&doc.front, "touches"), EdgeKind::Touches, true);
    }
    if t == NodeType::Usecase {
        add(&front_list(&doc.front, "actors"), EdgeKind::Drives, false);
    }
    if t == NodeType::Code && !doc.is_connection() {
        for kind in DATAFLOW {
            add(&front_list(&doc.front, kind.as_str()), kind, true);
        }
    }
    out.sort();
    out.dedup();
    out
}

/// Where an edge of `kind` lives in a node of this type.
enum Slot {
    Child(&'static str),
    Front(&'static str),
    Connects(&'static str),
}

fn slot_for(doc: &NodeDoc, kind: EdgeKind) -> Result<Slot, NodeErr> {
    let t = doc.nodetype;
    let wrong = Err(NodeErr::WrongKind { kind, nodetype: t });
    if t == NodeType::Agentmem {
        return wrong;
    }
    match kind {
        EdgeKind::Codenodes => Ok(Slot::Child("codenodes")),
        EdgeKind::Uses if t == NodeType::Usecase => Ok(Slot::Child("codenodes")),
        EdgeKind::Tests => Ok(Slot::Child("tests")),
        EdgeKind::Reqs => Ok(Slot::Child("reqs")),
        EdgeKind::Supplies if doc.is_connection() => Ok(Slot::Connects("from")),
        EdgeKind::Connects if doc.is_connection() => Ok(Slot::Connects("to")),
        EdgeKind::Drives if t == NodeType::Actor => Ok(Slot::Front("drives")),
        EdgeKind::Drives if t == NodeType::Usecase => Ok(Slot::Front("actors")),
        EdgeKind::Touches if t == NodeType::Actor => Ok(Slot::Front("touches")),
        EdgeKind::Calls if t == NodeType::Code && !doc.is_connection() => Ok(Slot::Front("calls")),
        EdgeKind::Pulls if t == NodeType::Code && !doc.is_connection() => Ok(Slot::Front("pulls")),
        EdgeKind::Pushes if t == NodeType::Code && !doc.is_connection() => Ok(Slot::Front("pushes")),
        _ => wrong,
    }
}

fn read_slot(doc: &NodeDoc, slot: &Slot) -> Vec<String> {
    match slot {
        Slot::Child("codenodes") => doc.children.codenodes.clone(),
        Slot::Child("tests") => doc.children.tests.clone(),
        Slot::Child(_) => doc.children.reqs.clone(),
        Slot::Front(k) => front_list(&doc.front, k),
        Slot::Connects(side) => connects_list(doc, side),
    }
}

fn write_slot(doc: &mut NodeDoc, slot: &Slot, list: Vec<String>) {
    let as_value = |l: Vec<String>| Value::Array(l.into_iter().map(Value::String).collect());
    match slot {
        Slot::Child("codenodes") => doc.children.codenodes = list,
        Slot::Child("tests") => doc.children.tests = list,
        Slot::Child(_) => doc.children.reqs = list,
        Slot::Front(k) => {
            doc.front.insert(k.to_string(), as_value(list));
        }
        Slot::Connects(side) => {
            let c = doc.front.entry("connects").or_insert_with(|| Value::Object(Map::new()));
            if !c.is_object() {
                *c = Value::Object(Map::new());
            }
            let cm = c.as_object_mut().unwrap();
            cm.insert(side.to_string(), as_value(list));
            for other in ["from", "to"] {
                cm.entry(other).or_insert_with(|| Value::Array(Vec::new()));
            }
        }
    }
}

/// Add an edge to the node that owns it, writing the right key:
/// `children.codenodes|tests|reqs` (usecase `uses` → `children.codenodes`),
/// connection `supplies` → `connects.from`, `connects` → `connects.to`,
/// actor `drives` / `touches`, usecase `drives` (target = actor) → `actors`,
/// code `calls` / `pulls` / `pushes` → the same-named front-matter list.
/// A target already covered by an existing entry (exact, glob or directory)
/// is a no-op. Err(WrongKind) when this node type cannot hold `kind`.
pub fn add_child(doc: &mut NodeDoc, kind: EdgeKind, target_path: &str) -> Result<(), NodeErr> {
    let slot = slot_for(doc, kind)?;
    let mut list = read_slot(doc, &slot);
    let target = target_path.trim().to_string();
    let probe = vec![target.clone()];
    if list.iter().any(|e| !resolve(e, &doc.path, &probe).is_empty()) {
        return Ok(());
    }
    list.push(target);
    write_slot(doc, &slot, list);
    Ok(())
}

/// Remove the entry naming `target_path` from the owning key. Refuses
/// (`GlobOnly`, nothing changed) when the target is (also) matched by a glob
/// or directory entry — removing the exact entry would leave the edge in
/// place. `NoSuchChild` when no entry matches at all.
pub fn remove_child(doc: &mut NodeDoc, kind: EdgeKind, target_path: &str) -> Result<(), NodeErr> {
    let slot = slot_for(doc, kind)?;
    let list = read_slot(doc, &slot);
    let target = expand_entry(target_path, &doc.path, None).trim_end_matches('/').to_string();
    let probe = vec![target.clone()];
    let (exact, rest): (Vec<String>, Vec<String>) =
        list.into_iter().partition(|e| names_exactly(e, &doc.path, &target));
    if let Some(g) = rest.iter().find(|e| !resolve(e, &doc.path, &probe).is_empty()) {
        return Err(NodeErr::GlobOnly { entry: g.clone(), target });
    }
    if exact.is_empty() {
        return Err(NodeErr::NoSuchChild(target));
    }
    write_slot(doc, &slot, rest);
    if DATAFLOW.contains(&kind) {
        let _ = set_protocol(doc, kind, &target, None);
    }
    Ok(())
}

/// The front-matter key holding the protocol of a code node's direct edges:
/// `protocols: {calls: {"<target path>": "gRPC"}, pushes: {…}}` (2026-10-11).
pub const PROTOCOLS: &str = "protocols";

/// The protocols the webui offers for a calls / pulls / pushes edge; any other
/// short name is accepted too.
pub const KNOWN_PROTOCOLS: &[&str] =
    &["TCP", "UDP", "HTTP", "HTTPS", "gRPC", "WebSocket", "GraphQL", "SQL", "Kafka", "AMQP", "MQTT", "Redis", "SSH", "File", "In-process"];

/// The protocol recorded for this node's `kind` edge to `target_path`, if any.
pub fn protocol_of(doc: &NodeDoc, kind: EdgeKind, target_path: &str) -> Option<String> {
    let target = expand_entry(target_path, &doc.path, None).trim_end_matches('/').to_string();
    let m = doc.front.get(PROTOCOLS)?.get(kind.as_str())?.as_object()?;
    m.iter()
        .find(|(k, _)| expand_entry(k, &doc.path, None).trim_end_matches('/') == target)
        .and_then(|(_, v)| v.as_str())
        .map(str::to_string)
        .filter(|p| !p.trim().is_empty())
}

/// Record (Some) or clear (None) the protocol of this node's `kind` edge to
/// `target_path`; only calls / pulls / pushes carry one. Empty maps are dropped.
pub fn set_protocol(doc: &mut NodeDoc, kind: EdgeKind, target_path: &str, protocol: Option<&str>) -> Result<(), NodeErr> {
    if !DATAFLOW.contains(&kind) || doc.nodetype != NodeType::Code || doc.is_connection() {
        return Err(NodeErr::WrongKind { kind, nodetype: doc.nodetype });
    }
    let target = expand_entry(target_path, &doc.path, None).trim_end_matches('/').to_string();
    let this = doc.path.clone();
    let root = doc.front.entry(PROTOCOLS).or_insert_with(|| Value::Object(Map::new()));
    if !root.is_object() {
        *root = Value::Object(Map::new());
    }
    let rm = root.as_object_mut().unwrap();
    let km = rm.entry(kind.as_str()).or_insert_with(|| Value::Object(Map::new()));
    if !km.is_object() {
        *km = Value::Object(Map::new());
    }
    let km = km.as_object_mut().unwrap();
    km.retain(|k, _| expand_entry(k, &this, None).trim_end_matches('/') != target);
    if let Some(p) = protocol.map(str::trim).filter(|p| !p.is_empty()) {
        km.insert(target, Value::String(p.to_string()));
    }
    if km.is_empty() {
        rm.remove(kind.as_str());
    }
    if rm.is_empty() {
        doc.front.remove(PROTOCOLS);
    }
    Ok(())
}
