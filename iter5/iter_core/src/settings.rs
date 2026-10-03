//! The settings graph (iter5 spec §7): every setting lives on a node or an
//! edge. Nodes are the existing records (project, engine, agent, tooling,
//! user) plus `account`, `provider`, `workitem_type`; edges are `SysEdge`
//! rows. Node id = `<type>:<name>`; every type has a non-deletable
//! placeholder `<type>:_deactivated` — an edge whose endpoint sits on a
//! placeholder keeps its settings but is inactive.
//!
//! Shared by iter_data (store + API) and iter_engine (reads assignments).

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// The placeholder name every node type has.
pub const PLACEHOLDER: &str = "_deactivated";
/// The one iter_data node.
pub const ITER_DATA_SELF: &str = "iter_data:self";

/// Settings-graph node types (spec §7).
pub const NODE_TYPES: &[&str] = &[
    "iter_data",
    "iter_engine",
    "project",
    "workitem_type",
    "agent",
    "agent_tools",
    "user",
    "account",
    "provider",
];

/// Providers seeded at startup (spec §5).
pub const PROVIDERS: &[&str] = &["claude", "mock"];

/// The logical storage table holding records of a node type (`None` for
/// `iter_data`, which is virtual: one node, server info).
pub fn table_of(node_type: &str) -> Option<&'static str> {
    Some(match node_type {
        "iter_engine" => "engine",
        "project" => "project",
        "workitem_type" => "workitem_type",
        "agent" => "agent",
        "agent_tools" => "agent_tooling",
        "user" => "webui_user",
        "account" => "account",
        "provider" => "provider",
        _ => return None,
    })
}

/// One row of the edge-type table: the edge type and its endpoint types.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EdgeTypeDef {
    pub edge: &'static str,
    pub from: &'static str,
    pub to: &'static str,
    pub desc: &'static str,
}

/// The edge-type table (spec §7). (from, to) pairs are unique, so the edge
/// type is always inferable from its endpoints.
pub const EDGE_TYPES: &[EdgeTypeDef] = &[
    EdgeTypeDef { edge: "serves", from: "iter_engine", to: "project", desc: "engine runs the project (settings: topdir, read_only)" },
    EdgeTypeDef { edge: "bills", from: "account", to: "project", desc: "project may use the account (settings: order, switch, stop, model)" },
    EdgeTypeDef { edge: "holds", from: "iter_engine", to: "account", desc: "engine has the credential locally (settings: token_envar override)" },
    EdgeTypeDef { edge: "of", from: "account", to: "provider", desc: "the account's provider for dispatch" },
    EdgeTypeDef { edge: "member", from: "user", to: "project", desc: "user may see/act on the project (settings: role user|viewer)" },
    EdgeTypeDef { edge: "owns", from: "user", to: "iter_engine", desc: "whose token the engine uses" },
    EdgeTypeDef { edge: "runs", from: "agent", to: "project", desc: "agent enabled on the project (settings: per-project agent override)" },
    EdgeTypeDef { edge: "handles", from: "agent", to: "workitem_type", desc: "default: every agent handles queued" },
    EdgeTypeDef { edge: "allows", from: "project", to: "workitem_type", desc: "per-state policy (failed: retry policy, scheduled: on/off, question: notify)" },
    EdgeTypeDef { edge: "uses", from: "agent_tools", to: "agent", desc: "tooling the agent can look up" },
    EdgeTypeDef { edge: "hosts", from: "iter_data", to: "project", desc: "display only" },
    EdgeTypeDef { edge: "hosts", from: "iter_data", to: "iter_engine", desc: "display only" },
];

/// The edge type joining a `from_type` node to a `to_type` node, if any.
pub fn edge_type_for(from_type: &str, to_type: &str) -> Option<&'static str> {
    EDGE_TYPES.iter().find(|d| d.from == from_type && d.to == to_type).map(|d| d.edge)
}

/// Is `edge` a valid type between these endpoint types?
pub fn edge_allowed(edge: &str, from_type: &str, to_type: &str) -> bool {
    EDGE_TYPES.iter().any(|d| d.edge == edge && d.from == from_type && d.to == to_type)
}

pub fn is_edge_type(edge: &str) -> bool {
    EDGE_TYPES.iter().any(|d| d.edge == edge)
}

pub fn is_node_type(t: &str) -> bool {
    NODE_TYPES.contains(&t)
}

/// `<type>:<name>`
pub fn node_id(node_type: &str, name: &str) -> String {
    format!("{node_type}:{name}")
}

/// `<type>:_deactivated`
pub fn placeholder_id(node_type: &str) -> String {
    node_id(node_type, PLACEHOLDER)
}

