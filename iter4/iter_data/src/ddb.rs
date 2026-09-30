//! DynamoDB, the iter3 store — iter4 only ever READS it, as the source of
//! `--migrate-from dynamodb` (opened with `new_readonly`, which cannot create
//! tables). One physical table per logical table, `<prefix><logical>`
//! (default prefix "iter3_"), pk(S)/sk(S) keys, body as a JSON string.

use crate::storage::{KeyedRow, Storage, StorageError};
use async_trait::async_trait;
use aws_sdk_dynamodb::Client;
use aws_sdk_dynamodb::types::AttributeValue;
use iter_core::{VersionRow, now_utc};
use serde_json::Value;
use std::collections::HashMap;

pub struct DdbBackend {
    client: Client,
    prefix: String,
}

fn berr<E: std::fmt::Debug>(e: E) -> StorageError {
    StorageError::Backend(format!("{e:?}"))
}

fn av_s(s: &str) -> AttributeValue {
    AttributeValue::S(s.to_string())
}

impl DdbBackend {
    /// Migration source: connect without creating anything. A migration only
    /// ever reads DynamoDB, so it must not be able to create tables either.
    pub async fn new_readonly(region: &str, prefix: &str) -> Result<Self, StorageError> {
        let cfg = aws_config::defaults(aws_config::BehaviorVersion::latest())
            .region(aws_config::Region::new(region.to_string()))
            .load()
            .await;
        Ok(Self { client: Client::new(&cfg), prefix: prefix.to_string() })
    }

    /// Every item in a physical table, raw attributes (paginated scan).
    async fn scan_items(&self, table: &str) -> Result<Vec<HashMap<String, AttributeValue>>, StorageError> {
        let mut out = Vec::new();
        let mut last_key = None;
        loop {
            let mut req = self.client.scan().table_name(self.phys(table));
            if let Some(k) = last_key {
                req = req.set_exclusive_start_key(Some(k));
            }
            let resp = match req.send().await {
                Ok(r) => r,
                // a logical table that was never created has no rows to copy
                Err(e) if format!("{e:?}").contains("ResourceNotFound") => return Ok(out),
                Err(e) => return Err(berr(e)),
            };
            out.extend(resp.items().iter().cloned());
            match resp.last_evaluated_key {
                Some(k) if !k.is_empty() => last_key = Some(k),
                _ => break,
            }
        }
        Ok(out)
    }

    fn phys(&self, logical: &str) -> String {
        format!("{}{}", self.prefix, logical)
    }

    fn item_body(item: &HashMap<String, AttributeValue>) -> Option<Value> {
        item.get("body")
            .and_then(|v| v.as_s().ok())
            .and_then(|s| serde_json::from_str(s).ok())
    }
}

#[async_trait]
impl Storage for DdbBackend {
    async fn get(&self, table: &str, pk: &str, sk: &str) -> Result<Option<Value>, StorageError> {
        let out = self
            .client
            .get_item()
            .table_name(self.phys(table))
            .key("pk", av_s(pk))
            .key("sk", av_s(sk))
            .send()
            .await
            .map_err(berr)?;
        Ok(out.item.as_ref().and_then(Self::item_body))
    }

    async fn put(&self, table: &str, pk: &str, sk: &str, body: &Value) -> Result<(), StorageError> {
        // keep the native version attribute in step with the body: versioned
        // writes condition on it, so a plain put must not leave it missing
        let mut req = self
            .client
            .put_item()
            .table_name(self.phys(table))
            .item("pk", av_s(pk))
            .item("sk", av_s(sk))
            .item("body", av_s(&body.to_string()));
        if let Some(v) = body.get("version").and_then(|v| v.as_u64()) {
            req = req.item("version", AttributeValue::N(v.to_string()));
        }
        req.send().await.map_err(berr)?;
        Ok(())
    }

    async fn delete(&self, table: &str, pk: &str, sk: &str) -> Result<bool, StorageError> {
        let out = self
            .client
            .delete_item()
            .table_name(self.phys(table))
            .key("pk", av_s(pk))
            .key("sk", av_s(sk))
            .return_values(aws_sdk_dynamodb::types::ReturnValue::AllOld)
            .send()
            .await
            .map_err(berr)?;
        Ok(out.attributes.is_some())
    }

    async fn query(&self, table: &str, pk: &str) -> Result<Vec<Value>, StorageError> {
        let mut out = Vec::new();
        let mut last_key = None;
        loop {
            let mut req = self
                .client
                .query()
                .table_name(self.phys(table))
                .key_condition_expression("pk = :pk")
                .expression_attribute_values(":pk", av_s(pk));
            if let Some(k) = last_key {
                req = req.set_exclusive_start_key(Some(k));
            }
            let resp = req.send().await.map_err(berr)?;
            for item in resp.items() {
                if let Some(b) = Self::item_body(item) {
                    out.push(b);
                }
            }
            match resp.last_evaluated_key {
                Some(k) if !k.is_empty() => last_key = Some(k),
                _ => break,
            }
        }
        Ok(out)
    }

    async fn scan(&self, table: &str) -> Result<Vec<Value>, StorageError> {
        let mut out = Vec::new();
        let mut last_key = None;
        loop {
            let mut req = self.client.scan().table_name(self.phys(table));
            if let Some(k) = last_key {
                req = req.set_exclusive_start_key(Some(k));
            }
            let resp = req.send().await.map_err(berr)?;
            for item in resp.items() {
                if let Some(b) = Self::item_body(item) {
                    out.push(b);
                }
            }
            match resp.last_evaluated_key {
                Some(k) if !k.is_empty() => last_key = Some(k),
                _ => break,
            }
        }
        Ok(out)
    }

