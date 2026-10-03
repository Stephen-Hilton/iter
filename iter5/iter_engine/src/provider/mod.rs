//! Multi-provider dispatch (iter5 spec §5).
//!
//! Every model call the engine makes — work turns, the close-gate verifier,
//! ELI5 explain, the dedup judge, GraphRAG summaries and OCR, the
//! `iter critreview` critic and the connectivity nudge — goes through
//! [`dispatch_agent`].  The provider is the account's (`account.provider`
//! from the engine's assignments, default `claude`); `""` (the ambient
//! login, no account configured) uses `$ITER_PROVIDER`, default `claude`.
//!
//! Usage collection is NOT part of dispatch: after a call the engine asks
//! [`get_usage`] for the account's snapshot (claude: the stream's
//! `rate_limit_event`; mock: `ITER_MOCK_USAGE`), and the idle probe asks it
//! with no dispatch output at all.

pub mod claude;
pub mod mock;

use crate::usage::Usage;
use iter_core::Project;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{OnceLock, RwLock};
use std::time::Duration;

pub const DEFAULT_PROVIDER: &str = "claude";
pub const KNOWN_PROVIDERS: &[&str] = &["claude", "mock"];

/// What the call is for.  Providers that really run a model ignore it; the
/// mock provider uses it to answer each kind of call the way the engine
/// parses it (a verdict for the verifier, JSON for the judge and the summary
/// agent) and to run directives only on work turns.  (An addition to the
/// spec's `AgentContext`, listed in the ENG-RUN report.)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Role {
    #[default]
    Work,
    Verifier,
    Judge,
    Explain,
    Summary,
    Ocr,
    Critic,
    Nudge,
}

/// Everything a provider needs to run one agent turn.
#[derive(Debug, Clone, Default)]
pub struct AgentContext {
    pub prompt: String,
    pub cwd: PathBuf,
    /// extra environment for the agent process (ITER_* addressing, PATH with the shim)
    pub env: Vec<(String, String)>,
    /// the `iter` MCP server config file for this session
    pub mcp_config: Option<PathBuf>,
    /// continue this provider session (claude `--resume`)
    pub resume: Option<String>,
    /// comma-separated tool allow-list (claude `--allowedTools`)
    pub allowed_tools: Option<String>,
    /// provider-specific flags passed through verbatim (agent `flags`, `--disallowedTools`, …)
    pub extra_args: Vec<String>,
    pub role: Role,
}

/// The run the dispatch belongs to: a stop request or a revocation for it
/// (crate::work) ends the provider process at once.  Empty = never stopped.
#[derive(Debug, Clone, Default)]
pub struct StopCheck {
    pub workid: String,
    pub lease: String,
}

impl StopCheck {
    pub fn none() -> Self {
        Self::default()
    }

    /// The run this worker thread is executing now (crate::work's thread-locals).
    pub fn current() -> Self {
        let (workid, lease) = crate::work::current_run();
        Self { workid, lease }
    }

    /// `Some(error text)` when the run must end now.
    pub fn check(&self) -> Option<String> {
        crate::work::stop_reason(&self.workid, &self.lease)
    }
}

#[derive(Debug, Clone)]
pub struct DispatchSettings {
    pub account: String,
    pub token: Option<String>,
    pub timeout: Duration,
    pub max_turns: Option<u32>,
    pub stop: StopCheck,
}

impl Default for DispatchSettings {
    fn default() -> Self {
        Self { account: String::new(), token: None, timeout: Duration::from_secs(3600), max_turns: None, stop: StopCheck::none() }
    }
}

