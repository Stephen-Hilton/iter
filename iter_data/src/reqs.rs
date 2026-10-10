//! Requirements inside requirement files (iter5 spec §2.8).
//!
//! A bizreq / techreq node file holds many requirements, one per `## `
//! section, each carrying a permanent id in its `<!-- req: id=… status=… -->`
//! marker. iter_data derives one `req` node per section (stored in `node`
//! with `nodetype: "req"`, never a file of its own, so never in
//! `files/pending`) and a `contains` edge file → req, rebuilt from the file
//! nodes on every load and save (nodes.rs).
//!
//! Every requirement write goes through its owning file: parse the file body
//! (`iter_core::nodefile::reqs`), change the section list, render it back,
//! `commit_edit` (conform, node_version + 1, pending_write or designed),
//! save (one transaction: the files, the re-derived req nodes, the edges).
//!
//! Ids are unique per project: when two files carry the same req id, the
//! first by path keeps it and the other file is rewritten with a fresh id
//! (`Graph::repair_req_ids`, run by every save).

use crate::api::{ApiError, AppState, AuthUser};
use crate::nodes::{self, Graph, StoredNode, bad, not_found};
use iter_core::nodefile::reqs as rl;
use axum::body::Bytes;
use axum::extract::{Path, Query, State};
use axum::routing::{patch, post};
use axum::{Json, Router};
use iter_core::nodefile::{self as nf, EdgeKind, NodeDoc, NodeType};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::HashSet;
use std::sync::Arc;

pub use rl::ReqItem;

type Ctx = State<Arc<AppState>>;

pub const REQ: &str = "req";
pub const CONTAINS: &str = "contains";

/// One derived requirement node (spec §2.8), as stored and served.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ReqNode {
    pub project: String,
    pub id: String,
    pub nodetype: String,
    /// `KEY — title` | `title`
    pub name: String,
    /// the first sentence of the text
    pub desc: String,
    pub key: String,
    pub title: String,
    pub status: String,
    pub text: String,
    /// position in its file (0 = first section)
    pub order: usize,
    /// the owning file node's id
    pub file: String,
    /// bizreq | techreq
    pub file_type: String,
    /// `<file path>#<req id>`
    pub path: String,
    /// the owning file's sync state and version
    pub file_state: String,
    pub node_version: u64,
    pub deleted: bool,
}

impl ReqNode {
    pub fn json(&self) -> Value {
        serde_json::to_value(self).unwrap_or(Value::Null)
    }
}

pub fn is_req_file(t: NodeType) -> bool {
    matches!(t, NodeType::Bizreq | NodeType::Techreq)
}

fn valid_id(id: &str) -> bool {
    nf::is_valid_id(id)
}

impl Graph {
    /// Rebuild the req nodes from the live bizreq / techreq file nodes (first
    /// by path keeps a duplicated id; sections without a valid id are left
    /// out until `repair_req_ids` gives them one).
    pub fn rederive_reqs(&mut self) {
        let file_ids: HashSet<String> = self.nodes.iter().map(|n| n.doc.id.clone()).collect();
        let mut files: Vec<&StoredNode> = self.live().filter(|n| is_req_file(n.doc.nodetype)).collect();
        files.sort_by(|a, b| a.doc.path.cmp(&b.doc.path));
        let mut seen: HashSet<String> = HashSet::new();
        let mut out = Vec::new();
        for f in files {
            let (_, items) = rl::parse_reqs(&f.doc.body);
            for (i, it) in items.into_iter().enumerate() {
                if !valid_id(&it.id) || file_ids.contains(&it.id) || !seen.insert(it.id.clone()) {
                    continue;
                }
                out.push(ReqNode {
                    project: self.project.clone(),
                    name: rl::req_node_name(&it),
                    desc: rl::first_sentence(&it.text),
                    path: format!("{}#{}", f.doc.path, it.id),
                    id: it.id,
                    nodetype: REQ.into(),
                    key: it.key,
                    title: it.title,
                    status: if it.status.is_empty() { "draft".into() } else { it.status },
                    text: it.text,
                    order: i,
                    file: f.doc.id.clone(),
                    file_type: f.doc.nodetype.as_str().into(),
                    file_state: f.file_state.clone(),
                    node_version: f.node_version,
                    deleted: false,
                });
            }
        }
        self.req_idx = out.iter().enumerate().map(|(i, r)| (r.id.clone(), i)).collect();
        self.reqs = out;
    }

