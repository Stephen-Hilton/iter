//! Usage% tracking per account (5h / 7d windows), from two sources that need
//! nothing but the account's long-lived token:
//!
//!  1. **Per-run** — every headless session runs with `--output-format
//!     stream-json`; Claude Code emits a `rate_limit_event` line carrying
//!     `unifiedWindows` (5h/7d utilization 0..1 + resetsAt).  work.rs hands
//!     that line to `record_event` and the engine writes the account's
//!     snapshot itself.  (The V2 statusline-collector never fires in `-p`
//!     mode — verified on CLI 2.1.260 — so it is gone.)
//!  2. **Idle probe** — a 1-output-token haiku POST straight to
//!     `/v1/messages` with the token as Bearer; the
//!     `anthropic-ratelimit-unified-*` response headers carry the same
//!     numbers, on a 429 rejection too.  ~9 tokens per call, no `claude`
//!     process.  `ITER_USAGE_PROBE_URL` overrides the endpoint (e2e fake).
//!
//! Snapshot files: `$ITER_USAGE_DIR/iter3-usage-<account>.json` (default dir
//! `~/.claude`; account "" -> "default", and the V2 machine-wide
//! `iter-usage-snapshot.json` is still read as a fallback for "default").
//! Format keeps the V2 collector shape so hand-written fixtures still load:
//!   {"ts","source","status","rate_limits":{"five_hour":{"used_percentage",
//!    "resets_at"},"seven_day":{...}},"overage":{"is_using","used_percentage"}}

use chrono::{DateTime, Utc};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::path::PathBuf;

/// Warn when working but the snapshot is older than this (V2 constant).
pub const SNAPSHOT_STALE_WARN_SEC: i64 = 900;

const DEFAULT_PROBE_URL: &str = "https://api.anthropic.com/v1/messages";

pub fn usage_dir() -> PathBuf {
    if let Ok(d) = std::env::var("ITER_USAGE_DIR") {
        if !d.trim().is_empty() {
            return PathBuf::from(d);
        }
    }
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
    PathBuf::from(home).join(".claude")
}

fn account_key(account: &str) -> String {
    if account.trim().is_empty() { "default".into() } else { account.trim().to_string() }
}

pub fn snapshot_path(account: &str) -> PathBuf {
    usage_dir().join(format!("iter3-usage-{}.json", account_key(account)))
}

#[derive(Debug, Clone, Default)]
pub struct Usage {
    pub ts: Option<DateTime<Utc>>,
    pub five_hour_pct: f64,
    pub five_hour_resets_at: Option<i64>,
    pub seven_day_pct: f64,
    pub seven_day_resets_at: Option<i64>,
    /// the account is past a window and requests are being billed as extra
    /// usage (pay-as-you-go) instead of rejected — treated as 100% used
    pub is_using_overage: bool,
    pub overage_pct: f64,
    /// server status word: allowed | allowed_warning | rejected | ...
    pub status: String,
    /// where the numbers came from: stream | probe | file
    pub source: String,
}

impl Usage {
    /// Effective utilization now: an expired window (resets_at in the past)
    /// reads as 0 — the V2 "expired 5h window zeroes out" rule.  An account
    /// running on overage reads as 100 regardless: nothing should be routed
    /// to it until a window resets.
    pub fn effective_pct(&self, now: DateTime<Utc>) -> f64 {
        if self.is_using_overage {
            return 100.0;
        }
        let live = |pct: f64, resets: Option<i64>| -> f64 {
            match resets {
                Some(epoch) if epoch < now.timestamp() => 0.0,
                _ => pct,
            }
        };
        live(self.five_hour_pct, self.five_hour_resets_at)
            .max(live(self.seven_day_pct, self.seven_day_resets_at))
    }

    pub fn age_sec(&self, now: DateTime<Utc>) -> Option<i64> {
        self.ts.map(|t| (now - t).num_seconds())
    }

    fn to_snapshot(&self) -> Value {
        json!({
            "ts": self.ts.map(|t| t.to_rfc3339_opts(chrono::SecondsFormat::Secs, true)),
            "source": self.source,
            "status": self.status,
            "rate_limits": {
                "five_hour": {"used_percentage": self.five_hour_pct, "resets_at": self.five_hour_resets_at},
                "seven_day": {"used_percentage": self.seven_day_pct, "resets_at": self.seven_day_resets_at},
            },
            "overage": {"is_using": self.is_using_overage, "used_percentage": self.overage_pct},
        })
    }
}

