//! Test databases (ArangoDB-only): every test that needs a store gets its own
//! throwaway database on the dev ArangoDB (`./deploy.sh local` starts it on
//! :8529). ITER5_TEST_ARANGO_URL and ITER5_TEST_ARANGO_PASSWORD override the
//! defaults (the ITER4_ names are still read as a fallback).
//!
//! Databases are named `iter5_test_<unix-seconds>_<uuid>`. The first test of a
//! run drops only `iter5_test_*` databases created more than two hours ago, so
//! a concurrently running test run (another team, another checkout) or iter4's
//! own `iter4_test_*` databases are never touched.

use crate::arango::ArangoBackend;
use tokio::sync::OnceCell;

const PREFIX: &str = "iter5_test_";
const STALE_SECS: u64 = 2 * 60 * 60;

fn env2(a: &str, b: &str) -> Option<String> {
    std::env::var(a).ok().or_else(|| std::env::var(b).ok())
}

fn url() -> String {
    env2("ITER5_TEST_ARANGO_URL", "ITER4_TEST_ARANGO_URL").unwrap_or_else(|| "http://127.0.0.1:8529".into())
}

fn password() -> String {
    env2("ITER5_TEST_ARANGO_PASSWORD", "ITER4_TEST_ARANGO_PASSWORD").unwrap_or_else(|| "iter4dev".into())
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

static SWEPT: OnceCell<()> = OnceCell::const_new();

/// A fresh, empty database with every collection and index iter uses.
pub async fn store() -> (ArangoBackend, String) {
    SWEPT.get_or_init(sweep).await;
    let db = format!("{PREFIX}{}_{}", now_secs(), uuid::Uuid::new_v4().simple());
    let a = ArangoBackend::new(&url(), &db, "root", &password())
        .await
        .unwrap_or_else(|e| panic!("the tests need the dev ArangoDB at {} (./deploy.sh local): {e}", url()));
    (a, db)
}

pub async fn drop(db: &str) {
    let _ = reqwest::Client::new()
        .delete(format!("{}/_db/_system/_api/database/{db}", url()))
        .basic_auth("root", Some(password()))
        .send()
        .await;
}

/// The creation time encoded in a test database name, if it is one of ours.
fn created_at(name: &str) -> Option<u64> {
    name.strip_prefix(PREFIX)?.split('_').next()?.parse().ok()
}

/// Databases a previous test run left behind (a panicking test never drops
/// its own) — only ours, and only once they are older than two hours.
async fn sweep() {
    let Ok(r) = reqwest::Client::new()
        .get(format!("{}/_db/_system/_api/database", url()))
        .basic_auth("root", Some(password()))
        .send()
        .await
    else {
        return;
    };
    let Ok(v) = r.json::<serde_json::Value>().await else { return };
    let now = now_secs();
    for name in v["result"].as_array().into_iter().flatten().filter_map(|n| n.as_str()) {
        if let Some(t) = created_at(name) {
            if now.saturating_sub(t) > STALE_SECS {
                drop(name).await;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn only_old_iter5_names_are_stale_candidates() {
        assert_eq!(super::created_at("iter5_test_1700000000_abc"), Some(1700000000));
        assert_eq!(super::created_at("iter4_test_abc"), None);
        assert_eq!(super::created_at("iter5_test_abcdef"), None);
    }
}
