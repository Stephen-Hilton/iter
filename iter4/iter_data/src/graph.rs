//! The architecture map (iter4, decided 2026-09-28): every `*.iter.md` node
//! file is a vertex, every `children` link an edge. The engine — the only
//! component with repo access — builds a snapshot (iter_local::graph) and
//! PUTs it here; iter_data replaces the project's vertices and edges in one
//! pass, keeping each vertex's test results across syncs.
//!
//! On ArangoDB the map is the named graph `iter_map` (vertex collection
//! `node`, edge collection `link`) and traversals are AQL. Other backends
//! keep the same documents as ordinary rows (`graph_node`, `graph_link`) and
//! answer the same routes by walking them in Rust.

use crate::api::{ApiError, AppState, AuthUser};
use crate::arango::{self, GRAPH_NAME, LINK_COLL, NODE_COLL};
use crate::storage::Storage;
use axum::extract::{Path, Query, State};
use axum::routing::{get, post};
use axum::{Json, Router};
use axum::http::StatusCode;
use iter_core::now_utc;
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

type Ctx = State<Arc<AppState>>;

/// The map lives in ArangoDB (iter4 stores nothing anywhere else).
fn arango(store: &dyn Storage) -> Result<&crate::arango::ArangoBackend, ApiError> {
    store.arango().ok_or_else(|| ApiError::Status(axum::http::StatusCode::INTERNAL_SERVER_ERROR, "the architecture map needs the ArangoDB store".into()))
}

/// the snapshot's hash + counts live on the iter3-era structure row
const META: &str = "project_structure";
const NOSK: &str = "-";

pub fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/api/projects/{name}/graph", get(graph_get).put(graph_put))
        .route("/api/projects/{name}/graph/stats", get(graph_stats))
        .route("/api/projects/{name}/graph/view", get(graph_view))
        .route("/api/projects/{name}/graph/edits", post(graph_edit))
        .route("/api/projects/{name}/graph/lookup", get(graph_lookup))
        .route("/api/projects/{name}/graph/owner", get(graph_owner))
        .route("/api/projects/{name}/graph/usecases/{ucid}", get(graph_usecase))
        .route("/api/projects/{name}/graph/nodes/{id}", get(node_get))
        .route("/api/projects/{name}/graph/nodes/{id}/neighbors", get(node_neighbors))
        .route("/api/projects/{name}/graph/nodes/{id}/testresult", post(node_testresult))
        // a large repository's snapshot is megabytes (pdy-dev: 1234 files,
        // 3337 links); axum's 2 MB default refused it
        .layer(axum::extract::DefaultBodyLimit::max(128 * 1024 * 1024))
}

fn bad(msg: impl Into<String>) -> ApiError {
    ApiError::Status(StatusCode::BAD_REQUEST, msg.into())
}

fn not_found(msg: impl Into<String>) -> ApiError {
    ApiError::Status(StatusCode::NOT_FOUND, msg.into())
}

fn backend(e: impl std::fmt::Display) -> ApiError {
    ApiError::Status(StatusCode::INTERNAL_SERVER_ERROR, e.to_string())
}

fn s<'a>(v: &'a Value, k: &str) -> &'a str {
    v.get(k).and_then(|x| x.as_str()).unwrap_or("")
}

fn node_key(project: &str, id: &str) -> String {
    arango::doc_key(project, id)
}

fn link_sk(e: &Value) -> String {
    format!("{}|{}|{}", s(e, "from"), s(e, "kind"), s(e, "to"))
}

/// Strip storage attributes so every backend answers the same shape.
fn clean(mut v: Value) -> Value {
    if let Some(o) = v.as_object_mut() {
        for k in ["_key", "_id", "_rev", "_from", "_to"] {
            o.remove(k);
        }
    }
    v
}

// ---------- storage: arango-native or row fallback ----------

async fn load(store: &dyn Storage, project: &str) -> Result<(Vec<Value>, Vec<Value>), ApiError> {
    let a = arango(store)?;
    let v = a
        .aql("FOR n IN node FILTER n.project == @p SORT n.path RETURN n", json!({"p": project}))
        .await
        .map_err(backend)?;
    let e = a
        .aql("FOR l IN link FILTER l.project == @p SORT l.from, l.kind, l.to RETURN l", json!({"p": project}))
        .await
        .map_err(backend)?;
    Ok((v.into_iter().map(clean).collect(), e.into_iter().map(clean).collect()))
}

