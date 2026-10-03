//! The iter5 project graph store (spec §3.1): nodes = files.
//!
//! Every `*.iter.md` node file of a project is one document of the Arango
//! collection `node` (key `doc_key(project, id)`), holding the parsed file
//! (`iter_core::nodefile::NodeDoc`, always conformed: every server write goes
//! through the same `render`/`conform` the engine uses) plus the sync state:
//! `node_version` (+1 on every change), `file_version` (the node_version last
//! written to / read from the file), `file_hash`, `file_state`
//! (synced | pending_write | pending_delete | designed), `deleted`, `test`.
//!
//! Edges (collection `link`, `{project, from, kind, to}`) are fully derived:
//! after every change the whole project's edges are recomputed from all live
//! nodes with `nodefile::edges_of` against the project's node path list, and
//! only the difference is written (one stream transaction with the nodes).
//!
//! Writes to one project are serialised in-process (`lock`): load the
//! project's nodes, change them in memory (`Graph`), `save`.

use crate::api::ApiError;
use crate::arango::{self, ArangoBackend, ArangoErr, LINK_COLL, NODE_COLL};
use crate::storage::Storage;
use axum::http::StatusCode;
use iter_core::nodefile::{self as nf, Edge, EdgeKind, NodeDoc, NodeErr, NodeType};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::{BTreeSet, HashMap, HashSet};
use std::sync::{Arc, LazyLock};

pub const CONFLICT_COLL: &str = "node_conflict";
pub const TEST_LOG_COLL: &str = "test_log";

pub const SYNCED: &str = "synced";
pub const PENDING_WRITE: &str = "pending_write";
pub const PENDING_DELETE: &str = "pending_delete";
pub const DESIGNED: &str = "designed";

/// Collections and indexes of the project graph beyond `node`/`link`
/// (called once from `ArangoBackend::ensure_schema`).
pub async fn ensure_schema(a: &ArangoBackend) -> Result<(), ArangoErr> {
    for name in [CONFLICT_COLL, TEST_LOG_COLL] {
        match a.dbcall("POST", "/_api/collection", Some(&json!({"name": name, "type": 2}))).await {
            Ok(_) => {}
            Err(e) if e.num == 1207 => {}
            Err(e) => return Err(e),
        }
    }
    for (coll, fields, name) in [
        (NODE_COLL, json!(["project", "file_state"]), "project_file_state"),
        (TEST_LOG_COLL, json!(["project", "node", "at"]), "project_node_at"),
        (TEST_LOG_COLL, json!(["project", "at"]), "project_at"),
        (CONFLICT_COLL, json!(["project", "at"]), "project_at"),
    ] {
        a.dbcall("POST", &format!("/_api/index?collection={coll}"), Some(&json!({"type": "persistent", "fields": fields, "name": name})))
            .await?;
    }
    Ok(())
}

/// One stored node: the file's content plus its sync state.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoredNode {
    #[serde(flatten)]
    pub doc: NodeDoc,
    #[serde(default)]
    pub project: String,
    /// +1 on every change (server edit or file sync); 0 = an iter4 snapshot row
    #[serde(default)]
    pub node_version: u64,
    /// the node_version last written to / read from the file (0 = never on disk)
    #[serde(default)]
    pub file_version: u64,
    /// content hash of the file as last seen / written
    #[serde(default)]
    pub file_hash: String,
    #[serde(default)]
    pub file_state: String,
    #[serde(default)]
    pub deleted: bool,
    /// the last test result summary (test nodes)
    #[serde(default)]
    pub test: Value,
    #[serde(default)]
    pub updated_by: String,
    #[serde(default)]
    pub updated: String,
    /// a pending move: the path the file still has on disk
    #[serde(default)]
    pub old_path: String,
    /// what the last server-side change did (the engine's commit message)
    #[serde(default)]
    pub change: String,
    /// created by the server as a project default (removed again when the
    /// repo's real project node arrives before anyone touched it)
    #[serde(default)]
    pub seeded: bool,
    /// `children.documents` expanded to `{topdir}/…` paths (GraphRAG links)
    #[serde(default)]
    pub documents: Vec<String>,
    #[serde(default)]
    pub last_commit: String,
}

impl StoredNode {
    pub fn id(&self) -> &str {
        &self.doc.id
    }
    pub fn live(&self) -> bool {
        !self.deleted && self.file_state != PENDING_DELETE
    }
    pub fn json(&self) -> Value {
        serde_json::to_value(self).unwrap_or(Value::Null)
    }
}