/// Split `<type>:<name>` (the name may itself contain ':').
pub fn parse_node_id(id: &str) -> Option<(&str, &str)> {
    let (t, n) = id.split_once(':')?;
    if !is_node_type(t) || n.is_empty() {
        return None;
    }
    Some((t, n))
}

pub fn type_of_id(id: &str) -> Option<&str> {
    parse_node_id(id).map(|(t, _)| t)
}

pub fn is_placeholder(id: &str) -> bool {
    parse_node_id(id).map(|(_, n)| n == PLACEHOLDER).unwrap_or(false)
}

/// A node that may never be deleted: every placeholder and `iter_data:self`.
pub fn is_protected(id: &str) -> bool {
    is_placeholder(id) || id == ITER_DATA_SELF
}

/// One settings-graph edge (collection `sys_edge`).
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct SysEdge {
    pub id: String,
    #[serde(rename = "type")]
    pub edge_type: String,
    pub from: String,
    pub to: String,
    #[serde(default)]
    pub tag: String,
    #[serde(default = "empty_obj")]
    pub settings: Value,
    /// the admin's on/off switch; the edge counts only when this is true AND
    /// neither endpoint is a placeholder (see `is_active`)
    #[serde(default = "yes")]
    pub active: bool,
    #[serde(default)]
    pub created: String,
    #[serde(default)]
    pub updated: String,
}
fn empty_obj() -> Value {
    Value::Object(Default::default())
}
fn yes() -> bool {
    true
}

impl SysEdge {
    /// Counts for the system: switched on and both endpoints real.
    pub fn is_active(&self) -> bool {
        self.active && !is_placeholder(&self.from) && !is_placeholder(&self.to)
    }
    pub fn setting(&self, key: &str) -> Option<&Value> {
        self.settings.get(key)
    }
    pub fn setting_str(&self, key: &str) -> String {
        self.settings.get(key).and_then(|v| v.as_str()).unwrap_or("").to_string()
    }
    /// Endpoint names (the part after `<type>:`).
    pub fn from_name(&self) -> &str {
        parse_node_id(&self.from).map(|(_, n)| n).unwrap_or("")
    }
    pub fn to_name(&self) -> &str {
        parse_node_id(&self.to).map(|(_, n)| n).unwrap_or("")
    }
}

/// Validate an edge's endpoints against the table: both ids well-formed and
/// the (from type, to type) pair allowed for the edge type. A placeholder of
/// the right type counts. Returns a reason on refusal.
pub fn validate_endpoints(edge_type: &str, from: &str, to: &str) -> Result<(), String> {
    let ft = type_of_id(from).ok_or_else(|| format!("'{from}' is not a node id (<type>:<name>)"))?;
    let tt = type_of_id(to).ok_or_else(|| format!("'{to}' is not a node id (<type>:<name>)"))?;
    if !is_edge_type(edge_type) {
        return Err(format!("unknown edge type '{edge_type}'"));
    }
    if !edge_allowed(edge_type, ft, tt) {
        let want: Vec<String> = EDGE_TYPES.iter().filter(|d| d.edge == edge_type).map(|d| format!("{} → {}", d.from, d.to)).collect();
        return Err(format!("a {edge_type} edge joins {}; got {ft} → {tt}", want.join(" or ")));
    }
    Ok(())
}

/// Check the known setting keys of an edge type (unknown keys are kept).
pub fn validate_settings(edge_type: &str, settings: &Value) -> Result<(), String> {
    if !settings.is_object() {
        return Err("settings must be an object".into());
    }
    let pct = |k: &str| -> Result<(), String> {
        match settings.get(k) {
            None | Some(Value::Null) => Ok(()),
            Some(v) => match v.as_u64() {
                Some(n) if n <= 100 => Ok(()),
                _ => Err(format!("{edge_type}.{k} must be a percent 0–100")),
            },
        }
    };
    let string = |k: &str| -> Result<(), String> {
        match settings.get(k) {
            None | Some(Value::Null) | Some(Value::String(_)) => Ok(()),
            _ => Err(format!("{edge_type}.{k} must be a string")),
        }
    };
    match edge_type {
        "serves" => {
            string("topdir")?;
            match settings.get("read_only") {
                None | Some(Value::Null) | Some(Value::Bool(_)) => {}
                _ => return Err("serves.read_only must be true/false".into()),
            }
        }
        "bills" => {
            pct("switch")?;
            pct("stop")?;
            string("model")?;
            match settings.get("order") {
                None | Some(Value::Null) => {}
                Some(v) if v.as_i64().is_some() => {}
                _ => return Err("bills.order must be an integer".into()),
            }
        }
        "holds" => string("token_envar")?,
        "member" => {
            let r = settings.get("role").and_then(|v| v.as_str()).unwrap_or("user");
            if !["user", "viewer"].contains(&r) {
                return Err("member.role must be user or viewer".into());
            }
        }
        _ => {}
    }
    Ok(())
}

