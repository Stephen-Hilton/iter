//! On/off switches (decided 2026-10-08): four levels, each Active or Stopped,
//! each on its own record:
//! - the iter_data server (one row, `webui` / `server`): the master switch;
//! - an engine (`engine.active`);
//! - an LLM account (`account.active`);
//! - a project (`project.state`, Running | Draining | Stopped — unchanged).
//!
//! Stopped means "start nothing new": work already in progress runs to its
//! end. A record without `active` is Active. The server enforces the server
//! and engine switches itself (the assignments report a Running project as
//! Stopped, `next` refuses, no GraphRAG work is offered); an account switch
//! travels on the assignments (`BilledAccount.active`) and the engine's
//! ladder never picks a stopped account.

use crate::api::{ApiError, AppState, AuthUser, GLOBAL, NOSK, bad, forbidden, notfound};
use crate::storage::{Storage, StorageError, body_str};
use axum::extract::State;
use axum::routing::{get, post};
use axum::{Json, Router};
use iter_core::now_utc;
use iter_core::settings::{ITER_DATA_SELF, node_id, parse_node_id};
use serde_json::{Value, json};
use std::sync::Arc;

/// Where the server switch lives: the `webui` table (server-wide settings).
const SERVER_TABLE: &str = "webui";
const SERVER_PK: &str = "server";

/// A record's switch: `active` absent = Active.
pub fn is_on(rec: &Value) -> bool {
    rec.get("active").and_then(|v| v.as_bool()).unwrap_or(true)
}

pub async fn server_row(store: &dyn Storage) -> Result<Value, StorageError> {
    Ok(store.get(SERVER_TABLE, SERVER_PK, NOSK).await?.unwrap_or_else(|| json!({"active": true})))
}

pub async fn server_on(store: &dyn Storage) -> Result<bool, StorageError> {
    Ok(is_on(&server_row(store).await?))
}

/// Why `engine` may start nothing new: the server switch, then the engine's
/// own. None = it may (a missing engine record is not a switch).
pub async fn engine_block(store: &dyn Storage, engine: &str) -> Result<Option<&'static str>, StorageError> {
    if !server_on(store).await? {
        return Ok(Some("server"));
    }
    let on = store.get("engine", engine, NOSK).await?.map(|e| is_on(&e)).unwrap_or(true);
    Ok(if on { None } else { Some("engine") })
}

pub fn routes() -> Router<Arc<AppState>> {
    Router::new().route("/api/server", get(server_get)).route("/api/switch", post(switch_set))
}

async fn server_get(_u: AuthUser, State(st): State<Arc<AppState>>) -> Result<Json<Value>, ApiError> {
    let mut row = server_row(st.store.as_ref()).await?;
    row["active"] = json!(is_on(&row));
    Ok(Json(row))
}

#[derive(serde::Deserialize)]
struct SwitchReq {
    /// `iter_data:self` (or `server`), `iter_engine:<id|name>`, `account:<id|name>`
    target: String,
    active: bool,
}

/// `POST /api/switch {target, active}`: the server and accounts are an
/// admin's; an engine also its owner's. Projects keep their own `state`
/// (`PUT /api/projects/{p}`), which carries the Draining step.
async fn switch_set(user: AuthUser, State(st): State<Arc<AppState>>, Json(req): Json<SwitchReq>) -> Result<Json<Value>, ApiError> {
    let store = st.store.as_ref();
    let stamp = |rec: &mut Value| {
        rec["active"] = json!(req.active);
        rec["active_changed"] = json!(now_utc());
        rec["active_by"] = json!(user.sub);
    };
    let target = req.target.trim();
    if target == ITER_DATA_SELF || target == "server" {
        user.require_admin()?;
        let mut row = server_row(store).await?;
        stamp(&mut row);
        store.put(SERVER_TABLE, SERVER_PK, NOSK, &row).await?;
        store.bump_seq(GLOBAL, SERVER_TABLE).await?;
        return Ok(Json(json!({"target": ITER_DATA_SELF, "active": req.active})));
    }
    let id = crate::settings::resolve_node_ref(store, target).await?;
    let (t, key) = parse_node_id(&id).ok_or_else(|| bad(format!("'{target}' is not a switch (iter_data:self, iter_engine:<id>, account:<id>)")))?;
    let table = match t {
        "iter_engine" => "engine",
        "account" => "account",
        "project" => return Err(bad("a project's switch is its state: PUT /api/projects/{p} with state Running|Draining|Stopped")),
        _ => return Err(bad(format!("{t} nodes have no switch (iter_data:self, iter_engine, account)"))),
    };
    let mut rec = store.get(table, key, NOSK).await?.ok_or_else(notfound)?;
    if user.role != "admin" {
        let mine = t == "iter_engine" && {
            let edges = crate::settings::load_edges(store).await?;
            body_str(&rec, "user") == user.sub || crate::settings::engines_of_user(store, &edges, &user.sub).await?.iter().any(|e| e == key)
        };
        if !mine {
            return Err(forbidden());
        }
    }
    stamp(&mut rec);
    store.put(table, key, NOSK, &rec).await?;
    store.bump_seq(GLOBAL, table).await?;
    Ok(Json(json!({"target": node_id(t, key), "active": req.active})))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn absent_is_active() {
        assert!(is_on(&json!({})));
        assert!(is_on(&json!({"active": true})));
        assert!(!is_on(&json!({"active": false})));
        // a non-boolean is not a switch position: treated as absent
        assert!(is_on(&json!({"active": "no"})));
    }
}
