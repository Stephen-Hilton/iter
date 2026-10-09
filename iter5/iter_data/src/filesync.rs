//! Project graph ⇄ repo sync (iter5 spec §3.2–§3.4). iter_data never
//! touches a repo: the engine serving a project pushes its node files here
//! (`files/sync`) and pulls the node edits waiting to be written
//! (`files/pending` → write + commit → `files/ack`). Designed projects are
//! built the same way (`build` → engine writes everything → `build/done`).
//!
//! files/sync rules, per file (the engine has already conformed it):
//! - parsed and re-rendered with the same library; when the canonical text
//!   differs from what was sent, the reply asks the engine to rewrite it;
//! - upsert by id; an id another file of the batch (or another live node
//!   whose file still exists) already has → this file gets a new id (rewrite);
//! - when the node holds a server edit not yet written (`node_version >
//!   base_version`, pending) and the file changed too → conflict: the newer
//!   `timestamps.last_modified` wins, a tie goes to the server; the loser is
//!   kept as a `node_conflict` row; a server win is answered with a rewrite.
//!   A newer test result on the node is carried onto the file's content
//!   either way, and a pending edit that is only test results is no conflict;
//! - `deleted` paths (and, with `full: true`, every file the engine did not
//!   send that was once written) mark their nodes deleted.

use crate::api::{ApiError, AppState, AuthUser, NOSK};
use crate::nodes::{self, DESIGNED, Graph, PENDING_DELETE, PENDING_WRITE, SYNCED, StoredNode, bad, not_found};
use crate::testlogs::TEST_RESULT_CHANGE;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use iter_core::nodefile::{self as nf, NodeType};
use iter_core::now_utc;
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

type Ctx = State<Arc<AppState>>;

pub fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/api/projects/{name}/files/sync", post(files_sync))
        .route("/api/projects/{name}/files/pending", get(files_pending))
        .route("/api/projects/{name}/files/ack", post(files_ack))
        .route("/api/projects/{name}/build", post(build).get(build_get))
        .route("/api/projects/{name}/build/done", post(build_done))
}

#[derive(serde::Deserialize)]
pub struct SyncFile {
    pub path: String,
    pub text: String,
    #[serde(default)]
    pub hash: String,
    #[serde(default)]
    pub base_version: u64,
}

#[derive(serde::Deserialize)]
pub struct SyncReq {
    #[serde(default)]
    pub engine: String,
    #[serde(default)]
    pub full: bool,
    #[serde(default)]
    pub files: Vec<SyncFile>,
    #[serde(default)]
    pub deleted: Vec<String>,
}

#[derive(Default, serde::Serialize)]
pub struct SyncReply {
    pub applied: Vec<Value>,
    pub rewrite: Vec<Value>,
    pub removed: Vec<String>,
    pub conflicts: Vec<Value>,
    /// server edits still waiting to be written (the file is unchanged)
    pub waiting: Vec<Value>,
    pub ignored: Vec<Value>,
    pub errors: Vec<Value>,
}

fn conflict_row(project: &str, id: &str, path: &str, winner: &str, loser: Value, why: &str, engine: &str) -> Value {
    json!({"project": project, "id": id, "path": path, "winner": winner, "loser": loser, "why": why, "engine": engine,
           "at": now_utc(), "_key": format!("{}-{}", chrono::Utc::now().format("%Y%m%dT%H%M%S%6f"), &uuid::Uuid::new_v4().simple().to_string()[..8])})
}

fn carry_test_result(from: &nf::NodeDoc, to: &mut nf::NodeDoc) {
    match from.front.get("last_result") {
        Some(v) => {
            to.front.insert("last_result".into(), v.clone());
        }
        None => {
            to.front.remove("last_result");
        }
    }
    to.timestamps.last_tested = from.timestamps.last_tested.clone();
}