    /// Give every section whose id is missing, malformed, or already taken
    /// (by an earlier section, a file earlier by path, or a node) a fresh id
    /// and rewrite its file. Returns the files rewritten.
    pub fn repair_req_ids(&mut self, by: &str) -> Result<Vec<String>, ApiError> {
        let mut seen: HashSet<String> = self.nodes.iter().map(|n| n.doc.id.clone()).collect();
        let mut files: Vec<(String, String)> =
            self.live().filter(|n| is_req_file(n.doc.nodetype)).map(|n| (n.doc.path.clone(), n.doc.id.clone())).collect();
        files.sort();
        let mut touched = Vec::new();
        for (_, fid) in files {
            let doc = self.get(&fid).unwrap().doc.clone();
            let (pre, mut items) = rl::parse_reqs(&doc.body);
            let mut changed = false;
            for it in items.iter_mut() {
                if !valid_id(&it.id) || !seen.insert(it.id.clone()) {
                    it.id = uuid::Uuid::new_v4().to_string();
                    seen.insert(it.id.clone());
                    changed = true;
                }
            }
            if changed {
                let mut d = doc.clone();
                d.body = rl::render_reqs(&pre, &items);
                self.commit_edit(&fid, d, by, &format!("requirement ids made unique in {}", doc.name), false)?;
                touched.push(fid);
            }
        }
        Ok(touched)
    }

    pub fn req(&self, id: &str) -> Option<&ReqNode> {
        self.req_idx.get(id).map(|i| &self.reqs[*i])
    }

    /// (file id, req id) of every derived `contains` edge.
    pub fn contains_edges(&self) -> Vec<(String, String)> {
        self.reqs.iter().map(|r| (r.file.clone(), r.id.clone())).collect()
    }

    /// Every edge of the graph as (from, kind, to): the file-declared ones
    /// plus the derived `contains` edges.
    pub fn all_edges(&self) -> Vec<(String, String, String)> {
        let mut v: Vec<(String, String, String)> =
            self.edges().into_iter().map(|e| (e.from, e.kind.as_str().to_string(), e.to)).collect();
        v.extend(self.contains_edges().into_iter().map(|(f, t)| (f, CONTAINS.to_string(), t)));
        v
    }

    /// Any live node (file node or req node) as JSON.
    pub fn node_json(&self, id: &str) -> Option<Value> {
        if let Some(n) = self.get(id).filter(|n| n.live()) {
            return Some(n.json());
        }
        self.req(id).map(ReqNode::json)
    }
}

// ---------- file-level helpers ----------

/// A live requirement file of this project, or why not.
fn req_file<'a>(g: &'a Graph, id: &str) -> Result<&'a StoredNode, ApiError> {
    let f = g.live_node(id)?;
    if !is_req_file(f.doc.nodetype) {
        return Err(bad(format!("{} is a {} node, not a requirement file (bizreq | techreq)", f.doc.name, nodes::type_label(&f.doc))));
    }
    Ok(f)
}

/// Write a file's section list back through the node (render → conform →
/// node_version + 1 → pending_write / designed).
fn write_items(g: &mut Graph, file: &str, pre: &str, items: &[ReqItem], by: &str, change: &str) -> Result<(), ApiError> {
    let mut doc = g.get(file).ok_or_else(|| not_found(format!("no node {file}")))?.doc.clone();
    doc.body = rl::render_reqs(pre, items);
    g.commit_edit(file, doc, by, change, true)
}

fn s<'a>(v: &'a Value, k: &str) -> &'a str {
    v.get(k).and_then(|x| x.as_str()).unwrap_or("")
}

/// An optional string field: absent / null → None; not a string → 400.
fn opt_str(v: &Value, k: &str) -> Result<Option<String>, ApiError> {
    match v.get(k) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(x)) => Ok(Some(x.clone())),
        Some(_) => Err(bad(format!("{k} must be a string"))),
    }
}

fn check_key(k: &str) -> Result<(), ApiError> {
    if !k.is_empty() && !rl::is_req_key(k) {
        return Err(bad(format!("key {k:?}: letters, digits, _ . - only")));
    }
    Ok(())
}

fn check_title(t: &str) -> Result<String, ApiError> {
    let t = t.trim();
    if t.is_empty() {
        return Err(bad("a requirement needs a title"));
    }
    if t.contains('\n') {
        return Err(bad("a requirement title is one line"));
    }
    Ok(t.to_string())
}

