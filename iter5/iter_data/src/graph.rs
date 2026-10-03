//! The project graph API (iter5 spec §6.2): nodes = files.
//!
//! Reads answer from the stored nodes and derived edges (nodes.rs); every
//! edit changes the node data directly — conformed with the same library the
//! engine uses, `node_version + 1`, `file_state = pending_write` (or
//! `designed` before the project has a repo), edges recomputed — and is
//! visible at once. An engine serving the project later writes the files
//! (filesync.rs: `files/pending` → write + commit → `files/ack`).

use crate::api::{ApiError, AppState, AuthUser};
use crate::nodes::{self, Graph, StoredNode, bad, not_found};
use axum::body::Bytes;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use iter_core::nodefile::{self as nf, EdgeKind, NodeDoc, NodeType};
use serde_json::{Value, json};
use std::collections::{BTreeMap, HashSet, VecDeque};
use std::sync::Arc;

type Ctx = State<Arc<AppState>>;

pub fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/api/projects/{name}/graph", get(graph_get))
        .route("/api/projects/{name}/graph/stats", get(graph_stats))
        .route("/api/projects/{name}/graph/view", get(graph_view))
        .route("/api/projects/{name}/graph/lookup", get(graph_lookup))
        .route("/api/projects/{name}/graph/owner", get(graph_owner))
        .route("/api/projects/{name}/graph/conflicts", get(graph_conflicts))
        .route("/api/projects/{name}/graph/usecases/{ucid}", get(graph_usecase))
        .route("/api/projects/{name}/graph/nodes", post(node_create))
        .route("/api/projects/{name}/graph/nodes/{id}", get(node_get).patch(node_patch).delete(node_delete))
        .route("/api/projects/{name}/graph/nodes/{id}/neighbors", get(node_neighbors))
        .route("/api/projects/{name}/graph/nodes/{id}/move", post(node_move))
        .route("/api/projects/{name}/graph/edges", post(edge_add).delete(edge_remove))
        .route("/api/projects/{name}/graph/edges/move", post(edge_move))
        .merge(crate::testlogs::routes())
        .merge(crate::reqs::routes())
        .merge(crate::filesync::routes())
        // a large repository's full file sync is megabytes
        .layer(axum::extract::DefaultBodyLimit::max(128 * 1024 * 1024))
}

fn s<'a>(v: &'a Value, k: &str) -> &'a str {
    v.get(k).and_then(|x| x.as_str()).unwrap_or("")
}

pub(crate) fn edge_kind(k: &str) -> Result<EdgeKind, ApiError> {
    if k.trim() == crate::reqs::CONTAINS {
        return Err(bad("contains edges are derived from a requirement file's sections: use graph/reqs (create, delete, move) or edges/move with new_from"));
    }
    EdgeKind::from_name(k.trim()).ok_or_else(|| {
        bad(format!("unknown edge kind {k:?} ({})", EdgeKind::ALL.iter().map(|k| k.as_str()).collect::<Vec<_>>().join(" | ")))
    })
}

/// A JSON body that may be absent (DELETE): `{}` then.
fn body_json(b: &Bytes) -> Result<Value, ApiError> {
    if b.iter().all(|c| c.is_ascii_whitespace()) {
        return Ok(json!({}));
    }
    serde_json::from_slice(b).map_err(|e| bad(format!("body is not JSON: {e}")))
}

async fn project_exists(st: &AppState, project: &str) -> Result<(), ApiError> {
    if st.store.get("project", project, crate::api::NOSK).await?.is_none() {
        return Err(not_found(format!("no project {project}")));
    }
    Ok(())
}

/// Load the graph, creating a designed project's project node on first read.
pub(crate) async fn load_ensured(st: &AppState, project: &str) -> Result<Graph, ApiError> {
    let g = Graph::load(st.store.as_ref(), project).await?;
    if g.project_node().is_none() && !g.served && st.store.get("project", project, crate::api::NOSK).await?.is_some() {
        nodes::ensure_project_node(st.store.as_ref(), project).await?;
        return Graph::load(st.store.as_ref(), project).await;
    }
    Ok(g)
}

