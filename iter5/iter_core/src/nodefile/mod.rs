//! iter5 node files (`*.iter.md`) — format v5 (spec §2).
//!
//! One library, used identically by the server (iter_data, node side) and the
//! engine (iter_engine, file side) so both agree byte-for-byte on what a node
//! file says and how it is written. Everything here works on strings only: the
//! engine passes file text, the server passes node paths. No filesystem access.
//!
//! Paths are stored as `"{topdir}/rel/path.x.iter.md"`.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fmt;

mod conform;
mod edges;
mod parse;
mod paths;
mod render;
pub mod reqs;
mod yaml;

#[cfg(test)]
mod tests;
#[cfg(test)]
mod reqs_tests;

pub use conform::{Conformed, conform, conform_against};
pub use edges::{Edge, EdgeKind, add_child, edges_of, owner_is_target, remove_child};
pub use parse::{parse, parse_tolerant};
pub use paths::{Attach, Naming, dir_of, expand_entry, file_name_of, plan_path, resolve, resolve_in, slug, stem_of};
pub use render::render;
pub use reqs::{ReqItem, conform_reqs, first_sentence, is_req_key, parse_reqs, render_reqs, req_edges, req_node_name};

/// The node types of format v5 (§2.1). The type is the filename segment after
/// the last dot before `.iter.md`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum NodeType {
    Project,
    Code,
    Test,
    Bizreq,
    Techreq,
    Philosophy,
    Usecase,
    Actor,
    Agentmem,
}

impl NodeType {
    pub const ALL: [NodeType; 9] = [
        NodeType::Project,
        NodeType::Code,
        NodeType::Test,
        NodeType::Bizreq,
        NodeType::Techreq,
        NodeType::Philosophy,
        NodeType::Usecase,
        NodeType::Actor,
        NodeType::Agentmem,
    ];

    /// The canonical filename tag (`code` in `x.code.iter.md`).
    pub fn as_str(&self) -> &'static str {
        match self {
            NodeType::Project => "project",
            NodeType::Code => "code",
            NodeType::Test => "test",
            NodeType::Bizreq => "bizreq",
            NodeType::Techreq => "techreq",
            NodeType::Philosophy => "philosophy",
            NodeType::Usecase => "usecase",
            NodeType::Actor => "actor",
            NodeType::Agentmem => "agentmem",
        }
    }

    /// Canonical tags only (`code`, `test`, …). Legacy tags are recognised by
    /// [`type_of`] on filenames, not here.
    pub fn from_tag(tag: &str) -> Option<NodeType> {
        NodeType::ALL.iter().copied().find(|t| t.as_str() == tag)
    }

    /// bizreq, techreq, philosophy: the kinds a `children.reqs` edge points at.
    pub fn is_req(&self) -> bool {
        matches!(self, NodeType::Bizreq | NodeType::Techreq | NodeType::Philosophy)
    }
}

impl fmt::Display for NodeType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// The type tag of a node filename (a bare name or a whole path), or `None`
/// for a plain context doc / non-`.iter.md` file. Case-sensitive.
/// Legacy iter4 names are recognised so the converter and conform can use
/// them: `main.iter.md` → project, `*.tests.iter.md` / `*.testgroup.iter.md`
/// → test, `*.agentmemory.iter.md` → agentmem.
pub fn type_of(filename: &str) -> Option<NodeType> {
    let tag = type_tag(filename)?;
    NodeType::from_tag(tag).or(match tag {
        "main" => Some(NodeType::Project),
        "tests" | "testgroup" => Some(NodeType::Test),
        "agentmemory" => Some(NodeType::Agentmem),
        _ => None,
    })
}

/// True when the filename uses an iter4 type tag (`main`, `tests`,
/// `testgroup`, `agentmemory`) that `iter migrate5` renames.
pub fn is_legacy_filename(filename: &str) -> bool {
    matches!(type_tag(filename), Some("main" | "tests" | "testgroup" | "agentmemory"))
}

fn type_tag(filename: &str) -> Option<&str> {
    let base = filename.rsplit('/').next().unwrap_or(filename);
    let stem = base.strip_suffix(".iter.md")?;
    Some(stem.rsplit('.').next().unwrap_or(stem))
}

/// Whether nodes of this type are synced to the project graph (false only for
/// engine-written agent memory).
pub fn is_synced(t: NodeType) -> bool {
    t != NodeType::Agentmem
}

/// Valid `level:` values of a code node.
pub const LEVELS: [&str; 4] = ["context", "container", "component", "connection"];
/// Valid `teststate:` values (iter4 semantics).
pub const TESTSTATES: [&str; 4] = ["inherit", "include", "omit", "block"];
/// Valid `status:` values of bizreq / techreq.
pub const REQ_STATUSES: [&str; 3] = ["draft", "agreed", "done"];
/// Valid `owner:` values of a code node.
pub const OWNERS: [&str; 3] = ["bespoke", "oss", "3rdparty"];

/// `children:` — paths or globs (§2.2).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Children {
    #[serde(default)]
    pub codedirs: Vec<String>,
    #[serde(default)]
    pub codenodes: Vec<String>,
    #[serde(default)]
    pub tests: Vec<String>,
    #[serde(default)]
    pub reqs: Vec<String>,
    /// any other `children.<key>` list, kept verbatim (never an edge)
    #[serde(flatten, default)]
    pub extra: BTreeMap<String, Vec<String>>,
}