#[derive(Default, serde::Serialize)]
struct ReplaceCounts {
    added: usize,
    changed: usize,
    unchanged: usize,
    removed: usize,
    edges: usize,
    edges_dropped: usize,
}

/// Replace a project's map. Vertices upsert by id (a vertex's `test` block
/// survives: the snapshot never carries it); vanished vertices and every old
/// edge go; the new edges go in. One stream transaction on ArangoDB.
async fn replace(store: &dyn Storage, project: &str, vertices: &[Value], edges: &[Value]) -> Result<ReplaceCounts, ApiError> {
    let (old_v, _) = load(store, project).await?;
    let old: HashMap<String, &Value> = old_v.iter().map(|v| (s(v, "id").to_string(), v)).collect();
    let new_ids: HashSet<&str> = vertices.iter().map(|v| s(v, "id")).collect();
    let mut c = ReplaceCounts::default();
    for v in vertices {
        match old.get(s(v, "id")) {
            None => c.added += 1,
            Some(o) if s(o, "vhash") != s(v, "vhash") || s(o, "vhash").is_empty() => c.changed += 1,
            Some(_) => c.unchanged += 1,
        }
    }
    let removed: Vec<String> = old.keys().filter(|id| !new_ids.contains(id.as_str())).cloned().collect();
    c.removed = removed.len();
    // an edge whose end is not a vertex of this snapshot cannot be stored
    let kept: Vec<&Value> = edges
        .iter()
        .filter(|e| new_ids.contains(s(e, "from")) && new_ids.contains(s(e, "to")))
        .collect();
    c.edges = kept.len();
    c.edges_dropped = edges.len() - kept.len();
    let now = now_utc();

    {
        let a = arango(store)?;
        let docs: Vec<Value> = vertices
            .iter()
            .map(|v| {
                let mut d = v.clone();
                d["_key"] = json!(node_key(project, s(v, "id")));
                d["project"] = json!(project);
                d["updated"] = json!(now);
                d.as_object_mut().map(|o| o.remove("test"));
                d
            })
            .collect();
        let links: Vec<Value> = kept
            .iter()
            .map(|e| {
                let mut d = (*e).clone();
                d["_key"] = json!(arango::doc_key(project, &link_sk(e)));
                d["_from"] = json!(format!("{NODE_COLL}/{}", node_key(project, s(e, "from"))));
                d["_to"] = json!(format!("{NODE_COLL}/{}", node_key(project, s(e, "to"))));
                d["project"] = json!(project);
                d
            })
            .collect();
        let gone: Vec<String> = removed.iter().map(|id| node_key(project, id)).collect();
        let trx = a.begin(&[NODE_COLL, LINK_COLL]).await.map_err(backend)?;
        let run = async {
            a.aql_in(&trx, "FOR l IN link FILTER l.project == @p REMOVE l IN link", json!({"p": project})).await?;
            a.aql_in(&trx, "FOR k IN @ks REMOVE {_key: k} IN node OPTIONS {ignoreErrors: true}", json!({"ks": gone})).await?;
            a.aql_in(
                &trx,
                "FOR d IN @ds UPSERT {_key: d._key} INSERT d REPLACE MERGE(d, {test: OLD.test}) IN node",
                json!({"ds": docs}),
            )
            .await?;
            a.aql_in(&trx, "FOR d IN @ds INSERT d IN link OPTIONS {overwriteMode: 'replace'}", json!({"ds": links})).await?;
            Ok::<(), arango::ArangoErr>(())
        };
        match run.await {
            Ok(()) => a.commit(&trx).await.map_err(backend)?,
            Err(e) => {
                a.abort(&trx).await;
                return Err(backend(e));
            }
        }
        return Ok(c);
    }

}

/// replace() for tests in sibling modules
#[cfg(test)]
pub async fn replace_pub(store: &dyn Storage, project: &str, v: &[Value], e: &[Value]) -> Result<(), String> {
    replace(store, project, v, e).await.map(|_| ()).map_err(|e| match e {
        ApiError::Status(c, m) => format!("{c}: {m}"),
        _ => "conflict or refusal".into(),
    })
}

async fn get_node(store: &dyn Storage, project: &str, id: &str) -> Result<Option<Value>, ApiError> {
    let r = arango(store)?
        .aql("RETURN DOCUMENT(CONCAT('node/', @k))", json!({"k": node_key(project, id)}))
        .await
        .map_err(backend)?;
    Ok(r.into_iter().next().filter(|v| !v.is_null()).map(clean))
}

