//! `iter sweep` (iter4, decided 2026-09-28 — the iter-prime idea brought
//! back): run the project's testgroups off the architecture map and turn red
//! ones into work items. The map (iter_data's graph) says which groups exist,
//! which node owns each (the edge that declared it) and, walking from main,
//! whether each chain allows testing: `teststate` is evaluated per chain
//! (structureV2), so a group runs when at least one chain reaching its owner
//! says include. Results land on the testgroup's vertex; a red group files
//! one fix item locked to its owner's codedirs, deduplicated by the same
//! `check:`/`container:` keys `iter runtests` uses, so a sweep and an agent's
//! run never file the same defect twice.
//!
//! Scheduled as an ordinary `exec` template: `iter sweep --install-schedule`.

use crate::client::Api;
use crate::sync::Conn;
use iter_local::runtests as rt;
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

/// Chain state while walking down from main.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
enum Chain {
    Include,
    Omit,
    Block,
}

fn step(state: Chain, own: &str) -> Chain {
    match (state, own) {
        (Chain::Block, _) => Chain::Block, // only a human lifts a block, and only on the file itself
        (_, "block" | "blocked") => Chain::Block,
        (_, "omit") => Chain::Omit,
        (_, "include") => Chain::Include,
        (s, _) => s, // inherit / unset
    }
}

/// For each testgroup vertex id: its owner id and whether some chain from
/// main includes it (the reasons are kept for the report).
pub fn eligible(vertices: &[Value], edges: &[Value]) -> Vec<(String, String, bool, String)> {
    let byid: HashMap<&str, &Value> = vertices.iter().filter_map(|v| v["id"].as_str().map(|i| (i, v))).collect();
    let mut out_edges: HashMap<&str, Vec<(&str, &str)>> = HashMap::new();
    for e in edges {
        if let (Some(f), Some(k), Some(t)) = (e["from"].as_str(), e["kind"].as_str(), e["to"].as_str()) {
            out_edges.entry(f).or_default().push((k, t));
        }
    }
    // every (vertex, chain-state) pair reachable from main through linking edges
    let main = vertices.iter().find(|v| v["nodetype"] == "main").and_then(|v| v["id"].as_str());
    let mut seen: HashSet<(&str, Chain)> = HashSet::new();
    let mut stack: Vec<(&str, Chain)> = main.map(|m| vec![(m, Chain::Include)]).unwrap_or_default();
    while let Some((cur, st)) = stack.pop() {
        if !seen.insert((cur, st)) {
            continue;
        }
        for (kind, to) in out_edges.get(cur).cloned().unwrap_or_default() {
            // chains run through the executable DAG; req and test files are leaves
            if !["root", "codenodes", "inputs", "outputs"].contains(&kind) {
                continue;
            }
            let own = byid.get(to).and_then(|v| v["teststate"].as_str()).unwrap_or("");
            stack.push((to, step(st, own)));
        }
    }
    let mut owner_states: HashMap<&str, Vec<Chain>> = HashMap::new();
    for (v, st) in &seen {
        owner_states.entry(v).or_default().push(*st);
    }
    let mut result = Vec::new();
    // `tests` since iter4; maps synced before the rename say `testgroups`
    for e in edges.iter().filter(|e| e["kind"] == "tests" || e["kind"] == "testgroups") {
        let (Some(owner), Some(tg)) = (e["from"].as_str(), e["to"].as_str()) else { continue };
        let states = owner_states.get(owner).cloned().unwrap_or_default();
        let (ok, why) = if states.is_empty() {
            (false, "owner not reachable from main".to_string())
        } else if states.contains(&Chain::Include) {
            (true, "included".to_string())
        } else if states.contains(&Chain::Omit) {
            (false, "omitted on every chain".to_string())
        } else {
            (false, "blocked".to_string())
        };
        result.push((tg.to_string(), owner.to_string(), ok, why));
    }
    result.sort();
    result.dedup();
    result
}