pub fn status(code: StatusCode, msg: impl Into<String>) -> ApiError {
    ApiError::Status(code, msg.into())
}
pub fn bad(msg: impl Into<String>) -> ApiError {
    status(StatusCode::BAD_REQUEST, msg)
}
pub fn not_found(msg: impl Into<String>) -> ApiError {
    status(StatusCode::NOT_FOUND, msg)
}
pub fn backend(e: impl std::fmt::Display) -> ApiError {
    status(StatusCode::INTERNAL_SERVER_ERROR, e.to_string())
}

pub fn arango(store: &dyn Storage) -> Result<&ArangoBackend, ApiError> {
    store.arango().ok_or_else(|| backend("the project graph needs the ArangoDB store"))
}

pub fn node_key(project: &str, id: &str) -> String {
    arango::doc_key(project, id)
}

fn link_key(project: &str, from: &str, kind: &str, to: &str) -> String {
    arango::doc_key(project, &format!("{from}|{kind}|{to}"))
}

static LOCKS: LazyLock<std::sync::Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>> =
    LazyLock::new(|| std::sync::Mutex::new(HashMap::new()));

/// Serialise graph writes of one project (one iter_data process).
pub async fn lock(project: &str) -> tokio::sync::OwnedMutexGuard<()> {
    let m = {
        let mut g = LOCKS.lock().unwrap();
        g.entry(project.to_string()).or_insert_with(|| Arc::new(tokio::sync::Mutex::new(()))).clone()
    };
    m.lock_owned().await
}

/// Conform a node the way the engine would: render, conform, keep the result.
pub fn canonical(doc: &NodeDoc, now: &str, creator: &str) -> NodeDoc {
    let c = nf::conform(&doc.path, &nf::render(doc), now, creator);
    let mut out = c.doc.unwrap_or_else(|| doc.clone());
    out.path = doc.path.clone();
    out
}

/// Does any active `serves` edge point at this project?
pub async fn is_served(store: &dyn Storage, project: &str) -> Result<bool, ApiError> {
    let pid = iter_core::settings::node_id("project", project);
    let edges = crate::settings::load_edges(store).await?;
    Ok(edges.iter().any(|e| e.edge_type == "serves" && e.to == pid && e.is_active()))
}

/// The in-memory working copy of one project's graph.
pub struct Graph {
    pub project: String,
    pub nodes: Vec<StoredNode>,
    idx: HashMap<String, usize>,
    dirty: BTreeSet<String>,
    /// node documents to remove physically (designed nodes deleted, iter4 rows)
    gone: BTreeSet<String>,
    old_links: HashSet<(String, String, String)>,
    /// the project has an active serves edge (edits become pending writes)
    pub served: bool,
    /// the derived `req` nodes (one per `## ` section of every live bizreq /
    /// techreq file, spec §2.8), rebuilt from the file nodes (reqs.rs)
    pub reqs: Vec<crate::reqs::ReqNode>,
    pub(crate) req_idx: HashMap<String, usize>,
    /// the req node documents as stored (id → document), for the save diff
    pub(crate) old_reqs: HashMap<String, Value>,
}

pub struct Saved {
    pub edges: usize,
    pub edges_added: usize,
    pub edges_removed: usize,
}