/// Vertices reachable from `id` within `depth` hops, each with the edge that
/// reached it first (breadth-first) and its distance.
async fn neighbors(store: &dyn Storage, project: &str, id: &str, depth: u32, dir: &str) -> Result<Vec<Value>, ApiError> {
    {
        let a = arango(store)?;
        let d = match dir {
            "in" => "INBOUND",
            "any" => "ANY",
            _ => "OUTBOUND",
        };
        let q = format!(
            "FOR v, e, p IN 1..@depth {d} @start GRAPH '{GRAPH_NAME}'
               OPTIONS {{order: 'bfs', uniqueVertices: 'global'}}
               RETURN {{vertex: UNSET(v, '_key', '_id', '_rev'), via: e.kind, from: e.from, to: e.to, depth: LENGTH(p.edges)}}"
        );
        let start = format!("{NODE_COLL}/{}", node_key(project, id));
        a.aql(&q, json!({"depth": depth, "start": start})).await.map_err(backend)
    }
}

// ---------- handlers ----------

async fn graph_get(_u: AuthUser, State(st): Ctx, Path(name): Path<String>) -> Result<Json<Value>, ApiError> {
    let (vertices, edges) = load(st.store.as_ref(), &name).await?;
    let meta = st.store.get(META, &name, NOSK).await?.unwrap_or(Value::Null);
    Ok(Json(json!({"project": name, "meta": meta, "vertices": vertices, "edges": edges})))
}

/// The engine's sync. Body: {hash, vertices, edges, orphans?, unresolved?}.
/// An unchanged hash is answered without touching the map.
async fn graph_put(user: AuthUser, State(st): Ctx, Path(name): Path<String>, Json(body): Json<Value>) -> Result<Json<Value>, ApiError> {
    user.require_writer()?;
    let mut vertices = body.get("vertices").and_then(|v| v.as_array()).cloned().unwrap_or_default();
    let edges = body.get("edges").and_then(|v| v.as_array()).cloned().unwrap_or_default();
    let hash = s(&body, "hash").to_string();
    let mut ids = HashSet::new();
    for v in &vertices {
        let id = s(v, "id");
        if id.is_empty() {
            return Err(bad(format!("vertex without an id: {}", s(v, "path"))));
        }
        if !ids.insert(id.to_string()) {
            return Err(bad(format!("duplicate vertex id {id} (run `iter ids --fix`)")));
        }
    }
    let prev = st.store.get(META, &name, NOSK).await?.unwrap_or(Value::Null);
    if !hash.is_empty() && s(&prev, "hash") == hash && body.get("force").and_then(|f| f.as_bool()) != Some(true) {
        return Ok(Json(json!({"unchanged": true, "hash": hash})));
    }
    // use cases are drawn from tags: every node on a use case's ownership
    // chains carries its id (iter_local/src/usecase_map.rs)
    let ucmap = iter_local::usecase_map::usecase_map(&mut vertices, &edges);
    let counts = replace(st.store.as_ref(), &name, &vertices, &edges).await?;
    let meta = json!({
        "projectname": name,
        "hash": hash,
        "updated": now_utc(),
        "by": user.sub,
        "vertices": vertices.len(),
        "edges": counts.edges,
        "orphans": body.get("orphans").cloned().unwrap_or(json!([])),
        "unresolved": body.get("unresolved").cloned().unwrap_or(json!([])),
        "actors": body.get("actors").cloned().unwrap_or(json!([])),
        "repo": body.get("repo").cloned().unwrap_or(Value::Null),
        "actors_file": body.get("actors_file").cloned().unwrap_or(Value::Null),
    });
    st.store.put(META, &name, NOSK, &meta).await?;
    st.store.bump_seq(&name, "graph").await?;
    Ok(Json(json!({"unchanged": false, "hash": hash, "counts": counts, "usecase_map": ucmap})))
}