fn edges_json(g: &Graph) -> Vec<Value> {
    g.all_edges().into_iter().map(|(f, k, t)| json!({"from": f, "kind": k, "to": t})).collect()
}

fn stats_of(g: &Graph) -> Value {
    let mut by_type: BTreeMap<String, usize> = BTreeMap::new();
    for n in g.live() {
        *by_type.entry(nodes::type_label(&n.doc)).or_default() += 1;
    }
    if !g.reqs.is_empty() {
        by_type.insert(crate::reqs::REQ.to_string(), g.reqs.len());
    }
    let edges = g.all_edges();
    let mut by_kind: BTreeMap<&str, usize> = BTreeMap::new();
    for (_, k, _) in &edges {
        *by_kind.entry(k.as_str()).or_default() += 1;
    }
    let pnode = g.project_node().map(|p| p.doc.id.clone());
    let mut reach: HashSet<String> = HashSet::new();
    if let Some(p) = &pnode {
        let mut q = VecDeque::from([p.clone()]);
        reach.insert(p.clone());
        while let Some(x) = q.pop_front() {
            for (_, _, to) in edges.iter().filter(|e| e.0 == x) {
                if reach.insert(to.clone()) {
                    q.push_back(to.clone());
                }
            }
        }
    }
    let unreachable: Vec<Value> = g
        .live()
        .filter(|n| pnode.is_some() && !reach.contains(n.id()))
        .map(|n| json!({"id": n.id(), "nodetype": n.doc.nodetype, "name": n.doc.name, "path": n.doc.path}))
        .collect();
    let red: Vec<Value> = g
        .live()
        .filter(|n| n.test.get("overall_success").and_then(|b| b.as_bool()) == Some(false))
        .map(|n| json!({"id": n.id(), "name": n.doc.name, "path": n.doc.path}))
        .collect();
    let pending = g.live().filter(|n| n.file_state == nodes::PENDING_WRITE).count()
        + g.nodes.iter().filter(|n| !n.deleted && n.file_state == nodes::PENDING_DELETE).count();
    json!({
        "nodes": g.live().count() + g.reqs.len(), "files": g.live().count(), "reqs": g.reqs.len(), "edges": edges.len(), "by_nodetype": by_type, "by_kind": by_kind,
        "file_states": nodes::state_counts(g), "pending_files": pending,
        "designed": g.live().filter(|n| n.file_state == nodes::DESIGNED).count(),
        "served": g.served, "project_node": pnode,
        "reachable_from_project": if pnode.is_some() { reach.len() } else { 0 }, "unreachable": unreachable, "red_tests": red,
    })
}

// ---------- reads ----------

async fn graph_get(_u: AuthUser, State(st): Ctx, Path(name): Path<String>) -> Result<Json<Value>, ApiError> {
    let g = load_ensured(&st, &name).await?;
    let mut nodes: Vec<Value> = g.live().map(StoredNode::json).collect();
    nodes.extend(g.reqs.iter().map(crate::reqs::ReqNode::json));
    Ok(Json(json!({"project": name, "nodes": nodes, "edges": edges_json(&g), "stats": stats_of(&g)})))
}

async fn graph_stats(_u: AuthUser, State(st): Ctx, Path(name): Path<String>) -> Result<Json<Value>, ApiError> {
    let g = Graph::load(st.store.as_ref(), &name).await?;
    let mut v = stats_of(&g);
    v["project"] = json!(name);
    v["backend"] = json!(st.store.backend_name());
    Ok(Json(v))
}