impl Graph {
    pub async fn load(store: &dyn Storage, project: &str) -> Result<Graph, ApiError> {
        let a = arango(store)?;
        let rows = a
            .aql(
                "FOR n IN node FILTER n.project == @p AND n.nodetype != 'req' SORT n.path RETURN UNSET(n, '_id', '_rev')",
                json!({"p": project}),
            )
            .await
            .map_err(backend)?;
        let mut nodes = Vec::new();
        let mut gone = BTreeSet::new();
        for r in rows {
            let key = r.get("_key").and_then(|k| k.as_str()).unwrap_or("").to_string();
            match serde_json::from_value::<StoredNode>(r) {
                Ok(n) if n.node_version > 0 && !n.doc.id.is_empty() => nodes.push(n),
                // an iter4 snapshot vertex (or anything unreadable): dropped on the next save
                _ => {
                    gone.insert(key);
                }
            }
        }
        let links = a
            .aql("FOR l IN link FILTER l.project == @p RETURN [l.from, l.kind, l.to]", json!({"p": project}))
            .await
            .map_err(backend)?;
        let old_links = links
            .iter()
            .filter_map(|l| {
                let s = |i: usize| l.get(i).and_then(|x| x.as_str()).unwrap_or("").to_string();
                Some((s(0), s(1), s(2)))
            })
            .collect();
        let old_reqs: HashMap<String, Value> = a
            .aql("FOR n IN node FILTER n.project == @p AND n.nodetype == 'req' RETURN UNSET(n, '_id', '_rev', '_key')", json!({"p": project}))
            .await
            .map_err(backend)?
            .into_iter()
            .filter_map(|r| Some((r.get("id")?.as_str()?.to_string(), r)))
            .collect();
        let served = is_served(store, project).await?;
        let mut g = Graph {
            project: project.to_string(),
            nodes,
            idx: HashMap::new(),
            dirty: BTreeSet::new(),
            gone: BTreeSet::new(),
            old_links,
            served,
            reqs: Vec::new(),
            req_idx: HashMap::new(),
            old_reqs,
        };
        g.reindex();
        g.rederive_reqs();
        // keys of unreadable rows: removed by key, not by id
        g.gone = gone.into_iter().map(|k| format!("key:{k}")).collect();
        Ok(g)
    }

    fn reindex(&mut self) {
        self.idx = self.nodes.iter().enumerate().map(|(i, n)| (n.doc.id.clone(), i)).collect();
    }

    pub fn get(&self, id: &str) -> Option<&StoredNode> {
        self.idx.get(id).map(|i| &self.nodes[*i])
    }

    /// A live node (not deleted, not pending delete).
    pub fn live_node(&self, id: &str) -> Result<&StoredNode, ApiError> {
        self.get(id).filter(|n| n.live()).ok_or_else(|| not_found(format!("no node {id} in {}", self.project)))
    }

    pub fn get_mut(&mut self, id: &str) -> Option<&mut StoredNode> {
        let i = *self.idx.get(id)?;
        self.dirty.insert(id.to_string());
        Some(&mut self.nodes[i])
    }

    pub fn live(&self) -> impl Iterator<Item = &StoredNode> {
        self.nodes.iter().filter(|n| n.live())
    }

    /// (id, path) of every live node — what edges are resolved against.
    pub fn pairs(&self) -> Vec<(String, String)> {
        self.live().map(|n| (n.doc.id.clone(), n.doc.path.clone())).collect()
    }

    /// Paths a new file may not take: every node not yet gone from disk.
    pub fn taken_paths(&self) -> HashSet<String> {
        let mut s = HashSet::new();
        for n in self.nodes.iter().filter(|n| !n.deleted) {
            s.insert(n.doc.path.clone());
            if !n.old_path.is_empty() {
                s.insert(n.old_path.clone());
            }
        }
        s
    }

    /// Every edge the live nodes declare (§2.4), deduped.
    pub fn edges(&self) -> Vec<Edge> {
        let pairs = self.pairs();
        let mut set: BTreeSet<Edge> = BTreeSet::new();
        for n in self.live() {
            set.extend(nf::edges_of(&n.doc, &pairs));
        }
        set.into_iter().collect()
    }

    pub fn project_node(&self) -> Option<&StoredNode> {
        let mut ps: Vec<&StoredNode> = self.live().filter(|n| n.doc.nodetype == NodeType::Project).collect();
        // a node that came from the repo outranks a designed one
        ps.sort_by_key(|n| (n.file_version == 0, n.doc.path.clone()));
        ps.into_iter().next()
    }

    pub fn naming(&self) -> nf::Naming {
        let s = self.project_node().and_then(|p| p.doc.front.get("file_naming")).and_then(|v| v.as_str()).unwrap_or("");
        nf::Naming::from_setting(s)
    }