fn new_from_file(project: &str, doc: nf::NodeDoc, prev_version: u64, hash: String, engine: &str) -> StoredNode {
    StoredNode {
        doc,
        project: project.to_string(),
        node_version: prev_version + 1,
        file_version: prev_version + 1,
        file_hash: hash,
        file_state: SYNCED.into(),
        deleted: false,
        test: Value::Null,
        updated_by: format!("engine.{engine}"),
        updated: now_utc(),
        old_path: String::new(),
        change: String::new(),
        seeded: false,
        documents: Vec::new(),
        last_commit: String::new(),
    }
}

/// The sync itself, on a loaded graph (the caller holds the project lock).
pub fn apply_sync(g: &mut Graph, req: &SyncReq) -> (SyncReply, Vec<Value>) {
    let project = g.project.clone();
    let engine = if req.engine.is_empty() { "?" } else { req.engine.as_str() };
    let creator = format!("engine.{engine}");
    let now = nf::now_ts();
    let mut rep = SyncReply::default();
    let mut conflicts: Vec<Value> = Vec::new();
    let deleted: HashSet<&str> = req.deleted.iter().map(|s| s.as_str()).collect();
    let sent: HashSet<&str> = req.files.iter().map(|f| f.path.as_str()).collect();
    let mut files: Vec<&SyncFile> = req.files.iter().collect();
    // a file sitting where its node already is goes first, so a copy is the one renamed
    files.sort_by_key(|f| (!g.nodes.iter().any(|n| n.doc.path == f.path), f.path.clone()));
    let mut batch_ids: HashMap<String, String> = HashMap::new();
    let mut touched: HashSet<String> = HashSet::new();
    let mut real_project_node = false;

    for f in files {
        let path = f.path.trim();
        if !path.starts_with("{topdir}/") || path.split('/').any(|s| s == "..") {
            rep.errors.push(json!({"path": path, "error": "paths are {topdir}/… inside the project"}));
            continue;
        }
        match nf::type_of(path) {
            Some(t) if nf::is_synced(t) => {}
            Some(_) => {
                rep.ignored.push(json!({"path": path, "reason": "agent memory is never synced"}));
                continue;
            }
            None => {
                rep.ignored.push(json!({"path": path, "reason": "not a node file"}));
                continue;
            }
        }
        let c = nf::conform(path, &f.text, &now, &creator);
        let Some(mut doc) = c.doc else {
            rep.errors.push(json!({"path": path, "error": "could not be read as a node file"}));
            continue;
        };
        doc.path = path.to_string();
        let mut text = nf::render(&doc);
        let mut rewrite = text != f.text;
        // id collision: another file of this batch, or a live node whose file is still there
        let existing = g.get(&doc.id).cloned();
        let collide = batch_ids.get(&doc.id).is_some_and(|p| p != path)
            || existing.as_ref().is_some_and(|n| {
                n.live()
                    && n.doc.path != path
                    && n.old_path != path
                    && n.file_version > 0
                    && !deleted.contains(n.doc.path.as_str())
                    && (!req.full || sent.contains(n.doc.path.as_str()))
            });
        let existing = if collide {
            let old = doc.id.clone();
            doc.id = uuid::Uuid::new_v4().to_string();
            text = nf::render(&doc);
            rewrite = true;
            rep.conflicts.push(json!({"path": path, "kind": "id-collision", "old_id": old, "id": doc.id}));
            None
        } else {
            existing
        };
        batch_ids.insert(doc.id.clone(), path.to_string());
        let mut hash = if rewrite || f.hash.is_empty() { nf::content_hash(&text) } else { f.hash.clone() };
        let id = doc.id.clone();
        touched.insert(id.clone());
        if doc.nodetype == NodeType::Project {
            real_project_node = true;
        }
        let push_rewrite = |rep: &mut SyncReply, text: &str, v: u64| rep.rewrite.push(json!({"path": path, "id": id, "text": text, "node_version": v}));

        let Some(n) = existing.filter(|n| !n.deleted) else {
            // new (or revived) node
            let prev = g.get(&id).map(|n| n.node_version).unwrap_or(0);
            let sn = new_from_file(&project, doc, prev, hash, engine);
            let v = sn.node_version;
            g.push(sn);
            if rewrite {
                push_rewrite(&mut rep, &text, v);
            }
            rep.applied.push(json!({"id": id, "path": path, "node_version": v}));
            continue;
        };
        let at_old = !n.old_path.is_empty() && n.old_path == path;
        let moved = n.doc.path != path && !at_old;
        let file_changed = hash != n.file_hash || moved;
        let unwritten = matches!(n.file_state.as_str(), PENDING_WRITE | PENDING_DELETE | DESIGNED) && n.node_version > f.base_version;
        let same_content = nf::semantic_hash(&doc) == nf::semantic_hash(&n.doc) && !moved;
        if !file_changed && !rewrite {
            if unwritten {
                rep.waiting.push(json!({"id": id, "path": path, "node_version": n.node_version}));
            }
            continue;
        }
        if same_content && n.file_state != PENDING_DELETE && !at_old {
            // the file says what the node says (often: the engine wrote it, ack not yet in)
            let x = g.get_mut(&id).unwrap();
            x.file_hash = hash;
            x.file_version = x.node_version;
            x.file_state = SYNCED.into();
            if rewrite {
                push_rewrite(&mut rep, &text, x.node_version);
            }
            rep.applied.push(json!({"id": id, "path": path, "node_version": n.node_version}));
            continue;
        }
        // A test result is the server's alone (posted by the engine, never edited
        // in a file), so a newer one waiting to be written is carried onto the
        // file's content rather than lost with the node's version.  When that
        // result is all the pending edit holds, nothing is in conflict: on
        // 2026-10-09 every one of pdy-dev's 12 conflicts was a test agent's file
        // edit landing a minute after the engine posted its run's result.
        let result_only = unwritten && n.change.starts_with(TEST_RESULT_CHANGE);
        // (>=: last_tested is to the second, and two runs can land in one)
        let newer_result = n.doc.timestamps.last_tested >= doc.timestamps.last_tested
            && (n.doc.timestamps.last_tested != doc.timestamps.last_tested || n.doc.front.get("last_result") != doc.front.get("last_result"));
        if unwritten && newer_result {
            carry_test_result(&n.doc, &mut doc);
            if !at_old {
                text = nf::render(&doc);
                hash = nf::content_hash(&text);
                rewrite = true;
            }
        }
        if unwritten && !result_only {
            let file_newer = doc.timestamps.last_modified > n.doc.timestamps.last_modified;
            if file_newer {
                conflicts.push(conflict_row(&project, &id, path, "file", n.json(), "file and node both changed; the file is newer", engine));
                rep.conflicts.push(json!({"id": id, "path": path, "kind": "edit", "winner": "file"}));
                // fall through: the file's content is applied below
            } else {
                conflicts.push(conflict_row(&project, &id, path, "server", json!({"path": path, "text": f.text}), "file and node both changed; the node is newer (or equal)", engine));
                rep.conflicts.push(json!({"id": id, "path": path, "kind": "edit", "winner": "server"}));
                if n.file_state == PENDING_DELETE {
                    continue; // the delete still goes out through files/pending
                }
                let stext = nf::render(&n.doc);
                let x = g.get_mut(&id).unwrap();
                if n.doc.path == path {
                    // the engine rewrites the file now: that is the write
                    x.file_state = SYNCED.into();
                    x.file_version = x.node_version;
                    x.file_hash = nf::content_hash(&stext);
                }
                push_rewrite(&mut rep, &stext, n.node_version);
                continue;
            }
        }
        // the file wins / plain file change
        let x = g.get_mut(&id).unwrap();
        let keep_test = x.test.clone();
        x.node_version += 1;
        if at_old {
            // a server move is still pending: take the content, keep the move
            let target = x.doc.path.clone();
            x.doc = doc;
            x.doc.path = target;
            x.file_hash = hash;
            x.file_state = PENDING_WRITE.into();
        } else {
            x.doc = doc;
            x.file_hash = hash;
            x.file_version = x.node_version;
            x.file_state = SYNCED.into();
            x.old_path.clear();
        }
        x.test = keep_test;
        x.deleted = false;
        x.seeded = false;
        x.updated_by = creator.clone();
        x.updated = now_utc();
        let v = x.node_version;
        if rewrite {
            push_rewrite(&mut rep, &text, v);
        }
        rep.applied.push(json!({"id": id, "path": path, "node_version": v}));
    }

    // deletions: named, or (full) every once-written file not sent
    let candidates: Vec<StoredNode> = g
        .nodes
        .iter()
        .filter(|n| !n.deleted && n.file_version > 0 && !touched.contains(n.id()))
        .filter(|n| {
            deleted.contains(n.doc.path.as_str())
                || (req.full && !sent.contains(n.doc.path.as_str()) && !(!n.old_path.is_empty() && sent.contains(n.old_path.as_str())))
        })
        .cloned()
        .collect();
    for n in candidates {
        let id = n.doc.id.clone();
        if n.file_state == PENDING_WRITE && !n.old_path.is_empty() && !deleted.contains(n.doc.path.as_str()) {
            continue; // a pending move: its file is still at old_path
        }
        if n.file_state == PENDING_WRITE {
            conflicts.push(conflict_row(&project, &id, &n.doc.path, "file", n.json(), "the file was deleted while a node edit waited to be written", engine));
            rep.conflicts.push(json!({"id": id, "path": n.doc.path, "kind": "deleted", "winner": "file"}));
        }
        let x = g.get_mut(&id).unwrap();
        if x.file_state != PENDING_DELETE {
            x.node_version += 1;
        }
        x.deleted = true;
        x.file_state = SYNCED.into();
        x.file_version = x.node_version;
        x.updated_by = creator.clone();
        x.updated = now_utc();
        rep.removed.push(id);
    }

    // the repo's own project node replaces the server's untouched seeds
    if real_project_node {
        let seeds: Vec<String> = g
            .nodes
            .iter()
            .filter(|n| n.seeded && n.file_version == 0 && n.node_version <= 1 && !touched.contains(n.id()))
            .map(|n| n.doc.id.clone())
            .collect();
        for id in seeds {
            g.purge(&id);
        }
    }
    (rep, conflicts)
}