async fn graph_view(_u: AuthUser, State(st): Ctx, Path(name): Path<String>) -> Result<Json<Value>, ApiError> {
    let g = load_ensured(&st, &name).await?;
    let rec = st.store.get("project", &name, crate::api::NOSK).await?.unwrap_or(Value::Null);
    Ok(Json(crate::graph_view::build(&g, &rec)))
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

/// A node by path first, then by name (+ nodetype). Answers every match.
async fn graph_lookup(_u: AuthUser, State(st): Ctx, Path(name): Path<String>, Query(q): Query<LookupQ>) -> Result<Json<Value>, ApiError> {
    if q.path.is_empty() && q.name.is_empty() {
        return Err(bad("lookup needs path= or name= (+ nodetype=)"));
    }
    let g = Graph::load(st.store.as_ref(), &name).await?;
    let mut by = "path";
    let mut hits: Vec<Value> = g.live().filter(|n| !q.path.is_empty() && n.doc.path == q.path).map(StoredNode::json).collect();
    hits.extend(g.reqs.iter().filter(|r| !q.path.is_empty() && r.path == q.path).map(crate::reqs::ReqNode::json));
    if hits.is_empty() && !q.name.is_empty() {
        by = "name";
        hits = g
            .live()
            .filter(|n| n.doc.name == q.name && (q.nodetype.is_empty() || n.doc.nodetype.as_str() == q.nodetype || nodes::type_label(&n.doc) == q.nodetype))
            .map(StoredNode::json)
            .collect();
        if q.nodetype.is_empty() || q.nodetype == crate::reqs::REQ {
            hits.extend(g.reqs.iter().filter(|r| r.name == q.name || (!r.key.is_empty() && r.key == q.name)).map(crate::reqs::ReqNode::json));
        }
    }
    Ok(Json(json!({"matched_by": if hits.is_empty() { "" } else { by }, "matches": hits})))
}

#[derive(serde::Deserialize)]
struct OwnerQ {
    path: String,
}

/// Code nodes whose codedirs contain `path` (`{topdir}/…`), deepest first.
async fn graph_owner(_u: AuthUser, State(st): Ctx, Path(name): Path<String>, Query(q): Query<OwnerQ>) -> Result<Json<Value>, ApiError> {
    let g = Graph::load(st.store.as_ref(), &name).await?;
    let list: Vec<&StoredNode> = g.live().collect();
    Ok(Json(json!({"owners": owners(&list, &q.path).into_iter().map(StoredNode::json).collect::<Vec<_>>()})))
}

/// The directory a codedirs entry covers (`{thisfiledir}/**` → the file's dir).
fn codedir_prefix(entry: &str, this: &str) -> String {
    let e = nf::expand_entry(entry, this, None);
    let cut = e.find(['*', '?', '[']).map(|i| &e[..i]).unwrap_or(&e);
    cut.trim_end_matches('/').to_string()
}

pub fn owners<'a>(vs: &[&'a StoredNode], path: &str) -> Vec<&'a StoredNode> {
    let mut hits: Vec<(usize, &StoredNode)> = vs
        .iter()
        .filter(|v| v.doc.nodetype == NodeType::Code)
        .filter_map(|v| {
            let best = v
                .doc
                .children
                .codedirs
                .iter()
                .map(|d| codedir_prefix(d, &v.doc.path))
                .filter(|d| path == d || path.starts_with(&format!("{d}/")))
                .map(|d| d.len())
                .max()?;
            Some((best, *v))
        })
        .collect();
    hits.sort_by(|a, b| b.0.cmp(&a.0));
    hits.into_iter().map(|(_, v)| v).collect()
}

async fn graph_conflicts(_u: AuthUser, State(st): Ctx, Path(name): Path<String>) -> Result<Json<Value>, ApiError> {
    let rows = nodes::arango(st.store.as_ref())?
        .aql("FOR c IN node_conflict FILTER c.project == @p SORT c.at DESC LIMIT 500 RETURN UNSET(c, '_id', '_rev', '_key')", json!({"p": name}))
        .await
        .map_err(nodes::backend)?;
    Ok(Json(json!({"conflicts": rows})))
}