    /// The file_state a server edit leaves behind.
    pub fn edit_state(&self, current: &str) -> &'static str {
        if current == DESIGNED || (current.is_empty() && !self.served) { DESIGNED } else { PENDING_WRITE }
    }

    /// Add a brand-new node (already planned + canonical).
    pub fn insert_new(&mut self, doc: NodeDoc, by: &str, change: &str) -> &StoredNode {
        let state = self.edit_state("");
        let n = StoredNode {
            doc,
            project: self.project.clone(),
            node_version: 1,
            file_version: 0,
            file_hash: String::new(),
            file_state: state.to_string(),
            deleted: false,
            test: Value::Null,
            updated_by: by.to_string(),
            updated: iter_core::now_utc(),
            old_path: String::new(),
            change: change.to_string(),
            seeded: false,
            documents: Vec::new(),
            last_commit: String::new(),
        };
        self.push(n)
    }

    pub fn push(&mut self, n: StoredNode) -> &StoredNode {
        let id = n.doc.id.clone();
        if let Some(i) = self.idx.get(&id).copied() {
            self.nodes[i] = n;
        } else {
            self.nodes.push(n);
            self.idx.insert(id.clone(), self.nodes.len() - 1);
        }
        self.gone.remove(&id);
        self.dirty.insert(id.clone());
        self.get(&id).unwrap()
    }

    /// Remove a node's document outright (never written to a file).
    pub fn purge(&mut self, id: &str) {
        if let Some(i) = self.idx.get(id).copied() {
            self.nodes.remove(i);
            self.reindex();
            self.dirty.remove(id);
            self.gone.insert(id.to_string());
        }
    }

    /// Record a server-side edit of `doc` on node `id`: conform, bump the
    /// version, mark it for writing.
    pub fn commit_edit(&mut self, id: &str, mut doc: NodeDoc, by: &str, change: &str, bump_modified: bool) -> Result<(), ApiError> {
        let now = nf::now_ts();
        let served = self.served;
        let n = self.get_mut(id).ok_or_else(|| not_found(format!("no node {id}")))?;
        if bump_modified {
            doc.timestamps.last_modified = now.clone();
        }
        doc.id = n.doc.id.clone();
        let creator = doc.creator.clone();
        let doc = canonical(&doc, &now, &creator);
        n.doc = doc;
        n.node_version += 1;
        n.file_state = if n.file_state == DESIGNED || (n.file_state.is_empty() && !served) { DESIGNED.into() } else { PENDING_WRITE.into() };
        n.updated_by = by.to_string();
        n.updated = iter_core::now_utc();
        n.change = change.to_string();
        Ok(())
    }

    /// Add one edge to the node that owns it (§2.4). No-op when an existing
    /// entry already covers it. Returns the owner id when it changed.
    pub fn add_edge(&mut self, from: &str, to: &str, kind: EdgeKind, by: &str) -> Result<Option<String>, ApiError> {
        let (f, t) = (self.live_node(from)?.clone(), self.live_node(to)?.clone());
        if from == to {
            return Err(bad("an edge needs two different nodes"));
        }
        let (owner, target) = if nf::owner_is_target(kind) { (&t, &f) } else { (&f, &t) };
        // drives may be written on either side; an actor → usecase drives edge
        // lives on the actor unless the caller made the usecase the owner
        let mut doc = owner.doc.clone();
        nf::add_child(&mut doc, kind, &target.doc.path).map_err(|e| bad(e.to_string()))?;
        let want = Edge { from: from.to_string(), kind, to: to.to_string() };
        if !nf::edges_of(&doc, &self.pairs()).contains(&want) {
            return Err(bad(format!(
                "a {} edge cannot go from a {} node to a {} node",
                kind.as_str(),
                type_label(&f.doc),
                type_label(&t.doc)
            )));
        }
        if doc == owner.doc {
            return Ok(None);
        }
        let oid = owner.doc.id.clone();
        let change = format!("{} {} → {}", kind.as_str(), f.doc.name, t.doc.name);
        self.commit_edit(&oid, doc, by, &change, true)?;
        Ok(Some(oid))
    }

    /// Remove one edge from every node that declares it. GlobOnly → 409.
    pub fn remove_edge(&mut self, from: &str, to: &str, kind: EdgeKind, by: &str, reason: &str) -> Result<Vec<String>, ApiError> {
        let (f, t) = (self.live_node(from)?.clone(), self.live_node(to)?.clone());
        let mut owners: Vec<(&StoredNode, &StoredNode)> = vec![if nf::owner_is_target(kind) { (&t, &f) } else { (&f, &t) }];
        if kind == EdgeKind::Drives {
            owners.push((&t, &f));
        }
        let mut changed: Vec<(String, NodeDoc)> = Vec::new();
        for (owner, target) in owners {
            let mut doc = owner.doc.clone();
            match nf::remove_child(&mut doc, kind, &target.doc.path) {
                Ok(()) => changed.push((owner.doc.id.clone(), doc)),
                Err(NodeErr::NoSuchChild(_)) | Err(NodeErr::WrongKind { .. }) => {}
                Err(NodeErr::GlobOnly { entry, target }) => {
                    return Err(ApiError::Refused(
                        StatusCode::CONFLICT,
                        json!({"error": format!("{target} is matched by the pattern {entry:?} in {}: edit or remove that pattern instead (removing one exact entry would leave the edge in place)", owner.doc.path),
                               "refused": "glob", "entry": entry, "target": target, "owner": owner.doc.id}),
                    ));
                }
                Err(e) => return Err(bad(e.to_string())),
            }
        }
        if changed.is_empty() {
            return Err(not_found(format!("no {} edge {} → {} is declared", kind.as_str(), f.doc.name, t.doc.name)));
        }
        let change = format!("remove {} {} → {}: {}", kind.as_str(), f.doc.name, t.doc.name, reason);
        let mut out = Vec::new();
        for (id, doc) in changed {
            self.commit_edit(&id, doc, by, &change, true)?;
            out.push(id);
        }
        Ok(out)
    }

    /// Delete a node: pending_delete (removed outright when it never reached
    /// a file), and dropped from every other node's children entries that
    /// name it exactly. Returns the touched ids and entries kept by a glob.
    pub fn delete_node(&mut self, id: &str, by: &str, reason: &str) -> Result<(Vec<String>, Vec<Value>), ApiError> {
        let n = self.live_node(id)?.clone();
        if n.doc.nodetype == NodeType::Project {
            return Err(bad("the project node cannot be deleted"));
        }
        let mut touched = Vec::new();
        let mut kept = Vec::new();
        for e in self.edges().into_iter().filter(|e| e.from == id || e.to == id) {
            let mut owners: Vec<&str> = vec![if nf::owner_is_target(e.kind) { e.to.as_str() } else { e.from.as_str() }];
            if e.kind == EdgeKind::Drives {
                owners = vec![e.from.as_str(), e.to.as_str()];
            }
            for oid in owners.into_iter().filter(|o| *o != id) {
                let Some(owner) = self.get(oid).cloned() else { continue };
                let mut doc = owner.doc.clone();
                match nf::remove_child(&mut doc, e.kind, &n.doc.path) {
                    Ok(()) => {
                        let change = format!("drop {} {} (deleted: {reason})", e.kind.as_str(), n.doc.name);
                        self.commit_edit(oid, doc, by, &change, true)?;
                        touched.push(oid.to_string());
                    }
                    Err(NodeErr::GlobOnly { entry, .. }) => kept.push(json!({"owner": oid, "entry": entry})),
                    Err(_) => {}
                }
            }
        }
        if n.file_version == 0 {
            self.purge(id);
        } else {
            let x = self.get_mut(id).unwrap();
            x.file_state = PENDING_DELETE.into();
            x.node_version += 1;
            x.updated_by = by.to_string();
            x.updated = iter_core::now_utc();
            x.change = format!("delete {}: {reason}", n.doc.name);
        }
        touched.sort();
        touched.dedup();
        Ok((touched, kept))
    }

    /// Move a node's file (an explicit move op): exact references to the old
    /// path in other nodes are rewritten.
    pub fn move_node(&mut self, id: &str, new_path: &str, by: &str) -> Result<Vec<String>, ApiError> {
        let n = self.live_node(id)?.clone();
        let new_path = new_path.trim().to_string();
        check_node_path(&new_path, n.doc.nodetype)?;
        if new_path == n.doc.path {
            return Ok(Vec::new());
        }
        if self.taken_paths().contains(&new_path) {
            return Err(status(StatusCode::CONFLICT, format!("{new_path} is taken by another node")));
        }
        let old = n.doc.path.clone();
        let mut touched = Vec::new();
        let others: Vec<StoredNode> = self.live().filter(|x| x.doc.id != id).cloned().collect();
        for o in others {
            let mut doc = o.doc.clone();
            if rewrite_refs(&mut doc, &old, &new_path) {
                self.commit_edit(&o.doc.id, doc, by, &format!("follow move of {}", n.doc.name), true)?;
                touched.push(o.doc.id.clone());
            }
        }
        let mut doc = n.doc.clone();
        doc.path = new_path.clone();
        self.commit_edit(id, doc, by, &format!("move {old} → {new_path}"), false)?;
        let x = self.get_mut(id).unwrap();
        if x.file_version > 0 && x.old_path.is_empty() {
            x.old_path = old;
        }
        if x.old_path == x.doc.path {
            x.old_path.clear();
        }
        touched.push(id.to_string());
        Ok(touched)
    }

    /// Write the changed nodes and the edge difference in one transaction.
    pub async fn save(&mut self, store: &dyn Storage) -> Result<Saved, ApiError> {
        let a = arango(store)?;
        let project = self.project.clone();
        // req ids are unique per project: a later file's copy gets a new id
        self.repair_req_ids("iter_data")?;
        self.rederive_reqs();
        let mut docs = Vec::new();
        for id in &self.dirty {
            let Some(n) = self.get(id) else { continue };
            let mut n = n.clone();
            n.project = project.clone();
            n.documents = documents_of(&n.doc);
            let mut d = n.json();
            d["_key"] = json!(node_key(&project, id));
            docs.push(d);
        }
        let gone: Vec<String> = self
            .gone
            .iter()
            .map(|g| g.strip_prefix("key:").map(String::from).unwrap_or_else(|| node_key(&project, g)))
            .collect();
        let new_reqs: HashMap<String, Value> = self.reqs.iter().map(|r| (r.id.clone(), r.json())).collect();
        for (id, v) in &new_reqs {
            if self.old_reqs.get(id) != Some(v) {
                let mut d = v.clone();
                d["_key"] = json!(node_key(&project, id));
                docs.push(d);
            }
        }
        let mut gone = gone;
        gone.extend(self.old_reqs.keys().filter(|id| !new_reqs.contains_key(*id)).map(|id| node_key(&project, id)));
        let mut new_links: HashSet<(String, String, String)> =
            self.edges().into_iter().map(|e| (e.from, e.kind.as_str().to_string(), e.to)).collect();
        new_links.extend(self.contains_edges().into_iter().map(|(f, t)| (f, crate::reqs::CONTAINS.to_string(), t)));
        let removed: Vec<String> =
            self.old_links.difference(&new_links).map(|(f, k, t)| link_key(&project, f, k, t)).collect();
        let added: Vec<Value> = new_links
            .difference(&self.old_links)
            .map(|(f, k, t)| {
                json!({"_key": link_key(&project, f, k, t), "_from": format!("{NODE_COLL}/{}", node_key(&project, f)),
                       "_to": format!("{NODE_COLL}/{}", node_key(&project, t)), "project": project, "from": f, "kind": k, "to": t})
            })
            .collect();
        let saved = Saved { edges: new_links.len(), edges_added: added.len(), edges_removed: removed.len() };
        if docs.is_empty() && gone.is_empty() && removed.is_empty() && added.is_empty() {
            return Ok(saved);
        }
        let trx = a.begin(&[NODE_COLL, LINK_COLL]).await.map_err(backend)?;
        let run = async {
            if !gone.is_empty() {
                a.aql_in(&trx, "FOR k IN @ks REMOVE {_key: k} IN node OPTIONS {ignoreErrors: true}", json!({"ks": gone})).await?;
            }
            if !docs.is_empty() {
                a.aql_in(&trx, "FOR d IN @ds INSERT d INTO node OPTIONS {overwriteMode: 'replace'}", json!({"ds": docs})).await?;
            }
            if !removed.is_empty() {
                a.aql_in(&trx, "FOR k IN @ks REMOVE {_key: k} IN link OPTIONS {ignoreErrors: true}", json!({"ks": removed})).await?;
            }
            if !added.is_empty() {
                a.aql_in(&trx, "FOR d IN @ds INSERT d INTO link OPTIONS {overwriteMode: 'replace'}", json!({"ds": added})).await?;
            }
            Ok::<(), ArangoErr>(())
        };
        match run.await {
            Ok(()) => a.commit(&trx).await.map_err(backend)?,
            Err(e) => {
                a.abort(&trx).await;
                return Err(backend(e));
            }
        }
        self.old_links = new_links;
        self.old_reqs = new_reqs;
        self.dirty.clear();
        self.gone.clear();
        store.bump_seq(&project, "graph").await?;
        Ok(saved)
    }
}

