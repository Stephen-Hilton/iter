//! ArangoDB backend (iter4+, decided 2026-09-28): one document collection per
//! logical table in database `--arango-db` (default "iter5"), reached over the
//! plain HTTP API — no driver. Rows are `{_key, pk, sk, version, expires,
//! workid, body}`; `_key` is pk:sk with forbidden characters %-escaped, so a
//! get is a primary-index hit, and a persistent [pk, sk] index serves query.
//!
//! The three operations needing backend-native atomicity are single AQL
//! statements (one transaction each); ArangoDB's write-write conflict (1200)
//! and unique-constraint violation (1210) are the "someone else won" signal.
//!
//! The graph (vertex collection `node`, edge collection `link`, named graph
//! `iter_map`) lives in the same database — see graph.rs.

use crate::storage::{Storage, StorageError};
use async_trait::async_trait;
use iter_core::{VersionRow, now_utc};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

pub const GRAPH_NAME: &str = "iter_map";
pub const NODE_COLL: &str = "node";
pub const LINK_COLL: &str = "link";
/// seq counters; kept out of the generic row shape
const VERSIONS_COLL: &str = "versions";

pub struct ArangoBackend {
    http: reqwest::Client,
    /// http://host:port
    server: String,
    db: String,
    user: String,
    password: String,
}

/// An ArangoDB error answer: HTTP code + errorNum + message.
#[derive(Debug)]
pub struct ArangoErr {
    pub code: u16,
    pub num: i64,
    pub msg: String,
}

impl std::fmt::Display for ArangoErr {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "arango {} (errorNum {}): {}", self.code, self.num, self.msg)
    }
}

pub const ERR_CONFLICT: i64 = 1200;
pub const ERR_UNIQUE: i64 = 1210;
const ERR_DUPLICATE_NAME: i64 = 1207;
const ERR_GRAPH_DUPLICATE: i64 = 1925;

fn berr<E: std::fmt::Display>(e: E) -> StorageError {
    StorageError::Backend(e.to_string())
}

/// Characters ArangoDB accepts in a document key besides [A-Za-z0-9].
/// ':' is our pk/sk separator and '%' our escape, so both are escaped too.
fn key_char_ok(c: char) -> bool {
    c.is_ascii_alphanumeric() || "_-.@()+,=;$!*'".contains(c)
}

fn escape_part(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        if key_char_ok(c) {
            out.push(c);
        } else {
            let mut buf = [0u8; 4];
            for b in c.encode_utf8(&mut buf).bytes() {
                out.push_str(&format!("%{b:02X}"));
            }
        }
    }
    out
}

/// Deterministic document key for (pk, sk). Readable when it fits the
/// 254-byte key limit, else "h:" + sha256 (pk and sk stay on the row).
pub fn doc_key(pk: &str, sk: &str) -> String {
    let k = format!("{}:{}", escape_part(pk), escape_part(sk));
    if k.len() <= 254 {
        return k;
    }
    let mut h = Sha256::new();
    h.update(pk.as_bytes());
    h.update([0u8]);
    h.update(sk.as_bytes());
    let d = h.finalize();
    let hex: String = d.iter().map(|b| format!("{b:02x}")).collect();
    format!("h:{hex}")
}