/// One use case's parts: the code nodes it uses (`tops`) and everything they
/// own through codenodes edges.
async fn graph_usecase(_u: AuthUser, State(st): Ctx, Path((name, ucid)): Path<(String, String)>) -> Result<Json<Value>, ApiError> {
    let g = Graph::load(st.store.as_ref(), &name).await?;
    let want = ucid.strip_prefix("usecase:").unwrap_or(&ucid);
    let uc = g
        .live()
        .find(|n| n.doc.nodetype == NodeType::Usecase && (n.id() == want || n.doc.name == want || nf::stem_of(&n.doc.path) == want))
        .ok_or_else(|| not_found(format!("no use case {ucid} in {name}")))?;
    let edges = g.edges();
    let tops: Vec<String> = edges.iter().filter(|e| e.from == uc.id() && e.kind == EdgeKind::Uses).map(|e| e.to.clone()).collect();
    let mut seen: Vec<String> = Vec::new();
    let mut q: VecDeque<String> = tops.iter().cloned().collect();
    while let Some(x) = q.pop_front() {
        if seen.contains(&x) {
            continue;
        }
        seen.push(x.clone());
        for e in edges.iter().filter(|e| e.from == x && e.kind == EdgeKind::Codenodes) {
            q.push_back(e.to.clone());
        }
    }
    let nodes: Vec<Value> = seen.iter().filter_map(|id| g.get(id)).map(StoredNode::json).collect();
    let actors: Vec<String> = edges.iter().filter(|e| e.to == uc.id() && e.kind == EdgeKind::Drives).map(|e| e.from.clone()).collect();
    Ok(Json(json!({"usecase": uc.json(), "tops": tops, "actors": actors, "nodes": nodes})))
}