    async fn put_versioned(
        &self,
        table: &str,
        pk: &str,
        sk: &str,
        body: &Value,
        expect: u64,
    ) -> Result<(), StorageError> {
        let new_version = expect + 1;
        let mut req = self
            .client
            .put_item()
            .table_name(self.phys(table))
            .item("pk", av_s(pk))
            .item("sk", av_s(sk))
            .item("version", AttributeValue::N(new_version.to_string()))
            .item("body", av_s(&body.to_string()));
        if expect == 0 {
            req = req.condition_expression("attribute_not_exists(pk)");
        } else {
            req = req
                .condition_expression("version = :expect")
                .expression_attribute_values(":expect", AttributeValue::N(expect.to_string()));
        }
        match req.send().await {
            Ok(_) => Ok(()),
            Err(e) => {
                let msg = format!("{e:?}");
                if msg.contains("ConditionalCheckFailed") {
                    let current = self.get(table, pk, sk).await.unwrap_or(None);
                    Err(StorageError::Conflict(current))
                } else {
                    Err(StorageError::Backend(msg))
                }
            }
        }
    }

    async fn acquire_lock(
        &self,
        table: &str,
        pk: &str,
        sk: &str,
        body: &Value,
        now: &str,
        holder_workid: &str,
    ) -> Result<(), StorageError> {
        let expires = body.get("expires").and_then(|v| v.as_str()).unwrap_or("");
        let req = self
            .client
            .put_item()
            .table_name(self.phys(table))
            .item("pk", av_s(pk))
            .item("sk", av_s(sk))
            .item("expires", av_s(expires))
            .item("workid", av_s(holder_workid))
            .item("body", av_s(&body.to_string()))
            .condition_expression("attribute_not_exists(pk) OR expires < :now OR workid = :wid")
            .expression_attribute_values(":now", av_s(now))
            .expression_attribute_values(":wid", av_s(holder_workid));
        match req.send().await {
            Ok(_) => Ok(()),
            Err(e) => {
                let msg = format!("{e:?}");
                if msg.contains("ConditionalCheckFailed") {
                    let current = self.get(table, pk, sk).await.unwrap_or(None);
                    Err(StorageError::Conflict(current))
                } else {
                    Err(StorageError::Backend(msg))
                }
            }
        }
    }

    async fn bump_seq(&self, project: &str, logical_table: &str) -> Result<u64, StorageError> {
        let out = self
            .client
            .update_item()
            .table_name(self.phys("versions"))
            .key("pk", av_s(project))
            .key("sk", av_s(logical_table))
            .update_expression("ADD seq_num :one SET updated = :ts")
            .expression_attribute_values(":one", AttributeValue::N("1".into()))
            .expression_attribute_values(":ts", av_s(&now_utc()))
            .return_values(aws_sdk_dynamodb::types::ReturnValue::AllNew)
            .send()
            .await
            .map_err(berr)?;
        let seq = out
            .attributes
            .as_ref()
            .and_then(|a| a.get("seq_num"))
            .and_then(|v| v.as_n().ok())
            .and_then(|n| n.parse::<u64>().ok())
            .unwrap_or(0);
        Ok(seq)
    }

    async fn get_versions(&self, project: &str) -> Result<Vec<VersionRow>, StorageError> {
        let resp = self
            .client
            .query()
            .table_name(self.phys("versions"))
            .key_condition_expression("pk = :pk")
            .expression_attribute_values(":pk", av_s(project))
            .send()
            .await
            .map_err(berr)?;
        let mut out = Vec::new();
        for item in resp.items() {
            out.push(VersionRow {
                projectname: item.get("pk").and_then(|v| v.as_s().ok()).cloned().unwrap_or_default(),
                table: item.get("sk").and_then(|v| v.as_s().ok()).cloned().unwrap_or_default(),
                seq: item
                    .get("seq_num")
                    .and_then(|v| v.as_n().ok())
                    .and_then(|n| n.parse().ok())
                    .unwrap_or(0),
                updated: item.get("updated").and_then(|v| v.as_s().ok()).cloned().unwrap_or_default(),
            });
        }
        Ok(out)
    }

    async fn scan_keyed(&self, table: &str) -> Result<Vec<KeyedRow>, StorageError> {
        let s = |item: &HashMap<String, AttributeValue>, k: &str| {
            item.get(k).and_then(|v| v.as_s().ok()).cloned().unwrap_or_default()
        };
        Ok(self
            .scan_items(table)
            .await?
            .iter()
            .filter_map(|item| {
                let body = Self::item_body(item)?;
                let version = item
                    .get("version")
                    .and_then(|v| v.as_n().ok())
                    .and_then(|n| n.parse::<u64>().ok())
                    .or_else(|| body.get("version").and_then(|v| v.as_u64()))
                    .unwrap_or(0);
                Some(KeyedRow { pk: s(item, "pk"), sk: s(item, "sk"), version, body })
            })
            .collect())
    }

    async fn all_versions(&self) -> Result<Vec<VersionRow>, StorageError> {
        Ok(self
            .scan_items("versions")
            .await?
            .iter()
            .map(|item| VersionRow {
                projectname: item.get("pk").and_then(|v| v.as_s().ok()).cloned().unwrap_or_default(),
                table: item.get("sk").and_then(|v| v.as_s().ok()).cloned().unwrap_or_default(),
                seq: item.get("seq_num").and_then(|v| v.as_n().ok()).and_then(|n| n.parse().ok()).unwrap_or(0),
                updated: item.get("updated").and_then(|v| v.as_s().ok()).cloned().unwrap_or_default(),
            })
            .collect())
    }

    fn backend_name(&self) -> &'static str {
        "dynamodb"
    }
}
