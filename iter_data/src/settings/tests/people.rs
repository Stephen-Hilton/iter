//! The people-only MCP tools and self-minted tokens (2026-10-21): a person's
//! token (or an agent connected as that person) manages the work queue and
//! the GraphRAG documents; engine sessions — iter's own agents — cannot.

use super::hardening::tool;
use super::*;

/// alice (a person) and engine e1 (eng1) both work on `p`.
async fn world() -> T {
    let t = T::new().await;
    t.served("p", "e1", "eng1").await;
    t.ok("adm", "POST", "/api/settings/edges", Some(json!({"from": "user:alice", "to": "project:p"}))).await;
    t
}

async fn listed(t: &T, who: &str) -> Vec<String> {
    let (c, v) = t.call(who, "POST", "/mcp", Some(json!({"jsonrpc": "2.0", "id": 1, "method": "tools/list"}))).await;
    assert_eq!(c, 200, "{v}");
    v["result"]["tools"].as_array().unwrap().iter().map(|x| x["name"].as_str().unwrap().to_string()).collect()
}

#[tokio::test]
async fn engine_sessions_neither_list_nor_call_people_tools() {
    let t = world().await;
    let people = listed(&t, "alice").await;
    let engine = listed(&t, "eng1").await;
    for n in crate::mcp_docs::PEOPLE_ONLY {
        assert!(people.contains(&n.to_string()), "a person sees {n}");
        assert!(!engine.contains(&n.to_string()), "an engine session does not see {n}");
        let msg = json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": {"name": n, "arguments": {"project": "p", "id": "x"}}});
        let (_, v) = t.call("eng1", "POST", "/mcp", Some(msg)).await;
        assert!(v["error"]["message"].as_str().unwrap_or("").contains("for a person's token"), "{n}: {v}");
    }
    assert_eq!(engine.len() + crate::mcp_docs::PEOPLE_ONLY.len(), people.len());
    // an engine session cannot choose a new item's state; a person can
    t.item("p", json!({"id": "run-1", "state": "in-progress", "engine": "e1", "agent": "code"})).await;
    let (e, out) = tool(&t, "eng1", "workitem_create", json!({"project": "p", "workid": "run-1", "title": "x", "request": "do x", "state": "parked"})).await;
    assert!(e && out.to_string().contains("only a person's token"), "{out}");
    let (e, out) = tool(&t, "alice", "workitem_create", json!({"project": "p", "title": "later", "request": "do it later", "state": "parked"})).await;
    assert!(!e && out["state"] == "parked", "{out}");
    t.done().await;
}