/// A graph edit from the webui (spec R14): iter_data has no checkout, so a
/// file-changing edit is accepted as a pending datasync row that the first
/// engine serving the project applies, commits, pushes and re-syncs (usually
/// within one heartbeat); `run_tests` is a deterministic `test` work item.
async fn graph_edit(user: AuthUser, State(st): Ctx, Path(name): Path<String>, Json(op): Json<Value>) -> Result<Json<Value>, ApiError> {
    user.require_writer()?;
    if st.store.get("project", &name, NOSK).await?.is_none() {
        return Err(not_found(format!("no project {name}")));
    }
    let kind = s(&op, "op").to_string();
    if kind == "run_tests" {
        // a test run is work: a deterministic `test` item on the queue
        let group = s(&op, "group").to_string();
        if group.is_empty() || group.contains(['"', '`', '$', '\\', '\n']) {
            return Err(bad("run_tests needs a plain testgroup label (group)"));
        }
        let item = json!({"name": format!("Run tests: {group}"), "agent": "test", "state": "queued", "priority": 5,
               "exec_shell": format!("iter runtests --project \"$ITER_PROJECT\" --group \"{group}\""),
               "lockdirs": [], "blockedby": [], "context": [], "tags": [{"text": "tests", "color": ""}],
               "requestedby": user.sub, "prework": [], "postwork": [],
               "request": format!("Run testgroup \"{group}\" once (queued from the Project graph).")});
        return crate::api::workitem_create(user, State(st), Path(name), Json(item)).await;
    }
    const OPS: &[&str] = &["new_node", "connect", "new_global", "edit_body", "link_child", "unlink_child", "disconnect", "define_tests",
        "new_actor", "edit_actor", "actor_uses", "actor_unuse", "usecase_needs", "usecase_unneed", "name_parts"];
    if !OPS.contains(&kind.as_str()) {
        return Err(bad(format!("unknown op {kind:?} ({} | run_tests)", OPS.join(" | "))));
    }
    let mut op = op;
    if ["new_actor", "edit_actor", "actor_uses", "actor_unuse"].contains(&kind.as_str()) && s(&op, "actors_file").is_empty() {
        // the checkout's actors file, as the last sync reported it
        let meta = st.store.get(META, &name, NOSK).await?.unwrap_or(Value::Null);
        if !s(&meta, "actors_file").is_empty() {
            op["actors_file"] = json!(s(&meta, "actors_file"));
        }
    }
    if ["unlink_child", "disconnect", "actor_unuse", "usecase_unneed"].contains(&kind.as_str()) && s(&op, "reason").trim().is_empty() {
        return Err(bad("removing an edge needs a reason (it is recorded in the commit)"));
    }
    // everything else changes files: accepted now as a pending datasync row,
    // applied by the first engine serving the project (datasync.rs)
    let row = crate::datasync::create(st.store.as_ref(), &name, &op, &user.sub).await?;
    Ok(Json(row))
}

/// The map in the Project graph tab's shape (see graph_view.rs).
async fn graph_view(_u: AuthUser, State(st): Ctx, Path(name): Path<String>) -> Result<Json<Value>, ApiError> {
    let (vertices, edges) = load(st.store.as_ref(), &name).await?;
    let meta = st.store.get(META, &name, NOSK).await?.unwrap_or(Value::Null);
    let settings = st.store.get("project", &name, NOSK).await?.and_then(|p| p.get("graph").cloned()).unwrap_or(Value::Null);
    Ok(Json(crate::graph_view::build(&name, &vertices, &edges, &meta, &settings)))
}