async fn node_get(_u: AuthUser, State(st): Ctx, Path((name, id)): Path<(String, String)>) -> Result<Json<Value>, ApiError> {
    let g = Graph::load(st.store.as_ref(), &name).await?;
    let edges = g.all_edges();
    let mut v = if let Some(r) = g.req(&id) {
        r.json()
    } else {
        let n = g.get(&id).filter(|n| !n.deleted).ok_or_else(|| not_found(format!("no node {id} in {name}")))?;
        let mut v = n.json();
        v["text"] = json!(nf::render(&n.doc));
        v
    };
    v["edges_out"] = json!(edges.iter().filter(|e| e.0 == id).map(|e| json!({"kind": e.1, "to": e.2})).collect::<Vec<_>>());
    v["edges_in"] = json!(edges.iter().filter(|e| e.2 == id).map(|e| json!({"kind": e.1, "from": e.0})).collect::<Vec<_>>());
    Ok(Json(v))
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

/// Nodes reachable within `depth` hops (breadth-first), each with the edge
/// that reached it first and its distance.
async fn node_neighbors(_u: AuthUser, State(st): Ctx, Path((name, id)): Path<(String, String)>, Query(q): Query<NeighborsQ>) -> Result<Json<Value>, ApiError> {
    let g = Graph::load(st.store.as_ref(), &name).await?;
    if g.req(&id).is_none() {
        g.live_node(&id)?;
    }
    let dir = if q.direction.is_empty() { "out" } else { q.direction.as_str() };
    if !["out", "in", "any"].contains(&dir) {
        return Err(bad("direction must be out | in | any"));
    }
    let depth = q.depth.clamp(1, 64);
    let edges = g.all_edges();
    let mut seen: HashSet<String> = HashSet::from([id.clone()]);
    let mut q2: VecDeque<(String, u32)> = VecDeque::from([(id.clone(), 0)]);
    let mut out = Vec::new();
    while let Some((x, d)) = q2.pop_front() {
        if d >= depth {
            continue;
        }
        for (from, kind, to) in &edges {
            let next = if (dir == "out" || dir == "any") && *from == x {
                to
            } else if (dir == "in" || dir == "any") && *to == x {
                from
            } else {
                continue;
            };
            if seen.insert(next.clone()) {
                if let Some(n) = g.node_json(next) {
                    out.push(json!({"vertex": n, "via": kind, "from": from, "to": to, "depth": d + 1}));
                }
                q2.push_back((next.clone(), d + 1));
            }
        }
    }
    Ok(Json(json!({"start": id, "direction": dir, "depth": depth, "neighbors": out})))
}

// ---------- node edits ----------

fn str_list(v: &Value, what: &str) -> Result<Vec<String>, ApiError> {
    match v {
        Value::Null => Ok(Vec::new()),
        Value::Array(a) => a
            .iter()
            .map(|x| x.as_str().map(|s| s.trim().to_string()).ok_or_else(|| bad(format!("{what}: entries are strings"))))
            .filter(|r| r.as_ref().map(|s| !s.is_empty()).unwrap_or(true))
            .collect(),
        _ => Err(bad(format!("{what} must be a list of paths"))),
    }
}

/// `children` from a request: the lists given replace the node's.
fn apply_children(doc: &mut NodeDoc, v: &Value) -> Result<(), ApiError> {
    let Some(o) = v.as_object() else {
        return if v.is_null() { Ok(()) } else { Err(bad("children must be an object of path lists")) };
    };
    for (k, list) in o {
        let l = str_list(list, &format!("children.{k}"))?;
        match k.as_str() {
            "codedirs" => doc.children.codedirs = l,
            "codenodes" => doc.children.codenodes = l,
            "tests" => doc.children.tests = l,
            "reqs" => doc.children.reqs = l,
            other => {
                if l.is_empty() && list.is_null() {
                    doc.children.extra.remove(other);
                } else {
                    doc.children.extra.insert(other.to_string(), l);
                }
            }
        }
    }
    Ok(())
}

const COMMON_KEYS: &[&str] = &["id", "name", "desc", "creator", "teststate", "level", "children", "timestamps", "nodetype", "path", "body"];

/// `front` from a request: keys set, `null` removes one.
fn apply_front(doc: &mut NodeDoc, v: &Value) -> Result<(), ApiError> {
    let Some(o) = v.as_object() else {
        return if v.is_null() { Ok(()) } else { Err(bad("front must be an object")) };
    };
    for (k, x) in o {
        if COMMON_KEYS.contains(&k.as_str()) {
            return Err(bad(format!("front.{k}: set {k} with its own field")));
        }
        if x.is_null() {
            doc.front.remove(k);
        } else {
            doc.front.insert(k.clone(), x.clone());
        }
    }
    Ok(())
}

fn check_level(t: NodeType, level: &str) -> Result<(), ApiError> {
    if t != NodeType::Code {
        return Err(bad("level is only set on code nodes"));
    }
    if !nf::LEVELS.contains(&level) {
        return Err(bad(format!("level must be {}", nf::LEVELS.join(" | "))));
    }
    Ok(())
}

/// The edge a new node hangs from its parent by, when the caller gave none.
fn infer_kind(parent: &NodeDoc, child: &NodeDoc) -> Option<EdgeKind> {
    use NodeType::*;
    match (parent.nodetype, child.nodetype) {
        (_, Test) if parent.nodetype != Test => Some(EdgeKind::Tests),
        (_, t) if t.is_req() => Some(EdgeKind::Reqs),
        (Usecase, Code) => Some(EdgeKind::Uses),
        (Actor, Code) => Some(EdgeKind::Touches),
        (Actor, Usecase) | (Usecase, Actor) => Some(EdgeKind::Drives),
        (Code, Code) if parent.is_connection() && !child.is_connection() => Some(EdgeKind::Connects),
        (Code, Code) if child.is_connection() && !parent.is_connection() => Some(EdgeKind::Supplies),
        (Code | Project, Code) => Some(EdgeKind::Codenodes),
        _ => None,
    }
}

/// The (from, to) of the edge that attaches `child` under `parent`.
fn attach_ends<'a>(parent: &'a NodeDoc, child: &'a NodeDoc, kind: EdgeKind) -> (&'a str, &'a str) {
    match kind {
        EdgeKind::Supplies if parent.is_connection() => (&child.id, &parent.id),
        EdgeKind::Drives if parent.nodetype == NodeType::Usecase => (&child.id, &parent.id),
        _ => (&parent.id, &child.id),
    }
}