async fn insert_conflicts(st: &AppState, rows: &[Value]) -> Result<(), ApiError> {
    if rows.is_empty() {
        return Ok(());
    }
    nodes::arango(st.store.as_ref())?
        .aql("FOR r IN @rs INSERT r INTO node_conflict", json!({"rs": rows}))
        .await
        .map_err(nodes::backend)?;
    Ok(())
}

async fn files_sync(user: AuthUser, State(st): Ctx, Path(name): Path<String>, Json(req): Json<SyncReq>) -> Result<Json<Value>, ApiError> {
    user.require_writer()?;
    let _g = nodes::lock(&name).await;
    let mut g = Graph::load(st.store.as_ref(), &name).await?;
    let (rep, rows) = apply_sync(&mut g, &req);
    let saved = g.save(st.store.as_ref()).await?;
    insert_conflicts(&st, &rows).await?;
    let mut v = serde_json::to_value(&rep).unwrap_or(json!({}));
    v["edges"] = json!({"total": saved.edges, "added": saved.edges_added, "removed": saved.edges_removed});
    Ok(Json(v))
}

/// One pending file operation (write | delete | move).
pub fn pending_ops(g: &Graph) -> Vec<Value> {
    let mut ops: Vec<Value> = Vec::new();
    for n in g.nodes.iter().filter(|n| !n.deleted) {
        match n.file_state.as_str() {
            PENDING_WRITE => {
                let text = nf::render(&n.doc);
                let mut op = json!({"id": n.id(), "op": if n.old_path.is_empty() { "write" } else { "move" }, "path": n.doc.path,
                                    "text": text, "hash": nf::content_hash(&text), "node_version": n.node_version, "summary": n.change});
                if !n.old_path.is_empty() {
                    op["old_path"] = json!(n.old_path);
                }
                ops.push(op);
            }
            PENDING_DELETE => ops.push(json!({"id": n.id(), "op": "delete", "path": n.doc.path, "text": "",
                                              "node_version": n.node_version, "summary": n.change})),
            _ => {}
        }
    }
    ops.sort_by(|a, b| a["path"].as_str().cmp(&b["path"].as_str()));
    ops
}