/// The text must stay one section: a `## ` line (outside a code fence)
/// would start another requirement.
fn check_text(t: &str) -> Result<String, ApiError> {
    let it = ReqItem::new("", "t", t, "");
    let (_, back) = rl::parse_reqs(&rl::render_reqs("", std::slice::from_ref(&it)));
    if back.len() != 1 {
        return Err(bad("requirement text cannot hold `## ` headings (each one starts a new requirement): use ### or lower"));
    }
    Ok(it.text)
}

fn check_status(st: &str) -> Result<String, ApiError> {
    let st = st.trim();
    if st.is_empty() {
        return Ok("draft".into());
    }
    if !nf::REQ_STATUSES.contains(&st) {
        return Err(bad(format!("status must be {}", nf::REQ_STATUSES.join(" | "))));
    }
    Ok(st.to_string())
}

/// Insert `it` after `after` (a req id of the same list) or at the end.
fn insert_after(items: &mut Vec<ReqItem>, it: ReqItem, after: Option<&str>) -> Result<(), ApiError> {
    match after.filter(|a| !a.is_empty()) {
        None => items.push(it),
        Some(a) => {
            let i = items.iter().position(|x| x.id == a).ok_or_else(|| bad(format!("after: {a} is not a requirement of the target file")))?;
            items.insert(i + 1, it);
        }
    }
    Ok(())
}

fn body_json(b: &Bytes) -> Result<Value, ApiError> {
    if b.iter().all(|c| c.is_ascii_whitespace()) {
        return Ok(json!({}));
    }
    serde_json::from_slice(b).map_err(|e| bad(format!("body is not JSON: {e}")))
}

// ---------- graph operations ----------

/// The requirement file of `node` for type `t`: the one it already names in
/// `children.reqs`, else a new one at the designer path (§2.5 / §2.8),
/// added to the node's `children.reqs`. Returns (file id, created).
fn file_for_node(g: &mut Graph, node: &str, t: NodeType, by: &str) -> Result<(String, bool), ApiError> {
    let owner = g.live_node(node)?.clone();
    if !matches!(owner.doc.nodetype, NodeType::Code | NodeType::Project) {
        return Err(bad(format!("requirements attach to a code node or the project node, not a {}", nodes::type_label(&owner.doc))));
    }
    let mut have: Vec<&StoredNode> = g
        .edges()
        .into_iter()
        .filter(|e| e.from == node && e.kind == EdgeKind::Reqs)
        .filter_map(|e| g.get(&e.to))
        .filter(|n| n.live() && n.doc.nodetype == t)
        .collect();
    // prefer the file in the node's own reqs/ folder
    let own_dir = format!("{}/reqs/", nf::dir_of(&owner.doc.path));
    have.sort_by_key(|n| (!n.doc.path.starts_with(&own_dir), n.doc.path.clone()));
    if let Some(f) = have.first() {
        return Ok((f.doc.id.clone(), false));
    }
    let now = nf::now_ts();
    let what = if t == NodeType::Bizreq { "business requirements" } else { "technical requirements" };
    let mut doc = NodeDoc::new(t, &format!("{} {what}", owner.doc.name), by, &now);
    doc.path = nf::plan_path(&doc, Some(&owner.doc), nf::Attach::Under(EdgeKind::Reqs), &g.taken_paths(), g.naming());
    // the file may exist already without the node naming it: use it
    let existing = g.nodes.iter().find(|n| !n.deleted && (n.doc.path == doc.path || n.old_path == doc.path)).map(|n| (n.doc.id.clone(), n.live() && n.doc.nodetype == t));
    let (fid, fpath, created) = match existing {
        Some((id, true)) => (id, doc.path.clone(), false),
        Some(_) => return Err(nodes::status(axum::http::StatusCode::CONFLICT, format!("{} is taken by a node that is not a live {} file", doc.path, t.as_str()))),
        None => {
            let doc = nodes::canonical(&doc, &now, by);
            let (fid, fpath) = (doc.id.clone(), doc.path.clone());
            g.insert_new(doc, by, &format!("new {} file of {}", t.as_str(), owner.doc.name));
            (fid, fpath, true)
        }
    };
    // the owner names it in children.reqs (relative to its own folder when inside it)
    let covered = owner.doc.children.reqs.iter().any(|e| !nf::resolve(e, &owner.doc.path, std::slice::from_ref(&fpath)).is_empty());
    if !covered {
        let dir = nf::dir_of(&owner.doc.path);
        let entry = match fpath.strip_prefix(&format!("{dir}/")) {
            Some(rest) if !dir.is_empty() => format!("{{thisfiledir}}/{rest}"),
            _ => fpath.clone(),
        };
        let mut od = owner.doc.clone();
        od.children.reqs.push(entry);
        g.commit_edit(node, od, by, &format!("reqs {} → {}", owner.doc.name, t.as_str()), true)?;
    }
    Ok((fid, created))
}

