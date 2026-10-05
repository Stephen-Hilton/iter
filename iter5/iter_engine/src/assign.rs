//! The engine's assignments (iter5 spec §4.2): which projects this engine
//! serves, where each checkout lives, and which accounts each may bill —
//! all from `GET /api/engines/{name}/assignments`, derived on the server
//! from the settings graph (`serves`, `bills`, `holds`, `of` edges).
//! Nothing about projects or accounts is read from disk.

use serde::Deserialize;
use serde_json::Value;

fn default_provider() -> String {
    crate::provider::DEFAULT_PROVIDER.into()
}

/// One account as the assignments name it.  Per project (`bills` edge):
/// order / switch / stop / model; top level (`holds` edge): the engine's
/// `token_envar` override.
#[derive(Debug, Clone, Deserialize, Default, PartialEq)]
pub struct AssignedAccount {
    pub name: String,
    /// "" = not named here (the other side of the merge, else claude)
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

/// One served project (an active `serves` edge to this engine).
#[derive(Debug, Clone, Deserialize, Default, PartialEq)]
pub struct Assignment {
    pub project: String,
    #[serde(default)]
    pub topdir: String,
    #[serde(default)]
    pub read_only: bool,
    /// Running | Draining | Stopped ("" = take the project record's)
    #[serde(default)]
    pub state: String,
    #[serde(default)]
    pub accounts: Vec<AssignedAccount>,
    #[serde(default)]
    pub edge_tag: String,
}

#[derive(Debug, Clone, Deserialize, Default, PartialEq)]
pub struct Assignments {
    #[serde(default)]
    pub engine: String,
    #[serde(default)]
    pub projects: Vec<Assignment>,
    /// the accounts this engine holds credentials for (`holds` edges)
    #[serde(default)]
    pub accounts: Vec<AssignedAccount>,
}

impl Assignments {
    pub fn parse(v: &Value) -> Result<Self, String> {
        serde_json::from_value(v.clone()).map_err(|e| format!("bad assignments: {e}"))
    }

    /// The project's accounts with the engine-level settings folded in: the
    /// engine's `holds` edge overrides `token_envar` (and names the provider
    /// when the project side left it empty).
    pub fn accounts_of(&self, a: &Assignment) -> Vec<AssignedAccount> {
        a.accounts
            .iter()
            .map(|pa| {
                let mut x = pa.clone();
                if let Some(held) = self.accounts.iter().find(|h| h.name == pa.name) {
                    if !held.token_envar.trim().is_empty() {
                        x.token_envar = held.token_envar.clone();
                    }
                    if x.provider.trim().is_empty() {
                        x.provider = held.provider.clone();
                    }
                }
                if x.provider.trim().is_empty() {
                    x.provider = default_provider();
                }
                x
            })
            .collect()
    }

    pub fn get(&self, project: &str) -> Option<&Assignment> {
        self.projects.iter().find(|p| p.project == project)
    }
}

/// The ladder's view of an assigned account (iter_core::pick_account input).
pub fn core_account(a: &AssignedAccount) -> iter_core::Account {
    iter_core::Account { name: a.name.clone(), token_envar: a.token_envar.clone(), order: a.order, switch: a.switch, stop: a.stop }
}

pub fn core_accounts(list: &[AssignedAccount]) -> Vec<iter_core::Account> {
    list.iter().map(core_account).collect()
}

/// `~/x` -> `$HOME/x`.
pub fn expand_topdir(topdir: &str) -> String {
    if let Some(rest) = topdir.strip_prefix("~/") {
        if let Some(home) = iter_core::platform::home_dir() {
            return format!("{}/{rest}", home.display());
        }
    }
    topdir.to_string()
}

/// Does a heartbeat-reply list (`files_waiting`, `build_waiting`) name
/// `project`?  Accepts `["p", …]`, `[{"project": "p"}, …]` and `{"p": n}`.
pub fn reply_names(reply: &Value, key: &str, project: &str) -> bool {
    match reply.get(key) {
        Some(Value::Array(a)) => a.iter().any(|x| x.as_str() == Some(project) || x.get("project").and_then(|p| p.as_str()) == Some(project)),
        Some(Value::Object(o)) => o.get(project).map(|v| !matches!(v, Value::Null | Value::Bool(false)) && v.as_u64() != Some(0)).unwrap_or(false),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn spec_shape_parses_and_holds_override_the_envar() {
        let v = json!({"engine":"mbp","projects":[{"project":"pdy","topdir":"~/dev/pdy","read_only":false,"state":"Running",
            "accounts":[{"name":"main","provider":"claude","token_envar":"CLAUDE_TOKEN_MAIN","order":1,"switch":80,"stop":95,"model":""},
                        {"name":"alt","token_envar":"ALT_T","order":2}],
            "edge_tag":"pdy_default"}],
            "accounts":[{"name":"main","provider":"claude","token_envar":"MBP_MAIN_TOKEN"}]});
        let a = Assignments::parse(&v).unwrap();
        assert_eq!(a.engine, "mbp");
        let p = a.get("pdy").unwrap();
        assert_eq!((p.topdir.as_str(), p.state.as_str(), p.edge_tag.as_str()), ("~/dev/pdy", "Running", "pdy_default"));
        let accts = a.accounts_of(p);
        assert_eq!(accts[0].token_envar, "MBP_MAIN_TOKEN", "the engine's holds edge wins");
        assert_eq!((accts[0].switch, accts[0].stop), (80, 95));
        assert_eq!((accts[1].provider.as_str(), accts[1].token_envar.as_str()), ("claude", "ALT_T"), "provider defaults to claude");
        let core = core_accounts(&accts);
        assert_eq!((core[1].name.as_str(), core[1].order), ("alt", 2));
        assert!(Assignments::parse(&json!({"projects": "nope"})).is_err());
        assert_eq!(Assignments::parse(&json!({})).unwrap().projects.len(), 0);
    }

    #[test]
    fn reply_lists_name_projects_in_every_shape() {
        let r = json!({"files_waiting": ["a", {"project": "b"}], "build_waiting": {"c": 2, "d": 0}});
        assert!(reply_names(&r, "files_waiting", "a") && reply_names(&r, "files_waiting", "b"));
        assert!(!reply_names(&r, "files_waiting", "c"));
        assert!(reply_names(&r, "build_waiting", "c") && !reply_names(&r, "build_waiting", "d"));
        assert!(!reply_names(&r, "nope", "a"));
    }
}