async fn files_pending(_u: AuthUser, State(st): Ctx, Path(name): Path<String>) -> Result<Json<Value>, ApiError> {
    let g = Graph::load(st.store.as_ref(), &name).await?;
    let ops = pending_ops(&g);
    Ok(Json(json!({"project": name, "count": ops.len(), "pending": ops})))
}

struct Ack {
    id: String,
    node_version: u64,
    path: String,
    hash: String,
    commit: String,
}

impl Ack {
    /// lenient: missing / null fields are empty
    fn from(v: &Value) -> Ack {
        let st = |k: &str| v.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string();
        Ack { id: st("id"), node_version: v.get("node_version").and_then(|x| x.as_u64()).unwrap_or(0), path: st("path"), hash: st("hash"), commit: st("commit") }
    }
}

#[derive(serde::Deserialize)]
struct AckReq {
    #[serde(default)]
    engine: String,
    #[serde(default)]
    acks: Vec<Value>,
}

async fn files_ack(user: AuthUser, State(st): Ctx, Path(name): Path<String>, Json(req): Json<AckReq>) -> Result<Json<Value>, ApiError> {
    user.require_writer()?;
    let _g = nodes::lock(&name).await;
    let mut g = Graph::load(st.store.as_ref(), &name).await?;
    let (mut acked, mut stale, mut unknown) = (Vec::new(), Vec::new(), Vec::new());
    for a in req.acks.iter().map(Ack::from) {
        let Some(n) = g.get(&a.id).cloned() else {
            unknown.push(a.id.clone());
            continue;
        };
        let x = g.get_mut(&a.id).unwrap();
        if !a.commit.is_empty() {
            x.last_commit = a.commit.clone();
        }
        if a.node_version >= n.node_version {
            if n.file_state == PENDING_DELETE {
                x.deleted = true;
                x.file_hash.clear();
            } else {
                x.file_hash = if a.hash.is_empty() { nf::content_hash(&nf::render(&n.doc)) } else { a.hash.clone() };
            }
            x.file_state = SYNCED.into();
            x.file_version = n.node_version;
            x.old_path.clear();
            acked.push(a.id.clone());
        } else {
            // an older version reached the file; the newer edit still waits
            x.file_version = x.file_version.max(a.node_version);
            if !a.hash.is_empty() {
                x.file_hash = a.hash.clone();
            }
            if !a.path.is_empty() && a.path == n.doc.path {
                x.old_path.clear();
            }
            stale.push(a.id.clone());
        }
    }
    g.save(st.store.as_ref()).await?;
    let left = pending_ops(&g).len();
    Ok(Json(json!({"engine": req.engine, "acked": acked, "stale": stale, "unknown": unknown, "pending": left})))
}