/// Move requirement `id` to `to_file` (after `after`, else at the end), same
/// id. Within one file this reorders. Returns the files touched.
pub fn move_req(g: &mut Graph, id: &str, to_file: &str, after: Option<&str>, by: &str) -> Result<Vec<String>, ApiError> {
    let r = g.req(id).cloned().ok_or_else(|| not_found(format!("no requirement {id} in {}", g.project)))?;
    if !g.get(to_file).is_some_and(|n| n.live()) {
        return Err(not_found(format!("no node {to_file} in {}", g.project)));
    }
    let target = req_file(g, to_file)?.clone();
    if after == Some(id) {
        return Err(bad("after: a requirement cannot follow itself"));
    }
    let src = g.get(&r.file).unwrap().clone();
    let (spre, mut sitems) = rl::parse_reqs(&src.doc.body);
    let pos = sitems.iter().position(|x| x.id == id).ok_or_else(|| not_found(format!("no requirement {id} in {}", src.doc.path)))?;
    let item = sitems.remove(pos);
    let name = rl::req_node_name(&item);
    if target.doc.id == src.doc.id {
        insert_after(&mut sitems, item, after)?;
        write_items(g, &src.doc.id, &spre, &sitems, by, &format!("reorder requirement {name}"))?;
        return Ok(vec![src.doc.id.clone()]);
    }
    let (tpre, mut titems) = rl::parse_reqs(&target.doc.body);
    insert_after(&mut titems, item, after)?;
    write_items(g, &src.doc.id, &spre, &sitems, by, &format!("move requirement {name} to {}", target.doc.name))?;
    write_items(g, &target.doc.id, &tpre, &titems, by, &format!("move requirement {name} from {}", src.doc.name))?;
    Ok(vec![src.doc.id.clone(), target.doc.id.clone()])
}

/// Refuse a target file that lives in another project (clear message), or
/// does not exist at all.
async fn check_target(st: &AppState, g: &Graph, to_file: &str) -> Result<(), ApiError> {
    if to_file.is_empty() {
        return Err(bad("move needs to_file (a requirement file node id)"));
    }
    if g.get(to_file).is_some_and(|n| n.live()) {
        return Ok(());
    }
    if let Some(a) = st.store.arango() {
        let rows = a
            .aql("FOR n IN node FILTER n.id == @id AND n.project != @p LIMIT 1 RETURN n.project", json!({"id": to_file, "p": g.project}))
            .await
            .map_err(nodes::backend)?;
        if let Some(p) = rows.first().and_then(|p| p.as_str()) {
            return Err(bad(format!("{to_file} belongs to project {p}: requirements move only between files of one project")));
        }
    }
    Err(not_found(format!("no node {to_file} in {}", g.project)))
}

// ---------- routes ----------

pub fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/api/projects/{name}/graph/reqs", post(req_create))
        .route("/api/projects/{name}/graph/reqs/{id}", patch(req_patch).delete(req_delete))
        .route("/api/projects/{name}/graph/reqs/{id}/move", post(req_move))
}

fn reply(g: &Graph, id: &str, file: &str, touched: &[String]) -> Value {
    json!({"req": g.req(id).map(ReqNode::json), "file": g.get(file).map(StoredNode::json), "touched": touched})
}

