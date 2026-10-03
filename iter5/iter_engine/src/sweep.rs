//! `iter sweep` (iter5): run the project's test nodes off the project graph
//! and turn what it finds into work items.
//!
//! The graph (`GET /api/projects/{p}/graph` → `{nodes, edges}`) says which
//! test nodes exist and which node owns each (`tests` edges). Teststate is
//! evaluated per chain, as in iter4: chains start at the project node and at
//! every use case / actor, and run down `codenodes` and `uses` edges; a test
//! node runs when its own teststate is not `omit`/`block` and at least one
//! chain reaching an owner says include. Each run's aggregated standard result
//! is posted to `POST …/graph/nodes/{id}/testresult`; a failed node files one
//! `code` fix item locked to its owner's codedirs (deduplicated by the
//! `check:` + `container:` keys `iter runtests` uses). "Could not run" is
//! recorded but files nothing — an infrastructure problem is not a defect.
//!
//! Then the filing halves: leaf code nodes with no runnable tests → one
//! `test` item each; test nodes short of their `coverage` → one top-up item
//! per owner; code nodes whose text breaks the node-text rules → `ingest`.
//!
//! Scheduled as the project's engine-owned "Test sweep" (`iter sweep`).

use crate::client::Api;
use crate::sync::Conn;
use iter_core::nodefile::{self, NodeDoc};
use iter_core::testresult::Outcome;
use iter_local::runtests as rt;
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet};
use std::path::Path;

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
enum Chain {
    Include,
    Omit,
    Block,
}

fn step(state: Chain, own: &str) -> Chain {
    match (state, own) {
        (Chain::Block, _) => Chain::Block, // only a human lifts a block, and only on the node itself
        (_, "block" | "blocked") => Chain::Block,
        (_, "omit") => Chain::Omit,
        (_, "include") => Chain::Include,
        (s, _) => s, // inherit / unset
    }
}

fn s<'a>(v: &'a Value, k: &str) -> &'a str {
    v.get(k).and_then(|x| x.as_str()).unwrap_or("")
}

/// Every chain state each node is reached with (empty = unreachable).
fn chain_states<'a>(nodes: &'a [Value], edges: &'a [Value]) -> HashMap<&'a str, Vec<Chain>> {
    let byid: HashMap<&str, &Value> = nodes.iter().filter_map(|v| v["id"].as_str().map(|i| (i, v))).collect();
    let mut out_edges: HashMap<&str, Vec<&str>> = HashMap::new();
    for e in edges {
        if let (Some(f), Some(k), Some(t)) = (e["from"].as_str(), e["kind"].as_str(), e["to"].as_str()) {
            if k == "codenodes" || k == "uses" {
                out_edges.entry(f).or_default().push(t);
            }
        }
    }
    let mut stack: Vec<(&str, Chain)> = nodes
        .iter()
        .filter(|v| matches!(s(v, "nodetype"), "project" | "usecase" | "actor"))
        .filter_map(|v| Some((v["id"].as_str()?, step(Chain::Include, s(v, "teststate")))))
        .collect();
    let mut seen: HashSet<(&str, Chain)> = HashSet::new();
    while let Some((cur, st)) = stack.pop() {
        if !seen.insert((cur, st)) {
            continue;
        }
        for to in out_edges.get(cur).cloned().unwrap_or_default() {
            let own = byid.get(to).map(|v| s(v, "teststate")).unwrap_or("");
            stack.push((to, step(st, own)));
        }
    }
    let mut out: HashMap<&str, Vec<Chain>> = HashMap::new();
    for (v, st) in &seen {
        out.entry(v).or_default().push(*st);
    }
    out
}

/// For each test node: (test id, owner id, runs?, why). A test node owned by
/// several nodes is listed once per owner; it runs when any owner allows it.
pub fn eligible(nodes: &[Value], edges: &[Value]) -> Vec<(String, String, bool, String)> {
    let states = chain_states(nodes, edges);
    let own: HashMap<&str, &str> = nodes.iter().filter_map(|v| Some((v["id"].as_str()?, s(v, "teststate").trim()))).collect();
    let is_test: HashSet<&str> = nodes.iter().filter(|v| s(v, "nodetype") == "test").filter_map(|v| v["id"].as_str()).collect();
    let mut result = Vec::new();
    for e in edges.iter().filter(|e| e["kind"] == "tests") {
        let (Some(owner), Some(tn)) = (e["from"].as_str(), e["to"].as_str()) else { continue };
        if !is_test.contains(tn) {
            continue;
        }
        let st = states.get(owner).cloned().unwrap_or_default();
        let (ok, why) = match own.get(tn).copied() {
            Some("omit") => (false, "omitted on the test node itself".to_string()),
            Some("block" | "blocked") => (false, "blocked on the test node itself".to_string()),
            _ if st.is_empty() => (false, "owner not reachable from the project, a use case or an actor".to_string()),
            _ if st.contains(&Chain::Include) => (true, "included".to_string()),
            _ if st.contains(&Chain::Omit) => (false, "omitted on every chain".to_string()),
            _ => (false, "blocked".to_string()),
        };
        result.push((tn.to_string(), owner.to_string(), ok, why));
    }
    result.sort();
    result.dedup();
    result
}