async fn node_create(user: AuthUser, State(st): Ctx, Path(name): Path<String>, Json(req): Json<Value>) -> Result<Json<Value>, ApiError> {
    user.require_writer()?;
    project_exists(&st, &name).await?;
    let t = NodeType::from_tag(s(&req, "nodetype")).ok_or_else(|| {
        bad(format!("nodetype must be one of {}", NodeType::ALL.iter().filter(|t| nf::is_synced(**t)).map(|t| t.as_str()).collect::<Vec<_>>().join(" | ")))
    })?;
    if !nf::is_synced(t) {
        return Err(bad("agentmem files are written by the engine only"));
    }
    let nname = s(&req, "name").trim().to_string();
    if nname.is_empty() {
        return Err(bad("a node needs a name"));
    }
    let _g = nodes::lock(&name).await;
    let mut g = load_ensured(&st, &name).await?;
    if t == NodeType::Project && g.project_node().is_some() {
        return Err(nodes::status(StatusCode::CONFLICT, "the project has its project node already"));
    }
    let now = nf::now_ts();
    let mut doc = NodeDoc::new(t, &nname, &user.sub, &now);
    let level = s(&req, "level").trim();
    if !level.is_empty() {
        check_level(t, level)?;
        doc.level = Some(level.to_string());
    }
    doc.desc = s(&req, "desc").to_string();
    doc.body = s(&req, "body").to_string();
    if t == NodeType::Code && !doc.is_connection() {
        doc.children.codedirs = vec!["{thisfiledir}/**".into()];
    }
    apply_front(&mut doc, req.get("front").unwrap_or(&Value::Null))?;
    apply_children(&mut doc, req.get("children").unwrap_or(&Value::Null))?;
    // the parent: attach_to, else the project node for root contexts and reqs
    let attach_to = s(&req, "attach_to").trim().to_string();
    let parent: Option<NodeDoc> = if !attach_to.is_empty() {
        Some(g.live_node(&attach_to)?.doc.clone())
    } else if (t == NodeType::Code && doc.level.as_deref() == Some("context")) || t.is_req() {
        g.project_node().map(|p| p.doc.clone())
    } else {
        None
    };
    let kind = match (&parent, s(&req, "attach_kind").trim()) {
        (None, k) if !k.is_empty() => return Err(bad("attach_kind needs attach_to")),
        (None, _) => None,
        (Some(p), "") => Some(infer_kind(p, &doc).ok_or_else(|| {
            bad(format!("say how a {} hangs under a {} (attach_kind)", nodes::type_label(&doc), nodes::type_label(p)))
        })?),
        (Some(_), k) => Some(edge_kind(k)?),
    };
    let attach = kind.map(nf::Attach::Under).unwrap_or(nf::Attach::Root);
    doc.path = nf::plan_path(&doc, parent.as_ref(), attach, &g.taken_paths(), g.naming());
    // one bizreq / techreq file per attachment point (§2.8): plan_path names
    // the existing one — its requirements are added with graph/reqs
    if let Some(have) = g.nodes.iter().find(|n| !n.deleted && (n.doc.path == doc.path || n.old_path == doc.path)) {
        return Err(ApiError::Refused(
            StatusCode::CONFLICT,
            json!({"error": format!("{} exists already ({}): add requirements to it with POST graph/reqs", doc.path, have.doc.name),
                   "refused": "exists", "existing": have.doc.id, "path": doc.path}),
        ));
    }
    let doc = nodes::canonical(&doc, &now, &user.sub);
    let id = doc.id.clone();
    g.insert_new(doc, &user.sub, &format!("new {} {}", t.as_str(), nname));
    let mut touched = vec![id.clone()];
    if let (Some(p), Some(k)) = (&parent, kind) {
        let child = g.get(&id).unwrap().doc.clone();
        let (from, to) = attach_ends(p, &child, k);
        let (from, to) = (from.to_string(), to.to_string());
        if let Some(owner) = g.add_edge(&from, &to, k, &user.sub)? {
            if owner != id {
                touched.push(owner);
            }
        }
    }
    g.save(st.store.as_ref()).await?;
    let node = g.get(&id).unwrap().json();
    Ok(Json(json!({"node": node, "touched": touched})))
}

