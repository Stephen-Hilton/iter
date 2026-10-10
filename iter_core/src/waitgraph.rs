//! Lease-bound locks and the wait-for graph (CR 2026-09-25, "locks held by
//! items that are not running, and the wait cycle they closed").
//!
//! The incident: a queued item still held 42 lock rows (an orphaned shell
//! loop its agent had started kept re-acquiring them), the dispatcher
//! counted those rows as live, and a lock edge closed a loop with two
//! dependency edges — four P0 items stalled for 5 h 43 min with nothing on
//! any record saying why.
//!
//! The fix has two halves, both pure and shared by iter_data and the engine:
//! - a lock is tied to a RUN: the claim writes a random `lease` on the item,
//!   every exit from the run clears it, and a lock row counts only while its
//!   lease is its holder's current lease (`live_lock_rows`, `lock_grant`);
//! - a wait-for graph over every open item (`wait_edges`) with a cycle
//!   detector (`find_wait_cycles`) and a write-time check that follows deep
//!   (`createdby`) edges as well as declared ones (`dependency_cycle_on_write`).
//!
//! Why no cycle can contain a lock edge once leases are enforced: a lock edge
//! ends at a holder with a live lease, a live lease exists only while a run
//! is live, and only QUEUED items have outgoing edges — so nothing leaves the
//! holder and no loop can pass through it.

use crate::{LockRow, WorkItem, children_index, deep_waits, paths_overlap};
use serde::Serialize;
use std::collections::{HashMap, HashSet, VecDeque};

/// A lock row lives this long unless the engine running its holder renews it.
pub const LOCK_LEASE_TTL_SEC: i64 = 600;
/// How often the engine renews the rows of the runs its threads execute.
pub const LOCK_RENEW_EVERY_SEC: u64 = 60;

/// The storage key of a row in the lock table.  A lock is keyed by its path;
/// a reservation by `reserve:<path>` (2026-09-25: the two used to share one
/// row per path, so a reservation made a strictly-better item's lock on the
/// same path fail until the reservation lapsed).  `path` stays in the body.
pub fn lock_sk(kind: &str, path: &str) -> String {
    if kind == "reserve" { format!("reserve:{path}") } else { path.to_string() }
}

/// Open = still part of the queue's life (not closed, not a template).
pub fn is_open(state: &str) -> bool {
    matches!(state, "queued" | "question" | "parked" | "paused" | "in-progress")
}

/// Does this lock row count?  Only an unexpired `lock` row whose holder
/// exists, is not queued, and carries the row's lease.  A queued holder never
/// counts: it has outgoing waits of its own, and counting its rows is exactly
/// the 2026-09-25 cycle.  Always enforced in iter4 (decided 2026-09-28: the
/// rollout switch `lock_lease_enforce` is gone — only iter4 engines talk to
/// an iter4 server, so every row an engine writes carries a lease).
pub fn lock_row_is_live(row: &LockRow, holder: Option<&WorkItem>, now_iso: &str) -> bool {
    if row.kind != "lock" || !(row.expires.is_empty() || row.expires.as_str() >= now_iso) {
        return false;
    }
    let Some(h) = holder else { return false };
    if h.state == "queued" {
        return false;
    }
    !h.lease.is_empty() && row.lease == h.lease
}

/// The lock rows dispatch honours (see `lock_row_is_live`).
pub fn live_lock_rows<'a>(rows: &'a [LockRow], by_id: &HashMap<String, &WorkItem>, now_iso: &str) -> Vec<&'a LockRow> {
    rows.iter().filter(|r| lock_row_is_live(r, by_id.get(&r.workid).copied(), now_iso)).collect()
}

/// The holders whose live rows overlap this item's lockdirs: (holder workid,
/// the locked path), one entry per holder.  Moved here from the engine's
/// dispatch so the server's /deadlocks and the tests share it.
pub fn lock_holders(item: &WorkItem, live: &[&LockRow]) -> Vec<(String, String)> {
    let mut v: Vec<(String, String)> = Vec::new();
    for d in &item.lockdirs {
        for r in live {
            if r.workid != item.id && paths_overlap(d, &r.path) && !v.iter().any(|(w, _)| w == &r.workid) {
                v.push((r.workid.clone(), r.path.clone()));
            }
        }
    }
    v
}