fn parse_snapshot(text: &str) -> Option<Usage> {
    let v: Value = serde_json::from_str(text).ok()?;
    let ts = v
        .get("ts")
        .and_then(|t| t.as_str())
        .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
        .map(|d| d.with_timezone(&Utc));
    let rl = v.get("rate_limits")?;
    let win = |key: &str| -> (f64, Option<i64>) {
        let w = rl.get(key);
        (
            w.and_then(|w| w.get("used_percentage")).and_then(|p| p.as_f64()).unwrap_or(0.0),
            w.and_then(|w| w.get("resets_at")).and_then(|r| r.as_i64()),
        )
    };
    let (five_hour_pct, five_hour_resets_at) = win("five_hour");
    let (seven_day_pct, seven_day_resets_at) = win("seven_day");
    let ov = v.get("overage");
    Some(Usage {
        ts,
        five_hour_pct,
        five_hour_resets_at,
        seven_day_pct,
        seven_day_resets_at,
        is_using_overage: ov.and_then(|o| o.get("is_using")).and_then(|b| b.as_bool()).unwrap_or(false),
        overage_pct: ov.and_then(|o| o.get("used_percentage")).and_then(|p| p.as_f64()).unwrap_or(0.0),
        status: v.get("status").and_then(|s| s.as_str()).unwrap_or("").to_string(),
        source: v.get("source").and_then(|s| s.as_str()).unwrap_or("file").to_string(),
    })
}

pub fn read_usage(account: &str) -> Option<Usage> {
    let text = std::fs::read_to_string(snapshot_path(account)).ok().or_else(|| {
        // V2 compatibility: the old machine-wide snapshot feeds "default"
        if account_key(account) == "default" {
            std::fs::read_to_string(usage_dir().join("iter-usage-snapshot.json")).ok()
        } else {
            None
        }
    })?;
    parse_snapshot(&text)
}

/// Atomically write the account's snapshot (temp + rename, like the V2 collector).
pub fn write_snapshot(account: &str, u: &Usage) -> Result<(), String> {
    let path = snapshot_path(account);
    let dir = path.parent().ok_or("snapshot path has no parent")?;
    std::fs::create_dir_all(dir).map_err(|e| format!("mkdir {}: {e}", dir.display()))?;
    let tmp = dir.join(format!(".iter3-usage-{}.{}.tmp", account_key(account), std::process::id()));
    std::fs::write(&tmp, u.to_snapshot().to_string()).map_err(|e| format!("write {}: {e}", tmp.display()))?;
    std::fs::rename(&tmp, &path).map_err(|e| format!("rename {}: {e}", path.display()))
}

/// Source 1: a `rate_limit_event` line from `--output-format stream-json`:
/// {"type":"rate_limit_event","rate_limit_info":{"status":"allowed_warning",
///  "isUsingOverage":false,"unifiedWindows":{"five_hour":{"utilization":0.0,
///  "resetsAt":1788556800},"seven_day":{"utilization":0.88,"resetsAt":...}}}}
pub fn usage_from_stream_event(v: &Value) -> Option<Usage> {
    if v.get("type").and_then(|t| t.as_str()) != Some("rate_limit_event") {
        return None;
    }
    let info = v.get("rate_limit_info")?;
    let wins = info.get("unifiedWindows")?;
    let win = |key: &str| -> (f64, Option<i64>) {
        let w = wins.get(key);
        (
            w.and_then(|w| w.get("utilization")).and_then(|p| p.as_f64()).unwrap_or(0.0) * 100.0,
            w.and_then(|w| w.get("resetsAt")).and_then(|r| r.as_i64()),
        )
    };
    let (five_hour_pct, five_hour_resets_at) = win("five_hour");
    let (seven_day_pct, seven_day_resets_at) = win("seven_day");
    Some(Usage {
        ts: Some(Utc::now()),
        five_hour_pct,
        five_hour_resets_at,
        seven_day_pct,
        seven_day_resets_at,
        is_using_overage: info.get("isUsingOverage").and_then(|b| b.as_bool()).unwrap_or(false),
        overage_pct: 0.0,
        status: info.get("status").and_then(|s| s.as_str()).unwrap_or("").to_string(),
        source: "stream".into(),
    })
}