impl ArangoBackend {
    pub async fn new(server: &str, db: &str, user: &str, password: &str) -> Result<Self, StorageError> {
        let http = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(60))
            .build()
            .map_err(berr)?;
        let b = Self {
            http,
            server: server.trim_end_matches('/').to_string(),
            db: db.to_string(),
            user: user.to_string(),
            password: password.to_string(),
        };
        // arangod may still be starting (the container entrypoint races it)
        let mut last = String::new();
        for _ in 0..60 {
            match b.raw("GET", "/_db/_system/_api/version", None).await {
                Ok(_) => {
                    last.clear();
                    break;
                }
                Err(e) => last = e.to_string(),
            }
            tokio::time::sleep(std::time::Duration::from_secs(1)).await;
        }
        if !last.is_empty() {
            return Err(StorageError::Backend(format!("arango at {server} not reachable: {last}")));
        }
        b.ensure_schema().await?;
        Ok(b)
    }

    pub fn db_name(&self) -> &str {
        &self.db
    }

    /// One HTTP call. `path` starts with "/" and includes any /_db/<db> prefix.
    async fn raw(&self, method: &str, path: &str, body: Option<&Value>) -> Result<Value, ArangoErr> {
        self.raw_trx(method, path, body, None).await
    }

    async fn raw_trx(&self, method: &str, path: &str, body: Option<&Value>, trx: Option<&str>) -> Result<Value, ArangoErr> {
        let url = format!("{}{}", self.server, path);
        let m = reqwest::Method::from_bytes(method.as_bytes()).unwrap_or(reqwest::Method::GET);
        let mut req = self.http.request(m, &url).basic_auth(&self.user, Some(&self.password));
        if let Some(t) = trx {
            req = req.header("x-arango-trx-id", t);
        }
        if let Some(b) = body {
            req = req.json(b);
        }
        let resp = req.send().await.map_err(|e| ArangoErr { code: 0, num: 0, msg: e.to_string() })?;
        let code = resp.status().as_u16();
        let text = resp.text().await.unwrap_or_default();
        let v: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
        if code >= 400 || v.get("error").and_then(|e| e.as_bool()).unwrap_or(false) {
            return Err(ArangoErr {
                code,
                num: v.get("errorNum").and_then(|n| n.as_i64()).unwrap_or(0),
                msg: v
                    .get("errorMessage")
                    .and_then(|m| m.as_str())
                    .map(|s| s.to_string())
                    .unwrap_or_else(|| text.chars().take(300).collect()),
            });
        }
        Ok(v)
    }

    /// Call inside this backend's database.
    pub async fn dbcall(&self, method: &str, path: &str, body: Option<&Value>) -> Result<Value, ArangoErr> {
        let p = format!("/_db/{}{}", self.db, path);
        self.raw(method, &p, body).await
    }

    /// Run AQL, following the cursor to the end.
    pub async fn aql(&self, query: &str, bind: Value) -> Result<Vec<Value>, ArangoErr> {
        let first = self
            .dbcall("POST", "/_api/cursor", Some(&json!({"query": query, "bindVars": bind, "batchSize": 5000})))
            .await?;
        let mut out: Vec<Value> = first.get("result").and_then(|r| r.as_array()).cloned().unwrap_or_default();
        let mut more = first.get("hasMore").and_then(|h| h.as_bool()).unwrap_or(false);
        let id = first.get("id").and_then(|i| i.as_str()).unwrap_or("").to_string();
        while more && !id.is_empty() {
            let next = self.dbcall("POST", &format!("/_api/cursor/{id}"), None).await?;
            if let Some(arr) = next.get("result").and_then(|r| r.as_array()) {
                out.extend(arr.iter().cloned());
            }
            more = next.get("hasMore").and_then(|h| h.as_bool()).unwrap_or(false);
        }
        Ok(out)
    }

    /// Begin a stream transaction writing `write` collections; returns its id.
    pub async fn begin(&self, write: &[&str]) -> Result<String, ArangoErr> {
        let r = self
            .dbcall("POST", "/_api/transaction/begin", Some(&json!({"collections": {"write": write}})))
            .await?;
        Ok(r["result"]["id"].as_str().unwrap_or("").to_string())
    }

    /// AQL inside a stream transaction (results fit one batch: graph writes return nothing).
    pub async fn aql_in(&self, trx: &str, query: &str, bind: Value) -> Result<Vec<Value>, ArangoErr> {
        let p = format!("/_db/{}/_api/cursor", self.db);
        let r = self
            .raw_trx("POST", &p, Some(&json!({"query": query, "bindVars": bind, "batchSize": 100000})), Some(trx))
            .await?;
        Ok(r.get("result").and_then(|x| x.as_array()).cloned().unwrap_or_default())
    }

    pub async fn commit(&self, trx: &str) -> Result<(), ArangoErr> {
        self.dbcall("PUT", &format!("/_api/transaction/{trx}"), None).await.map(|_| ())
    }

    pub async fn abort(&self, trx: &str) {
        let _ = self.dbcall("DELETE", &format!("/_api/transaction/{trx}"), None).await;
    }

    /// AQL with retries on write-write conflicts (for idempotent upserts).
    pub async fn aql_retry(&self, query: &str, bind: Value) -> Result<Vec<Value>, ArangoErr> {
        let mut delay = 5u64;
        for attempt in 0..20 {
            match self.aql(query, bind.clone()).await {
                Err(e) if (e.num == ERR_CONFLICT || e.num == ERR_UNIQUE) && attempt < 19 => {
                    tokio::time::sleep(std::time::Duration::from_millis(delay)).await;
                    delay = (delay * 2).min(200);
                }
                other => return other,
            }
        }
        unreachable!()
    }

    async fn ensure_schema(&self) -> Result<(), StorageError> {
        match self.raw("POST", "/_db/_system/_api/database", Some(&json!({"name": self.db}))).await {
            Ok(_) => {}
            Err(e) if e.num == ERR_DUPLICATE_NAME => {}
            Err(e) => return Err(berr(e)),
        }
        let mut colls: Vec<(&str, u8)> = iter_core::TABLES
            .iter()
            .filter(|t| **t != VERSIONS_COLL)
            .map(|t| (*t, 2u8))
            .collect();
        colls.push((VERSIONS_COLL, 2));
        colls.push((NODE_COLL, 2));
        colls.push((LINK_COLL, 3));
        for (name, ty) in &colls {
            match self.dbcall("POST", "/_api/collection", Some(&json!({"name": name, "type": ty}))).await {
                Ok(_) => {}
                Err(e) if e.num == ERR_DUPLICATE_NAME => {}
                Err(e) => return Err(berr(e)),
            }
        }
        // index creation is idempotent in ArangoDB (same definition = same index)
        for t in iter_core::TABLES.iter().filter(|t| **t != VERSIONS_COLL) {
            self.dbcall(
                "POST",
                &format!("/_api/index?collection={t}"),
                Some(&json!({"type": "persistent", "fields": ["pk", "sk"], "name": "pk_sk"})),
            )
            .await
            .map_err(berr)?;
        }
        for (coll, fields, name) in [
            (VERSIONS_COLL, json!(["projectname"]), "project"),
            (NODE_COLL, json!(["project", "path"]), "project_path"),
            (NODE_COLL, json!(["project", "nodetype", "name"]), "project_type_name"),
            (LINK_COLL, json!(["project"]), "project"),
        ] {
            self.dbcall(
                "POST",
                &format!("/_api/index?collection={coll}"),
                Some(&json!({"type": "persistent", "fields": fields, "name": name})),
            )
            .await
            .map_err(berr)?;
        }
        crate::rag::ensure_schema(self).await.map_err(berr)?;
        crate::nodes::ensure_schema(self).await.map_err(berr)?;
        let graph = json!({
            "name": GRAPH_NAME,
            "edgeDefinitions": [{"collection": LINK_COLL, "from": [NODE_COLL], "to": [NODE_COLL]}]
        });
        match self.dbcall("POST", "/_api/gharial", Some(&graph)).await {
            Ok(_) => {}
            Err(e) if e.num == ERR_GRAPH_DUPLICATE || e.code == 409 => {}
            Err(e) => return Err(berr(e)),
        }
        Ok(())
    }

    fn row(pk: &str, sk: &str, version: Option<u64>, body: &Value) -> Value {
        let mut d = json!({
            "_key": doc_key(pk, sk),
            "pk": pk,
            "sk": sk,
            "expires": body.get("expires").and_then(|v| v.as_str()).unwrap_or(""),
            "workid": body.get("workid").and_then(|v| v.as_str()).unwrap_or(""),
            "body": body,
        });
        if let Some(v) = version {
            d["version"] = json!(v);
        }
        d
    }

    /// Liveness for /health: our own database must answer, not just the server
    /// (a server that lost or never had the database is not healthy).
    pub async fn ping(&self) -> bool {
        self.dbcall("GET", "/_api/version", None).await.is_ok()
    }
}