async fn req_create(user: AuthUser, State(st): Ctx, Path(name): Path<String>, Json(req): Json<Value>) -> Result<Json<Value>, ApiError> {
    user.require_writer()?;
    if st.store.get("project", &name, crate::api::NOSK).await?.is_none() {
        return Err(not_found(format!("no project {name}")));
    }
    let title = check_title(s(&req, "title"))?;
    let key = s(&req, "key").trim().to_string();
    check_key(&key)?;
    let text = check_text(s(&req, "text"))?;
    let status = check_status(s(&req, "status"))?;
    let after = opt_str(&req, "after")?;
    let _g = nodes::lock(&name).await;
    let mut g = crate::graph::load_ensured(&st, &name).await?;
    let (file, node) = (s(&req, "file").trim(), s(&req, "node").trim());
    let mut touched = Vec::new();
    let fid = match (file.is_empty(), node.is_empty()) {
        (false, true) => req_file(&g, file)?.doc.id.clone(),
        (true, false) => {
            let t = match s(&req, "type").trim() {
                "bizreq" => NodeType::Bizreq,
                "techreq" => NodeType::Techreq,
                _ => return Err(bad("with node, type is bizreq | techreq")),
            };
            let before = g.live_node(node)?.node_version;
            let (fid, _created) = file_for_node(&mut g, node, t, &user.sub)?;
            if g.get(node).is_some_and(|n| n.node_version != before) {
                touched.push(node.to_string());
            }
            fid
        }
        _ => return Err(bad("name the requirement file (file) or the node + type (node, type: bizreq | techreq) — one of them")),
    };
    let f = g.get(&fid).unwrap().clone();
    let (pre, mut items) = rl::parse_reqs(&f.doc.body);
    let it = ReqItem::new(&key, &title, &text, &status);
    let id = it.id.clone();
    let rname = rl::req_node_name(&it);
    insert_after(&mut items, it, after.as_deref())?;
    write_items(&mut g, &fid, &pre, &items, &user.sub, &format!("new requirement {rname}"))?;
    if !touched.contains(&fid) {
        touched.push(fid.clone());
    }
    g.save(st.store.as_ref()).await?;
    Ok(Json(reply(&g, &id, &fid, &touched)))
}

async fn req_patch(user: AuthUser, State(st): Ctx, Path((name, id)): Path<(String, String)>, Json(req): Json<Value>) -> Result<Json<Value>, ApiError> {
    user.require_writer()?;
    let _g = nodes::lock(&name).await;
    let mut g = Graph::load(st.store.as_ref(), &name).await?;
    let r = g.req(&id).cloned().ok_or_else(|| not_found(format!("no requirement {id} in {name}")))?;
    let f = g.get(&r.file).unwrap().clone();
    if let Some(ev) = req.get("expect_version").filter(|v| !v.is_null()) {
        let ev = ev.as_u64().ok_or_else(|| bad("expect_version is the file node's node_version (an integer)"))?;
        if ev != f.node_version {
            return Err(ApiError::Conflict(json!({"req": r.json(), "file": f.json()})));
        }
    }
    let (pre, mut items) = rl::parse_reqs(&f.doc.body);
    let it = items.iter_mut().find(|x| x.id == id).ok_or_else(|| not_found(format!("no requirement {id} in {}", f.doc.path)))?;
    let before = it.clone();
    if let Some(k) = opt_str(&req, "key")? {
        check_key(k.trim())?;
        it.key = k.trim().to_string();
    }
    if let Some(t) = opt_str(&req, "title")? {
        it.title = check_title(&t)?;
    }
    if let Some(t) = opt_str(&req, "text")? {
        it.text = check_text(&t)?;
    }
    if let Some(st_) = opt_str(&req, "status")? {
        it.status = check_status(&st_)?;
    }
    if *it != before {
        let what: Vec<&str> = ["key", "title", "text", "status"].into_iter().filter(|k| req.get(*k).is_some_and(|v| !v.is_null())).collect();
        let change = format!("edit requirement {} ({})", rl::req_node_name(it), what.join(", "));
        write_items(&mut g, &f.doc.id, &pre, &items, &user.sub, &change)?;
        g.save(st.store.as_ref()).await?;
    }
    Ok(Json(reply(&g, &id, &f.doc.id, &[f.doc.id.clone()])))
}

#[derive(serde::Deserialize, Default)]
struct ReasonQ {
    #[serde(default)]
    reason: String,
}