/// `code/context` for code nodes, the nodetype otherwise.
pub fn type_label(d: &NodeDoc) -> String {
    match (&d.nodetype, &d.level) {
        (NodeType::Code, Some(l)) => format!("code/{l}"),
        (t, _) => t.as_str().to_string(),
    }
}

/// A node path the designer may use: `{topdir}/…/<name>.<type>.iter.md`,
/// inside the project, of this node's type.
pub fn check_node_path(path: &str, t: NodeType) -> Result<(), ApiError> {
    if !path.starts_with("{topdir}/") || path.split('/').any(|s| s == ".." || s.is_empty()) {
        return Err(bad(format!("{path:?}: a node path is {{topdir}}/…/<name>.<type>.iter.md, inside the project")));
    }
    if nf::type_of(path) != Some(t) || nf::is_legacy_filename(path) {
        return Err(bad(format!("{path:?} is not a {} node filename (…{}.iter.md)", t.as_str(), t.as_str())));
    }
    Ok(())
}

/// Rewrite every children / front entry that names `old` exactly.
fn rewrite_refs(doc: &mut NodeDoc, old: &str, new: &str) -> bool {
    let this = doc.path.clone();
    let fix = |list: &mut Vec<String>| -> bool {
        let mut hit = false;
        for e in list.iter_mut() {
            if nf::expand_entry(e, &this, None).trim_end_matches('/') == old {
                *e = new.to_string();
                hit = true;
            }
        }
        hit
    };
    let mut hit = false;
    hit |= fix(&mut doc.children.codenodes);
    hit |= fix(&mut doc.children.tests);
    hit |= fix(&mut doc.children.reqs);
    let fix_value = |v: &mut Value| -> bool {
        let Some(arr) = v.as_array_mut() else { return false };
        let mut hit = false;
        for x in arr.iter_mut() {
            if let Some(s) = x.as_str() {
                if nf::expand_entry(s, &this, None).trim_end_matches('/') == old {
                    *x = Value::String(new.to_string());
                    hit = true;
                }
            }
        }
        hit
    };
    for k in ["drives", "touches", "actors"] {
        if let Some(v) = doc.front.get_mut(k) {
            hit |= fix_value(v);
        }
    }
    if let Some(Value::Object(c)) = doc.front.get_mut("connects") {
        for side in ["from", "to"] {
            if let Some(v) = c.get_mut(side) {
                hit |= fix_value(v);
            }
        }
    }
    hit
}