// ---------- designer → build (§3.4) ----------

#[derive(serde::Deserialize)]
struct BuildReq {
    engine: String,
    topdir: String,
    #[serde(default)]
    queue_plan: bool,
    #[serde(default)]
    plan_note: String,
}

/// Create (or re-activate) the serves edge engine → project with this topdir.
pub async fn activate_serves(st: &AppState, engine: &str, project: &str, topdir: &str) -> Result<String, ApiError> {
    use iter_core::settings::{SysEdge, node_id};
    let (eid, pid) = (node_id("iter_engine", engine), node_id("project", project));
    let edges = crate::settings::load_edges(st.store.as_ref()).await?;
    let now = now_utc();
    let edge = match edges.into_iter().find(|e| e.edge_type == "serves" && e.from == eid && e.to == pid) {
        Some(mut e) => {
            if !e.settings.is_object() {
                e.settings = json!({});
            }
            e.settings["topdir"] = json!(topdir);
            e.active = true;
            e.updated = now;
            e
        }
        None => SysEdge {
            id: uuid::Uuid::new_v4().simple().to_string(),
            edge_type: "serves".into(),
            from: eid,
            to: pid,
            tag: String::new(),
            settings: json!({"topdir": topdir, "read_only": false}),
            active: true,
            created: now.clone(),
            updated: now,
        },
    };
    crate::settings::put_edge(st.store.as_ref(), &edge).await?;
    Ok(edge.id)
}