/// Write the snapshot for `account` from a stream line, if it is the
/// rate-limit event.  Returns true when a snapshot was written.
pub fn record_event(account: &str, v: &Value) -> bool {
    match usage_from_stream_event(v) {
        Some(u) => match write_snapshot(account, &u) {
            Ok(()) => true,
            Err(e) => {
                eprintln!("[engine] usage snapshot for '{}' not written: {e}", account_key(account));
                false
            }
        },
        None => false,
    }
}

fn probe_url() -> String {
    std::env::var("ITER_USAGE_PROBE_URL").ok().filter(|u| !u.trim().is_empty()).unwrap_or_else(|| DEFAULT_PROBE_URL.into())
}

/// Source 2: the idle probe.  One haiku request with max_tokens 1 (about 9
/// tokens) using the long-lived token as Bearer; the answer is in the
/// `anthropic-ratelimit-unified-*` headers, which the server also sends on
/// a 429 once a window is exhausted.  Only the headers matter — a rejection
/// with headers is a successful probe (that account is at 100%).
pub fn probe(token: &str) -> Result<Usage, String> {
    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .map_err(|e| e.to_string())?;
    let resp = client
        .post(probe_url())
        .header("Authorization", format!("Bearer {}", token.trim()))
        .header("anthropic-beta", "oauth-2025-04-20")
        .header("anthropic-version", "2023-06-01")
        .header("User-Agent", "claude-code/2.1.260 iter3-usage-probe")
        .header("Content-Type", "application/json")
        .body(r#"{"model":"claude-haiku-4-5","max_tokens":1,"messages":[{"role":"user","content":"."}]}"#)
        .send()
        .map_err(|e| format!("probe request failed: {e}"))?;
    let status = resp.status();
    let h = |name: &str| -> Option<String> {
        resp.headers().get(name).and_then(|v| v.to_str().ok()).map(|s| s.trim().to_string())
    };
    let pct = |name: &str| h(name).and_then(|s| s.parse::<f64>().ok()).map(|f| f * 100.0);
    let epoch = |name: &str| h(name).and_then(|s| s.parse::<i64>().ok());
    let (five, seven) = (pct("anthropic-ratelimit-unified-5h-utilization"), pct("anthropic-ratelimit-unified-7d-utilization"));
    if five.is_none() && seven.is_none() {
        let body: String = resp.text().unwrap_or_default().chars().take(300).collect();
        return Err(format!("probe HTTP {} without rate-limit headers: {}", status.as_u16(), body));
    }
    let unified = h("anthropic-ratelimit-unified-status").unwrap_or_default();
    let rejected = |name: &str| h(name).as_deref() == Some("rejected");
    // overage = a window is exhausted yet the request is still allowed, or
    // the server names overage as the binding claim
    let is_using_overage = h("anthropic-ratelimit-unified-representative-claim").as_deref() == Some("overage")
        || (unified.starts_with("allowed")
            && (rejected("anthropic-ratelimit-unified-5h-status") || rejected("anthropic-ratelimit-unified-7d-status")));
    Ok(Usage {
        ts: Some(Utc::now()),
        five_hour_pct: five.unwrap_or(0.0),
        five_hour_resets_at: epoch("anthropic-ratelimit-unified-5h-reset"),
        seven_day_pct: seven.unwrap_or(0.0),
        seven_day_resets_at: epoch("anthropic-ratelimit-unified-7d-reset"),
        is_using_overage,
        overage_pct: pct("anthropic-ratelimit-unified-overage-utilization").unwrap_or(0.0),
        status: if unified.is_empty() { format!("http_{}", status.as_u16()) } else { unified },
        source: "probe".into(),
    })
}

/// Probe + write.  Returns the fresh usage.
pub fn probe_and_record(account: &str, token: &str) -> Result<Usage, String> {
    let u = probe(token)?;
    write_snapshot(account, &u)?;
    Ok(u)
}

/// The snapshot as iter_data/webui see it (engine heartbeat "usage").
pub fn snapshot_json(account: &str, now: DateTime<Utc>) -> Option<Value> {
    let u = read_usage(account)?;
    Some(json!({
        "account": account,
        "five_hour_pct": (u.five_hour_pct * 10.0).round() / 10.0,
        "seven_day_pct": (u.seven_day_pct * 10.0).round() / 10.0,
        "five_hour_resets_at": u.five_hour_resets_at,
        "seven_day_resets_at": u.seven_day_resets_at,
        "effective_pct": u.effective_pct(now).round(),
        "is_using_overage": u.is_using_overage,
        "status": u.status,
        "source": u.source,
        "ts": u.ts.map(|t| t.to_rfc3339_opts(chrono::SecondsFormat::Secs, true)),
        "age_sec": u.age_sec(now),
    }))
}

/// account name -> effective pct (rounded), for the ladder + maxagents gates.
/// Missing snapshot = 0 (unknown usage never blocks; the stale warning and
/// the idle probe are what fill it in).
pub fn usage_map(accounts: &[iter_core::Account], now: DateTime<Utc>) -> BTreeMap<String, u8> {
    let mut out = BTreeMap::new();
    for a in accounts {
        let pct = read_usage(&a.name).map(|u| u.effective_pct(now)).unwrap_or(0.0);
        out.insert(a.name.clone(), pct.round().clamp(0.0, 100.0) as u8);
    }
    out
}

/// Effective pct for a single (possibly unnamed/default) account.
pub fn effective_pct_for(account: &str, now: DateTime<Utc>) -> u8 {
    read_usage(account)
        .map(|u| u.effective_pct(now).round().clamp(0.0, 100.0) as u8)
        .unwrap_or(0)
}

/// When `account` next satisfies `used < stop` on BOTH windows (unix secs).
/// `now` when it already does.  Rules (decided 2026-09-11):
///  - a window over its stop% clears at its `resets_at`; an expired window
///    already reads 0 (the V2 rule effective_pct applies);
///  - the two windows are independent — a 7d reset leaves the 5h window
///    where it was, so an account can sit at "5h 100% | 7d 0%" for up to
///    five hours — hence the later of the two clearing times;
///  - overage = both windows treated as over;
///  - stop 0 = no limit; no snapshot = unknown usage, which never blocks.
///  None = over its stop% with no reset time on record (cannot be estimated).
pub fn available_at(u: Option<&Usage>, stop: u8, now: DateTime<Utc>) -> Option<i64> {
    let ts = now.timestamp();
    let Some(u) = u else { return Some(ts) };
    if stop == 0 {
        return Some(ts);
    }
    let over = u.is_using_overage;
    let win = |pct: f64, resets: Option<i64>| -> Option<i64> {
        match resets {
            Some(r) if r <= ts => Some(ts),
            _ if !over && pct.round().clamp(0.0, 100.0) < stop as f64 => Some(ts),
            Some(r) => Some(r),
            None => None,
        }
    };
    let t5 = win(u.five_hour_pct, u.five_hour_resets_at)?;
    let t7 = win(u.seven_day_pct, u.seven_day_resets_at)?;
    Some(t5.max(t7))
}

/// Every configured account's windows, reset times and `available_at`, for
/// the engine record (webui: which account comes back first, and when).
/// Sorted soonest-available first (unknown last), then by ladder order.
pub fn accounts_json(accounts: &[iter_core::Account], in_use: &[String], now: DateTime<Utc>) -> Vec<Value> {
    let mut rows: Vec<(Option<i64>, i64, Value)> = accounts
        .iter()
        .map(|a| {
            let u = read_usage(&a.name);
            let at = available_at(u.as_ref(), a.stop, now);
            let row = json!({
                "name": a.name, "order": a.order, "switch": a.switch, "stop": a.stop,
                "in_use": in_use.contains(&a.name),
                // is the account's token set in the engine's env_file right now
                // (spec: account hot reload, 2026-09-11) — the webui marks "unset"
                "token": if crate::envstore::get(&a.token_envar).is_some() { "set" } else { "unset" },
                "five_hour_pct": u.as_ref().map(|u| (u.five_hour_pct * 10.0).round() / 10.0),
                "seven_day_pct": u.as_ref().map(|u| (u.seven_day_pct * 10.0).round() / 10.0),
                "five_hour_resets_at": u.as_ref().and_then(|u| u.five_hour_resets_at),
                "seven_day_resets_at": u.as_ref().and_then(|u| u.seven_day_resets_at),
                "effective_pct": u.as_ref().map(|u| u.effective_pct(now).round()),
                "is_using_overage": u.as_ref().map(|u| u.is_using_overage).unwrap_or(false),
                "status": u.as_ref().map(|u| u.status.clone()),
                "ts": u.as_ref().and_then(|u| u.ts).map(|t| t.to_rfc3339_opts(chrono::SecondsFormat::Secs, true)),
                "age_sec": u.as_ref().and_then(|u| u.age_sec(now)),
                "available_at": at,
            });
            (at, a.order, row)
        })
        .collect();
    rows.sort_by_key(|(at, order, _)| (at.is_none(), at.unwrap_or(i64::MAX), *order));
    rows.into_iter().map(|(_, _, r)| r).collect()
}

/// The account that comes back first: {"account", "available_at"} from a
/// sorted `accounts_json` list, or Null when nothing can be estimated.
pub fn next_json(accounts: &[Value]) -> Value {
    accounts
        .iter()
        .find(|a| a.get("available_at").and_then(|t| t.as_i64()).is_some())
        .map(|a| json!({"account": a["name"], "available_at": a["available_at"]}))
        .unwrap_or(Value::Null)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every heartbeat row says whether the account's token is set
    /// (2026-09-11), so the webui can mark an account the engine cannot use.
    #[test]
    fn accounts_json_carries_the_token_flag() {
        crate::envstore::set_for_test("USAGE_T11_SET_TOKEN", "tok");
        crate::envstore::unset_for_test("USAGE_T11_UNSET_TOKEN");
        let accounts = vec![
            iter_core::Account { name: "Has".into(), token_envar: "USAGE_T11_SET_TOKEN".into(), order: 1, ..Default::default() },
            iter_core::Account { name: "Not".into(), token_envar: "USAGE_T11_UNSET_TOKEN".into(), order: 2, ..Default::default() },
        ];
        let rows = accounts_json(&accounts, &[], Utc::now());
        let flag = |name: &str| rows.iter().find(|r| r["name"] == name).unwrap()["token"].as_str().unwrap().to_string();
        assert_eq!(flag("Has"), "set");
        assert_eq!(flag("Not"), "unset");
    }

    #[test]
    fn expired_window_zeroes_out() {
        let now = Utc::now();
        let u = Usage {
            ts: Some(now),
            five_hour_pct: 41.0,
            five_hour_resets_at: Some(now.timestamp() - 10),
            seven_day_pct: 14.0,
            seven_day_resets_at: Some(now.timestamp() + 1000),
            ..Default::default()
        };
        assert_eq!(u.effective_pct(now), 14.0);
        let u2 = Usage { five_hour_resets_at: Some(now.timestamp() + 1000), ..u.clone() };
        assert_eq!(u2.effective_pct(now), 41.0);
    }

    #[test]
    fn overage_reads_as_full() {
        let now = Utc::now();
        let u = Usage { five_hour_pct: 3.0, seven_day_pct: 100.0, is_using_overage: true, ..Default::default() };
        assert_eq!(u.effective_pct(now), 100.0);
    }

    #[test]
    fn snapshot_parses_collector_format() {
        let text = r#"{"ts":"2026-09-01T12:00:00Z","rate_limits":{
            "five_hour":{"used_percentage":30.0,"resets_at":99999999999},
            "seven_day":{"used_percentage":14.0,"resets_at":99999999999}}}"#;
        let u = parse_snapshot(text).unwrap();
        assert_eq!(u.five_hour_pct, 30.0);
        assert_eq!(u.seven_day_pct, 14.0);
        assert!(u.ts.is_some());
        assert!(!u.is_using_overage);
    }

    #[test]
    fn stream_event_becomes_usage_and_roundtrips() {
        let line = r#"{"type":"rate_limit_event","rate_limit_info":{"status":"allowed_warning",
            "resetsAt":1788624000,"rateLimitType":"seven_day","utilization":0.88,"isUsingOverage":false,
            "surpassedThreshold":0.75,"unifiedWindows":{"five_hour":{"utilization":0,"resetsAt":1788556800},
            "seven_day":{"utilization":0.88,"resetsAt":1788624000}}},"uuid":"x","session_id":"y"}"#;
        let v: Value = serde_json::from_str(line).unwrap();
        let u = usage_from_stream_event(&v).unwrap();
        assert_eq!(u.five_hour_pct, 0.0);
        assert!((u.seven_day_pct - 88.0).abs() < 1e-9);
        assert_eq!(u.seven_day_resets_at, Some(1788624000));
        assert_eq!(u.status, "allowed_warning");
        let back = parse_snapshot(&u.to_snapshot().to_string()).unwrap();
        assert!((back.seven_day_pct - 88.0).abs() < 1e-9);
        assert_eq!(back.source, "stream");
        // other stream lines are not events
        let other: Value = serde_json::from_str(r#"{"type":"result","subtype":"success"}"#).unwrap();
        assert!(usage_from_stream_event(&other).is_none());
    }
    fn u(five: f64, r5: i64, seven: f64, r7: i64) -> Usage {
        Usage { five_hour_pct: five, five_hour_resets_at: Some(r5), seven_day_pct: seven, seven_day_resets_at: Some(r7), ..Default::default() }
    }

    #[test]
    fn available_at_is_the_later_window_clearing() {
        let now = Utc::now();
        let t = now.timestamp();
        // under stop on both windows: now
        assert_eq!(available_at(Some(&u(10.0, t + 100, 20.0, t + 5000)), 55, now), Some(t));
        // 5h over, 7d under: the 5h reset
        assert_eq!(available_at(Some(&u(103.0, t + 100, 20.0, t + 5000)), 99, now), Some(t + 100));
        // 7d over, 5h under: the 7d reset — the 5h number is meaningless meanwhile
        assert_eq!(available_at(Some(&u(1.0, t + 100, 99.0, t + 5000)), 99, now), Some(t + 5000));
        // both over, 5h resets AFTER the 7d reset: the 5h reset wins
        assert_eq!(available_at(Some(&u(100.0, t + 9000, 100.0, t + 5000)), 99, now), Some(t + 9000));
        // an expired window already reads 0
        assert_eq!(available_at(Some(&u(100.0, t - 1, 10.0, t + 5000)), 55, now), Some(t));
        // exact stop% is "at stop" (engine picks only used < stop)
        assert_eq!(available_at(Some(&u(55.0, t + 100, 0.0, t + 5000)), 55, now), Some(t + 100));
        // 54.6 rounds to 55: at stop
        assert_eq!(available_at(Some(&u(54.6, t + 100, 0.0, t + 5000)), 55, now), Some(t + 100));
        // overage: both windows count as over
        let ov = Usage { is_using_overage: true, ..u(1.0, t + 100, 2.0, t + 5000) };
        assert_eq!(available_at(Some(&ov), 99, now), Some(t + 5000));
        // stop 0 = no limit; no snapshot = unknown never blocks
        assert_eq!(available_at(Some(&u(100.0, t + 100, 100.0, t + 5000)), 0, now), Some(t));
        assert_eq!(available_at(None, 55, now), Some(t));
        // over with no reset on record: cannot estimate
        let nr = Usage { five_hour_pct: 100.0, ..Default::default() };
        assert_eq!(available_at(Some(&nr), 55, now), None);
    }

    #[test]
    fn next_is_the_soonest_account() {
        let t = 1_800_000_000;
        let rows = vec![
            json!({"name": "A", "available_at": t + 500}),
            json!({"name": "B", "available_at": Value::Null}),
            json!({"name": "C", "available_at": t + 10}),
        ];
        // accounts_json sorts; next_json takes the first estimable row
        let mut sorted = rows.clone();
        sorted.sort_by_key(|r| (r["available_at"].is_null(), r["available_at"].as_i64().unwrap_or(i64::MAX)));
        assert_eq!(next_json(&sorted), json!({"account": "C", "available_at": t + 10}));
        assert_eq!(next_json(&[json!({"name": "B", "available_at": Value::Null})]), Value::Null);
    }

}