/// `children.documents` (GraphRAG links) as `{topdir}/…` paths.
pub fn documents_of(doc: &NodeDoc) -> Vec<String> {
    doc.children
        .extra
        .get("documents")
        .map(|l| l.iter().map(|e| nf::expand_entry(e, &doc.path, None)).collect())
        .unwrap_or_default()
}

// ---------- the project node (designer, §3.4) ----------

/// Make sure a project that has no repo yet has its project node and the
/// default global requirements (one philosophy, bizreq, techreq — empty
/// bodies, attached through the project node's `children.reqs`). A project
/// with an active serves edge gets its project node from the repo's file
/// sync instead, so nothing is created for it. Returns true when it created.
pub async fn ensure_project_node(store: &dyn Storage, project: &str) -> Result<bool, ApiError> {
    if store.arango().is_none() {
        return Ok(false);
    }
    let _g = lock(project).await;
    let mut g = Graph::load(store, project).await?;
    if g.project_node().is_some() || g.served {
        return Ok(false);
    }
    let rec = store.get("project", project, crate::api::NOSK).await?;
    let by = "iter_data";
    let now = nf::now_ts();
    let mut pdoc = NodeDoc::new(NodeType::Project, project, by, &now);
    pdoc.desc = rec.as_ref().and_then(|r| r.get("desc")).and_then(|d| d.as_str()).unwrap_or("").to_string();
    pdoc.path = nf::plan_path(&pdoc, None, nf::Attach::Root, &g.taken_paths(), nf::Naming::Sequence);
    let pdoc = canonical(&pdoc, &now, by);
    let pid = pdoc.id.clone();
    g.insert_new(pdoc, by, "project created");
    for (t, name) in [(NodeType::Philosophy, "Philosophy"), (NodeType::Bizreq, "Business requirements"), (NodeType::Techreq, "Technical requirements")] {
        let parent = g.get(&pid).unwrap().doc.clone();
        let mut d = NodeDoc::new(t, name, by, &now);
        d.path = nf::plan_path(&d, Some(&parent), nf::Attach::Under(EdgeKind::Reqs), &g.taken_paths(), g.naming());
        let d = canonical(&d, &now, by);
        let id = d.id.clone();
        g.insert_new(d, by, "default global requirement");
        g.add_edge(&pid, &id, EdgeKind::Reqs, by)?;
    }
    // the project node and its defaults are seeds: version 1 again, so a
    // repo's own project node can replace them untouched
    let ids: Vec<String> = g.live().map(|n| n.doc.id.clone()).collect();
    for id in ids {
        let n = g.get_mut(&id).unwrap();
        n.seeded = true;
        n.node_version = 1;
    }
    g.save(store).await?;
    Ok(true)
}