/// What iter_data does with a lock or reservation request for `holder`.
#[derive(Debug, Clone, PartialEq)]
pub enum LockGrant {
    /// write the row stamped with this lease
    Grant { lease: String },
    /// 409: `refused` is the machine word, `msg` the sentence for a human
    Refuse { refused: &'static str, msg: String },
}

/// The grant rule (CR 6.2 item 3).  A lock goes only to an item with a live
/// lease, stamped with that lease; a request naming a different lease is a
/// stale run.  A reservation goes only to a queued item.
pub fn lock_grant(kind: &str, holder: &WorkItem, req_lease: &str) -> LockGrant {
    let id12 = &holder.id[holder.id.len().saturating_sub(12)..];
    if kind == "reserve" {
        if holder.state != "queued" {
            return LockGrant::Refuse {
                refused: "not queued",
                msg: format!("refused: a reservation is held only by a queued item; {id12} is {}", holder.state),
            };
        }
        return LockGrant::Grant { lease: String::new() };
    }
    let not_running = format!(
        "refused: work item {id12} is not running (state {}); locks are held only by a running item, and the engine releases them when the run ends",
        holder.state
    );
    if holder.lease.is_empty() {
        return LockGrant::Refuse { refused: "not running", msg: not_running };
    }
    if !req_lease.is_empty() && req_lease != holder.lease {
        return LockGrant::Refuse {
            refused: "stale lease",
            msg: format!("refused: lease {} is not the live lease of {id12} (a run that has ended)", &req_lease[..8.min(req_lease.len())]),
        };
    }
    LockGrant::Grant { lease: holder.lease.clone() }
}

/// Why the lock sweep removes this row, or None to keep it.  A lock row goes
/// when it would not count (`lock_row_is_live`); a reservation goes when its
/// holder is missing or no longer queued.
pub fn sweep_reason(row: &LockRow, holder: Option<&WorkItem>, now_iso: &str) -> Option<String> {
    if row.kind == "reserve" {
        return match holder {
            None => Some("reservation holder no longer exists".into()),
            Some(h) if h.state != "queued" => Some(format!("reservation holder is {}", h.state)),
            _ => None,
        };
    }
    if lock_row_is_live(row, holder, now_iso) {
        return None;
    }
    Some(match holder {
        _ if !row.expires.is_empty() && row.expires.as_str() < now_iso => "expired".into(),
        None => "holder no longer exists".into(),
        Some(h) if h.lease.is_empty() || h.state == "queued" => format!("holder is not running (state {})", h.state),
        Some(_) => "the row's lease is not the holder's live lease".into(),
    })
}

/// One edge of the wait-for graph: `from` cannot start until `to` has.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum WaitKind {
    /// `to` is in `from.blockedby` and still open
    Blocker,
    /// `to` is an open follow-up (by `createdby`) of `via`, a complete blocker of `from`
    Deep { via: String },
    /// `to` holds a live lock row on `path`, which overlaps `from`'s lockdirs
    Lock { path: String },
}