async fn node_patch(user: AuthUser, State(st): Ctx, Path((name, id)): Path<(String, String)>, Json(req): Json<Value>) -> Result<Json<Value>, ApiError> {
    user.require_writer()?;
    let _g = nodes::lock(&name).await;
    let mut g = Graph::load(st.store.as_ref(), &name).await?;
    let n = g.live_node(&id)?.clone();
    if let Some(ev) = req.get("expect_version").and_then(|v| v.as_u64()) {
        if ev != n.node_version {
            return Err(ApiError::Conflict(n.json()));
        }
    }
    let mut doc = n.doc.clone();
    for (k, field) in [("name", &mut doc.name), ("desc", &mut doc.desc), ("body", &mut doc.body)] {
        if let Some(v) = req.get(k).filter(|v| !v.is_null()) {
            *field = v.as_str().ok_or_else(|| bad(format!("{k} must be a string")))?.to_string();
        }
    }
    if doc.name.trim().is_empty() {
        return Err(bad("a node needs a name"));
    }
    if let Some(ts) = req.get("teststate").and_then(|v| v.as_str()) {
        if !nf::TESTSTATES.contains(&ts) {
            return Err(bad(format!("teststate must be {}", nf::TESTSTATES.join(" | "))));
        }
        doc.teststate = ts.to_string();
    }
    if let Some(l) = req.get("level").and_then(|v| v.as_str()) {
        check_level(doc.nodetype, l)?;
        doc.level = Some(l.to_string());
    }
    apply_front(&mut doc, req.get("front").unwrap_or(&Value::Null))?;
    apply_children(&mut doc, req.get("children").unwrap_or(&Value::Null))?;
    if doc != n.doc {
        let what: Vec<&str> = ["name", "desc", "body", "teststate", "level", "front", "children"].into_iter().filter(|k| req.get(*k).is_some()).collect();
        g.commit_edit(&id, doc, &user.sub, &format!("edit {} ({})", n.doc.name, what.join(", ")), true)?;
        g.save(st.store.as_ref()).await?;
    }
    Ok(Json(g.get(&id).unwrap().json()))
}

#[derive(serde::Deserialize, Default)]
struct ReasonQ {
    #[serde(default)]
    reason: String,
}

fn reason_of(body: &Value, q: &ReasonQ) -> Result<String, ApiError> {
    let r = if s(body, "reason").trim().is_empty() { q.reason.trim() } else { s(body, "reason").trim() };
    if r.is_empty() {
        return Err(bad("a removal needs a reason (it is recorded in the commit)"));
    }
    Ok(r.to_string())
}

async fn node_delete(user: AuthUser, State(st): Ctx, Path((name, id)): Path<(String, String)>, Query(q): Query<ReasonQ>, body: Bytes) -> Result<Json<Value>, ApiError> {
    user.require_writer()?;
    let body = body_json(&body)?;
    let reason = reason_of(&body, &q)?;
    let _g = nodes::lock(&name).await;
    let mut g = Graph::load(st.store.as_ref(), &name).await?;
    let (touched, kept) = g.delete_node(&id, &user.sub, &reason)?;
    let state = g.get(&id).map(|n| n.file_state.clone()).unwrap_or_else(|| "removed".into());
    g.save(st.store.as_ref()).await?;
    Ok(Json(json!({"id": id, "file_state": state, "touched": touched, "kept_by_glob": kept})))
}

