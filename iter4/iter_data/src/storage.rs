//! Storage interface (decided 2026-09-01): one trait between the API and the
//! persistence layer. iter4 serves from ArangoDB only (2026-09-29); the
//! DynamoDB implementation remains as the read-only iter3 migration source.
//!
//! Data model: every logical table is (pk, sk) -> body(JSON). Three operations
//! need backend-native atomicity and get dedicated methods instead of generic
//! put: versioned workitem writes, conditional lock acquire, and seq bumps.

use async_trait::async_trait;
use iter_core::VersionRow;
use serde_json::Value;

#[derive(Debug)]
pub enum StorageError {
    /// versioned write lost the race / lock already held; carries current row if known
    Conflict(Option<Value>),
    Backend(String),
}

impl std::fmt::Display for StorageError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StorageError::Conflict(_) => write!(f, "conflict"),
            StorageError::Backend(e) => write!(f, "backend error: {e}"),
        }
    }
}

#[async_trait]
pub trait Storage: Send + Sync {
    async fn get(&self, table: &str, pk: &str, sk: &str) -> Result<Option<Value>, StorageError>;
    /// unconditional upsert
    async fn put(&self, table: &str, pk: &str, sk: &str, body: &Value) -> Result<(), StorageError>;
    async fn delete(&self, table: &str, pk: &str, sk: &str) -> Result<bool, StorageError>;
    /// all rows sharing a partition key, ordered by sk
    async fn query(&self, table: &str, pk: &str) -> Result<Vec<Value>, StorageError>;
    /// all rows in the table
    async fn scan(&self, table: &str) -> Result<Vec<Value>, StorageError>;

    /// Versioned write. expect == 0 means "create: must not exist yet".
    /// body must already carry the NEW version (expect + 1).
    async fn put_versioned(
        &self,
        table: &str,
        pk: &str,
        sk: &str,
        body: &Value,
        expect: u64,
    ) -> Result<(), StorageError>;

    /// Atomic "create if absent OR expired OR held by the same workid".
    /// `expires` and `workid` inside body are also lifted to native attributes
    /// so the backend can express the condition.
    async fn acquire_lock(
        &self,
        table: &str,
        pk: &str,
        sk: &str,
        body: &Value,
        now: &str,
        holder_workid: &str,
    ) -> Result<(), StorageError>;

    /// Bump the change-signal counter for (project, logical table); returns new seq.
    async fn bump_seq(&self, project: &str, logical_table: &str) -> Result<u64, StorageError>;
    async fn get_versions(&self, project: &str) -> Result<Vec<VersionRow>, StorageError>;

    /// Raw rows with their keys and native version (migration source).
    async fn scan_keyed(&self, table: &str) -> Result<Vec<KeyedRow>, StorageError> {
        let _ = table;
        Err(StorageError::Backend(format!("{} cannot scan raw rows", self.backend_name())))
    }
    /// Unconditional raw write keeping pk/sk/version exactly (migration target).
    async fn put_keyed(&self, table: &str, row: &KeyedRow) -> Result<(), StorageError> {
        self.put(table, &row.pk, &row.sk, &row.body).await
    }
    /// Every seq counter across all projects (migration source).
    async fn all_versions(&self) -> Result<Vec<VersionRow>, StorageError> {
        Err(StorageError::Backend(format!("{} cannot list all versions", self.backend_name())))
    }
    /// Set a seq counter to an exact value (migration target).
    async fn set_seq(&self, row: &VersionRow) -> Result<(), StorageError> {
        let _ = row;
        Err(StorageError::Backend(format!("{} cannot set seq", self.backend_name())))
    }
    /// The ArangoDB backend, when this is one — graph routes use native AQL
    /// traversals there and a row-based fallback everywhere else.
    fn arango(&self) -> Option<&crate::arango::ArangoBackend> {
        None
    }

    fn backend_name(&self) -> &'static str;
}

/// One stored row with its keys (migration only).
#[derive(Debug, Clone)]
pub struct KeyedRow {
    pub pk: String,
    pub sk: String,
    pub version: u64,
    pub body: Value,
}

pub fn body_str(v: &Value, key: &str) -> String {
    v.get(key).and_then(|x| x.as_str()).unwrap_or("").to_string()
}

pub fn body_u64(v: &Value, key: &str) -> u64 {
    v.get(key).and_then(|x| x.as_u64()).unwrap_or(0)
}