async fn build(user: AuthUser, State(st): Ctx, Path(name): Path<String>, Json(req): Json<BuildReq>) -> Result<Json<Value>, ApiError> {
    user.require_writer()?;
    let engine = req.engine.trim();
    let topdir = req.topdir.trim();
    if engine.is_empty() || topdir.is_empty() {
        return Err(bad("build needs engine and topdir"));
    }
    let Some(erow) = st.store.get("engine", engine, NOSK).await? else {
        return Err(not_found(format!("no engine {engine}")));
    };
    // admins build anywhere; a user builds on an engine they own
    if user.role != "admin" && crate::storage::body_str(&erow, "user") != user.sub {
        return Err(crate::api::forbidden());
    }
    let Some(mut prow) = st.store.get("project", &name, NOSK).await? else {
        return Err(not_found(format!("no project {name}")));
    };
    // a designed project has its project node before the engine takes it on
    nodes::ensure_project_node(st.store.as_ref(), &name).await?;
    let edge = activate_serves(&st, engine, &name, topdir).await?;
    let build = json!({"state": "requested", "engine": engine, "topdir": topdir, "at": now_utc(), "by": user.sub,
                       "queue_plan": req.queue_plan, "plan_note": req.plan_note});
    prow["build"] = build.clone();
    st.store.put("project", &name, NOSK, &prow).await?;
    st.store.bump_seq(&name, "project").await?;
    let _g = nodes::lock(&name).await;
    let mut g = Graph::load(st.store.as_ref(), &name).await?;
    let designed: Vec<String> = g.nodes.iter().filter(|n| !n.deleted && n.file_state == DESIGNED).map(|n| n.doc.id.clone()).collect();
    for id in &designed {
        let x = g.get_mut(id).unwrap();
        x.file_state = PENDING_WRITE.into();
        if x.change.is_empty() {
            x.change = "build from design".into();
        }
    }
    g.save(st.store.as_ref()).await?;
    Ok(Json(json!({"build": build, "serves_edge": edge, "pending": pending_ops(&g).len(), "flipped": designed.len()})))
}

#[derive(serde::Deserialize)]
struct DoneReq {
    #[serde(default)]
    engine: String,
    #[serde(default)]
    commit: String,
    #[serde(default)]
    error: String,
}