async fn node_move(user: AuthUser, State(st): Ctx, Path((name, id)): Path<(String, String)>, Json(req): Json<Value>) -> Result<Json<Value>, ApiError> {
    user.require_writer()?;
    let _g = nodes::lock(&name).await;
    let mut g = Graph::load(st.store.as_ref(), &name).await?;
    let touched = g.move_node(&id, s(&req, "path"), &user.sub)?;
    g.save(st.store.as_ref()).await?;
    Ok(Json(json!({"node": g.get(&id).unwrap().json(), "touched": touched})))
}

// ---------- edge edits ----------

fn edge_req(v: &Value) -> Result<(String, String, EdgeKind), ApiError> {
    let (f, t) = (s(v, "from").trim().to_string(), s(v, "to").trim().to_string());
    if f.is_empty() || t.is_empty() {
        return Err(bad("an edge needs from and to (node ids) and kind"));
    }
    Ok((f, t, edge_kind(s(v, "kind"))?))
}

fn touched_nodes(g: &Graph, ids: &[String]) -> Vec<Value> {
    ids.iter().filter_map(|i| g.get(i)).map(StoredNode::json).collect()
}

async fn edge_add(user: AuthUser, State(st): Ctx, Path(name): Path<String>, Json(req): Json<Value>) -> Result<Json<Value>, ApiError> {
    user.require_writer()?;
    let (f, t, k) = edge_req(&req)?;
    let _g = nodes::lock(&name).await;
    let mut g = Graph::load(st.store.as_ref(), &name).await?;
    let touched: Vec<String> = g.add_edge(&f, &t, k, &user.sub)?.into_iter().collect();
    g.save(st.store.as_ref()).await?;
    Ok(Json(json!({"edge": {"from": f, "kind": k.as_str(), "to": t}, "touched": touched, "nodes": touched_nodes(&g, &touched)})))
}

async fn edge_remove(user: AuthUser, State(st): Ctx, Path(name): Path<String>, Query(q): Query<ReasonQ>, body: Bytes) -> Result<Json<Value>, ApiError> {
    user.require_writer()?;
    let req = body_json(&body)?;
    let (f, t, k) = edge_req(&req)?;
    let reason = reason_of(&req, &q)?;
    let _g = nodes::lock(&name).await;
    let mut g = Graph::load(st.store.as_ref(), &name).await?;
    let touched = g.remove_edge(&f, &t, k, &user.sub, &reason)?;
    g.save(st.store.as_ref()).await?;
    Ok(Json(json!({"removed": {"from": f, "kind": k.as_str(), "to": t}, "touched": touched, "nodes": touched_nodes(&g, &touched)})))
}

/// Drag an edge's endpoint: remove the old edge, add the new one — both or
/// neither.
async fn edge_move(user: AuthUser, State(st): Ctx, Path(name): Path<String>, Json(req): Json<Value>) -> Result<Json<Value>, ApiError> {
    user.require_writer()?;
    if s(&req, "kind").trim() == crate::reqs::CONTAINS {
        return crate::reqs::edge_move_contains(&user, &st, &name, &req).await.map(Json);
    }
    let (f, t, k) = edge_req(&req)?;
    let nf_ = if s(&req, "new_from").trim().is_empty() { f.clone() } else { s(&req, "new_from").trim().to_string() };
    let nt = if s(&req, "new_to").trim().is_empty() { t.clone() } else { s(&req, "new_to").trim().to_string() };
    if nf_ == f && nt == t {
        return Err(bad("edges/move needs new_from or new_to"));
    }
    let _g = nodes::lock(&name).await;
    let mut g = Graph::load(st.store.as_ref(), &name).await?;
    let mut touched = g.remove_edge(&f, &t, k, &user.sub, "moved to another node")?;
    if let Some(o) = g.add_edge(&nf_, &nt, k, &user.sub)? {
        touched.push(o);
    }
    touched.sort();
    touched.dedup();
    g.save(st.store.as_ref()).await?;
    Ok(Json(json!({"removed": {"from": f, "kind": k.as_str(), "to": t}, "edge": {"from": nf_, "kind": k.as_str(), "to": nt},
                    "touched": touched, "nodes": touched_nodes(&g, &touched)})))
}