fn doc_of(v: &Value) -> Option<NodeDoc> {
    serde_json::from_value(v.clone()).ok()
}

/// `{topdir}/x/**` style codedirs of a node → lock paths (`{topdir}/x/`).
pub fn lockdirs_of(node: &Value) -> Vec<String> {
    let path = s(node, "path");
    let mut out: Vec<String> = node["children"]["codedirs"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|x| x.as_str())
        .map(|e| {
            let p = nodefile::expand_entry(e, path, None);
            let p = p.trim_end_matches("**").trim_end_matches("/*").to_string();
            if p.ends_with('/') { p } else { format!("{p}/") }
        })
        .filter(|p| p.starts_with("{topdir}"))
        .collect();
    out.sort();
    out.dedup();
    if out.is_empty() && !path.is_empty() {
        out.push(format!("{}/", nodefile::dir_of(path)));
    }
    out
}

pub struct SweepOpts {
    /// only this test node (id, name or path)
    pub node: Option<String>,
    pub dry_run: bool,
    pub file_items: bool,
    pub timeout_min: u64,
    pub text_max: usize,
    pub tests_max: usize,
    pub coverage_max: usize,
}

/// Graph nodes + edges of the project (deleted nodes are not listed).
pub fn fetch_graph(api: &Api, project: &str) -> Result<(Vec<Value>, Vec<Value>), String> {
    let g = api.get(&format!("/api/projects/{project}/graph")).map_err(|e| format!("cannot read the graph: {e}"))?;
    let nodes = g["nodes"].as_array().cloned().unwrap_or_default().into_iter().filter(|n| n["deleted"] != true).collect();
    Ok((nodes, g["edges"].as_array().cloned().unwrap_or_default()))
}

/// `iter sweep`. Exit 0 all green, 1 something failed / could not run, 2 could not sweep.
pub fn sweep_verb(c: &Conn, o: &SweepOpts) -> i32 {
    let Some(api) = &c.api else {
        eprintln!("iter sweep: no iter_data connection (the sweep reads the project graph from iter_data)");
        return 2;
    };
    let (nodes, edges) = match fetch_graph(api, &c.project) {
        Ok(g) => g,
        Err(e) => {
            eprintln!("iter sweep: {e}");
            return 2;
        }
    };
    if nodes.is_empty() {
        eprintln!("iter sweep: project {} has no nodes yet", c.project);
        return 2;
    }
    let byid: HashMap<String, Value> = nodes.iter().filter_map(|v| v["id"].as_str().map(|i| (i.to_string(), v.clone()))).collect();
    let plan = eligible(&nodes, &edges);
    let mut decided: HashMap<String, (bool, String, String)> = HashMap::new(); // test id → (ok, owner, why)
    for (tn, owner, ok, why) in &plan {
        let e = decided.entry(tn.clone()).or_insert((*ok, owner.clone(), why.clone()));
        if *ok && !e.0 {
            *e = (true, owner.clone(), why.clone());
        }
    }
    let mut ids: Vec<&String> = decided.keys().collect();
    ids.sort_by_key(|id| s(&byid[*id], "path").to_string());
    let mut worst = 0;
    let (mut ran, mut red, mut skipped) = (0, 0, 0);
    for tn in ids {
        let t = &byid[tn];
        let (ok, owner_id, why) = &decided[tn];
        let path = s(t, "path");
        if let Some(n) = &o.node {
            if n != tn && n != s(t, "name") && n != path && !path.ends_with(n.as_str()) {
                continue;
            }
        }
        if !ok {
            skipped += 1;
            println!("skip  {path} ({why})");
            continue;
        }
        if o.dry_run {
            println!("would run {path}");
            continue;
        }
        // the scripts are on disk: read the node file itself when present
        let doc = rt::load_test_node(&c.topdir, &c.topdir.join(path.trim_start_matches("{topdir}/"))).ok().or_else(|| doc_of(t));
        let Some(doc) = doc else { continue };
        ran += 1;
        let run = match rt::run_node(&c.topdir, &doc, None, o.timeout_min) {
            Ok(r) => r,
            Err(e) => {
                println!("error {path}: {e}");
                worst = 1;
                continue;
            }
        };
        let tot = run.result.totals();
        println!("{:<6} {path} {}/{}", rt::outcome_word(run.outcome), tot.pass, tot.total);
        if run.outcome != Outcome::Pass {
            worst = 1;
            red += 1;
            if o.file_items && run.outcome == Outcome::Fail {
                let ucs = usecases_touching(&nodes, &edges, owner_id);
                file_fix_item(api, c, &run, byid.get(owner_id), &ucs);
            }
        }
        // the sweep owns filing (its own fix item above, none for could-not-run):
        // the server must not file a second, differently keyed item
        let body = serde_json::json!({"result": run.result, "outcome": rt::outcome_word(run.outcome), "file_workitem": false});
        if let Err(e) = api.post(&format!("/api/projects/{}/graph/nodes/{tn}/testresult", c.project), &body) {
            eprintln!("could not record the result on {path}: {e}");
        }
    }
    println!("sweep: {ran} test node(s) run, {red} not green, {skipped} skipped by teststate");
    if o.node.is_none() {
        if o.tests_max > 0 {
            untested_sweep(api, c, &nodes, &edges, o);
        }
        if o.coverage_max > 0 {
            coverage_sweep(api, c, &nodes, &edges, o);
        }
        if o.text_max > 0 {
            text_sweep(api, c, &nodes, o);
        }
    }
    worst
}