/// A project was deleted: remove its whole project graph — nodes, derived
/// links, sync conflicts and test logs — so re-creating a project of the same
/// name starts empty (before 2026-10-02 the rows stayed and a re-created
/// project showed two project nodes). Returns how many documents went.
pub async fn purge_project(store: &dyn Storage, project: &str) -> Result<usize, ApiError> {
    let Some(a) = store.arango() else { return Ok(0) };
    let _g = lock(project).await;
    let mut n = 0;
    for coll in [LINK_COLL, NODE_COLL, CONFLICT_COLL, TEST_LOG_COLL] {
        let q = format!("FOR d IN {coll} FILTER d.project == @p REMOVE d IN {coll} RETURN 1");
        n += a.aql_retry(&q, json!({"p": project})).await.map_err(backend)?.len();
    }
    Ok(n)
}

/// Counts used by stats and the build status.
pub fn state_counts(g: &Graph) -> Value {
    let mut c: HashMap<String, usize> = HashMap::new();
    for n in g.nodes.iter().filter(|n| !n.deleted) {
        *c.entry(n.file_state.clone()).or_default() += 1;
    }
    json!(c)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn refs_follow_a_move() {
        let mut d = NodeDoc::new(NodeType::Code, "a", "t", "2026-10-02 00:00:00Z");
        d.path = "{topdir}/src/a/a.code.iter.md".into();
        d.children.codenodes = vec!["{topdir}/src/b/b.code.iter.md".into(), "{topdir}/src/**/*.code.iter.md".into(), "../b/b.code.iter.md".into()];
        assert!(rewrite_refs(&mut d, "{topdir}/src/b/b.code.iter.md", "{topdir}/x/b.code.iter.md"));
        assert_eq!(d.children.codenodes[0], "{topdir}/x/b.code.iter.md");
        assert_eq!(d.children.codenodes[1], "{topdir}/src/**/*.code.iter.md", "globs untouched");
        assert_eq!(d.children.codenodes[2], "{topdir}/x/b.code.iter.md", "relative exact entries too");
    }

    #[test]
    fn node_paths_are_checked() {
        assert!(check_node_path("{topdir}/src/a/a.code.iter.md", NodeType::Code).is_ok());
        assert!(check_node_path("{topdir}/../a.code.iter.md", NodeType::Code).is_err());
        assert!(check_node_path("/etc/a.code.iter.md", NodeType::Code).is_err());
        assert!(check_node_path("{topdir}/a.test.iter.md", NodeType::Code).is_err());
    }

    #[test]
    fn stored_node_round_trips_through_json() {
        let mut d = NodeDoc::new(NodeType::Code, "a", "t", "2026-10-02 00:00:00Z");
        d.path = "{topdir}/src/a/a.code.iter.md".into();
        d.children.extra.insert("documents".into(), vec!["{topdir}/docs/x.pdf".into()]);
        let n = StoredNode {
            doc: d.clone(), project: "p".into(), node_version: 3, file_version: 2, file_hash: "h".into(), file_state: PENDING_WRITE.into(),
            deleted: false, test: Value::Null, updated_by: "u".into(), updated: "".into(), old_path: "".into(), change: "".into(),
            seeded: false, documents: vec![], last_commit: "".into(),
        };
        let mut v = n.json();
        v["_key"] = json!("k");
        let back: StoredNode = serde_json::from_value(v).unwrap();
        assert_eq!(back.doc, d);
        assert_eq!((back.node_version, back.file_version), (3, 2));
        assert_eq!(documents_of(&back.doc), vec!["{topdir}/docs/x.pdf".to_string()]);
    }
}