#[async_trait]
impl Storage for ArangoBackend {
    async fn get(&self, table: &str, pk: &str, sk: &str) -> Result<Option<Value>, StorageError> {
        let key = doc_key(pk, sk);
        match self.dbcall("GET", &format!("/_api/document/{table}/{}", urlenc(&key)), None).await {
            Ok(d) => Ok(d.get("body").cloned()),
            Err(e) if e.code == 404 => Ok(None),
            Err(e) => Err(berr(e)),
        }
    }

    async fn put(&self, table: &str, pk: &str, sk: &str, body: &Value) -> Result<(), StorageError> {
        // keep the native version in step with the body when it carries one;
        // otherwise an update leaves the stored version alone (an insert gets 0)
        let ver = body.get("version").and_then(|v| v.as_u64());
        let q = "UPSERT {_key: @k}
                 INSERT MERGE(@doc, {version: @v == null ? 0 : @v})
                 UPDATE {body: @doc.body, expires: @doc.expires, workid: @doc.workid,
                         version: @v == null ? OLD.version : @v}
                 IN @@c OPTIONS {mergeObjects: false}";
        let doc = Self::row(pk, sk, None, body);
        self.aql_retry(q, json!({"k": doc["_key"], "doc": doc, "v": ver, "@c": table}))
            .await
            .map_err(berr)?;
        Ok(())
    }