impl WaitKind {
    pub fn label(&self) -> &'static str {
        match self {
            WaitKind::Blocker => "blocker",
            WaitKind::Deep { .. } => "deep",
            WaitKind::Lock { .. } => "lock",
        }
    }
    fn rank(&self) -> (u8, &str) {
        match self {
            WaitKind::Blocker => (0, ""),
            WaitKind::Deep { via } => (1, via.as_str()),
            WaitKind::Lock { path } => (2, path.as_str()),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct WaitEdge {
    pub from: String,
    pub to: String,
    #[serde(flatten)]
    pub kind: WaitKind,
}

/// Every edge out of the items `source` accepts, in a fixed order.
fn edges_from(items: &[WorkItem], live: &[&LockRow], source: impl Fn(&WorkItem) -> bool) -> Vec<WaitEdge> {
    let by_id: HashMap<String, &WorkItem> = items.iter().map(|i| (i.id.clone(), i)).collect();
    let kids = children_index(items);
    let mut out: Vec<WaitEdge> = Vec::new();
    for x in items.iter().filter(|i| source(i)) {
        let mut seen: HashSet<(String, u8)> = HashSet::new();
        for b in &x.blockedby {
            if by_id.get(b).map(|bi| is_open(&bi.state)).unwrap_or(false) && seen.insert((b.clone(), 0)) {
                out.push(WaitEdge { from: x.id.clone(), to: b.clone(), kind: WaitKind::Blocker });
            }
        }
        for (c, via) in deep_waits(x, &by_id, &kids) {
            if seen.insert((c.clone(), 1)) {
                out.push(WaitEdge { from: x.id.clone(), to: c, kind: WaitKind::Deep { via } });
            }
        }
        for (h, path) in lock_holders(x, live) {
            out.push(WaitEdge { from: x.id.clone(), to: h, kind: WaitKind::Lock { path } });
        }
    }
    out.sort_by(|a, b| (a.from.as_str(), a.to.as_str(), a.kind.rank()).cmp(&(b.from.as_str(), b.to.as_str(), b.kind.rank())));
    out
}

/// The wait-for graph: every edge out of every QUEUED item — only queued
/// items are waiting to be dispatched (CR 5.1).  `live` comes from
/// `live_lock_rows`.
pub fn wait_edges(items: &[WorkItem], live: &[&LockRow]) -> Vec<WaitEdge> {
    edges_from(items, live, |i| i.state == "queued")
}

/// The shortest loop from `start` back to itself, walking edges in their
/// (sorted) order and staying inside `within` — breadth-first, so the result
/// is the canonical cycle of CR 5.5.
fn shortest_cycle(adj: &HashMap<&str, Vec<&WaitEdge>>, start: &str, within: &dyn Fn(&str) -> bool) -> Option<Vec<WaitEdge>> {
    let mut parent: HashMap<&str, &WaitEdge> = HashMap::new();
    let mut queue: VecDeque<&str> = VecDeque::from([start]);
    let mut visited: HashSet<&str> = HashSet::from([start]);
    while let Some(u) = queue.pop_front() {
        for e in adj.get(u).map(|v| v.as_slice()).unwrap_or(&[]) {
            if e.to == start {
                let mut path: Vec<WaitEdge> = vec![(*e).clone()];
                let mut cur = u;
                while cur != start {
                    let pe = parent[cur];
                    path.push(pe.clone());
                    cur = pe.from.as_str();
                }
                path.reverse();
                return Some(path);
            }
            if within(&e.to) && visited.insert(e.to.as_str()) {
                parent.insert(e.to.as_str(), e);
                queue.push_back(e.to.as_str());
            }
        }
    }
    None
}

/// Every deadlock in the graph: one canonical cycle per strongly connected
/// component with more than one member (or a member waiting on itself),
/// starting at the component's smallest id; the list is sorted by that id,
/// so the same graph always gives byte-identical output.  Tarjan's
/// algorithm, iterative — pdy-dev has thousands of items.
pub fn find_wait_cycles(edges: &[WaitEdge]) -> Vec<Vec<WaitEdge>> {
    let mut sorted: Vec<&WaitEdge> = edges.iter().collect();
    sorted.sort_by(|a, b| (a.from.as_str(), a.to.as_str(), a.kind.rank()).cmp(&(b.from.as_str(), b.to.as_str(), b.kind.rank())));
    let mut names: Vec<&str> = sorted.iter().flat_map(|e| [e.from.as_str(), e.to.as_str()]).collect();
    names.sort();
    names.dedup();
    let idx: HashMap<&str, usize> = names.iter().enumerate().map(|(i, n)| (*n, i)).collect();
    let mut adj_i: Vec<Vec<usize>> = vec![Vec::new(); names.len()];
    let mut adj: HashMap<&str, Vec<&WaitEdge>> = HashMap::new();
    for e in &sorted {
        adj_i[idx[e.from.as_str()]].push(idx[e.to.as_str()]);
        adj.entry(e.from.as_str()).or_default().push(e);
    }
    // iterative Tarjan
    let n = names.len();
    let mut index: Vec<Option<usize>> = vec![None; n];
    let mut low: Vec<usize> = vec![0; n];
    let mut on_stack: Vec<bool> = vec![false; n];
    let mut stack: Vec<usize> = Vec::new();
    let mut next = 0usize;
    let mut sccs: Vec<Vec<usize>> = Vec::new();
    for root in 0..n {
        if index[root].is_some() {
            continue;
        }
        let mut call: Vec<(usize, usize)> = vec![(root, 0)];
        index[root] = Some(next);
        low[root] = next;
        next += 1;
        stack.push(root);
        on_stack[root] = true;
        while let Some(&(u, pos)) = call.last() {
            if pos < adj_i[u].len() {
                call.last_mut().unwrap().1 += 1;
                let w = adj_i[u][pos];
                match index[w] {
                    None => {
                        index[w] = Some(next);
                        low[w] = next;
                        next += 1;
                        stack.push(w);
                        on_stack[w] = true;
                        call.push((w, 0));
                    }
                    Some(iw) if on_stack[w] => low[u] = low[u].min(iw),
                    _ => {}
                }
            } else {
                call.pop();
                if let Some(&(p, _)) = call.last() {
                    low[p] = low[p].min(low[u]);
                }
                if Some(low[u]) == index[u] {
                    let mut comp: Vec<usize> = Vec::new();
                    while let Some(w) = stack.pop() {
                        on_stack[w] = false;
                        comp.push(w);
                        if w == u {
                            break;
                        }
                    }
                    sccs.push(comp);
                }
            }
        }
    }
    let mut cycles: Vec<Vec<WaitEdge>> = Vec::new();
    for comp in sccs {
        let members: HashSet<&str> = comp.iter().map(|i| names[*i]).collect();
        let start = *members.iter().min().unwrap();
        let self_loop = adj.get(start).map(|v| v.iter().any(|e| e.to == start)).unwrap_or(false);
        if members.len() < 2 && !self_loop {
            continue;
        }
        if let Some(c) = shortest_cycle(&adj, start, &|x: &str| members.contains(x)) {
            cycles.push(c);
        }
    }
    cycles.sort_by(|a, b| a[0].from.cmp(&b[0].from));
    cycles
}

/// The cycle a proposed `blockedby` would close, over declared AND deep
/// edges (CR option (c), extended): `item` carries the proposed links and is
/// treated as queued — it will be the moment the link matters.  Every open
/// item is a source here, not only queued ones: a parked or in-progress item
/// with links goes back to queued later, and the loop with it.  Returns the
/// path from `item` back to itself, or None.
pub fn dependency_cycle_on_write(item: &WorkItem, items: &[WorkItem]) -> Option<Vec<WaitEdge>> {
    let mut all: Vec<WorkItem> = items.iter().filter(|i| i.id != item.id).cloned().collect();
    let mut me = item.clone();
    me.state = "queued".into();
    all.push(me);
    let edges = edges_from(&all, &[], |i| is_open(&i.state));
    let mut adj: HashMap<&str, Vec<&WaitEdge>> = HashMap::new();
    for e in &edges {
        adj.entry(e.from.as_str()).or_default().push(e);
    }
    shortest_cycle(&adj, &item.id, &|_| true)
}

/// "a054fa190fb2 → ed93b98a2679 → 63491afb13c0 → a054fa190fb2 (lock, blocker, blocker)"
/// — the text of the `blocked by: deadlock: …` tag and the log lines.
pub fn describe_cycle(cycle: &[WaitEdge]) -> String {
    let s = |x: &str| x[x.len().saturating_sub(12)..].to_string();
    let mut ids: Vec<String> = cycle.iter().map(|e| s(&e.from)).collect();
    if let Some(first) = cycle.first() {
        ids.push(s(&first.from));
    }
    let kinds: Vec<&str> = cycle.iter().map(|e| e.kind.label()).collect();
    format!("{} ({})", ids.join(" → "), kinds.join(", "))
}

/// True when the cycle runs through a lock held by an item that is not
/// running — possible only through a bug or a row from before leases — so
/// the engine may break it by releasing that holder's locks (CR 5.5).
pub fn auto_resolvable(cycle: &[WaitEdge], by_id: &HashMap<String, &WorkItem>) -> bool {
    !not_running_lock_holders(cycle, by_id).is_empty()
}

/// The holders of lock edges in `cycle` that are not running (no lease, not
/// in progress): the rows the engine releases to break the loop.
pub fn not_running_lock_holders(cycle: &[WaitEdge], by_id: &HashMap<String, &WorkItem>) -> Vec<String> {
    let mut v: Vec<String> = Vec::new();
    for e in cycle {
        if matches!(e.kind, WaitKind::Lock { .. }) {
            let running = by_id.get(&e.to).map(|h| !h.lease.is_empty() || h.state == "in-progress").unwrap_or(false);
            if !running && !v.contains(&e.to) {
                v.push(e.to.clone());
            }
        }
    }
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wi(id: &str, state: &str, createdby: &str, blockedby: &[&str]) -> WorkItem {
        WorkItem { id: id.into(), state: state.into(), createdby: createdby.into(),
            blockedby: blockedby.iter().map(|s| s.to_string()).collect(), ..Default::default() }
    }
    fn row(path: &str, workid: &str, lease: &str) -> LockRow {
        LockRow { project: "p".into(), path: path.into(), kind: "lock".into(), workid: workid.into(),
            lease: lease.into(), expires: "2026-09-25T18:00:00Z".into(), ..Default::default() }
    }
    const NOW: &str = "2026-09-25T17:20:39Z";

    /// The 2026-09-25 old rule: kind and expiry only (what dispatch counted).
    fn old_live<'a>(rows: &'a [LockRow]) -> Vec<&'a LockRow> {
        rows.iter().filter(|r| r.kind == "lock" && (r.expires.is_empty() || r.expires.as_str() >= NOW)).collect()
    }

    const A054: &str = "e84f5d4f-fb75-417e-afea-a054fa190fb2";
    const ED93: &str = "b5c2bc8f-0ab9-49bf-9f0a-ed93b98a2679";
    const P634: &str = "5d68139b-c7b7-43ea-a066-63491afb13c0";
    const I1943: &str = "849fe2db-9f13-419c-96a8-1943e6203935";
    const C146: &str = "f0000000-0000-4000-8000-146c269bae79";
    const C1294: &str = "f0000000-0000-4000-8000-1294cc1d9ec3";
    const C0837: &str = "f0000000-0000-4000-8000-0837b0f9efd5";

    /// The incident's records as read at 10:57:29Z (CR 1.1 and 8.1(i)).
    fn incident() -> (Vec<WorkItem>, Vec<LockRow>) {
        let mut a054 = wi(A054, "queued", P634, &[]);
        a054.run_now = true;
        a054.priority = 0;
        a054.lockdirs = vec![
            "{topdir}/core/repos/pdy_core_intake/src".into(),
            "{topdir}/core/repos/pdy_core_intake/proto".into(),
            "{topdir}/core/repos/pdy_core_intake/deploy".into(),
        ];
        let mut ed93 = wi(ED93, "queued", "", &[P634]);
        ed93.lockdirs = vec!["{topdir}/core/repos/pdy_core_intake/src".into()];
        ed93.lease = String::new();
        let p634 = wi(P634, "queued", "", &[A054, C146, C1294, C0837]);
        let mut i1943 = wi(I1943, "queued", "", &[]);
        i1943.lockdirs = vec!["{topdir}/demos/03_stream_employee_wages/settle_driver.py".into()];
        let items = vec![
            a054, ed93, p634, i1943,
            wi(C146, "complete", P634, &[]),
            wi(C1294, "complete", P634, &[]),
            wi(C0837, "queued", P634, &[A054, C146, C1294]),
        ];
        let rows = vec![
            row("{topdir}/core/repos/pdy_core_intake/src", ED93, "L5"),
            row("{topdir}/core/repos/pdy_core_intake/proto/intake.proto", ED93, "L5"),
            row("{topdir}/core/repos/pdy_core_intake/deploy/values.yaml", ED93, "L5"),
            row("{topdir}/devops/SECURITY_DELTA.md", ED93, "L5"),
            row("{topdir}/demos/03_stream_employee_wages/settle_driver.py", ED93, "L5"),
        ];
        (items, rows)
    }

    /// F1 (i): with ed93 not running (no lease), its five leaked rows are
    /// not live under either rule, nobody waits on it by lock, and there is
    /// no cycle.  Under the OLD live-row rule the detector finds exactly the
    /// incident's loop, and it is the auto-resolvable kind.
    #[test]
    fn pdy_dev_20260925_lock_dependency_cycle() {
        let (items, rows) = incident();
        let by_id: HashMap<String, &WorkItem> = items.iter().map(|i| (i.id.clone(), i)).collect();
        let a054 = by_id[A054];
        let i1943 = by_id[I1943];
        // today's behaviour, documented: the old rule makes ed93 a lock holder
        let old = old_live(&rows);
        assert!(!lock_holders(a054, &old).is_empty());
        assert_eq!(lock_holders(i1943, &old)[0].0, ED93);
        // after: a queued lease-less holder's rows are dead
        let live = live_lock_rows(&rows, &by_id, NOW);
        assert!(live.is_empty(), "{live:?}");
        assert!(lock_holders(a054, &live).is_empty());
        assert!(lock_holders(i1943, &live).is_empty());
        assert!(find_wait_cycles(&wait_edges(&items, &live)).is_empty());
        // the detector sees the incident's loop under the old rule
        let cycles = find_wait_cycles(&wait_edges(&items, &old));
        assert_eq!(cycles.len(), 1, "{cycles:?}");
        let c = &cycles[0];
        let hops: Vec<(&str, &str, &str)> = c.iter().map(|e| (e.from.as_str(), e.to.as_str(), e.kind.label())).collect();
        assert_eq!(hops, vec![(P634, A054, "blocker"), (A054, ED93, "lock"), (ED93, P634, "blocker")]);
        assert!(auto_resolvable(c, &by_id));
        assert_eq!(not_running_lock_holders(c, &by_id), vec![ED93.to_string()]);
        assert_eq!(describe_cycle(c), "63491afb13c0 → a054fa190fb2 → ed93b98a2679 → 63491afb13c0 (blocker, lock, blocker)");
    }

    #[test]
    fn a_running_holders_leased_rows_are_live_and_others_are_not() {
        let mut h = wi("h", "in-progress", "", &[]);
        h.lease = "L1".into();
        let q = wi("q", "queued", "", &[]);
        let legacy = wi("g", "in-progress", "", &[]);
        let items = vec![h, q, legacy];
        let by_id: HashMap<String, &WorkItem> = items.iter().map(|i| (i.id.clone(), i)).collect();
        let rows = vec![
            row("{topdir}/a", "h", "L1"),       // live
            row("{topdir}/b", "h", "L0"),       // an older run's lease
            row("{topdir}/c", "q", ""),         // queued holder
            row("{topdir}/d", "g", ""),         // no lease (a migrated iter3 row), holder running
            row("{topdir}/e", "ghost", "L1"),   // holder deleted
            LockRow { expires: "2026-09-25T17:00:00Z".into(), ..row("{topdir}/f", "h", "L1") }, // expired
            LockRow { kind: "reserve".into(), ..row("{topdir}/g", "q", "") },
        ];
        let live: Vec<&str> = live_lock_rows(&rows, &by_id, NOW).iter().map(|r| r.path.as_str()).collect();
        assert_eq!(live, vec!["{topdir}/a"]);
        // the sweep removes exactly what does not count, keeps the reservation of a queued item
        let removed: Vec<&str> = rows.iter().filter(|r| sweep_reason(r, by_id.get(&r.workid).copied(), NOW).is_some()).map(|r| r.path.as_str()).collect();
        assert_eq!(removed, vec!["{topdir}/b", "{topdir}/c", "{topdir}/d", "{topdir}/e", "{topdir}/f"]);
    }

    #[test]
    fn grant_rule_ties_locks_to_the_live_lease() {
        let mut h = wi("11112222-3333-4444-5555-666677778888", "in-progress", "", &[]);
        h.lease = "L1".into();
        assert_eq!(lock_grant("lock", &h, ""), LockGrant::Grant { lease: "L1".into() });
        assert_eq!(lock_grant("lock", &h, "L1"), LockGrant::Grant { lease: "L1".into() });
        assert!(matches!(lock_grant("lock", &h, "L0"), LockGrant::Refuse { refused: "stale lease", .. }));
        h.lease.clear();
        h.state = "queued".into();
        match lock_grant("lock", &h, "") {
            LockGrant::Refuse { refused, msg } => {
                assert_eq!(refused, "not running");
                assert!(msg.contains("666677778888") && msg.contains("state queued"), "{msg}");
            }
            g => panic!("granted: {g:?}"),
        }
        // no switch any more: an in-progress holder without a lease is refused too
        h.state = "in-progress".into();
        assert!(matches!(lock_grant("lock", &h, ""), LockGrant::Refuse { refused: "not running", .. }));
        // a reservation is a queued item's only
        assert!(matches!(lock_grant("reserve", &h, ""), LockGrant::Refuse { refused: "not queued", .. }));
        h.state = "queued".into();
        assert!(matches!(lock_grant("reserve", &h, ""), LockGrant::Grant { .. }));
        assert_eq!(lock_sk("reserve", "{topdir}/a"), "reserve:{topdir}/a");
        assert_eq!(lock_sk("lock", "{topdir}/a"), "{topdir}/a");
    }

    #[test]
    fn wait_edges_cover_blocker_deep_and_lock_and_leave_only_queued_items() {
        let mut h = wi("h", "in-progress", "", &["b"]);
        h.lease = "L".into();
        let mut x = wi("x", "queued", "", &["b", "p", "done"]);
        x.lockdirs = vec!["{topdir}/a/b".into()];
        let items = vec![
            x, h,
            wi("b", "queued", "", &[]),
            wi("p", "complete", "", &[]),
            wi("c", "queued", "p", &[]),          // open follow-up of complete p: deep
            wi("c2", "queued", "p", &["x"]),      // waits on x: the exception
            wi("done", "complete", "", &[]),
        ];
        let rows = vec![row("{topdir}/a", "h", "L")]; // ancestor of x's lockdir: overlaps
        let by_id: HashMap<String, &WorkItem> = items.iter().map(|i| (i.id.clone(), i)).collect();
        let live = live_lock_rows(&rows, &by_id, NOW);
        let e = wait_edges(&items, &live);
        let got: Vec<(&str, &str, &str)> = e.iter().map(|e| (e.from.as_str(), e.to.as_str(), e.kind.label())).collect();
        assert_eq!(got, vec![("c2", "x", "blocker"), ("x", "b", "blocker"), ("x", "c", "deep"), ("x", "h", "lock")]);
        assert!(e.iter().all(|e| e.from != "h"), "no edge leaves an in-progress item");
        // shallow: no deep edge
        let mut items2 = items.clone();
        items2[0].blockedby_shallow = true;
        assert!(!wait_edges(&items2, &live).iter().any(|e| e.kind.label() == "deep"));
        // a lock row nested under the lockdir overlaps too
        let rows = vec![row("{topdir}/a/b/c.rs", "h", "L")];
        let live = live_lock_rows(&rows, &by_id, NOW);
        assert!(wait_edges(&items, &live).iter().any(|e| e.to == "h"));
    }

    fn cyc(edges: &[(&str, &str)]) -> Vec<WaitEdge> {
        edges.iter().map(|(a, b)| WaitEdge { from: a.to_string(), to: b.to_string(), kind: WaitKind::Blocker }).collect()
    }
    fn members(c: &[WaitEdge]) -> Vec<&str> {
        c.iter().map(|e| e.from.as_str()).collect()
    }

    #[test]
    fn cycles_self_two_disjoint_and_deterministic() {
        assert_eq!(find_wait_cycles(&cyc(&[("a", "a")])).len(), 1);
        assert!(find_wait_cycles(&cyc(&[("a", "b"), ("b", "c")])).is_empty());
        let two = find_wait_cycles(&cyc(&[("b", "a"), ("a", "b")]));
        assert_eq!(members(&two[0]), vec!["a", "b"]);
        let g = cyc(&[("a", "b"), ("b", "a"), ("x", "y"), ("y", "z"), ("z", "x"), ("z", "q"), ("q", "q2")]);
        let c = find_wait_cycles(&g);
        assert_eq!(c.iter().map(|c| members(c)).collect::<Vec<_>>(), vec![vec!["a", "b"], vec!["x", "y", "z"]]);
        // shuffled input: byte-identical output
        let mut rng = Rng(7);
        for _ in 0..50 {
            let mut s = g.clone();
            for i in (1..s.len()).rev() {
                let j = rng.below(i + 1);
                s.swap(i, j);
            }
            assert_eq!(format!("{:?}", find_wait_cycles(&s)), format!("{c:?}"));
        }
    }

    /// CR 5.4: X waits on P's open child C (deep), C on Q's open child D
    /// (deep), D on X (declared).  The loop needs no declared link between
    /// X and C or C and D: the detector finds it, and the write check refuses
    /// its last link when P and Q are already complete.
    #[test]
    fn two_deep_cycle_is_found_and_refused_on_write() {
        let items = vec![
            wi("x", "queued", "", &["p"]),
            wi("p", "complete", "", &[]),
            wi("c", "queued", "p", &["q"]),
            wi("q", "complete", "", &[]),
            wi("d", "queued", "q", &["x"]),
        ];
        let c = find_wait_cycles(&wait_edges(&items, &[]));
        assert_eq!(c.len(), 1);
        assert_eq!(c[0].iter().map(|e| e.kind.label()).collect::<Vec<_>>(), vec!["deep", "blocker", "deep"]); // c -> d -> x -> c
        // write-time: d -> x closes it; d -> nothing does not
        let mut d = items[4].clone();
        let path = dependency_cycle_on_write(&d, &items).expect("refused");
        assert_eq!(path.first().unwrap().from, "d");
        assert_eq!(path.last().unwrap().to, "d");
        d.blockedby = vec![];
        assert!(dependency_cycle_on_write(&d, &items).is_none());
        // the old declared-only check cannot see it
        let by_id: HashMap<String, &WorkItem> = items.iter().map(|i| (i.id.clone(), i)).collect();
        assert!(crate::blockedby_cycle("d", &["x".into()], &by_id).is_none());
    }

    // ---------- property tests: a seeded xorshift, no new dependency ----------

    struct Rng(u64);
    impl Rng {
        fn next(&mut self) -> u64 {
            let mut x = self.0.max(1);
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            self.0 = x;
            x
        }
        fn below(&mut self, n: usize) -> usize {
            (self.next() % n.max(1) as u64) as usize
        }
        fn chance(&mut self, pct: u64) -> bool {
            self.next() % 100 < pct
        }
    }
    const POOL: &[&str] = &["{topdir}/a", "{topdir}/a/b", "{topdir}/a/b/c.rs", "{topdir}/d", "{topdir}/d/e", "{topdir}/f", "{topdir}/g/h.rs", "{topdir}/g"];
    const STATES: &[&str] = &["queued", "queued", "queued", "queued", "in-progress", "question", "parked", "complete", "complete", "failed"];

    fn random_graph(rng: &mut Rng) -> (Vec<WorkItem>, Vec<LockRow>) {
        let n = 2 + rng.below(39);
        let ids: Vec<String> = (0..n).map(|i| format!("i{i:02}")).collect();
        let mut items: Vec<WorkItem> = Vec::new();
        for (k, id) in ids.iter().enumerate() {
            let mut w = wi(id, STATES[rng.below(STATES.len())], "", &[]);
            if k > 0 && rng.chance(50) {
                w.createdby = ids[rng.below(k)].clone();
            }
            for _ in 0..rng.below(3) {
                w.blockedby.push(ids[rng.below(n)].clone());
            }
            w.blockedby_shallow = rng.chance(15);
            for _ in 0..rng.below(3) {
                w.lockdirs.push(POOL[rng.below(POOL.len())].into());
            }
            if rng.chance(40) {
                w.lease = format!("L{}", rng.below(3));
            }
            w.priority = rng.below(3) as i64;
            items.push(w);
        }
        let mut rows = Vec::new();
        for _ in 0..rng.below(31) {
            rows.push(row(POOL[rng.below(POOL.len())], &ids[rng.below(n)], &format!("L{}", rng.below(3))));
        }
        (items, rows)
    }

    fn reach(edges: &[WaitEdge]) -> (Vec<String>, Vec<Vec<bool>>) {
        let mut names: Vec<String> = edges.iter().flat_map(|e| [e.from.clone(), e.to.clone()]).collect();
        names.sort();
        names.dedup();
        let ix = |s: &str| names.iter().position(|n| n == s).unwrap();
        let mut r = vec![vec![false; names.len()]; names.len()];
        for e in edges {
            r[ix(&e.from)][ix(&e.to)] = true;
        }
        for k in 0..names.len() {
            for i in 0..names.len() {
                for j in 0..names.len() {
                    if r[i][k] && r[k][j] {
                        r[i][j] = true;
                    }
                }
            }
        }
        (names, r)
    }

    /// P1 (I5): enforced, no cycle holds a lock edge.  P2 (I4): every lock
    /// edge ends at a holder whose live lease is the row's lease.  P3: the
    /// detector reports a cycle iff Floyd–Warshall sees one, and every cycle
    /// it reports is a closed walk over input edges.
    #[test]
    fn property_lock_edges_never_close_a_cycle_and_the_detector_is_exact() {
        for seed in 1..=2000u64 {
            let mut rng = Rng(seed.wrapping_mul(0x9E3779B97F4A7C15));
            let (items, rows) = random_graph(&mut rng);
            let by_id: HashMap<String, &WorkItem> = items.iter().map(|i| (i.id.clone(), i)).collect();
            let live = live_lock_rows(&rows, &by_id, NOW);
            let edges = wait_edges(&items, &live);
            let cycles = find_wait_cycles(&edges);
            for c in &cycles {
                assert!(!c.iter().any(|e| matches!(e.kind, WaitKind::Lock { .. })), "seed {seed}: P1 lock edge in {c:?}");
                for (k, e) in c.iter().enumerate() {
                    assert!(edges.contains(e), "seed {seed}: P3 edge not in input");
                    assert_eq!(e.to, c[(k + 1) % c.len()].from, "seed {seed}: P3 not a closed walk");
                }
            }
            for e in &edges {
                if let WaitKind::Lock { path } = &e.kind {
                    let h = by_id[&e.to];
                    assert!(!h.lease.is_empty(), "seed {seed}: P2");
                    assert!(live.iter().any(|r| &r.path == path && r.workid == e.to && r.lease == h.lease), "seed {seed}: P2");
                }
            }
            let (_, r) = reach(&edges);
            let any = (0..r.len()).any(|i| r[i][i]);
            assert_eq!(!cycles.is_empty(), any, "seed {seed}: P3 detector vs reachability");
            // the OLD live rule is still exact (P3 over graphs that do have lock cycles)
            let old = old_live(&rows);
            let edges = wait_edges(&items, &old);
            let (_, r) = reach(&edges);
            assert_eq!(!find_wait_cycles(&edges).is_empty(), (0..r.len()).any(|i| r[i][i]), "seed {seed}: P3 old rule");
        }
    }

    /// P4: the write check refuses iff brute-force reachability over the
    /// dependency-only edges (item forced queued, every open item a source)
    /// finds a loop through the item.
    #[test]
    fn property_write_check_matches_brute_force() {
        for seed in 1..=2000u64 {
            let mut rng = Rng(seed.wrapping_mul(0xD1B54A32D192ED03));
            let (items, _) = random_graph(&mut rng);
            let k = rng.below(items.len());
            let mut me = items[k].clone();
            me.blockedby = (0..rng.below(3)).map(|_| items[rng.below(items.len())].id.clone()).collect();
            let got = dependency_cycle_on_write(&me, &items);
            let mut all: Vec<WorkItem> = items.iter().filter(|i| i.id != me.id).cloned().collect();
            let mut forced = me.clone();
            forced.state = "queued".into();
            all.push(forced);
            let edges = edges_from(&all, &[], |i| is_open(&i.state));
            let (names, r) = reach(&edges);
            let want = names.iter().position(|n| n == &me.id).map(|i| r[i][i]).unwrap_or(false);
            assert_eq!(got.is_some(), want, "seed {seed}");
            if let Some(p) = got {
                assert_eq!((p[0].from.as_str(), p.last().unwrap().to.as_str()), (me.id.as_str(), me.id.as_str()), "seed {seed}");
            }
        }
    }

    /// P5: a state-machine model of the server rules (grant, renew, the PUT
    /// that changes a lease deletes the old lease's rows, release_all, the
    /// sweep).  After every event I1 holds (every unexpired lock row carries
    /// its holder's live lease); after every tick I3 (no queued item holds a
    /// row) and I5 (no cycle through a lock edge).
    #[test]
    fn property_server_rules_keep_locks_on_running_items() {
        for seed in 1..=500u64 {
            let mut rng = Rng(seed.wrapping_mul(0xA24BAED4963EE407));
            let n = 2 + rng.below(8);
            let mut items: Vec<WorkItem> = (0..n)
                .map(|i| {
                    let mut w = wi(&format!("i{i}"), "queued", "", &[]);
                    w.lockdirs = vec![POOL[rng.below(POOL.len())].into()];
                    w
                })
                .collect();
            for i in 0..n {
                if rng.chance(30) {
                    let b = format!("i{}", rng.below(n));
                    items[i].blockedby.push(b);
                }
            }
            let mut rows: HashMap<String, LockRow> = HashMap::new();
            let mut clock: i64 = 0;
            let mut lease_seq = 0u64;
            let iso = |t: i64| format!("2026-09-25T{:02}:{:02}:{:02}Z", 10 + t / 3600, (t / 60) % 60, t % 60);
            // the server's "PUT changed the lease" half of I2
            fn set_lease(items: &mut [WorkItem], rows: &mut HashMap<String, LockRow>, i: usize, lease: String) {
                let old = std::mem::replace(&mut items[i].lease, lease);
                if !old.is_empty() && old != items[i].lease {
                    let id = items[i].id.clone();
                    rows.retain(|_, r| !(r.workid == id && r.lease == old));
                }
            }
            for _step in 0..200 {
                let i = rng.below(n);
                let now = iso(clock);
                match rng.below(8) {
                    0 if items[i].state == "queued" => {
                        lease_seq += 1;
                        items[i].state = "in-progress".into();
                        set_lease(&mut items, &mut rows, i, format!("lease{lease_seq}"));
                    }
                    1 | 2 => {
                        // an agent's acquire (with its lease) or a stray loop's (none)
                        let path = POOL[rng.below(POOL.len())].to_string();
                        let req = if rng.chance(50) { items[i].lease.clone() } else { String::new() };
                        if let LockGrant::Grant { lease, .. } = lock_grant("lock", &items[i], &req) {
                            let free = rows.get(&path).map(|r| r.expires < now || r.workid == items[i].id).unwrap_or(true);
                            if free {
                                let expires = iso(clock + LOCK_LEASE_TTL_SEC);
                                rows.insert(path.clone(), LockRow { expires, ..row(&path, &items[i].id.clone(), &lease) });
                            }
                        }
                    }
                    3 => {
                        // renew with a random lease: only the live one extends
                        let guess = if rng.chance(50) { items[i].lease.clone() } else { format!("lease{}", rng.below(lease_seq as usize + 1)) };
                        if !items[i].lease.is_empty() && guess == items[i].lease {
                            let id = items[i].id.clone();
                            for r in rows.values_mut().filter(|r| r.workid == id && r.lease == guess) {
                                r.expires = iso(clock + LOCK_LEASE_TTL_SEC);
                            }
                        }
                    }
                    4 if !items[i].lease.is_empty() => {
                        // close to any outcome: the final PUT clears the lease, then release_all
                        let to = ["complete", "failed", "queued", "queued", "question", "parked"][rng.below(6)];
                        items[i].state = to.into();
                        set_lease(&mut items, &mut rows, i, String::new());
                        let id = items[i].id.clone();
                        rows.retain(|_, r| r.workid != id);
                    }
                    5 if items[i].lease.is_empty() && items[i].state != "in-progress" => {
                        // a human moves an item that is not running
                        items[i].state = ["queued", "parked", "question"][rng.below(3)].into();
                    }
                    6 => clock += rng.below(900) as i64,
                    _ => {
                        // a tick: sweep, then the detector
                        let snapshot = items.clone();
                        let by_id: HashMap<String, &WorkItem> = snapshot.iter().map(|w| (w.id.clone(), w)).collect();
                        rows.retain(|_, r| sweep_reason(r, by_id.get(&r.workid).copied(), &now).is_none());
                        for q in snapshot.iter().filter(|w| w.state == "queued") {
                            assert!(!rows.values().any(|r| r.workid == q.id), "seed {seed}: I3 queued {} holds a row", q.id);
                        }
                        let all: Vec<LockRow> = rows.values().cloned().collect();
                        let live = live_lock_rows(&all, &by_id, &now);
                        for c in find_wait_cycles(&wait_edges(&snapshot, &live)) {
                            assert!(!c.iter().any(|e| matches!(e.kind, WaitKind::Lock { .. })), "seed {seed}: I5");
                        }
                    }
                }
                // I1 after every event
                let now = iso(clock);
                for r in rows.values().filter(|r| r.expires >= now) {
                    let h = items.iter().find(|w| w.id == r.workid).unwrap();
                    assert!(!h.lease.is_empty() && h.lease == r.lease, "seed {seed}: I1 row {r:?} holder lease '{}'", h.lease);
                }
            }
        }
    }
}