async fn build_done(user: AuthUser, State(st): Ctx, Path(name): Path<String>, Json(req): Json<DoneReq>) -> Result<Json<Value>, ApiError> {
    user.require_writer()?;
    let Some(mut prow) = st.store.get("project", &name, NOSK).await? else {
        return Err(not_found(format!("no project {name}")));
    };
    let b = prow.get("build").cloned().unwrap_or(Value::Null);
    if b["state"] != "requested" {
        return Err(ApiError::Status(StatusCode::CONFLICT, format!("no build of {name} is waiting (state {})", b["state"])));
    }
    if !req.engine.is_empty() && b["engine"].as_str() != Some(req.engine.as_str()) {
        return Err(ApiError::Status(StatusCode::CONFLICT, format!("the build of {name} was requested from engine {}", b["engine"])));
    }
    let mut nb = b.clone();
    nb["state"] = json!(if req.error.is_empty() { "done" } else { "failed" });
    nb["commit"] = json!(req.commit);
    nb["error"] = json!(req.error);
    nb["done_at"] = json!(now_utc());
    let mut plan = Value::Null;
    if req.error.is_empty() && b["queue_plan"] == true {
        let g = Graph::load(st.store.as_ref(), &name).await?;
        let pnode = g.project_node().map(|p| (p.doc.id.clone(), p.doc.path.clone()));
        let note = b["plan_note"].as_str().unwrap_or("");
        let item = json!({"name": "Build the project from its design", "agent": "plan", "state": "queued", "priority": 5,
            "node": pnode.as_ref().map(|p| p.0.clone()), "lockdirs": [], "blockedby": [], "context": pnode.as_ref().map(|p| vec![p.1.clone()]).unwrap_or_default(),
            "tags": [], "requestedby": b["by"].as_str().map(|u| format!("user:{u}")).unwrap_or_default(), "prework": [], "postwork": [],
            "request": format!("The project was designed in the Project graph and its repo was just created from the design (commit {}). \
Plan the build: read the project node, its global requirements and philosophy, and every node of the design; file the work items that build it, \
in dependency order.{}", req.commit, if note.is_empty() { String::new() } else { format!("\n\nNote from the designer: {note}") })});
        let who = AuthUser { sub: user.sub.clone(), role: user.role.clone() };
        let Json(v) = crate::api::workitem_create(who, State(st.clone()), Path(name.clone()), Json(item)).await?;
        nb["plan_item"] = v["id"].clone();
        plan = v;
    }
    prow["build"] = nb.clone();
    st.store.put("project", &name, NOSK, &prow).await?;
    st.store.bump_seq(&name, "project").await?;
    Ok(Json(json!({"build": nb, "plan": plan})))
}

async fn build_get(_u: AuthUser, State(st): Ctx, Path(name): Path<String>) -> Result<Json<Value>, ApiError> {
    let Some(prow) = st.store.get("project", &name, NOSK).await? else {
        return Err(not_found(format!("no project {name}")));
    };
    let g = Graph::load(st.store.as_ref(), &name).await?;
    let designed = g.nodes.iter().filter(|n| !n.deleted && n.file_state == DESIGNED).count();
    Ok(Json(json!({"project": name, "build": prow.get("build").cloned().unwrap_or(Value::Null), "served": g.served,
                    "designed": designed, "pending": pending_ops(&g).len(), "built": designed == 0 && g.served})))
}

// ---------- heartbeat helpers ----------

/// Projects (of `projects`) with node changes waiting to be written.
pub async fn files_waiting(store: &dyn crate::storage::Storage, projects: &[String]) -> Vec<String> {
    let Some(a) = store.arango() else { return Vec::new() };
    if projects.is_empty() {
        return Vec::new();
    }
    a.aql(
        "FOR n IN node FILTER n.project IN @ps AND n.file_state IN ['pending_write', 'pending_delete'] AND n.deleted != true AND n.nodetype != 'req'
         COLLECT p = n.project SORT p RETURN p",
        json!({"ps": projects}),
    )
    .await
    .unwrap_or_default()
    .into_iter()
    .filter_map(|v| v.as_str().map(String::from))
    .collect()
}

/// Projects (of `projects`) whose build was requested and is not done.
pub async fn build_waiting(store: &dyn crate::storage::Storage, projects: &[String]) -> Vec<String> {
    let mut out = Vec::new();
    for p in projects {
        if let Ok(Some(r)) = store.get("project", p, NOSK).await {
            if r.pointer("/build/state").and_then(|s| s.as_str()) == Some("requested") {
                out.push(p.clone());
            }
        }
    }
    out
}