async fn graph_stats(_u: AuthUser, State(st): Ctx, Path(name): Path<String>) -> Result<Json<Value>, ApiError> {
    let (vs, es) = load(st.store.as_ref(), &name).await?;
    let mut by_type: HashMap<String, usize> = HashMap::new();
    for v in &vs {
        *by_type.entry(s(v, "nodetype").to_string()).or_default() += 1;
    }
    let mut by_kind: HashMap<String, usize> = HashMap::new();
    for e in &es {
        *by_kind.entry(s(e, "kind").to_string()).or_default() += 1;
    }
    // reachability from the main node, through the store's own traversal
    let main_id = vs.iter().find(|v| s(v, "nodetype") == "main").map(|v| s(v, "id").to_string());
    let (reachable, unreachable) = match &main_id {
        Some(id) => {
            let hit: HashSet<String> = neighbors(st.store.as_ref(), &name, id, 64, "out")
                .await?
                .iter()
                .map(|r| s(&r["vertex"], "id").to_string())
                .collect();
            let un: Vec<Value> = vs
                .iter()
                .filter(|v| s(v, "id") != id && !hit.contains(s(v, "id")))
                .map(|v| json!({"id": s(v, "id"), "nodetype": s(v, "nodetype"), "path": s(v, "path"), "orphan": v.get("orphan")}))
                .collect();
            (hit.len() + 1, un)
        }
        None => (0, vec![]),
    };
    let red: Vec<Value> = vs
        .iter()
        .filter(|v| v.pointer("/test/result").and_then(|r| r.as_str()) == Some("red"))
        .map(|v| json!({"id": s(v, "id"), "name": s(v, "name"), "path": s(v, "path")}))
        .collect();
    let meta = st.store.get(META, &name, NOSK).await?.unwrap_or(Value::Null);
    Ok(Json(json!({
        "project": name,
        "backend": st.store.backend_name(),
        "vertices": vs.len(),
        "edges": es.len(),
        "by_nodetype": by_type,
        "by_kind": by_kind,
        "main": main_id,
        "reachable_from_main": reachable,
        "unreachable": unreachable,
        "red_testgroups": red,
        "hash": meta.get("hash"),
        "updated": meta.get("updated"),
        "datasync_pending": crate::datasync::waiting(st.store.as_ref(), &[name.clone()]).await?.get(&name).copied().unwrap_or(0),
    })))
}

#[derive(serde::Deserialize)]
struct LookupQ {
    #[serde(default)]
    path: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    nodetype: String,
}

/// Find the vertex for a node file that lost (or never had) its id line:
/// by path first, then by nodetype + name. Answers every match; the caller
/// reuses an id only when exactly one matched.
async fn graph_lookup(_u: AuthUser, State(st): Ctx, Path(name): Path<String>, Query(q): Query<LookupQ>) -> Result<Json<Value>, ApiError> {
    if q.path.is_empty() && q.name.is_empty() {
        return Err(bad("lookup needs path= or name= (+ nodetype=)"));
    }
    let (vs, _) = load(st.store.as_ref(), &name).await?;
    let mut hits: Vec<&Value> = if q.path.is_empty() { vec![] } else { vs.iter().filter(|v| s(v, "path") == q.path).collect() };
    let mut by = "path";
    if hits.is_empty() && !q.name.is_empty() {
        by = "name";
        hits = vs
            .iter()
            .filter(|v| s(v, "name") == q.name && (q.nodetype.is_empty() || s(v, "nodetype") == q.nodetype))
            .collect();
    }
    Ok(Json(json!({"matched_by": if hits.is_empty() { "" } else { by }, "matches": hits})))
}

/// One use case's picture by tag (iter_local/src/usecase_map.rs): the
/// use-case vertex plus every node tagged with its id — one lookup on the
/// `node.usecases[*]` array index on ArangoDB, no traversal. `tops` are the
/// nodes the use case links to; the rest hang under them by ownership.
async fn graph_usecase(_u: AuthUser, State(st): Ctx, Path((name, ucid)): Path<(String, String)>) -> Result<Json<Value>, ApiError> {
    let ucid = if ucid.starts_with("usecase:") { ucid } else { format!("usecase:{ucid}") };
    let a = arango(st.store.as_ref())?;
    let uc = a
        .aql("FOR n IN node FILTER n.project == @p AND n.nodetype == 'usecase' AND n.ucid == @u LIMIT 1 RETURN n", json!({"p": name, "u": ucid}))
        .await
        .map_err(backend)?
        .into_iter()
        .next()
        .map(clean);
    let nodes: Vec<Value> = a
        .aql("FOR n IN node FILTER n.project == @p AND @u IN n.usecases[*] SORT n.key RETURN n", json!({"p": name, "u": ucid}))
        .await
        .map_err(backend)?
        .into_iter()
        .map(clean)
        .collect();
    let Some(uc) = uc else {
        return Err(not_found(format!("no use case {ucid} in {name}")));
    };
    let tops = uc.get("uc_tops").cloned().unwrap_or(json!([]));
    Ok(Json(json!({"usecase": uc, "tops": tops, "nodes": nodes})))
}

#[derive(serde::Deserialize)]
struct OwnerQ {
    path: String,
}

/// Code nodes whose codedirs contain `path` (a `{topdir}/…` path), deepest first.
async fn graph_owner(_u: AuthUser, State(st): Ctx, Path(name): Path<String>, Query(q): Query<OwnerQ>) -> Result<Json<Value>, ApiError> {
    let (vs, _) = load(st.store.as_ref(), &name).await?;
    Ok(Json(json!({"owners": owners(&vs, &q.path)})))
}