/// Leaf code nodes (no child code nodes; connections excluded) some chain
/// includes, with no linked test node that has a script on disk.
pub fn untested(topdir: &Path, nodes: &[Value], edges: &[Value]) -> Vec<Value> {
    let states = chain_states(nodes, edges);
    let byid: HashMap<&str, &Value> = nodes.iter().filter_map(|v| v["id"].as_str().map(|i| (i, v))).collect();
    let mut out: Vec<Value> = nodes
        .iter()
        .filter(|v| s(v, "nodetype") == "code" && s(v, "level") != "connection")
        .filter(|v| {
            let id = s(v, "id");
            let runnable = edges
                .iter()
                .filter(|e| e["from"] == id && e["kind"] == "tests")
                .filter_map(|e| e["to"].as_str().and_then(|t| byid.get(t)))
                .filter_map(|t| doc_of(t))
                .any(|d| rt::scripts_of(topdir, &d).iter().any(|p| p.is_file()));
            let has_code_children = edges.iter().any(|e| e["from"] == id && e["kind"] == "codenodes" && e["to"].as_str().and_then(|t| byid.get(t)).is_some_and(|t| s(t, "nodetype") == "code"));
            let included = states.get(id).is_some_and(|s| s.contains(&Chain::Include));
            !runnable && !has_code_children && included
        })
        .cloned()
        .collect();
    out.sort_by(|a, b| s(a, "path").cmp(s(b, "path")));
    out
}

/// What keeps a test node short of its `coverage` targets ({normal,
/// longtail, failure} counts vs its last result's bucket totals).
pub fn coverage_gaps(t: &Value) -> Vec<String> {
    let front = &t["front"];
    let mut gaps = Vec::new();
    let Some(cov) = front["coverage"].as_object().filter(|_| !s(front, "input_space").trim().is_empty()) else {
        return vec!["input space not assessed: no `input_space` and `coverage` targets".into()];
    };
    let last = if t["test"].is_object() { &t["test"] } else { &front["last_result"] };
    for b in ["normal", "longtail", "failure"] {
        let want = cov.get(b).and_then(|x| x.as_u64()).unwrap_or(0).max(if b == "normal" { 1 } else { 0 });
        let have = last[b]["total"].as_u64().unwrap_or(0);
        if have < want {
            gaps.push(format!("{b}: {have} of {want}"));
        }
    }
    gaps
}

/// Owners some chain includes whose test nodes (with scripts) fall short.
pub fn under_covered(topdir: &Path, nodes: &[Value], edges: &[Value]) -> Vec<(Value, Vec<(String, Vec<String>)>)> {
    let states = chain_states(nodes, edges);
    let byid: HashMap<&str, &Value> = nodes.iter().filter_map(|v| v["id"].as_str().map(|i| (i, v))).collect();
    let mut out = Vec::new();
    for owner in nodes {
        let id = s(owner, "id");
        if !states.get(id).is_some_and(|s| s.contains(&Chain::Include)) {
            continue;
        }
        let short: Vec<(String, Vec<String>)> = edges
            .iter()
            .filter(|e| e["from"] == id && e["kind"] == "tests")
            .filter_map(|e| e["to"].as_str().and_then(|t| byid.get(t)))
            .filter(|t| s(t, "teststate") != "omit")
            .filter(|t| doc_of(t).is_some_and(|d| rt::scripts_of(topdir, &d).iter().any(|p| p.is_file())))
            .filter_map(|t| {
                let g = coverage_gaps(t);
                (!g.is_empty()).then(|| (s(t, "path").to_string(), g))
            })
            .collect();
        if !short.is_empty() {
            out.push((owner.clone(), short));
        }
    }
    out.sort_by(|a, b| s(&a.0, "path").cmp(s(&b.0, "path")));
    out
}