async fn req_delete(user: AuthUser, State(st): Ctx, Path((name, id)): Path<(String, String)>, Query(q): Query<ReasonQ>, body: Bytes) -> Result<Json<Value>, ApiError> {
    user.require_writer()?;
    let body = body_json(&body)?;
    let reason = if s(&body, "reason").trim().is_empty() { q.reason.trim().to_string() } else { s(&body, "reason").trim().to_string() };
    if reason.is_empty() {
        return Err(bad("a removal needs a reason (it is recorded in the commit)"));
    }
    let _g = nodes::lock(&name).await;
    let mut g = Graph::load(st.store.as_ref(), &name).await?;
    let r = g.req(&id).cloned().ok_or_else(|| not_found(format!("no requirement {id} in {name}")))?;
    let f = g.get(&r.file).unwrap().clone();
    let (pre, mut items) = rl::parse_reqs(&f.doc.body);
    items.retain(|x| x.id != id);
    write_items(&mut g, &f.doc.id, &pre, &items, &user.sub, &format!("remove requirement {}: {reason}", r.name))?;
    g.save(st.store.as_ref()).await?;
    Ok(Json(json!({"id": id, "removed": true, "file": g.get(&f.doc.id).map(StoredNode::json), "touched": [f.doc.id]})))
}

async fn req_move(user: AuthUser, State(st): Ctx, Path((name, id)): Path<(String, String)>, Json(req): Json<Value>) -> Result<Json<Value>, ApiError> {
    user.require_writer()?;
    let to_file = s(&req, "to_file").trim().to_string();
    let after = opt_str(&req, "after")?;
    move_route(&user, &st, &name, &id, &to_file, after.as_deref()).await.map(Json)
}

/// The move itself (shared with `graph/edges/move` kind `contains`).
pub async fn move_route(user: &AuthUser, st: &AppState, name: &str, id: &str, to_file: &str, after: Option<&str>) -> Result<Value, ApiError> {
    let _g = nodes::lock(name).await;
    let mut g = Graph::load(st.store.as_ref(), name).await?;
    let from_file = g.req(id).map(|r| r.file.clone()).ok_or_else(|| not_found(format!("no requirement {id} in {name}")))?;
    check_target(st, &g, to_file).await?;
    let touched = move_req(&mut g, id, to_file, after, &user.sub)?;
    g.save(st.store.as_ref()).await?;
    let mut v = reply(&g, id, to_file, &touched);
    v["from_file"] = json!(g.get(&from_file).map(StoredNode::json));
    v["edge"] = json!({"from": to_file, "kind": CONTAINS, "to": id});
    v["removed"] = json!({"from": from_file, "kind": CONTAINS, "to": id});
    Ok(v)
}

/// `graph/edges/move` with `kind: "contains"`: dragging the file end of a
/// file → req edge onto another requirement file moves the requirement.
pub async fn edge_move_contains(user: &AuthUser, st: &AppState, name: &str, req: &Value) -> Result<Value, ApiError> {
    let (from, to) = (s(req, "from").trim(), s(req, "to").trim());
    if from.is_empty() || to.is_empty() {
        return Err(bad("an edge needs from and to (node ids) and kind"));
    }
    let new_to = s(req, "new_to").trim();
    if !new_to.is_empty() && new_to != to {
        return Err(bad("a contains edge moves by its file end (new_from); its requirement end stays"));
    }
    let new_from = s(req, "new_from").trim();
    if new_from.is_empty() || new_from == from {
        return Err(bad("edges/move needs new_from (another requirement file)"));
    }
    {
        let g = Graph::load(st.store.as_ref(), name).await?;
        match g.req(to) {
            Some(r) if r.file == from => {}
            Some(_) => return Err(not_found(format!("{to} is not in file {from}"))),
            None => return Err(not_found(format!("no requirement {to} in {name}"))),
        }
    }
    let after = opt_str(req, "after")?;
    move_route(user, st, name, to, new_from, after.as_deref()).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checks() {
        assert!(check_key("PDY-TECH-034").is_ok());
        assert!(check_key("a b").is_err());
        assert!(check_title("  ").is_err());
        assert!(check_title("a\nb").is_err());
        assert!(check_text("x\n## y").is_err());
        assert!(check_text("x\n### y").is_ok());
        assert!(check_text("```\n## in a fence\n```").is_ok());
        assert_eq!(check_status("").unwrap(), "draft");
        assert!(check_status("nope").is_err());
        let mut v = vec![ReqItem { id: "a".into(), ..Default::default() }, ReqItem { id: "b".into(), ..Default::default() }];
        insert_after(&mut v, ReqItem { id: "c".into(), ..Default::default() }, Some("a")).unwrap();
        assert_eq!(v.iter().map(|x| x.id.as_str()).collect::<Vec<_>>(), ["a", "c", "b"]);
        assert!(insert_after(&mut v, ReqItem::default(), Some("zz")).is_err());
    }
}
