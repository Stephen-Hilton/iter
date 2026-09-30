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

/// Every chain state each vertex is reached with, walking the executable DAG
/// down from main (empty for a vertex main does not reach).
fn chain_states<'a>(vertices: &'a [Value], edges: &'a [Value]) -> HashMap<&'a str, Vec<Chain>> {
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
    owner_states
}

/// For each testgroup vertex id: its owner id and whether some chain from
/// main includes it (the reasons are kept for the report).
pub fn eligible(vertices: &[Value], edges: &[Value]) -> Vec<(String, String, bool, String)> {
    let owner_states = chain_states(vertices, edges);
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
    /// file a `test` item for each code node with no tests, at most this many
    /// new ones per sweep (0 = skip)
    pub tests_max: usize,
    /// file a `test` top-up item for each node whose tests fall short of
    /// their coverage, at most this many new ones per sweep (0 = skip)
    pub coverage_max: usize,
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
            match rt::run_group_stamped(&file, label, None, o.timeout_min, false) {
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
    if o.tests_max > 0 && o.group.is_none() {
        untested_sweep(api, c, &vertices, &edges, o);
    }
    if o.coverage_max > 0 && o.group.is_none() {
        coverage_sweep(api, c, &vertices, &edges, o);
    }
    if o.text_max > 0 && o.group.is_none() {
        text_sweep(api, c, &vertices, o);
    }
    worst
}

/// Code nodes the sweep should ask the `test` agent to write tests for
/// (2026-09-30): a LEAF code node (no child code nodes — a parent's tests
/// live with its parts) that has no tests at all — no tests file linked, or
/// linked files with no test registered in any group — and that some chain
/// from main includes (an omitted or blocked node is left alone, as its
/// tests would be). Sorted by path.
pub fn untested(vertices: &[Value], edges: &[Value]) -> Vec<Value> {
    let states = chain_states(vertices, edges);
    let is_code = |id: &str| vertices.iter().any(|v| v["id"] == id && v["nodetype"] == "code");
    let mut out: Vec<Value> = vertices
        .iter()
        .filter(|v| v["nodetype"] == "code")
        .filter(|v| {
            let id = v["id"].as_str().unwrap_or("");
            let registered: u64 = edges
                .iter()
                .filter(|e| e["from"] == id && (e["kind"] == "tests" || e["kind"] == "testgroups"))
                .filter_map(|e| vertices.iter().find(|t| t["id"] == e["to"]))
                .flat_map(|t| t["groups"].as_array().cloned().unwrap_or_default())
                .map(|g| g["tests"].as_u64().unwrap_or(0))
                .sum();
            let has_tests = registered > 0;
            let has_code_children = edges.iter().any(|e| e["from"] == id && e["kind"] == "codenodes" && e["to"].as_str().map(is_code).unwrap_or(false));
            let included = states.get(id).map(|s| s.contains(&Chain::Include)).unwrap_or(false);
            !has_tests && !has_code_children && included
        })
        .cloned()
        .collect();
    out.sort_by(|a, b| a["path"].as_str().cmp(&b["path"].as_str()));
    out
}

/// Nodes whose registered tests fall short of their coverage (2026-09-30):
/// for each node some chain from main includes, every group in its linked
/// tests files whose `coverage_gaps` (computed when the map was built) is not
/// empty. A node with no registered test at all is `untested`'s, not this.
/// Returns (node, [(tests file path, group label, gaps)]), sorted by path.
pub fn under_covered(vertices: &[Value], edges: &[Value]) -> Vec<(Value, Vec<(String, String, Vec<String>)>)> {
    let states = chain_states(vertices, edges);
    let mut out = Vec::new();
    for owner in vertices {
        let id = owner["id"].as_str().unwrap_or("");
        if !states.get(id).map(|s| s.contains(&Chain::Include)).unwrap_or(false) {
            continue;
        }
        let files: Vec<&Value> = edges
            .iter()
            .filter(|e| e["from"] == id && (e["kind"] == "tests" || e["kind"] == "testgroups"))
            .filter_map(|e| vertices.iter().find(|t| t["id"] == e["to"]))
            .collect();
        let groups = || files.iter().flat_map(|f| f["groups"].as_array().cloned().unwrap_or_default().into_iter().map(move |g| (*f, g)));
        if groups().map(|(_, g)| g["tests"].as_u64().unwrap_or(0)).sum::<u64>() == 0 {
            continue;
        }
        let short: Vec<(String, String, Vec<String>)> = groups()
            .filter(|(_, g)| g["tests"].as_u64().unwrap_or(0) > 0)
            .filter_map(|(f, g)| {
                let gaps: Vec<String> = g["coverage_gaps"].as_array()?.iter().filter_map(|x| x.as_str().map(String::from)).collect();
                (!gaps.is_empty()).then(|| (f["path"].as_str().unwrap_or("").to_string(), g["label"].as_str().unwrap_or("").to_string(), gaps))
            })
            .collect();
        if !short.is_empty() {
            out.push((owner.clone(), short));
        }
    }
    out.sort_by(|a, b| a.0["path"].as_str().cmp(&b.0["path"].as_str()));
    out
}

/// The coverage half of the sweep (2026-09-30): each node whose tests fall
/// short becomes one `test` top-up item. The test agent judges the input
/// space (how many practical permutations the code accepts) and writes the
/// targets; the sweep only compares the targets with what is registered.
/// Deduplicated per node by `check:test-coverage` + `container:<path>`.
fn coverage_sweep(api: &Api, c: &Conn, vertices: &[Value], edges: &[Value], o: &SweepOpts) {
    let nodes = under_covered(vertices, edges);
    println!("coverage: {} node(s) have tests short of their coverage", nodes.len());
    let mut filed = 0;
    for (v, short) in &nodes {
        if filed >= o.coverage_max {
            println!("coverage: stopping at {} new item(s) this sweep (--coverage-max)", o.coverage_max);
            break;
        }
        let path = v["path"].as_str().unwrap_or("");
        let name = v["name"].as_str().unwrap_or("");
        if o.dry_run {
            println!("  would file: coverage top-up for {path} ({} group(s))", short.len());
            filed += 1;
            continue;
        }
        let listing: Vec<String> = short.iter().map(|(f, l, g)| format!("- {f} [{l}]: {}", g.join("; "))).collect();
        let mut lockdirs: Vec<String> = short.iter().filter_map(|(f, _, _)| f.rsplit_once('/').map(|(d, _)| format!("{d}/"))).collect();
        lockdirs.sort();
        lockdirs.dedup();
        let request = format!(
            "The tests of map node \"{name}\" ({path}) fall short of their coverage. Bring each group below up to it.\n\n{}\n\n\
             Coverage is measured in four kinds of test, sized by the input space — how many practical permutations the code under test accepts (a function taking one boolean needs about two tests in all; one taking an open JSON document needs a collection of each kind, not millions):\n\
             - golden: the expected paths — every normal use, at least one;\n\
             - malformed: allowable malformed, incomplete or missing inputs the code must tolerate;\n\
             - longtail: rare but valid inputs — limits, sizes, unusual encodings, odd combinations;\n\
             - failure: inputs or states the code must refuse, and how it refuses.\n\n\
             For each group:\n\
             - Read the code under test and write `input_space` on the group's line in the testgroup block: what it accepts and roughly how many practical permutations that allows.\n\
             - Set `coverage` to the number of tests each kind needs, e.g. `{{\"golden\":3,\"malformed\":4,\"longtail\":2,\"failure\":3}}`. Use 0 only for a kind that cannot apply, and say why in `input_space`.\n\
             - Give every registered test a `kind` (golden, malformed, longtail or failure); classify existing tests before writing new ones, and never delete one.\n\
             - Write the missing tests, one deterministic shell script each (exit 0 = passes, 1 = fails, last line `ITER_RESULT pass=… fail=… total=…`), writing any output files only under `$ITER_TEST_OUT`, and register them.\n\
             - Run each group with `iter runtests --group <label>` and report what is green and what is red. Do not change the code under test: a red test is filed as a `code` item by the next sweep.",
            listing.join("\n"),
        );
        let mut tags = vec![
            json!({"text": format!("{}test-coverage", iter_core::dedup::CHECK_TAG_PREFIX), "color": ""}),
            json!({"text": format!("{}{}", iter_core::dedup::CONTAINER_TAG_PREFIX, path), "color": ""}),
            json!({"text": "sweep", "color": ""}),
        ];
        for u in usecases_touching(vertices, edges, v["id"].as_str().unwrap_or("")) {
            tags.push(json!({"text": format!("usecase:{u}"), "color": ""}));
        }
        let workid = std::env::var("ITER_WORKID").unwrap_or_default();
        let body = json!({"name": format!("Test coverage: top up the tests for {name}"), "agent": "test", "state": "queued",
            "lockdirs": lockdirs, "blockedby": [], "context": [], "model": "", "tags": tags,
            "createdby": workid, "requestedby": if workid.is_empty() { "user" } else { "agent:exec" }, "prework": [], "postwork": [],
            "request": request});
        match api.post(&format!("/api/projects/{}/workitems", c.project), &body) {
            Ok(created) if crate::cli::already_open(&created) => {}
            Ok(created) => {
                filed += 1;
                let id = created["id"].as_str().unwrap_or("");
                println!("  filed coverage item …{} for {path}", &id[id.len().saturating_sub(12)..]);
            }
            Err(e) => eprintln!("  could not file the coverage item for {path}: {e}"),
        }
    }
}

/// The no-tests half of the sweep (2026-09-30): each untested code node
/// becomes one `test` item that plans, writes and runs its first tests and
/// links them from the node file, so the next sweep runs them (and turns red
/// ones into `code` items). Deduplicated per node by `check:no-tests` +
/// `container:<path>`, capped per sweep.
fn untested_sweep(api: &Api, c: &Conn, vertices: &[Value], edges: &[Value], o: &SweepOpts) {
    let nodes = untested(vertices, edges);
    println!("no tests: {} code node(s) have no tests", nodes.len());
    let mut filed = 0;
    for v in &nodes {
        if filed >= o.tests_max {
            println!("no tests: stopping at {} new item(s) this sweep (--tests-max)", o.tests_max);
            break;
        }
        let path = v["path"].as_str().unwrap_or("");
        let name = v["name"].as_str().unwrap_or("");
        let dir = v["dir"].as_str().map(String::from).unwrap_or_else(|| path.rsplit_once('/').map(|(d, _)| d.to_string()).unwrap_or_default());
        let stem = path.rsplit('/').next().unwrap_or("").trim_end_matches(".code.iter.md");
        let tests_file = format!("{dir}/test/{stem}.tests.iter.md");
        if o.dry_run {
            println!("  would file: tests for {path}");
            filed += 1;
            continue;
        }
        let codedirs: Vec<String> = v["codedirs"].as_array().into_iter().flatten().filter_map(|x| x.as_str().map(String::from)).collect();
        let request = format!(
            "Map node \"{name}\" ({path}) has no tests (no tests file, or one with no test registered), so the test sweep cannot tell whether its code works. Write its first tests.\n\n\
             - Read the node file and the code it owns ({}) to learn what the part DOES — its inputs, outputs and the promises other parts rely on.\n\
             - Create {tests_file} (or fill the tests file the node already links) with a `## Planned tests` list, simplest first, and a testgroup block.\n\
             - Size the tests by the input space: write `input_space` (what the code accepts, roughly how many practical permutations) and `coverage` targets for the four kinds — golden (expected paths), malformed (allowable malformed, incomplete or missing inputs), longtail (rare but valid inputs), failure (what it must refuse) — on the group's line, and give every test its `kind`.\n\
             - Write one deterministic shell script per test beside it (exit 0 = passes, 1 = fails, last line `ITER_RESULT pass=… fail=… total=…`) and register each in the testgroup block, in order.\n\
             - Link the tests from the node file: add `\"{{thisfiledir}}/test/*.tests.iter.md\"` to `children.tests` in {path} unless it is already there (keep every other field as it is).\n\
             - A script that writes files writes them only under `$ITER_TEST_OUT` (emptied before every run, outside the checkout), never into the tree.\n\
             - Run the group with `iter runtests --group <label>` and report what is green and what is red. Do not change the code under test: a red test is filed as a `code` item by the next sweep.",
            if codedirs.is_empty() { "the files beside the node".to_string() } else { codedirs.join(", ") },
        );
        let mut tags = vec![
            json!({"text": format!("{}no-tests", iter_core::dedup::CHECK_TAG_PREFIX), "color": ""}),
            json!({"text": format!("{}{}", iter_core::dedup::CONTAINER_TAG_PREFIX, path), "color": ""}),
            json!({"text": "sweep", "color": ""}),
        ];
        for u in usecases_touching(vertices, edges, v["id"].as_str().unwrap_or("")) {
            tags.push(json!({"text": format!("usecase:{u}"), "color": ""}));
        }
        let workid = std::env::var("ITER_WORKID").unwrap_or_default();
        let body = json!({"name": format!("Tests first: write the first tests for {name}"), "agent": "test", "state": "queued",
            "lockdirs": [path, format!("{dir}/test/")], "blockedby": [], "context": [], "model": "", "tags": tags,
            "createdby": workid, "requestedby": if workid.is_empty() { "user" } else { "agent:exec" }, "prework": [], "postwork": [],
            "request": request});
        match api.post(&format!("/api/projects/{}/workitems", c.project), &body) {
            Ok(created) if crate::cli::already_open(&created) => {}
            Ok(created) => {
                filed += 1;
                let id = created["id"].as_str().unwrap_or("");
                println!("  filed test item …{} for {path}", &id[id.len().saturating_sub(12)..]);
            }
            Err(e) => eprintln!("  could not file the test item for {path}: {e}"),
        }
    }
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
         - Finish with `--fixed` on the same group.\n\
         - If the fix is very complex or risky (a redesign, a change across several parts, a data migration, anything you would want reviewed before it is built), do not attempt it: file one `plan` item with `\"$ITER_BIN\" add --agent plan` naming this group, the failing checks and why it is not a simple fix, then `\"$ITER_BIN\" wait --on <that item's id>` and end your turn. This item re-runs once the plan's work closes, and proves `--fixed` then.\n\n\
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

/// `iter sweep --install-schedule --every <N>[m|h]`: turn the project's one
/// test sweep on at this interval (2026-09-30). The engine normally creates
/// it paused; this sets it `scheduled` with the new interval, or creates it
/// when no engine has yet (users only — iter_data refuses a schedule from an
/// engine token unless it is the paused template).
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

    #[test]
    fn untested_means_an_included_leaf_code_node_with_no_tests() {
        // main -> P -> {L1, L2, L3, L4}; L1 has tests; L4 links a tests file
        // with nothing registered; L3 is omitted; O is unlinked; P has
        // children, so its tests live with its parts
        let vs = vec![
            v("m", "main", ""), v("P", "code", ""), v("L1", "code", ""), v("L2", "code", ""), v("L3", "code", "omit"),
            v("L4", "code", ""), v("O", "code", ""),
            json!({"id": "tg", "nodetype": "tests", "groups": [{"label": "a", "tests": 0}, {"label": "b", "tests": 2}]}),
            json!({"id": "empty", "nodetype": "tests", "groups": [{"label": "c", "tests": 0}]}),
        ];
        let es = vec![
            e("m", "root", "P"), e("P", "codenodes", "L1"), e("P", "codenodes", "L2"), e("P", "codenodes", "L3"),
            e("P", "codenodes", "L4"), e("L1", "tests", "tg"), e("L4", "tests", "empty"),
        ];
        let ids: Vec<String> = untested(&vs, &es).iter().map(|x| x["id"].as_str().unwrap().to_string()).collect();
        assert_eq!(ids, vec!["L2".to_string(), "L4".to_string()]);
    }

    #[test]
    fn under_covered_lists_the_short_groups_of_included_nodes() {
        let tf = |id: &str, groups: Value| json!({"id": id, "nodetype": "tests", "path": format!("{{topdir}}/{id}.tests.iter.md"), "groups": groups});
        let vs = vec![
            v("m", "main", ""), v("A", "code", ""), v("B", "code", ""), v("C", "code", "omit"), v("D", "code", ""),
            tf("ta", json!([{"label": "a1", "tests": 3, "coverage_gaps": ["longtail: 0 of 2"]}, {"label": "a2", "tests": 2, "coverage_gaps": []}])),
            tf("tb", json!([{"label": "b1", "tests": 4, "coverage_gaps": []}])),
            tf("tc", json!([{"label": "c1", "tests": 1, "coverage_gaps": ["golden: 0 of 1"]}])),
            tf("td", json!([{"label": "d1", "tests": 0, "coverage_gaps": ["input space not assessed"]}])),
        ];
        let es = vec![
            e("m", "root", "A"), e("m", "root", "B"), e("m", "root", "C"), e("m", "root", "D"),
            e("A", "tests", "ta"), e("B", "tests", "tb"), e("C", "tests", "tc"), e("D", "tests", "td"),
        ];
        let got = under_covered(&vs, &es);
        assert_eq!(got.len(), 1, "B is covered, C is omitted, D has no tests (untested's)");
        assert_eq!(got[0].0["id"], "A");
        assert_eq!(got[0].1, vec![("{topdir}/ta.tests.iter.md".to_string(), "a1".to_string(), vec!["longtail: 0 of 2".to_string()])]);
    }
}