fn post_item(api: &Api, c: &Conn, body: Value, what: &str, path: &str) -> bool {
    match api.post(&format!("/api/projects/{}/workitems", c.project), &body) {
        Ok(created) if crate::cli::already_open(&created) => false,
        Ok(created) => {
            let id = created["id"].as_str().unwrap_or("");
            println!("  filed {what} item …{} for {path}", &id[id.len().saturating_sub(12)..]);
            true
        }
        Err(e) => {
            eprintln!("  could not file the {what} item for {path}: {e}");
            false
        }
    }
}

fn item(name: String, agent: &str, lockdirs: Vec<String>, tags: Vec<Value>, request: String, node: &str) -> Value {
    let workid = std::env::var("ITER_WORKID").unwrap_or_default();
    json!({"name": name, "agent": agent, "state": "queued", "lockdirs": lockdirs, "blockedby": [], "context": [], "model": "",
        "tags": tags, "createdby": workid, "requestedby": if workid.is_empty() { "user" } else { "agent:exec" },
        "prework": [], "postwork": [], "request": request, "node": node})
}

fn key_tags(check: &str, container: &str, ucs: &[String]) -> Vec<Value> {
    let mut t = vec![
        json!({"text": format!("{}{check}", iter_core::dedup::CHECK_TAG_PREFIX), "color": ""}),
        json!({"text": format!("{}{container}", iter_core::dedup::CONTAINER_TAG_PREFIX), "color": ""}),
        json!({"text": "sweep", "color": ""}),
    ];
    for u in ucs {
        t.push(json!({"text": format!("usecase:{u}"), "color": ""}));
    }
    t
}

fn untested_sweep(api: &Api, c: &Conn, nodes: &[Value], edges: &[Value], o: &SweepOpts) {
    let list = untested(&c.topdir, nodes, edges);
    println!("no tests: {} code node(s) have no runnable tests", list.len());
    let mut filed = 0;
    for v in &list {
        if filed >= o.tests_max {
            println!("no tests: stopping at {} new item(s) this sweep (--tests-max)", o.tests_max);
            break;
        }
        let (path, name) = (s(v, "path"), s(v, "name"));
        if o.dry_run {
            println!("  would file: tests for {path}");
            filed += 1;
            continue;
        }
        let dir = nodefile::dir_of(path);
        let stem = nodefile::stem_of(&nodefile::file_name_of(path));
        let test_file = format!("{dir}/{stem}.test.iter.md");
        let request = format!(
            "Code node \"{name}\" ({path}) has no runnable tests, so the test sweep cannot tell whether its code works. Write its first tests.\n\n\
             - Read the node file and the code it owns ({}) to learn what the part DOES — its inputs, outputs and the promises other parts rely on.\n\
             - Create the test node {test_file} (`iter validate --fix` conforms it) with a `## Planned tests` list, simplest first. Its `children.tests` names the scripts (default `{{thisfiledir}}/tests/{{thisfilestem}}*.sh`).\n\
             - Write deterministic shell scripts under `{dir}/tests/`: exit 0 = pass, 1 = fail; the LAST stdout line is the standard result JSON \
               `{{\"name\":…,\"id\":…,\"overall_success\":…,\"normal\":{{\"total\":…,\"pass\":…,\"err\":…}},\"longtail\":{{…}},\"failure\":{{…}}}}` (normal = expected paths and tolerated malformed input, longtail = rare valid input, failure = what must be refused). Output files go only under `$ITER_TEST_OUT`.\n\
             - Write `input_space` (what the code accepts, roughly how many practical permutations) and `coverage: {{normal: N, longtail: N, failure: N}}` on the test node.\n\
             - Link it: add the test node's path to `children.tests` in {path}.\n\
             - Run `iter runtests {test_file}` and report what is green and what is red. Do not change the code under test: a red test is filed as a `code` item by the next sweep.",
            lockdirs_of(v).join(", "),
        );
        let tags = key_tags("no-tests", path, &usecases_touching(nodes, edges, s(v, "id")));
        if post_item(api, c, item(format!("Tests first: write the first tests for {name}"), "test", vec![path.to_string(), format!("{dir}/tests/"), test_file.clone()], tags, request, s(v, "id")), "test", path) {
            filed += 1;
        }
    }
}