/// `timestamps:` — `YYYY-MM-DD HH:MM:SSZ` (UTC) strings, `""` = never.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Timestamps {
    #[serde(default)]
    pub create: String,
    #[serde(default)]
    pub last_modified: String,
    #[serde(default)]
    pub last_tested: String,
    #[serde(flatten, default)]
    pub extra: BTreeMap<String, String>,
}

/// One node file, parsed. `front` holds every type-specific key other than
/// `level` (connects, flowmap, actors, drives, touches, status, scandirs,
/// file_naming, gitrepo, owner, last_result, and anything unknown).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NodeDoc {
    #[serde(default)]
    pub id: String,
    pub nodetype: NodeType,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub desc: String,
    #[serde(default)]
    pub creator: String,
    #[serde(default)]
    pub teststate: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub level: Option<String>,
    #[serde(default)]
    pub children: Children,
    #[serde(default)]
    pub timestamps: Timestamps,
    #[serde(default)]
    pub front: serde_json::Map<String, serde_json::Value>,
    #[serde(default)]
    pub body: String,
    #[serde(default)]
    pub path: String,
}

impl NodeDoc {
    /// A fresh, conformed node of `nodetype` (new uuid v4, type defaults such
    /// as `level: component`, `status: draft`, project `scandirs`, test script
    /// glob), timestamps `create = last_modified = now`. `path` is left empty:
    /// set it from [`plan_path`].
    pub fn new(nodetype: NodeType, name: &str, creator: &str, now: &str) -> NodeDoc {
        let mut doc = NodeDoc {
            id: uuid::Uuid::new_v4().to_string(),
            nodetype,
            name: name.to_string(),
            desc: String::new(),
            creator: creator.to_string(),
            teststate: "inherit".to_string(),
            level: None,
            children: Children::default(),
            timestamps: Timestamps {
                create: now.to_string(),
                last_modified: now.to_string(),
                last_tested: String::new(),
                extra: BTreeMap::new(),
            },
            front: serde_json::Map::new(),
            body: String::new(),
            path: String::new(),
        };
        if nodetype == NodeType::Test {
            doc.children.tests = vec!["{thisfiledir}/tests/{thisfilestem}*.sh".to_string()];
        }
        let mut sink = Vec::new();
        conform::apply_type_defaults(&mut doc, &mut sink);
        doc
    }

    /// The code level, when this is a code node.
    pub fn level_str(&self) -> &str {
        self.level.as_deref().unwrap_or("")
    }

    pub fn is_connection(&self) -> bool {
        self.nodetype == NodeType::Code && self.level.as_deref() == Some("connection")
    }
}

/// A conformance / parse observation. `code` is stable (machine-readable).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Finding {
    pub code: String,
    pub msg: String,
}

impl Finding {
    pub fn new(code: &str, msg: impl Into<String>) -> Finding {
        Finding { code: code.to_string(), msg: msg.into() }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NodeErr {
    /// the path is not a node file (`type_of` is None)
    NotANode(String),
    /// `id:` missing or not a hyphenated uuid (run `conform` first)
    BadId(String),
    /// this edge kind cannot be written on this node type
    WrongKind { kind: EdgeKind, nodetype: NodeType },
    /// remove_child: no entry of that key names the target
    NoSuchChild(String),
    /// remove_child: the target is matched by a glob / directory entry, which
    /// cannot be removed without dropping its other matches
    GlobOnly { entry: String, target: String },
}

impl fmt::Display for NodeErr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            NodeErr::NotANode(p) => write!(f, "not a node file: {}", p),
            NodeErr::BadId(id) => write!(f, "missing or malformed id: {:?}", id),
            NodeErr::WrongKind { kind, nodetype } => {
                write!(f, "a {} node cannot hold a {} edge", nodetype, kind.as_str())
            }
            NodeErr::NoSuchChild(t) => write!(f, "no child entry names {}", t),
            NodeErr::GlobOnly { entry, target } => write!(
                f,
                "{} is matched by the pattern {:?}; edit or remove that pattern instead",
                target, entry
            ),
        }
    }
}

impl std::error::Error for NodeErr {}

/// Now, in the node-file timestamp format `YYYY-MM-DD HH:MM:SSZ` (UTC).
pub fn now_ts() -> String {
    chrono::Utc::now().format("%Y-%m-%d %H:%M:%SZ").to_string()
}

/// A hyphenated uuid (`8-4-4-4-12` hex), the only id form conform keeps.
pub fn is_valid_id(id: &str) -> bool {
    id.len() == 36 && uuid::Uuid::parse_str(id).is_ok()
}

/// sha256 hex of the text.
pub fn content_hash(text: &str) -> String {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(text.as_bytes());
    hex(&h.finalize())
}

/// sha256 hex of the canonical rendering with timestamps blanked (and the path
/// ignored): equal for two docs that say the same thing.
pub fn semantic_hash(doc: &NodeDoc) -> String {
    let mut d = doc.clone();
    d.timestamps = Timestamps::default();
    content_hash(&render::render_full(&d))
}

fn hex(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push_str(&format!("{:02x}", b));
    }
    s
}