#[tokio::test]
async fn answer_set_state_edit_and_stop_do_what_the_webui_does() {
    let t = world().await;
    // answer: the newest form filled, the item queued, the approval cleared
    t.item("p", json!({"id": "q-1", "name": "which port?", "state": "question", "needs_approval": true})).await;
    let form = |title: &str| json!({"title": title, "summary": "", "detail": "", "fields": [
        {"key": "port", "label": "Port", "type": "int", "value": 0},
        {"key": "env", "label": "Env", "type": "radio", "options": [{"value": "dev", "desc": ""}, {"value": "prod", "desc": ""}], "value": null}]});
    t.ok("alice", "POST", "/api/projects/p/workitems/q-1/details", Some(json!({"key": "question", "valuetype": "json", "value": form("old")}))).await;
    t.ok("alice", "POST", "/api/projects/p/workitems/q-1/details", Some(json!({"key": "question", "valuetype": "json", "value": form("new")}))).await;
    let (e, out) = tool(&t, "alice", "workitem_answer", json!({"project": "p", "id": "q-1", "answer": "8400"})).await;
    assert!(e && out.to_string().contains("several fields"), "{out}");
    let (e, out) = tool(&t, "alice", "workitem_answer", json!({"project": "p", "id": "q-1", "answers": {"nope": 1}})).await;
    assert!(e && out.to_string().contains("no field"), "{out}");
    let (e, out) = tool(&t, "alice", "workitem_answer", json!({"project": "p", "id": "q-1", "answers": {"port": "8400", "env": "dev"}})).await;
    assert!(!e, "{out}");
    let item = t.ok("alice", "GET", "/api/projects/p/workitems/q-1", None).await;
    assert_eq!((item["state"].as_str(), item["needs_approval"].as_bool()), (Some("queued"), Some(false)), "{item}");
    let rows = t.ok("alice", "GET", "/api/projects/p/workitems/q-1/details", None).await;
    let forms: Vec<&Value> = rows.as_array().unwrap().iter().filter(|r| r["key"] == "question").collect();
    assert_eq!(forms[0]["value"]["fields"][0]["value"], json!(0), "the older form is untouched");
    assert_eq!((forms[1]["value"]["fields"][0]["value"].clone(), forms[1]["value"]["fields"][1]["value"].clone()), (json!(8400), json!("dev")));
    let (e, out) = tool(&t, "alice", "workitem_answer", json!({"project": "p", "id": "q-1", "answers": {"env": "staging"}})).await;
    assert!(e && out.to_string().contains("not one of its options"), "a value outside the options is refused: {out}");

    // set_state: park with a reason (recorded), queue with run_now
    let (e, out) = tool(&t, "alice", "workitem_set_state", json!({"project": "p", "id": "q-1", "state": "parked", "reason": "vendor"})).await;
    assert!(!e && out["was"] == "queued", "{out}");
    let rows = t.ok("alice", "GET", "/api/projects/p/workitems/q-1/details", None).await;
    assert!(rows.to_string().contains("queued -> parked: vendor"), "{rows}");
    let (e, out) = tool(&t, "alice", "workitem_set_state", json!({"project": "p", "id": "q-1", "state": "queued", "run_now": true})).await;
    assert!(!e, "{out}");
    let item = t.ok("alice", "GET", "/api/projects/p/workitems/q-1", None).await;
    assert_eq!((item["state"].as_str(), item["run_now"].as_bool()), (Some("queued"), Some(true)), "{item}");
    let (e, _) = tool(&t, "alice", "workitem_set_state", json!({"project": "p", "id": "q-1", "state": "parked", "run_now": true})).await;
    assert!(e, "run_now only with queued");

    // edit: request text, tags added and removed, a running item refused
    t.item("p", json!({"id": "ed-1", "name": "old name", "tags": [{"text": "a", "color": ""}, {"text": "b", "color": ""}]})).await;
    t.ok("alice", "PUT", "/api/projects/p/workitems/ed-1/details/0", Some(json!({"key": "request", "valuetype": "text", "value": "old request"}))).await;
    let (e, out) = tool(&t, "alice", "workitem_edit", json!({"project": "p", "id": "ed-1", "name": "new name", "request": "new request",
                                                          "add_tags": ["c:#ff0000"], "remove_tags": ["a"]})).await;
    assert!(!e, "{out}");
    let item = t.ok("alice", "GET", "/api/projects/p/workitems/ed-1", None).await;
    let tags: Vec<&str> = item["tags"].as_array().unwrap().iter().map(|x| x["text"].as_str().unwrap()).collect();
    assert_eq!((item["name"].as_str(), tags), (Some("new name"), vec!["b", "c"]));
    let rows = t.ok("alice", "GET", "/api/projects/p/workitems/ed-1/details", None).await;
    assert_eq!(rows[0]["value"], "new request", "{rows}");
    let (e, out) = tool(&t, "alice", "workitem_edit", json!({"project": "p", "id": "ed-1"})).await;
    assert!(e && out.to_string().contains("nothing to change"), "{out}");
    t.item("p", json!({"id": "run-2", "state": "in-progress", "engine": "e1"})).await;
    let (e, out) = tool(&t, "alice", "workitem_edit", json!({"project": "p", "id": "run-2", "name": "x"})).await;
    assert!(e && out.to_string().contains("in progress"), "{out}");

    // stop: only a running item; it stays in-progress with the flag the engine reads
    let (e, out) = tool(&t, "alice", "workitem_stop", json!({"project": "p", "id": "ed-1"})).await;
    assert!(e && out.to_string().contains("not in-progress"), "{out}");
    let (e, out) = tool(&t, "alice", "workitem_stop", json!({"project": "p", "id": "run-2"})).await;
    assert!(!e, "{out}");
    let item = t.ok("alice", "GET", "/api/projects/p/workitems/run-2", None).await;
    assert_eq!((item["state"].as_str(), item["stop_requested"].as_bool()), (Some("in-progress"), Some(true)));

    // reopen a failed item; delete one; every change recorded as alice
    t.item("p", json!({"id": "f-1", "state": "failed", "lasterror": "boom"})).await;
    let (e, out) = tool(&t, "alice", "workitem_reopen", json!({"project": "p", "id": "f-1", "reason": "fixed upstream"})).await;
    assert!(!e && out["state"] == "queued", "{out}");
    let (e, out) = tool(&t, "alice", "workitem_delete", json!({"project": "p", "id": "f-1"})).await;
    assert!(!e && out["deleted"] == true, "{out}");
    assert!(t.store().get("workitem", "p", "f-1").await.unwrap().is_none());
    let rows = t.ok("alice", "GET", "/api/projects/p/workitems/q-1/details", None).await;
    assert!(rows.as_array().unwrap().iter().filter(|r| r["key"] == "doc").all(|r| r["by"] == "alice"), "{rows}");
    t.done().await;
}