/// `GET /api/engines/{name}/assignments` (spec §4.2).
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Assignments {
    pub engine: String,
    #[serde(default)]
    pub projects: Vec<ProjectAssignment>,
    /// accounts the engine holds (active `holds` edges)
    #[serde(default)]
    pub accounts: Vec<HeldAccount>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ProjectAssignment {
    pub project: String,
    #[serde(default)]
    pub topdir: String,
    #[serde(default)]
    pub read_only: bool,
    /// the project record's state (Running | Draining | Stopped)
    #[serde(default)]
    pub state: String,
    /// accounts billed to the project that this engine also holds, by `order`
    #[serde(default)]
    pub accounts: Vec<BilledAccount>,
    /// the serves edge's tag
    #[serde(default)]
    pub edge_tag: String,
    /// enabled agents (active `runs` edges) → per-project override settings
    #[serde(default)]
    pub agents: std::collections::BTreeMap<String, Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct BilledAccount {
    pub name: String,
    #[serde(default)]
    pub provider: String,
    #[serde(default)]
    pub token_envar: String,
    #[serde(default)]
    pub order: i64,
    #[serde(default)]
    pub switch: u8,
    #[serde(default)]
    pub stop: u8,
    #[serde(default)]
    pub model: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct HeldAccount {
    pub name: String,
    #[serde(default)]
    pub provider: String,
    #[serde(default)]
    pub token_envar: String,
}

impl BilledAccount {
    /// The iter4 `Account` shape `pick_account` takes.
    pub fn to_account(&self) -> crate::Account {
        crate::Account { name: self.name.clone(), token_envar: self.token_envar.clone(), order: self.order, switch: self.switch, stop: self.stop }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn edge_types_are_inferable_and_validated() {
        assert_eq!(edge_type_for("iter_engine", "project"), Some("serves"));
        assert_eq!(edge_type_for("account", "project"), Some("bills"));
        assert_eq!(edge_type_for("iter_data", "iter_engine"), Some("hosts"));
        assert_eq!(edge_type_for("project", "iter_engine"), None);
        // (from, to) pairs are unique
        for (i, a) in EDGE_TYPES.iter().enumerate() {
            for b in &EDGE_TYPES[i + 1..] {
                assert!(!(a.from == b.from && a.to == b.to), "{a:?} / {b:?}");
            }
        }
        assert!(validate_endpoints("serves", "iter_engine:mbp", "project:pdy").is_ok());
        assert!(validate_endpoints("serves", "iter_engine:_deactivated", "project:pdy").is_ok());
        assert!(validate_endpoints("serves", "account:main", "project:pdy").is_err());
        assert!(validate_endpoints("serves", "nope:x", "project:pdy").is_err());
        assert!(validate_endpoints("bogus", "iter_engine:a", "project:b").is_err());
    }

    #[test]
    fn ids_and_placeholders() {
        assert_eq!(node_id("project", "pdy"), "project:pdy");
        assert_eq!(placeholder_id("account"), "account:_deactivated");
        assert_eq!(parse_node_id("project:a:b"), Some(("project", "a:b")));
        assert_eq!(parse_node_id("project:"), None);
        assert_eq!(parse_node_id("x:y"), None);
        assert!(is_placeholder("user:_deactivated") && !is_placeholder("user:bob"));
        assert!(is_protected(ITER_DATA_SELF) && is_protected("agent:_deactivated") && !is_protected("agent:code"));
        let mut e = SysEdge { id: "1".into(), edge_type: "serves".into(), from: "iter_engine:a".into(), to: "project:p".into(), active: true, ..Default::default() };
        assert!(e.is_active());
        e.from = placeholder_id("iter_engine");
        assert!(!e.is_active());
        e.from = "iter_engine:a".into();
        e.active = false;
        assert!(!e.is_active());
        // serde: type field, defaults
        let e: SysEdge = serde_json::from_value(json!({"id": "x", "type": "of", "from": "account:a", "to": "provider:claude"})).unwrap();
        assert!(e.active && e.settings.is_object());
        assert_eq!(serde_json::to_value(&e).unwrap()["type"], "of");
    }

    #[test]
    fn settings_validation() {
        assert!(validate_settings("bills", &json!({"order": 1, "switch": 80, "stop": 95, "model": ""})).is_ok());
        assert!(validate_settings("bills", &json!({"switch": 180})).is_err());
        assert!(validate_settings("serves", &json!({"topdir": "~/x", "read_only": true})).is_ok());
        assert!(validate_settings("serves", &json!({"read_only": "yes"})).is_err());
        assert!(validate_settings("member", &json!({"role": "viewer"})).is_ok());
        assert!(validate_settings("member", &json!({"role": "admin"})).is_err());
        assert!(validate_settings("runs", &json!([])).is_err());
    }
}