pub fn owners(vs: &[Value], path: &str) -> Vec<Value> {
    let mut hits: Vec<(usize, &Value)> = vs
        .iter()
        .filter(|v| s(v, "nodetype") == "code")
        .filter_map(|v| {
            let best = v
                .get("codedirs")
                .and_then(|c| c.as_array())
                .into_iter()
                .flatten()
                .filter_map(|d| d.as_str())
                .filter(|d| {
                    let d = d.trim_end_matches('/');
                    path == d || path.starts_with(&format!("{d}/"))
                })
                .map(|d| d.trim_end_matches('/').len())
                .max()?;
            Some((best, v))
        })
        .collect();
    hits.sort_by(|a, b| b.0.cmp(&a.0));
    hits.into_iter().map(|(_, v)| v.clone()).collect()
}

async fn node_get(_u: AuthUser, State(st): Ctx, Path((name, id)): Path<(String, String)>) -> Result<Json<Value>, ApiError> {
    match get_node(st.store.as_ref(), &name, &id).await? {
        Some(v) => Ok(Json(v)),
        None => Err(not_found(format!("no vertex {id} in {name}"))),
    }
}

#[derive(serde::Deserialize)]
struct NeighborsQ {
    #[serde(default = "one")]
    depth: u32,
    #[serde(default)]
    direction: String,
}

fn one() -> u32 {
    1
}

async fn node_neighbors(
    _u: AuthUser,
    State(st): Ctx,
    Path((name, id)): Path<(String, String)>,
    Query(q): Query<NeighborsQ>,
) -> Result<Json<Value>, ApiError> {
    if get_node(st.store.as_ref(), &name, &id).await?.is_none() {
        return Err(not_found(format!("no vertex {id} in {name}")));
    }
    let dir = if q.direction.is_empty() { "out" } else { q.direction.as_str() };
    if !["out", "in", "any"].contains(&dir) {
        return Err(bad("direction must be out | in | any"));
    }
    let r = neighbors(st.store.as_ref(), &name, &id, q.depth.clamp(1, 64), dir).await?;
    Ok(Json(json!({"start": id, "direction": dir, "depth": q.depth, "neighbors": r})))
}

/// A test sweep's verdict for one testgroup vertex: {result, counts, lastrun, failing?, workid?}.
async fn node_testresult(
    user: AuthUser,
    State(st): Ctx,
    Path((name, id)): Path<(String, String)>,
    Json(mut body): Json<Value>,
) -> Result<Json<Value>, ApiError> {
    user.require_writer()?;
    let Some(_) = get_node(st.store.as_ref(), &name, &id).await? else {
        return Err(not_found(format!("no vertex {id} in {name}")));
    };
    if !["green", "red", "error", "skipped"].contains(&s(&body, "result")) {
        return Err(bad("result must be green | red | error | skipped"));
    }
    if s(&body, "lastrun").is_empty() {
        body["lastrun"] = json!(now_utc());
    }
    body["by"] = json!(user.sub);
    arango(st.store.as_ref())?
        .aql(
            "UPDATE {_key: @k} WITH {test: @t} IN node OPTIONS {mergeObjects: false}",
            json!({"k": node_key(&name, &id), "t": body}),
        )
        .await
        .map_err(backend)?;
    st.store.bump_seq(&name, "graph").await?;
    Ok(Json(json!({"id": id, "test": body})))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(id: &str, t: &str, codedirs: &[&str]) -> Value {
        json!({"id": id, "nodetype": t, "name": id, "path": format!("{{topdir}}/{id}.{t}.iter.md"), "codedirs": codedirs, "vhash": id})
    }
    #[test]
    fn owner_is_deepest_codedir_first() {
        let vs = vec![
            v("ctx", "code", &["{topdir}/"]),
            v("data", "code", &["{topdir}/iter_data/"]),
            v("api", "code", &["{topdir}/iter_data/src/api/"]),
        ];
        let o = owners(&vs, "{topdir}/iter_data/src/api/x.rs");
        let ids: Vec<&str> = o.iter().map(|x| s(x, "id")).collect();
        assert_eq!(ids, ["api", "data", "ctx"]);
        // a sibling with a shared prefix is not an owner
        assert_eq!(owners(&vs, "{topdir}/iter_data_old/x").len(), 1);
    }

}