#[tokio::test]
async fn clone_follow_up_and_duplicate() {
    let t = world().await;
    t.item("p", json!({"id": "src-1", "name": "gateway client", "priority": 12, "tags": [{"text": "gw", "color": ""}]})).await;
    t.ok("alice", "PUT", "/api/projects/p/workitems/src-1/details/0", Some(json!({"key": "request", "valuetype": "text", "value": "build the client"}))).await;
    let (e, out) = tool(&t, "alice", "workitem_clone", json!({"project": "p", "id": "src-1", "as": "followup"})).await;
    assert!(e && out.to_string().contains("needs `request`"), "{out}");
    let (e, f) = tool(&t, "alice", "workitem_clone", json!({"project": "p", "id": "src-1", "as": "followup", "request": "use it in the export"})).await;
    assert!(!e && f["requestedby"] == "user:alice", "{f}");
    let fid = f["id"].as_str().unwrap();
    let item = t.ok("alice", "GET", &format!("/api/projects/p/workitems/{fid}"), None).await;
    assert_eq!((item["name"].as_str(), item["blockedby"].clone()), (Some("Follow-up: gateway client"), json!(["src-1"])), "{item}");
    let rows = t.ok("alice", "GET", &format!("/api/projects/p/workitems/{fid}/details"), None).await;
    let req = rows[0]["value"].as_str().unwrap();
    assert!(req.contains("build the client") && req.ends_with("use it in the export"), "{req}");
    let (e, out) = tool(&t, "alice", "workitem_clone", json!({"project": "p", "id": "src-1", "as": "run_now"})).await;
    assert!(e && out.to_string().contains("scheduled"), "{out}");
    // duplicate: closes complete; the survivor counts the repeat
    t.item("p", json!({"id": "dup-1", "name": "gateway client again"})).await;
    let (e, out) = tool(&t, "alice", "workitem_duplicate_of", json!({"project": "p", "id": "dup-1", "survivor": "src-1", "reason": "same client"})).await;
    assert!(!e && out["state"] == "complete", "{out}");
    t.done().await;
}

#[tokio::test]
async fn a_person_mints_their_own_token_and_whoami_shows_its_end() {
    let t = world().await;
    let (c, v) = t.call("alice", "POST", "/api/users/alice/token", Some(json!({"ttl_days": 90}))).await;
    assert_eq!(c, 200, "{v}");
    assert!(v["token"].as_str().unwrap().len() > 40 && v["role"] == "user" && !v["expires"].as_str().unwrap().is_empty(), "{v}");
    // the minted token works, and whoami (REST and MCP) names its end
    let tok = v["token"].as_str().unwrap().to_string();
    let req = Request::builder().uri("/api/projects/p/whoami").header("authorization", format!("Bearer {tok}")).body(Body::empty()).unwrap();
    let resp = t.app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status().as_u16(), 200);
    let who: Value = serde_json::from_slice(&axum::body::to_bytes(resp.into_body(), 1 << 20).await.unwrap()).unwrap();
    assert!(who["requestedby"] == "user:alice" && matches!(who["token_days_left"].as_i64(), Some(89..=90)), "{who}");
    assert_eq!(who["token_expires"], v["expires"], "{who}");
    let (e, out) = tool(&t, "alice", "whoami", json!({"project": "p"})).await;
    assert!(!e && out["role"] == "user" && out["token_days_left"].as_i64() == Some(0), "the harness token lasts an hour: {out}");
    // limits: 1–365 days for oneself; never for someone else; never an engine
    assert_eq!(t.call("alice", "POST", "/api/users/alice/token", Some(json!({"ttl_days": 366}))).await.0, 400);
    assert_eq!(t.call("alice", "POST", "/api/users/alice/token", Some(json!({"ttl_days": 0}))).await.0, 400);
    assert_eq!(t.call("alice", "POST", "/api/users/bob/token", Some(json!({"ttl_days": 30}))).await.0, 403);
    assert_eq!(t.call("eng1", "POST", "/api/users/eng1/token", Some(json!({"ttl_days": 30}))).await.0, 403);
    assert_eq!(t.call("adm", "POST", "/api/users/bob/token", Some(json!({"ttl_days": 30}))).await.0, 200, "an admin mints for anyone");
    // a password change ends it, like every token of hers
    let mut row = t.store().get("webui_user", "alice", NOSK).await.unwrap().unwrap();
    row["tokenver"] = json!(2);
    t.store().put("webui_user", "alice", NOSK, &row).await.unwrap();
    let req = Request::builder().uri("/api/projects/p/whoami").header("authorization", format!("Bearer {tok}")).body(Body::empty()).unwrap();
    assert_eq!(t.app.clone().oneshot(req).await.unwrap().status().as_u16(), 401);
    t.done().await;
}