fn coverage_sweep(api: &Api, c: &Conn, nodes: &[Value], edges: &[Value], o: &SweepOpts) {
    let list = under_covered(&c.topdir, nodes, edges);
    println!("coverage: {} node(s) have tests short of their coverage", list.len());
    let mut filed = 0;
    for (v, short) in &list {
        if filed >= o.coverage_max {
            println!("coverage: stopping at {} new item(s) this sweep (--coverage-max)", o.coverage_max);
            break;
        }
        let (path, name) = (s(v, "path"), s(v, "name"));
        if o.dry_run {
            println!("  would file: coverage top-up for {path} ({} test node(s))", short.len());
            filed += 1;
            continue;
        }
        let listing: Vec<String> = short.iter().map(|(f, g)| format!("- {f}: {}", g.join("; "))).collect();
        let mut lockdirs: Vec<String> = short.iter().map(|(f, _)| format!("{}/", nodefile::dir_of(f))).collect();
        lockdirs.sort();
        lockdirs.dedup();
        let request = format!(
            "The tests of \"{name}\" ({path}) fall short of their coverage. Bring each test node below up to it.\n\n{}\n\n\
             Coverage is counted in the standard result's buckets, sized by the input space (how many practical permutations the code accepts): \
             normal (expected paths, tolerated malformed or missing input — at least one), longtail (rare but valid input), failure (input or states it must refuse).\n\
             - Write `input_space` and `coverage: {{normal: N, longtail: N, failure: N}}` on each test node (0 only for a bucket that cannot apply; say why in input_space).\n\
             - Write the missing tests in its scripts (each prints the standard result JSON as its last line), never deleting one.\n\
             - Run `iter runtests <test node>` and report what is green and what is red. Do not change the code under test.",
            listing.join("\n"),
        );
        let tags = key_tags("test-coverage", path, &usecases_touching(nodes, edges, s(v, "id")));
        if post_item(api, c, item(format!("Test coverage: top up the tests for {name}"), "test", lockdirs, tags, request, s(v, "id")), "coverage", path) {
            filed += 1;
        }
    }
}