fn abs_path(topdir: &Path, p: &str) -> PathBuf {
    match p.strip_prefix("{topdir}/") {
        Some(rel) => topdir.join(rel),
        None => PathBuf::from(p),
    }
}

fn tail(s: &str, n: usize) -> String {
    let lines: Vec<&str> = s.lines().collect();
    lines[lines.len().saturating_sub(n)..].join("\n")
}

pub struct SweepOpts {
    pub group: Option<String>,
    pub dry_run: bool,
    pub file_items: bool,
    pub timeout_min: u64,
    /// check node text against docs/node_text_standard.md and file `ingest`
    /// items for failing nodes, at most this many new ones per sweep (0 = skip)
    pub text_max: usize,
}

/// `iter sweep`. Exit 0 all green, 1 something red/error, 2 could not run.
pub fn sweep_verb(c: &Conn, o: &SweepOpts) -> i32 {
    let Some(api) = &c.api else {
        eprintln!("iter sweep: no iter_data connection (the sweep reads the map from iter_data; run `iter sync` first)");
        return 2;
    };
    let g = match api.get(&format!("/api/projects/{}/graph", c.project)) {
        Ok(g) => g,
        Err(e) => {
            eprintln!("iter sweep: cannot read the map: {e}");
            return 2;
        }
    };
    let vertices = g["vertices"].as_array().cloned().unwrap_or_default();
    let edges = g["edges"].as_array().cloned().unwrap_or_default();
    if vertices.is_empty() {
        eprintln!("iter sweep: project {} has no map yet — run `iter sync`", c.project);
        return 2;
    }
    let byid: HashMap<String, Value> = vertices.iter().filter_map(|v| v["id"].as_str().map(|i| (i.to_string(), v.clone()))).collect();
    let plan = eligible(&vertices, &edges);
    let mut worst = 0;
    let (mut ran, mut red, mut skipped) = (0, 0, 0);
    for (tg_id, owner_id, ok, why) in &plan {
        let Some(tg) = byid.get(tg_id) else { continue };
        let tg_path = tg["path"].as_str().unwrap_or("");
        let labels: Vec<String> = tg["groups"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|x| x["label"].as_str().map(String::from))
            .filter(|l| o.group.as_ref().map(|g| g == l).unwrap_or(true))
            .collect();
        if labels.is_empty() {
            continue;
        }
        if !ok {
            skipped += 1;
            println!("skip  {tg_path} ({why})");
            continue;
        }
        if o.dry_run {
            println!("would run {tg_path}: {}", labels.join(", "));
            continue;
        }
        let file = abs_path(&c.topdir, tg_path);
        let mut results = Vec::new();
        let (mut pass, mut total) = (0u64, 0u64);
        let mut verdict = "green";
        let mut failing_logs = Vec::new();
        let mut red_labels: Vec<String> = Vec::new();
        for label in &labels {
            ran += 1;
            match rt::run_group(&file, label, None, o.timeout_min) {
                Ok(run) => {
                    pass += run.pass;
                    total += run.total;
                    let out = match run.outcome {
                        rt::Outcome::Green => "green",
                        rt::Outcome::Red => "red",
                        rt::Outcome::Error => "error",
                    };
                    if out == "red" || (out == "error" && verdict == "green") {
                        verdict = out;
                    }
                    if out == "red" {
                        red_labels.push(label.clone());
                    }
                    let failing: Vec<String> = run.runs.iter().filter(|t| t.outcome != rt::Outcome::Green && t.gates).map(|t| t.id.clone()).collect();
                    for t in run.runs.iter().filter(|t| t.outcome != rt::Outcome::Green && t.gates) {
                        failing_logs.push(format!("### {label} / {} ({}) exit {}\n```\n{}\n```", t.id, t.name, t.exit_code, tail(&t.log, 30)));
                    }
                    println!("{:<5} {tg_path} [{label}] {}/{}", out, run.pass, run.total);
                    results.push(json!({"label": label, "result": out, "pass": run.pass, "total": run.total, "failing": failing}));
                }
                Err(e) => {
                    verdict = if verdict == "green" { "error" } else { verdict };
                    println!("error {tg_path} [{label}] {e}");
                    results.push(json!({"label": label, "result": "error", "detail": e}));
                }
            }
        }
        let mut body = json!({"result": verdict, "counts": format!("{pass}/{total}"), "groups": results});
        if verdict != "green" {
            worst = 1;
            red += 1;
            // only red files work: "error" means the script itself broke (a
            // missing tool, no tests registered) — recorded on the vertex, but
            // an infrastructure problem is not a defect in the owner's code
            if o.file_items && verdict == "red" {
                let ucs = usecases_touching(&vertices, &edges, owner_id);
                if let Some(id) = file_fix_item(api, c, tg, byid.get(owner_id), &red_labels, pass, total, &failing_logs, &ucs) {
                    body["workid"] = json!(id);
                }
            }
        }
        if let Err(e) = api.post(&format!("/api/projects/{}/graph/nodes/{tg_id}/testresult", c.project), &body) {
            eprintln!("could not record the result on {tg_path}: {e}");
        }
    }
    println!("sweep: {ran} group(s) run, {red} testgroup file(s) non-green, {skipped} skipped by teststate");
    if o.text_max > 0 && o.group.is_none() {
        text_sweep(api, c, &vertices, o);
    }
    worst
}