/// What one dispatch produced.  `subtype` is the provider's result kind in
/// Claude Code's words ("success", "error_max_turns",
/// "error_during_execution", …); `raw` is the provider's raw output, kept for
/// `get_usage`.
#[derive(Debug, Clone, Default)]
pub struct DispatchOut {
    pub text: String,
    pub subtype: String,
    pub num_turns: u64,
    pub cost_usd: f64,
    /// uncached input tokens only (what the stream's `usage.input_tokens` reports)
    pub input_tokens: u64,
    pub output_tokens: u64,
    /// prompt tokens served from the cache — nearly all of an agent's context
    /// cost lives here, not in `input_tokens`
    pub cache_read_tokens: u64,
    /// prompt tokens written into the cache
    pub cache_create_tokens: u64,
    /// the provider session this run used (for continuation)
    pub session_id: String,
    pub raw: String,
}

impl DispatchOut {
    /// A successful result that is just text (exec items, human closes).
    pub fn plain(text: String) -> Self {
        Self { text, subtype: "success".into(), ..Default::default() }
    }
}

fn normalize(provider: &str) -> String {
    let p = provider.trim().to_ascii_lowercase();
    if p.is_empty() { DEFAULT_PROVIDER.into() } else { p }
}

/// Run one agent turn on `provider`.  `""` means the default (claude).
pub fn dispatch_agent(provider: &str, model: &str, ctx: &AgentContext, s: &DispatchSettings) -> Result<DispatchOut, String> {
    match normalize(provider).as_str() {
        "claude" => claude::dispatch_agent_claude(model, ctx, s),
        "mock" => mock::dispatch_agent_mock(model, ctx, s),
        other => Err(format!("unknown provider '{other}' (known: {})", KNOWN_PROVIDERS.join(", "))),
    }
}

/// The account's usage now.  `last` is the dispatch that just ran on the
/// account (None = an idle probe).  `None` = nothing known.
pub fn get_usage(provider: &str, account: &str, token: Option<&str>, last: Option<&DispatchOut>) -> Option<Usage> {
    match normalize(provider).as_str() {
        "claude" => claude::get_usage_claude(token, last),
        "mock" => mock::get_usage_mock(account),
        _ => None,
    }
}

/// `get_usage`, written to the account's machine-wide snapshot file (the
/// one the ladder, the maxagents gates and the heartbeat read).
pub fn record_usage(provider: &str, account: &str, token: Option<&str>, last: Option<&DispatchOut>) -> Option<Usage> {
    let u = get_usage(provider, account, token, last)?;
    if let Err(e) = crate::usage::write_snapshot(account, &u) {
        eprintln!("[engine] usage snapshot for '{}' not written: {e}", if account.is_empty() { "default" } else { account });
    }
    Some(u)
}

// ---- account -> provider / model, from the engine's assignments ----------

#[derive(Default)]
struct Registry {
    /// account name -> provider
    providers: HashMap<String, String>,
    /// (project, account) -> model override from the `bills` edge
    models: HashMap<(String, String), String>,
}

fn registry() -> &'static RwLock<Registry> {
    static R: OnceLock<RwLock<Registry>> = OnceLock::new();
    R.get_or_init(|| RwLock::new(Registry::default()))
}

/// Record a project's accounts as the assignments name them: (account,
/// provider, model override).  Called by the engine every tick.
pub fn register_accounts(project: &str, accounts: &[(String, String, String)]) {
    if let Ok(mut r) = registry().write() {
        r.models.retain(|(p, _), _| p != project);
        for (name, provider, model) in accounts {
            r.providers.insert(name.clone(), normalize(provider));
            if !model.trim().is_empty() {
                r.models.insert((project.to_string(), name.clone()), model.trim().to_string());
            }
        }
    }
}

/// The provider for the ambient login (no account): `$ITER_PROVIDER`, else claude.
pub fn ambient_provider() -> String {
    crate::envstore::get("ITER_PROVIDER")
        .or_else(|| std::env::var("ITER_PROVIDER").ok())
        .map(|p| normalize(&p))
        .unwrap_or_else(|| DEFAULT_PROVIDER.into())
}

/// The provider `account` dispatches to.
pub fn provider_for(account: &str) -> String {
    if account.trim().is_empty() {
        return ambient_provider();
    }
    registry().read().ok().and_then(|r| r.providers.get(account).cloned()).unwrap_or_else(|| DEFAULT_PROVIDER.into())
}

