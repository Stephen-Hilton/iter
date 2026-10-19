//! The default agents and agent tooling every new install starts with
//! (iter_data/defaults/, embedded): plan, code, test, usecase, refactor,
//! ingest, explain and summary, the `_shared` rules, the `_critic` persona,
//! the capabilities and the source instructions. Each record is two files:
//! `<name>.json` (every field but the prompt) and `<name>.md` (the prompt:
//! an agent's `promptbody`, a tooling row's `body`).
//!
//! Added once per database, at the first start that finds no `defaults_added`
//! marker: a record already there is never overwritten, and a default an admin
//! deletes afterwards stays deleted across restarts. `iter_data defaults add`
//! adds the missing ones again on request.
//!
//! The records came from the live iter database (2026-10-19); pdy-dev-only
//! ones (`deploy`, `_block_cluster_restart`) and retired aliases
//! (`testwriter`, `_interface_contracts`, `_testgroup_authoring`) are left out.

use crate::api::{GLOBAL, NOSK};
use crate::settings::{EDGE_TABLE, META_PK};
use crate::storage::{Storage, StorageError};
use serde_json::{json, Value};

const ADDED_SK: &str = "defaults_added";

macro_rules! record {
    ($dir:literal, $name:literal) => {
        (
            $name,
            include_str!(concat!("../defaults/", $dir, "/", $name, ".json")),
            include_str!(concat!("../defaults/", $dir, "/", $name, ".md")),
        )
    };
}

/// (name, fields JSON, prompt) per default agent.
pub const AGENTS: &[(&str, &str, &str)] = &[
    record!("agents", "code"),
    record!("agents", "explain"),
    record!("agents", "ingest"),
    record!("agents", "plan"),
    record!("agents", "refactor"),
    record!("agents", "summary"),
    record!("agents", "test"),
    record!("agents", "usecase"),
];

/// (name, fields JSON, body) per default tooling row.
pub const TOOLING: &[(&str, &str, &str)] = &[
    record!("tooling", "_shared"),
    record!("tooling", "_critic"),
    record!("tooling", "_ask_the_human"),
    record!("tooling", "_connections"),
    record!("tooling", "_create_new_workitem"),
    record!("tooling", "_critical_review"),
    record!("tooling", "_iter_file_authoring"),
    record!("tooling", "_reject_invalid_work"),
    record!("tooling", "_runtests"),
    record!("tooling", "_test_node_authoring"),
    record!("tooling", "_teststate"),
    record!("tooling", "_usecase_links"),
    record!("tooling", "agent"),
    record!("tooling", "error"),
    record!("tooling", "user"),
];

/// One default as the row stored in its table.
fn row(fields: &str, prompt: &str, prompt_key: &str) -> Value {
    let mut v: Value = serde_json::from_str(fields).expect("iter_data/defaults: a .json file does not parse");
    v[prompt_key] = Value::String(prompt.to_string());
    v
}

pub fn agent_row(fields: &str, prompt: &str) -> Value {
    row(fields, prompt, "promptbody")
}

pub fn tooling_row(fields: &str, body: &str) -> Value {
    row(fields, body, "body")
}

/// Adds every default that is missing; returns the names added.
pub async fn add_missing(store: &dyn Storage) -> Result<Vec<String>, StorageError> {
    let mut added = Vec::new();
    // tooling first: a new agent then picks up the shared rules' `uses` edge
    for (name, fields, body) in TOOLING {
        if store.get("agent_tooling", name, NOSK).await?.is_none() {
            let r = tooling_row(fields, body);
            store.put("agent_tooling", name, NOSK, &r).await?;
            store.bump_seq(GLOBAL, "agent_tooling").await?;
            crate::settings::on_tooling_created(store, &r).await?;
            added.push(name.to_string());
        }
    }
    for (name, fields, prompt) in AGENTS {
        if store.get("agent", name, NOSK).await?.is_none() {
            store.put("agent", name, NOSK, &agent_row(fields, prompt)).await?;
            store.bump_seq(GLOBAL, "agent").await?;
            crate::settings::on_agent_created(store, name).await?;
            added.push(name.to_string());
        }
    }
    Ok(added)
}

/// At startup: the defaults, once per database (see the module comment).
pub async fn add_once(store: &dyn Storage) -> Result<(), StorageError> {
    if store.get(EDGE_TABLE, META_PK, ADDED_SK).await?.is_some() {
        return Ok(());
    }
    let added = add_missing(store).await?;
    store.put(EDGE_TABLE, META_PK, ADDED_SK, &json!({"at": iter_core::now_utc(), "added": added})).await?;
    if added.is_empty() {
        println!("[iter_data] default agents: all already here");
    } else {
        println!("[iter_data] default agents and tooling added: {}", added.join(", "));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_default_file_is_embedded_and_parses() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("defaults");
        for (sub, list) in [("agents", AGENTS), ("tooling", TOOLING)] {
            let mut on_disk: Vec<String> = std::fs::read_dir(dir.join(sub))
                .unwrap()
                .filter_map(|e| e.ok()?.file_name().to_str()?.strip_suffix(".json").map(String::from))
                .collect();
            on_disk.sort();
            let mut listed: Vec<String> = list.iter().map(|(n, _, _)| n.to_string()).collect();
            listed.sort();
            assert_eq!(on_disk, listed, "defaults/{sub}: every <name>.json must be listed in defaults.rs");
        }
        for (name, fields, prompt) in AGENTS {
            let a: iter_core::AgentDef = serde_json::from_value(agent_row(fields, prompt)).unwrap();
            assert_eq!(a.name, *name);
            assert!(!a.promptbody.trim().is_empty(), "{name} has no prompt");
        }
        for (name, fields, body) in TOOLING {
            let t: iter_core::AgentTooling = serde_json::from_value(tooling_row(fields, body)).unwrap();
            assert_eq!(t.name, *name);
            assert!(iter_core::TOOLING_KINDS.contains(&t.kind.as_str()), "{name}: kind {}", t.kind);
        }
    }

    #[tokio::test]
    async fn defaults_are_added_once_and_a_deleted_one_stays_deleted() {
        let (a, _db) = crate::test_db::store().await;
        let store: &dyn Storage = &a;
        crate::settings::bootstrap(store).await.unwrap();
        // an admin's own edit made before the first start is kept
        store.put("agent", "code", NOSK, &json!({"name": "code", "promptbody": "mine"})).await.unwrap();
        add_once(store).await.unwrap();
        assert_eq!(store.get("agent", "code", NOSK).await.unwrap().unwrap()["promptbody"], "mine");
        for (name, _, _) in AGENTS {
            assert!(store.get("agent", name, NOSK).await.unwrap().is_some(), "{name} missing");
        }
        assert!(store.get("agent_tooling", "_critic", NOSK).await.unwrap().is_some());
        // deleted after the first start: the next start leaves it deleted
        store.delete("agent", "plan", NOSK).await.unwrap();
        add_once(store).await.unwrap();
        assert!(store.get("agent", "plan", NOSK).await.unwrap().is_none());
        // `iter_data defaults add` brings it back
        assert_eq!(add_missing(store).await.unwrap(), vec!["plan".to_string()]);
    }
}