/// The node-text half of the sweep (2026-09-29): every code node whose text
/// breaks docs/node_text_standard.md becomes one `ingest` work item (deduped
/// per node by the `check:node-text` + `container:<path>` keys), so unclear
/// maps are fixed through the queue like red tests are. Capped per sweep so a
/// large legacy project is not flooded in one go.
fn text_sweep(api: &Api, c: &Conn, vertices: &[Value], o: &SweepOpts) {
    let mut failing: Vec<(&Value, Vec<(&'static str, String)>)> = vertices
        .iter()
        .filter(|v| v["nodetype"] == "code")
        .filter_map(|v| {
            let body = format!("# Long Description\n{}\n", v["long_description"].as_str().unwrap_or(""));
            let f = iter_local::validate::node_text_findings(v["description"].as_str().unwrap_or(""), v["simple_description"].as_str().unwrap_or(""), &body);
            (!f.is_empty()).then_some((v, f))
        })
        .collect();
    // code no component owns (2026-09-29): the same item carries it
    for (v, f) in failing.iter_mut() {
        if let Some(u) = v["uncovered_files"].as_array().filter(|a| !a.is_empty()) {
            f.push(("uncovered-code", format!("{} file(s) sit beside code this node's components own but no component lists them: {}", u.len(),
                u.iter().filter_map(|x| x.as_str()).collect::<Vec<_>>().join(", "))));
        }
    }
    for v in vertices.iter().filter(|v| v["nodetype"] == "code" && v["uncovered_files"].as_array().map(|a| !a.is_empty()).unwrap_or(false)) {
        if !failing.iter().any(|(x, _)| x["id"] == v["id"]) {
            let u = v["uncovered_files"].as_array().unwrap();
            failing.push((v, vec![("uncovered-code", format!("{} file(s) sit beside code this node's components own but no component lists them: {}", u.len(),
                u.iter().filter_map(|x| x.as_str()).collect::<Vec<_>>().join(", ")))]));
        }
    }
    failing.sort_by(|a, b| a.0["path"].as_str().cmp(&b.0["path"].as_str()));
    println!("node text: {} code node(s) break the node-text standard", failing.len());
    let mut filed = 0;
    for (v, findings) in failing {
        if filed >= o.text_max {
            println!("node text: stopping at {} new item(s) this sweep (--text-max)", o.text_max);
            break;
        }
        let path = v["path"].as_str().unwrap_or("");
        let name = v["name"].as_str().unwrap_or("");
        let codes: Vec<&str> = findings.iter().map(|f| f.0).collect();
        if o.dry_run {
            println!("  would file: {path} ({})", codes.join(", "));
            continue;
        }
        let codedirs: Vec<String> = v["codedirs"].as_array().into_iter().flatten().filter_map(|x| x.as_str().map(String::from)).collect();
        let request = format!(
            "Bring the text of map node \"{name}\" ({path}) up to the node-text standard (docs/node_text_standard.md) so a reader who has never seen the code understands what this part DOES and why it matters.\n\n\
             Findings from `iter validate`:\n{}\n\n\
             Read the code it owns first: {}.\n\
             Then rewrite, in the node file only: `name` (what a person would call it), `description` (one sentence: it does X so that Y; a pure lookup says so), `simple_description` (one plain sentence, no jargon) and the `# Long Description` (3–6 short paragraphs: what, how — key functions and files, inputs/outputs by map name, why it matters, one example). Keep `id` and `children` exactly as they are. Finish with `iter validate` clean for this file.",
            findings.iter().map(|f| format!("- {}: {}", f.0, f.1)).collect::<Vec<_>>().join("\n"),
            if codedirs.is_empty() { "(no codedirs listed — read the files beside the node)".to_string() } else { codedirs.join(", ") },
        );
        let tags = json!([
            {"text": format!("{}node-text", iter_core::dedup::CHECK_TAG_PREFIX), "color": ""},
            {"text": format!("{}{}", iter_core::dedup::CONTAINER_TAG_PREFIX, path), "color": ""},
        ]);
        let body = json!({"name": format!("Node text: {name} — {}", codes.join(", ")), "agent": "ingest", "state": "queued",
            "lockdirs": [path], "blockedby": [], "context": [], "model": "", "tags": tags,
            "createdby": std::env::var("ITER_WORKID").unwrap_or_default(), "requestedby": "agent:test", "prework": [], "postwork": [],
            "request": request});
        match api.post(&format!("/api/projects/{}/workitems", c.project), &body) {
            Ok(created) if crate::cli::already_open(&created) => {}
            Ok(created) => {
                filed += 1;
                let id = created["id"].as_str().unwrap_or("");
                println!("  filed node-text item …{} for {path}", &id[id.len().saturating_sub(12)..]);
            }
            Err(e) => eprintln!("  could not file the node-text item for {path}: {e}"),
        }
    }
}

/// The use cases (by file stem) whose `codenodes` include this node or one of
/// its owners — the journeys a red test in it puts at risk.
pub fn usecases_touching(vertices: &[Value], edges: &[Value], node: &str) -> Vec<String> {
    let mut chain = vec![node.to_string()];
    let mut cur = node.to_string();
    for _ in 0..16 {
        let parent = edges.iter().find(|e| e["kind"] == "codenodes" && e["to"] == cur.as_str()
            && vertices.iter().any(|v| v["id"] == e["from"] && v["nodetype"] == "code"));
        match parent.and_then(|e| e["from"].as_str()) {
            Some(p) => { chain.push(p.to_string()); cur = p.to_string(); }
            None => break,
        }
    }
    let mut out: Vec<String> = vertices
        .iter()
        .filter(|v| v["nodetype"] == "usecase")
        .filter(|u| edges.iter().any(|e| e["from"] == u["id"] && e["kind"] == "codenodes" && chain.iter().any(|c| e["to"] == c.as_str())))
        .filter_map(|u| u["ucid"].as_str().map(|x| x.trim_start_matches("usecase:").to_string()))
        .collect();
    out.sort();
    out.dedup();
    out
}

/// One fix item per red testgroup, locked to the owner's codedirs (the
/// testgroup's own directory when the owner is an interface or use-case).
#[allow(clippy::too_many_arguments)]
fn file_fix_item(api: &Api, c: &Conn, tg: &Value, owner: Option<&Value>, labels: &[String], pass: u64, total: u64, logs: &[String], usecases: &[String]) -> Option<String> {
    let tg_path = tg["path"].as_str().unwrap_or("");
    let owner_name = owner.and_then(|o| o["name"].as_str()).unwrap_or("?");
    let owner_path = owner.and_then(|o| o["path"].as_str()).unwrap_or("");
    let mut lockdirs: Vec<String> = owner
        .filter(|o| o["nodetype"] == "code")
        .and_then(|o| o["codedirs"].as_array())
        .map(|a| a.iter().filter_map(|d| d.as_str().map(String::from)).collect())
        .unwrap_or_default();
    if lockdirs.is_empty() {
        lockdirs.push(tg["dir"].as_str().unwrap_or("{topdir}").to_string());
    }
    let label = labels.join(", ");
    let title = format!("Tests non-green: testgroup \"{label}\" {pass}/{total} in {}", tg_path.trim_start_matches("{topdir}/"));
    let request = format!(
        "The test sweep found testgroup \"{label}\" ({tg_path}) not green: {pass} of {total} checks passing on {when}.\n\
         It belongs to \"{owner_name}\" ({owner_path}) on the architecture map, so this item locks that node's code.\n\n\
         - Reproduce: `\"$ITER_BIN\" runtests --project \"$ITER_PROJECT\" --group \"{first}\" --broken` (a green result parks this item as stale).\n\
         - Fix the CODE the tests describe; if a test itself is wrong, say so in your output and fix the test.\n\
         - Finish with `--fixed` on the same group.\n\n\
         Failing checks (last 30 lines of each):\n\n{logs}\n",
        when = iter_core::now_utc(),
        first = labels.first().cloned().unwrap_or_default(),
        logs = logs.join("\n\n"),
    );
    let mut tags = vec![
        json!({"text": format!("{}tests-non-green", iter_core::dedup::CHECK_TAG_PREFIX), "color": ""}),
        json!({"text": format!("{}{}", iter_core::dedup::CONTAINER_TAG_PREFIX, labels.first().cloned().unwrap_or_default()), "color": ""}),
        json!({"text": "sweep", "color": ""}),
    ];
    // spec R6: the fix carries the use cases that need the red code (usecase:
    // tags drive the queue's use-case progress and priority lineage)
    for u in usecases {
        tags.push(json!({"text": format!("usecase:{u}"), "color": ""}));
    }
    let tags = json!(tags);
    let workid = std::env::var("ITER_WORKID").unwrap_or_default();
    let body = json!({
        "name": title, "agent": "code", "state": "queued",
        "lockdirs": lockdirs, "blockedby": [], "context": [], "model": "", "tags": tags,
        "createdby": workid, "requestedby": if workid.is_empty() { "user" } else { "agent:exec" },
        "prework": [], "postwork": [], "request": request,
    });
    match api.post(&format!("/api/projects/{}/workitems", c.project), &body) {
        Ok(created) => {
            let id = created["id"].as_str().unwrap_or("").to_string();
            if crate::cli::already_open(&created) {
                println!("      fix item already open: …{} (repeat recorded)", &id[id.len().saturating_sub(12)..]);
            } else {
                println!("      filed fix item …{} (code) locking {}", &id[id.len().saturating_sub(12)..], lockdirs.join(", "));
            }
            Some(id)
        }
        Err(e) => {
            eprintln!("      could not file the fix item: {e}");
            None
        }
    }
}

/// `iter sweep --install-schedule --every <N>[m|h]`: create the scheduled exec
/// template (users only — iter_data refuses schedules from engine tokens).
pub fn install_schedule(c: &Conn, every: &str) -> i32 {
    let Some(api) = &c.api else {
        eprintln!("iter sweep: no iter_data connection");
        return 2;
    };
    let every = every.trim();
    let mins: u64 = if let Some(h) = every.strip_suffix('h') {
        h.parse::<u64>().unwrap_or(0) * 60
    } else {
        every.trim_end_matches('m').parse().unwrap_or(0)
    };
    if mins == 0 {
        eprintln!("iter sweep: --every must be like 30m or 4h");
        return 2;
    }
    let body = json!({
        "name": format!("Test sweep, every {every}: run the map's testgroups, file fix items for red ones"),
        "agent": "test", "state": "scheduled", "exec_shell": "iter sweep",
        "sched": {"kind": "every", "every_min": mins},
        "lockdirs": [], "blockedby": [], "context": [], "tags": [{"text": "sweep", "color": ""}],
        "requestedby": "user", "prework": [], "postwork": [],
        "request": "Scheduled test sweep: `iter sweep` reads the architecture map, runs every testgroup a chain from main includes, records results on the map, and files one deduplicated fix item per red group.",
    });
    match api.post(&format!("/api/projects/{}/workitems", c.project), &body) {
        Ok(r) => {
            println!("schedule created: {} (every {mins} min)", r["id"].as_str().unwrap_or("?"));
            0
        }
        Err(e) => {
            eprintln!("iter sweep: could not create the schedule: {e}");
            1
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(id: &str, t: &str, ts: &str) -> Value {
        json!({"id": id, "nodetype": t, "teststate": ts})
    }
    fn e(f: &str, k: &str, t: &str) -> Value {
        json!({"from": f, "kind": k, "to": t})
    }

    #[test]
    fn fix_items_carry_the_use_cases_of_the_node_or_its_owners() {
        let vs = vec![v("ctx", "code", ""), v("cont", "code", ""), v("comp", "code", ""),
            json!({"id": "u1", "nodetype": "usecase", "ucid": "usecase:map-the-repo"}),
            json!({"id": "u2", "nodetype": "usecase", "ucid": "usecase:other"})];
        let es = vec![e("ctx", "codenodes", "cont"), e("cont", "codenodes", "comp"), e("u1", "codenodes", "cont"), e("u2", "codenodes", "elsewhere")];
        assert_eq!(usecases_touching(&vs, &es, "comp"), vec!["map-the-repo".to_string()], "through the owner chain");
        assert!(usecases_touching(&vs, &es, "ctx").is_empty());
    }

    #[test]
    fn teststate_is_per_chain() {
        // main -> A(omit) -> S ; main -> U(usecase, include) -> S ; S owns tg1
        // main -> B(block) -> C(include) owns tg2 ; D is unlinked and owns tg3
        let vs = vec![
            v("m", "main", ""), v("A", "code", "omit"), v("U", "usecase", ""), v("S", "code", ""),
            v("B", "code", "block"), v("C", "code", "include"), v("D", "code", ""),
            v("tg1", "testgroup", ""), v("tg2", "testgroup", ""), v("tg3", "testgroup", ""), v("tg4", "testgroup", ""),
        ];
        let es = vec![
            e("m", "root", "A"), e("A", "codenodes", "S"), e("m", "root", "U"), e("U", "codenodes", "S"),
            e("S", "testgroups", "tg1"), e("m", "root", "B"), e("B", "codenodes", "C"), e("C", "testgroups", "tg2"),
            e("D", "testgroups", "tg3"), e("A", "testgroups", "tg4"),
        ];
        let r: HashMap<String, (bool, String)> = eligible(&vs, &es).into_iter().map(|(t, _, ok, why)| (t, (ok, why))).collect();
        assert_eq!(r["tg1"], (true, "included".into()), "the use-case chain includes S");
        assert_eq!(r["tg2"], (false, "blocked".into()), "include below a block stays blocked");
        assert_eq!(r["tg3"].0, false);
        assert_eq!(r["tg4"], (false, "omitted on every chain".into()));
    }
}