/// The `bills` edge's model override for (project, account), if any.
pub fn model_override(project: &str, account: &str) -> Option<String> {
    registry().read().ok().and_then(|r| r.models.get(&(project.to_string(), account.to_string())).cloned())
}

/// The one-call path every engine model call uses: resolve the account's
/// provider and token (a named account without its token is an error, never
/// another account's token), dispatch, then record the account's usage.
pub fn call(project: &Project, account: &str, model: &str, ctx: &AgentContext, timeout_sec: u64, max_turns: Option<u32>) -> Result<DispatchOut, String> {
    let provider = provider_for(account);
    let token = crate::work::resolve_account_token(project, account, &crate::envstore::env_file())?;
    let s = DispatchSettings {
        account: account.to_string(),
        token: token.clone(),
        timeout: Duration::from_secs(timeout_sec.max(1)),
        max_turns,
        stop: StopCheck::current(),
    };
    let out = dispatch_agent(&provider, model, ctx, &s)?;
    record_usage(&provider, account, token.as_deref(), Some(&out));
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_provider_is_an_error_naming_the_known_ones() {
        let e = dispatch_agent("openai", "", &AgentContext::default(), &DispatchSettings::default()).unwrap_err();
        assert_eq!(e, "unknown provider 'openai' (known: claude, mock)");
        assert!(get_usage("openai", "a", None, None).is_none());
    }

    #[test]
    fn accounts_route_to_their_provider_and_model() {
        register_accounts("prov-p1", &[("prov-acct-m".into(), "Mock".into(), "m-1".into()), ("prov-acct-c".into(), "".into(), "".into())]);
        assert_eq!(provider_for("prov-acct-m"), "mock");
        assert_eq!(provider_for("prov-acct-c"), "claude");
        assert_eq!(provider_for("prov-never-seen"), "claude", "default provider");
        assert_eq!(model_override("prov-p1", "prov-acct-m").as_deref(), Some("m-1"));
        assert_eq!(model_override("prov-p1", "prov-acct-c"), None);
        // re-registering the project replaces its model overrides
        register_accounts("prov-p1", &[("prov-acct-m".into(), "mock".into(), "".into())]);
        assert_eq!(model_override("prov-p1", "prov-acct-m"), None);
    }

    /// `call` dispatches a mock account end to end and records its usage
    /// snapshot from the mock's ITER_MOCK_USAGE_<ACCOUNT>.
    #[test]
    fn call_dispatches_and_records_usage() {
        let dir = std::env::temp_dir().join(format!("iter5-prov-call-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        register_accounts("prov-p2", &[("provcall".into(), "mock".into(), "".into())]);
        crate::envstore::set_for_test("PROVCALL_TOKEN", "t");
        crate::envstore::set_for_test("ITER_MOCK_USAGE_PROVCALL", "33,44");
        let project = Project {
            name: "prov-p2".into(),
            accounts: vec![iter_core::Account { name: "provcall".into(), token_envar: "PROVCALL_TOKEN".into(), order: 1, ..Default::default() }],
            ..Default::default()
        };
        let ctx = AgentContext { prompt: "say hi\nmock: say hello there".into(), cwd: dir.clone(), ..Default::default() };
        let out = call(&project, "provcall", "", &ctx, 30, None).unwrap();
        assert_eq!((out.text.as_str(), out.subtype.as_str()), ("hello there", "success"));
        let u = crate::usage::read_usage("provcall").expect("snapshot recorded");
        assert_eq!((u.five_hour_pct, u.seven_day_pct, u.source.as_str()), (33.0, 44.0, "mock"));
        // a named account whose token is unset never dispatches
        crate::envstore::unset_for_test("PROVCALL_TOKEN");
        assert!(call(&project, "provcall", "", &ctx, 30, None).unwrap_err().contains("PROVCALL_TOKEN is not set"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