    async fn delete(&self, table: &str, pk: &str, sk: &str) -> Result<bool, StorageError> {
        let key = doc_key(pk, sk);
        match self.dbcall("DELETE", &format!("/_api/document/{table}/{}", urlenc(&key)), None).await {
            Ok(_) => Ok(true),
            Err(e) if e.code == 404 => Ok(false),
            Err(e) => Err(berr(e)),
        }
    }

    async fn query(&self, table: &str, pk: &str) -> Result<Vec<Value>, StorageError> {
        self.aql("FOR d IN @@c FILTER d.pk == @pk SORT d.sk RETURN d.body", json!({"@c": table, "pk": pk}))
            .await
            .map_err(berr)
    }

    async fn scan(&self, table: &str) -> Result<Vec<Value>, StorageError> {
        self.aql("FOR d IN @@c SORT d.pk, d.sk RETURN d.body", json!({"@c": table}))
            .await
            .map_err(berr)
    }

    async fn put_versioned(
        &self,
        table: &str,
        pk: &str,
        sk: &str,
        body: &Value,
        expect: u64,
    ) -> Result<(), StorageError> {
        let key = doc_key(pk, sk);
        let res = if expect == 0 {
            let doc = Self::row(pk, sk, Some(1), body);
            self.dbcall("POST", &format!("/_api/document/{table}?silent=true"), Some(&doc))
                .await
                .map(|_| true)
        } else {
            let q = "FOR d IN @@c FILTER d._key == @k AND d.version == @e
                     UPDATE d WITH {version: @e + 1, body: @b} IN @@c OPTIONS {mergeObjects: false}
                     RETURN 1";
            self.aql(q, json!({"@c": table, "k": key, "e": expect, "b": body}))
                .await
                .map(|r| !r.is_empty())
        };
        match res {
            Ok(true) => Ok(()),
            Ok(false) => Err(StorageError::Conflict(self.get(table, pk, sk).await.unwrap_or(None))),
            Err(e) if e.num == ERR_CONFLICT || e.num == ERR_UNIQUE || e.code == 409 => {
                Err(StorageError::Conflict(self.get(table, pk, sk).await.unwrap_or(None)))
            }
            Err(e) => Err(berr(e)),
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
        let mut doc = Self::row(pk, sk, Some(0), body);
        doc["workid"] = json!(holder_workid);
        let q = "LET cur = DOCUMENT(CONCAT(@cn, '/', @k))
                 LET free = cur == null OR (cur.expires != '' AND cur.expires < @now) OR cur.workid == @wid
                 LET w = (FOR x IN (free ? [1] : [])
                            UPSERT {_key: @k} INSERT @doc REPLACE @doc IN @@c
                            RETURN 1)
                 RETURN {free: free, cur: cur == null ? null : cur.body}";
        let bind = json!({"cn": table, "@c": table, "k": doc["_key"], "now": now, "wid": holder_workid, "doc": doc});
        match self.aql(q, bind).await {
            Ok(r) => {
                let first = r.into_iter().next().unwrap_or(Value::Null);
                if first.get("free").and_then(|f| f.as_bool()).unwrap_or(false) {
                    Ok(())
                } else {
                    Err(StorageError::Conflict(first.get("cur").cloned().filter(|c| !c.is_null())))
                }
            }
            Err(e) if e.num == ERR_CONFLICT || e.num == ERR_UNIQUE => {
                Err(StorageError::Conflict(self.get(table, pk, sk).await.unwrap_or(None)))
            }
            Err(e) => Err(berr(e)),
        }
    }

    async fn bump_seq(&self, project: &str, logical_table: &str) -> Result<u64, StorageError> {
        let q = "UPSERT {_key: @k}
                 INSERT {_key: @k, projectname: @p, table: @t, seq: 1, updated: @now}
                 UPDATE {seq: OLD.seq + 1, updated: @now}
                 IN versions RETURN NEW.seq";
        let r = self
            .aql_retry(q, json!({"k": doc_key(project, logical_table), "p": project, "t": logical_table, "now": now_utc()}))
            .await
            .map_err(berr)?;
        Ok(r.first().and_then(|v| v.as_u64()).unwrap_or(0))
    }

    async fn get_versions(&self, project: &str) -> Result<Vec<VersionRow>, StorageError> {
        let rows = self
            .aql("FOR d IN versions FILTER d.projectname == @p SORT d.table RETURN d", json!({"p": project}))
            .await
            .map_err(berr)?;
        Ok(rows.iter().map(version_row).collect())
    }

    fn arango(&self) -> Option<&ArangoBackend> {
        Some(self)
    }

    fn backend_name(&self) -> &'static str {
        "arango"
    }
}

fn version_row(d: &Value) -> VersionRow {
    VersionRow {
        projectname: d["projectname"].as_str().unwrap_or("").to_string(),
        table: d["table"].as_str().unwrap_or("").to_string(),
        seq: d["seq"].as_u64().unwrap_or(0),
        updated: d["updated"].as_str().unwrap_or("").to_string(),
    }
}

/// Percent-encode a key for a URL path segment ('%' itself must be encoded).
fn urlenc(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || b"-_.".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_are_safe_and_distinct() {
        let a = doc_key("pdy-dev", "{topdir}/core/repos/x/");
        assert!(a.chars().all(|c| key_char_ok(c) || c == '%' || c == ':'));
        // the separator is escaped inside parts, so these never collide
        assert_ne!(doc_key("a:b", "c"), doc_key("a", "b:c"));
        assert_ne!(doc_key("a%3Ab", "c"), doc_key("a:b", "c"));
        assert_eq!(doc_key("my project", "-"), "my%20project:-");
        let long = "x".repeat(400);
        let h = doc_key(&long, "y");
        assert!(h.starts_with("h:") && h.len() <= 254);
    }
}