fn text_sweep(api: &Api, c: &Conn, nodes: &[Value], o: &SweepOpts) {
    let mut failing: Vec<(&Value, Vec<(&'static str, String)>)> = nodes
        .iter()
        .filter(|v| s(v, "nodetype") == "code")
        .filter_map(|v| {
            let f = iter_local::validate::node_text_findings(s(v, "desc"), s(v, "body"));
            (!f.is_empty()).then_some((v, f))
        })
        .collect();
    failing.sort_by(|a, b| s(a.0, "path").cmp(s(b.0, "path")));
    println!("node text: {} code node(s) break the node-text rules", failing.len());
    let mut filed = 0;
    for (v, findings) in failing {
        if filed >= o.text_max {
            println!("node text: stopping at {} new item(s) this sweep (--text-max)", o.text_max);
            break;
        }
        let (path, name) = (s(v, "path"), s(v, "name"));
        let codes: Vec<&str> = findings.iter().map(|f| f.0).collect();
        if o.dry_run {
            println!("  would file: {path} ({})", codes.join(", "));
            filed += 1;
            continue;
        }
        let request = format!(
            "Bring the text of code node \"{name}\" ({path}) up to the node-text rules so a reader who has never seen the code understands what this part DOES and why it matters.\n\n\
             Findings:\n{}\n\nRead the code it owns first: {}.\n\
             Then rewrite, in the node file only: `name` (what a person would call it), `desc` (about 100 words: it does X so that Y — enough for an agent to decide whether to read the whole file) and the body \
             (3–6 short paragraphs: what, how — key functions and files, what it takes and hands on, why it matters, one example). Keep `id` and `children` exactly as they are. Finish with `iter validate` clean for this file.",
            findings.iter().map(|f| format!("- {}: {}", f.0, f.1)).collect::<Vec<_>>().join("\n"),
            lockdirs_of(v).join(", "),
        );
        let tags = key_tags("node-text", path, &[]);
        let mut it = item(format!("Node text: {name} — {}", codes.join(", ")), "ingest", vec![path.to_string()], tags, request, s(v, "id"));
        it["requestedby"] = json!("agent:test");
        if post_item(api, c, it, "node-text", path) {
            filed += 1;
        }
    }
}

/// The use cases (by file stem) that use this node or one of its owners.
pub fn usecases_touching(nodes: &[Value], edges: &[Value], node: &str) -> Vec<String> {
    let is_code = |id: &str| nodes.iter().any(|v| v["id"] == id && s(v, "nodetype") == "code");
    let mut chain = vec![node.to_string()];
    let mut cur = node.to_string();
    for _ in 0..16 {
        let parent = edges.iter().find(|e| e["kind"] == "codenodes" && e["to"] == cur.as_str() && e["from"].as_str().is_some_and(is_code));
        match parent.and_then(|e| e["from"].as_str()) {
            Some(p) if !chain.iter().any(|c| c == p) => {
                chain.push(p.to_string());
                cur = p.to_string();
            }
            _ => break,
        }
    }
    let mut out: Vec<String> = nodes
        .iter()
        .filter(|v| s(v, "nodetype") == "usecase")
        .filter(|u| edges.iter().any(|e| e["from"] == u["id"] && (e["kind"] == "uses" || e["kind"] == "codenodes") && chain.iter().any(|c| e["to"] == c.as_str())))
        .map(|u| nodefile::file_name_of(s(u, "path")).trim_end_matches(".usecase.iter.md").to_string())
        .collect();
    out.sort();
    out.dedup();
    out
}

/// One fix item per failed test node, locked to the owner's codedirs.
fn file_fix_item(api: &Api, c: &Conn, run: &rt::NodeRun, owner: Option<&Value>, usecases: &[String]) -> Option<String> {
    let owner_name = owner.map(|o| s(o, "name")).unwrap_or("?");
    let owner_path = owner.map(|o| s(o, "path")).unwrap_or("");
    let mut lockdirs = owner.filter(|o| s(o, "nodetype") == "code").map(lockdirs_of).unwrap_or_default();
    if lockdirs.is_empty() {
        lockdirs.push(format!("{}/", nodefile::dir_of(&run.path)));
    }
    let tot = run.result.totals();
    let logs: Vec<String> = run
        .scripts
        .iter()
        .filter(|t| t.outcome != Outcome::Pass)
        .map(|t| format!("### {} exit {}\n```\n{}\n```", t.script, t.exit_code, rt::tail_bytes(&t.log, 3000)))
        .collect();
    let request = format!(
        "The test sweep found test node \"{name}\" ({path}) failing: {pass} of {total} checks passing on {when}.\n\
         It tests \"{owner_name}\" ({owner_path}), so this item locks that node's code.\n\n\
         - Reproduce: `\"$ITER_BIN\" runtests \"{path}\" --broken` (a green result parks this item as stale).\n\
         - Fix the CODE the tests describe; if a test itself is wrong, say so in your output and fix the test.\n\
         - Finish with `\"$ITER_BIN\" runtests \"{path}\" --fixed`.\n\
         - If the fix is very complex or risky (a redesign, a change across several parts, a data migration), do not attempt it: file one `plan` item with `\"$ITER_BIN\" add --agent plan` naming this test node and why it is not a simple fix, then `\"$ITER_BIN\" wait --on <that item's id>` and end your turn.\n\n\
         Failing scripts:\n\n{logs}\n",
        name = run.name,
        path = run.path,
        pass = tot.pass,
        total = tot.total,
        when = iter_core::now_utc(),
        logs = logs.join("\n\n"),
    );
    let tags = key_tags("tests-non-green", &run.name, usecases);
    let body = item(format!("Tests failing: \"{}\" {}/{} in {}", run.name, tot.pass, tot.total, run.path.trim_start_matches("{topdir}/")), "code", lockdirs.clone(), tags, request, owner.map(|o| s(o, "id")).unwrap_or(""));
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

/// `iter sweep --install-schedule --every <N>[m|h]`: turn the project's one
/// test sweep on at this interval.
pub fn install_schedule(c: &Conn, every: &str) -> i32 {
    let Some(api) = &c.api else {
        eprintln!("iter sweep: no iter_data connection");
        return 2;
    };
    let every = every.trim();
    let mins: u64 = if let Some(h) = every.strip_suffix('h') { h.parse::<u64>().unwrap_or(0) * 60 } else { every.trim_end_matches('m').parse().unwrap_or(0) };
    if mins == 0 {
        eprintln!("iter sweep: --every must be like 30m or 4h");
        return 2;
    }
    let items = match api.get(&format!("/api/projects/{}/workitems", c.project)) {
        Ok(v) => v.as_array().cloned().unwrap_or_default(),
        Err(e) => {
            eprintln!("iter sweep: cannot read the work items: {e}");
            return 2;
        }
    };
    let existing = items.into_iter().find(|i| i["system"] == iter_core::TEST_SWEEP && !i["sched"].is_null());
    let result = match existing {
        Some(mut tpl) => {
            let id = tpl["id"].as_str().unwrap_or("").to_string();
            let version = tpl["version"].as_u64().unwrap_or(0);
            tpl["state"] = json!("scheduled");
            tpl["sched"]["every_min"] = json!(mins);
            api.put(&format!("/api/projects/{}/workitems/{id}?expect_version={version}", c.project), &tpl).map(|_| id)
        }
        None => api
            .post(&format!("/api/projects/{}/workitems", c.project), &iter_core::test_sweep_template_body("scheduled", mins))
            .map(|r| r["id"].as_str().unwrap_or("?").to_string()),
    };
    match result {
        Ok(id) => {
            println!("test sweep on: {id} (every {mins} min)");
            0
        }
        Err(e) => {
            eprintln!("iter sweep: could not turn the test sweep on: {e}");
            1
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client::fake;
    use std::sync::{Arc, Mutex};

    fn v(id: &str, t: &str, ts: &str) -> Value {
        json!({"id": id, "nodetype": t, "teststate": ts, "path": format!("{{topdir}}/{id}/{id}.{t}.iter.md"), "children": {}})
    }
    fn e(f: &str, k: &str, t: &str) -> Value {
        json!({"from": f, "kind": k, "to": t})
    }

    #[test]
    fn teststate_is_per_chain() {
        // P -> A(omit) -> S ; U(usecase) uses S ; S tests t1
        // P -> B(block) -> C(include) tests t2 ; D unlinked tests t3 ; A tests t4 ; S tests t5(omit)
        let vs = vec![
            v("P", "project", ""), v("A", "code", "omit"), v("U", "usecase", ""), v("S", "code", ""),
            v("B", "code", "block"), v("C", "code", "include"), v("D", "code", ""),
            v("t1", "test", ""), v("t2", "test", ""), v("t3", "test", ""), v("t4", "test", ""), v("t5", "test", "omit"),
        ];
        let es = vec![
            e("P", "codenodes", "A"), e("A", "codenodes", "S"), e("U", "uses", "S"), e("S", "tests", "t1"),
            e("P", "codenodes", "B"), e("B", "codenodes", "C"), e("C", "tests", "t2"), e("D", "tests", "t3"), e("A", "tests", "t4"), e("S", "tests", "t5"),
        ];
        let r: HashMap<String, (bool, String)> = eligible(&vs, &es).into_iter().map(|(t, _, ok, why)| (t, (ok, why))).collect();
        assert_eq!(r["t1"], (true, "included".into()), "the use-case chain includes S");
        assert_eq!(r["t2"], (false, "blocked".into()), "include below a block stays blocked");
        assert!(!r["t3"].0);
        assert_eq!(r["t4"], (false, "omitted on every chain".into()));
        assert_eq!(r["t5"], (false, "omitted on the test node itself".into()));
    }

    #[test]
    fn untested_coverage_and_usecases() {
        let top = std::env::temp_dir().join(format!("iter5_sweep_{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(top.join("L1/tests")).unwrap();
        std::fs::write(top.join("L1/tests/t1x.sh"), "exit 0").unwrap();
        let tnode = |id: &str, dir: &str, front: Value| json!({"id": id, "nodetype": "test", "teststate": "", "path": format!("{{topdir}}/{dir}/{id}.test.iter.md"),
            "children": {"tests": ["{thisfiledir}/tests/{thisfilestem}*.sh"]}, "front": front});
        let vs = vec![
            v("P", "project", ""), v("Par", "code", ""), v("L1", "code", ""), v("L2", "code", ""), v("L3", "code", "omit"), v("L4", "code", ""),
            json!({"id": "conn", "nodetype": "code", "level": "connection", "path": "{topdir}/global/connections/api.code.iter.md", "children": {}}),
            tnode("t1", "L1", json!({"input_space": "x", "coverage": {"normal": 2, "longtail": 1, "failure": 0}, "last_result": {"normal": {"total": 2}, "longtail": {"total": 0}}})),
            tnode("t4", "L4", json!({})),
            json!({"id": "U", "nodetype": "usecase", "path": "{topdir}/global/usecases/buy.usecase.iter.md"}),
        ];
        let es = vec![
            e("P", "codenodes", "Par"), e("Par", "codenodes", "L1"), e("Par", "codenodes", "L2"), e("Par", "codenodes", "L3"),
            e("Par", "codenodes", "L4"), e("P", "codenodes", "conn"), e("L1", "tests", "t1"), e("L4", "tests", "t4"), e("U", "uses", "Par"),
        ];
        let ids: Vec<String> = untested(&top, &vs, &es).iter().map(|x| x["id"].as_str().unwrap().to_string()).collect();
        assert_eq!(ids, vec!["L2".to_string(), "L4".to_string()], "L4's test node has no script on disk; connections and parents are skipped");
        let uc = under_covered(&top, &vs, &es);
        assert_eq!(uc.len(), 1);
        assert_eq!(uc[0].1[0].1, vec!["longtail: 0 of 1".to_string()]);
        assert_eq!(usecases_touching(&vs, &es, "L1"), vec!["buy".to_string()], "through the owner chain");
        assert_eq!(coverage_gaps(&json!({"front": {}})), vec!["input space not assessed: no `input_space` and `coverage` targets".to_string()]);
    }

    #[test]
    fn lockdirs_from_codedirs() {
        let n = json!({"path": "{topdir}/src/a/a.code.iter.md", "children": {"codedirs": ["{thisfiledir}/**", "lib/"]}});
        assert_eq!(lockdirs_of(&n), vec!["{topdir}/src/a/", "{topdir}/src/a/lib/"]);
        assert_eq!(lockdirs_of(&json!({"path": "{topdir}/b/b.code.iter.md", "children": {}})), vec!["{topdir}/b/"]);
    }

    #[test]
    fn sweep_runs_records_and_files_a_fix_item() {
        let top = std::env::temp_dir().join(format!("iter5_sweep_run_{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(top.join("a/tests")).unwrap();
        let mut tdoc = NodeDoc::new(iter_core::nodefile::NodeType::Test, "a tests", "t", "2026-10-02 00:00:00Z");
        tdoc.path = "{topdir}/a/a.test.iter.md".into();
        std::fs::write(top.join("a/a.test.iter.md"), nodefile::render(&tdoc)).unwrap();
        std::fs::write(top.join("a/tests/a01.sh"), "echo '{\"normal\":{\"total\":2,\"pass\":1,\"err\":1}}'\nexit 1\n").unwrap();
        let tid = tdoc.id.clone();
        let graph = json!({"nodes": [
            {"id": "P", "nodetype": "project", "path": "{topdir}/global/p.project.iter.md", "children": {}},
            {"id": "A", "nodetype": "code", "level": "component", "name": "A", "desc": "Does a things and hands them on.", "body": "word ".repeat(80), "path": "{topdir}/a/a.code.iter.md", "children": {"codedirs": ["{thisfiledir}/"]}},
            serde_json::to_value(&tdoc).unwrap(),
        ], "edges": [{"from": "P", "kind": "codenodes", "to": "A"}, {"from": "A", "kind": "tests", "to": tid}]});
        let filed = Arc::new(Mutex::new(Vec::<Value>::new()));
        let f2 = filed.clone();
        let srv = fake::serve(move |m, path, body| {
            if m == "GET" && path.ends_with("/graph") {
                return Some((200, graph.clone()));
            }
            if m == "POST" && path.ends_with("/workitems") {
                f2.lock().unwrap().push(body.clone());
                return Some((200, json!({"id": "w-000000000001"})));
            }
            Some((200, json!({})))
        });
        let c = Conn { api: Some(srv.api()), project: "p".into(), topdir: top.clone(), source: "test".into() };
        let o = SweepOpts { node: None, dry_run: false, file_items: true, timeout_min: 1, text_max: 5, tests_max: 5, coverage_max: 0 };
        assert_eq!(sweep_verb(&c, &o), 1);
        let rec = srv.calls_to("POST", &format!("/graph/nodes/{tid}/testresult"));
        assert_eq!(rec.len(), 1);
        assert_eq!(rec[0]["file_workitem"], false, "the sweep files the fix item itself: the server must not file a second");
        assert_eq!(rec[0]["outcome"], "failed");
        assert_eq!(rec[0]["result"]["overall_success"], false);
        assert_eq!(rec[0]["result"]["normal"], json!({"total": 2, "pass": 1, "err": 1}));
        assert_eq!(rec[0]["result"]["id"], tid.as_str());
        let items = filed.lock().unwrap().clone();
        assert_eq!(items.len(), 1, "one fix item; A has runnable tests and fine text: {items:?}");
        assert_eq!(items[0]["agent"], "code");
        assert_eq!(items[0]["lockdirs"], json!(["{topdir}/a/"]));
        assert!(items[0]["tags"].as_array().unwrap().iter().any(|t| t["text"] == format!("{}tests-non-green", iter_core::dedup::CHECK_TAG_PREFIX)));
    }
}
