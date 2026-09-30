//! Test databases (iter4 is ArangoDB-only, 2026-09-29): every test that needs
//! a store gets its own throwaway database on the dev ArangoDB
//! (`./deploy.sh local` starts it on :8529). ITER4_TEST_ARANGO_URL and
//! ITER4_TEST_ARANGO_PASSWORD override the defaults. Databases are named
//! `iter4_test_*`; the first test of a run drops any a previous run left.

use crate::arango::ArangoBackend;
use tokio::sync::OnceCell;

fn url() -> String {
    std::env::var("ITER4_TEST_ARANGO_URL").unwrap_or_else(|_| "http://127.0.0.1:8529".into())
}

fn password() -> String {
    std::env::var("ITER4_TEST_ARANGO_PASSWORD").unwrap_or_else(|_| "iter4dev".into())
}

static SWEPT: OnceCell<()> = OnceCell::const_new();

/// A fresh, empty database with every collection and index iter4 uses.
pub async fn store() -> (ArangoBackend, String) {
    SWEPT.get_or_init(sweep).await;
    let db = format!("iter4_test_{}", uuid::Uuid::new_v4().simple());
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

/// Databases a previous test run left behind (a panicking test never drops its own).
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
    for name in v["result"].as_array().into_iter().flatten().filter_map(|n| n.as_str()) {
        if name.starts_with("iter4_test_") {
            drop(name).await;
        }
    }
}
