use super::*;

fn tmp(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("iter_local_migrate5_{name}_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d.canonicalize().unwrap()
}

fn put(top: &Path, rel: &str, text: &str) {
    let p = top.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, text).unwrap();
}

/// A small iter4 checkout exercising every §10 rule.
fn iter4_tree(top: &Path) {
    put(top, "main.iter.md", r#"---
id: 11111111-1111-4111-8111-111111111111
projectname: "Demo Shop"
projectdescription: "Sells things."
globalscandirs: ["{topdir}/"]
globalinterfacedir: "{topdir}/interfaces/"
globalusecasedir: "{topdir}/usecases/"
actorsfile: "{topdir}/actors.yaml"
globalcontextfiles: ["{topdir}/reqs/bizreq.iter.md", "{topdir}/reqs/philosophy.iter.md", "{topdir}/reqs/rules.md"]
children:
  codenodes: ["{thisfiledir}/api/api.code.iter.md", "web/web.code.iter.md"]
---
# Demo Shop
"#);
    put(top, "reqs/bizreq.iter.md", r#"---
id: 22222222-2222-4222-8222-222222222222
name: "Shop bizreq"
description: "business rules"
children:
  reqpaths: []
---
# Shop business rules

Intro prose that is not a requirement.

## Selling

- **SHOP-B001** — Every order gets a receipt. The receipt names
  the buyer and the total.

  A second paragraph of the same rule.
- **SHOP-B002**: Refunds are allowed for 30 days.

```
- not a bullet (code)
```
"#);
    put(top, "reqs/techreq.iter.md", "---\nid: 33333333-3333-4333-8333-333333333333\nname: tech\n---\n- only one rule here\n");
    put(top, "reqs/philosophy.iter.md", "---\nname: philosophy\n---\nKeep it simple.\n");
    put(top, "reqs/rules.md", "plain rules doc\n");
    put(top, "api/api.code.iter.md", r#"---
id: 44444444-4444-4444-8444-444444444444
name: "API"
level: container
description: "Serves orders."
simple_description: "The shop's front door."
owner: bespoke
testgroup: tests/api.testgroup.iter.md
test_dir: tests
children:
  codedirs:   ["{thisfiledir}/"]
  codenodes:  []
  inputs:     []
  outputs:    ["{topdir}/interfaces/api-*.interface.iter.md", "{topdir}/interfaces/stock-events.interface.iter.md"]
  bizreqs:    ["{thisfiledir}/api.bizreq.iter.md"]
  techreqs:   []
  testgroups: ["{thisfiledir}/tests/*.testgroup.iter.md"]
---
# API
"#);
    put(top, "api/api.bizreq.iter.md", "---\nname: api reqs\n---\n- **API-1** first\n- **API-2** second\n- third rule without id\n");
    put(top, "api/api.techreq.iter.md", "---\nid: 99999999-9999-4999-8999-999999999999\nname: \"api tech\"\n---\n# api tech\n\nAll calls time out after 5s.\n");
    put(top, "api/reqs/extra.techreq.iter.md", "---\nname: api extra tech\nchildren:\n  reqpaths: [\"notes.md\"]\n---\nAbout retries.\n\n## API-T2 — Retries\nRetry three times.\n\n### Why\nNetworks drop.\n");
    put(top, "deploy/deploy.bizreq.iter.md", "---\nname: deploy\n---\n- Deploys are one command.\n- Rollback is one command too.\n");
    put(top, "api/tests/api.testgroup.iter.md", r#"---
id: 55555555-5555-4555-8555-555555555555
name: "api"
description: "API checks"
children:
  testpaths: ["{thisfiledir}/dev/*.sh"]
testpaths:
  dev: ["{thisfiledir}/dev/*.sh"]
  qa: ["{thisfiledir}/qa/*.sh"]
---
# api tests

<!-- iterapp:testgroups
{"label":"api","desc":"API checks","auto_fix":false,"lastrun":"2026-09-30T10:00:00Z","result":"passed","counts":"3/3","input_space":"orders","coverage":{"golden":2,"malformed":1,"longtail":1,"failure":1},"testlist":["t1.sh",{"id":"t2","name":"two","shell":"dev/t2.sh","kind":"golden"}]}
-->
"#);
    put(top, "api/tests/t1.sh", "echo ok\n");
    put(top, "api/tests/dev/t2.sh", "echo ok\n");
    put(top, "web/web.code.iter.md", r#"---
id: 66666666-6666-4666-8666-666666666666
name: "Web"
level: container
description: "Shows the shop."
children:
  codedirs: ["{thisfiledir}/"]
  inputs: ["{topdir}/interfaces/api-orders.interface.iter.md", "{topdir}/interfaces/stock-events.interface.iter.md"]
  testgroups: []
---
# Web
"#);
    put(top, "web/web.agentmemory.iter.md", "# memory\nnever touched\n");
    put(top, "interfaces/api-orders.interface.iter.md", "---\nid: 77777777-7777-4777-8777-777777777777\nname: api-orders\nkind: request-reply\ndescription: \"an order in, a receipt out\"\n---\n# contract\n");
    put(top, "interfaces/stock-events.interface.iter.md", "---\nname: stock-events\nkind: event\ndescription: \"stock moved\"\n---\n");
    put(top, "usecases/buy.usecase.iter.md", r#"---
id: 88888888-8888-4888-8888-888888888888
name: "Buy a thing"
description: "A buyer orders."
children:
  codenodes: ["{topdir}/api/api.code.iter.md", "{topdir}/web/web.code.iter.md"]
  testgroups: ["{thisfiledir}/buy.tests.iter.md"]
---
# Buy
"#);
    put(top, "usecases/buy.tests.iter.md", "---\nname: buy tests\n---\n# buy tests\n");
    put(top, "actors.yaml", "actors:\n  - id: buyer\n    name: \"Buyer\"\n    description: \"Someone who buys.\"\n    uses:\n      - pattern: \"api-*\"\n        why: \"orders through the API\"\n    serves: []\n");
    put(top, "notes.iter.md", "a plain context doc\n");
    let git = |a: &[&str]| std::process::Command::new("git").args(a).current_dir(top).output().unwrap();
    git(&["init", "-q"]);
    git(&["config", "user.email", "t@t"]);
    git(&["config", "user.name", "t"]);
    git(&["add", "-A"]);
    git(&["commit", "-q", "-m", "iter4"]);
}

fn read(top: &Path, rel: &str) -> String {
    std::fs::read_to_string(top.join(rel)).unwrap_or_else(|e| panic!("{rel}: {e}"))
}

fn doc(top: &Path, rel: &str) -> NodeDoc {
    nodefile::parse(&format!("{{topdir}}/{rel}"), &read(top, rel)).unwrap()
}

fn snapshot(top: &Path) -> BTreeMap<String, String> {
    let mut m = BTreeMap::new();
    let mut stack = vec![top.to_path_buf()];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d).unwrap().flatten() {
            let p = e.path();
            if p.is_dir() {
                if e.file_name() != ".git" {
                    stack.push(p);
                }
            } else {
                m.insert(p.strip_prefix(top).unwrap().to_string_lossy().into_owned(), std::fs::read_to_string(&p).unwrap_or_default());
            }
        }
    }
    m
}

#[test]
fn converts_every_rule_and_never_touches_from() {
    let base = tmp("full");
    let from = base.join("from");
    let to = base.join("to");
    iter4_tree(&from);
    let before = snapshot(&from);
    let head_before = std::process::Command::new("git").args(["rev-parse", "HEAD"]).current_dir(&from).output().unwrap().stdout;
    let rep = run(&from, &to, false).unwrap();
    rep.print();
    assert_eq!(snapshot(&from), before, "--from is never modified");
    assert_eq!(std::process::Command::new("git").args(["rev-parse", "HEAD"]).current_dir(&from).output().unwrap().stdout, head_before);
    assert!(to.join(".git").exists(), "history copied");
    assert!(rep.not_idempotent.is_empty(), "{:?}", rep.not_idempotent);

    // main → project
    assert!(!to.join("main.iter.md").exists());
    let p = doc(&to, "global/demo_shop.project.iter.md");
    assert_eq!(p.nodetype, NodeType::Project);
    assert_eq!((p.name.as_str(), p.desc.as_str()), ("Demo Shop", "Sells things."));
    assert_eq!(p.id, "11111111-1111-4111-8111-111111111111", "ids are kept");
    assert_eq!(p.children.codenodes, vec!["{topdir}/api/api.code.iter.md", "{topdir}/web/web.code.iter.md"], "{{thisfiledir}} made absolute when the file moved");
    assert_eq!(
        p.children.reqs,
        vec![
            "{topdir}/global/requirements/demo_shop.bizreq.iter.md",
            "{topdir}/reqs/philosophy.iter.md",
            "{topdir}/reqs/rules.md",
            "{topdir}/global/requirements/demo_shop.techreq.iter.md"
        ],
        "global requirement files follow the move; the project names its pair"
    );
    for k in ["globalinterfacedir", "globalusecasedir", "actorsfile"] {
        assert!(!p.front.contains_key(k), "{k} dropped");
    }
    assert_eq!(p.front["scandirs"], json!(["{topdir}/"]));
    assert_eq!(rep.count("main → project"), 1);

    // requirements (§2.8): one file per type per attachment point, one section per requirement
    assert!(!to.join("reqs/bizreq.iter.md").exists());
    assert!(!to.join("reqs/techreq.iter.md").exists());
    let g = doc(&to, "global/requirements/demo_shop.bizreq.iter.md");
    assert_eq!(g.id, "22222222-2222-4222-8222-222222222222", "the file keeps its id");
    assert_eq!(g.name, "Shop bizreq");
    let (pre, items) = nodefile::parse_reqs(&g.body);
    assert!(pre.contains("Intro prose that is not a requirement.") && pre.contains("### Selling"), "preamble kept, ## demoted: {pre}");
    assert!(pre.starts_with("# Shop business rules"), "{pre}");
    assert_eq!(items.len(), 2);
    assert_eq!((items[0].key.as_str(), items[0].status.as_str()), ("SHOP-B001", "agreed"));
    assert_eq!(items[0].title, "Every order gets a receipt");
    assert!(items[0].text.starts_with("Every order gets a receipt. The receipt names\nthe buyer and the total."), "{}", items[0].text);
    assert!(items[0].text.contains("A second paragraph of the same rule."), "continuation kept");
    assert_eq!(items[1].key, "SHOP-B002");
    assert!(items[1].text.contains("Refunds are allowed for 30 days.") && items[1].text.contains("- not a bullet (code)"), "text after the last bullet stays with it");
    assert!(items.iter().all(|i| nodefile::is_valid_id(&i.id)));
    let gt = doc(&to, "global/requirements/demo_shop.techreq.iter.md");
    let (_, ti) = nodefile::parse_reqs(&gt.body);
    assert_eq!(ti.len(), 1);
    assert_eq!(ti[0].text, "only one rule here");
    // beside a code node
    assert!(!to.join("api/api.bizreq.iter.md").exists());
    let ab = doc(&to, "api/reqs/api.bizreq.iter.md");
    let (_, ai) = nodefile::parse_reqs(&ab.body);
    assert_eq!(ai.iter().map(|i| i.key.as_str()).collect::<Vec<_>>(), vec!["API-1", "API-2"]);
    assert!(ai[1].text.contains("- third rule without id"), "an id-less bullet belongs to the requirement above it");
    // merged: api/api.techreq.iter.md + api/reqs/extra.techreq.iter.md
    assert!(!to.join("api/api.techreq.iter.md").exists() && !to.join("api/reqs/extra.techreq.iter.md").exists());
    let at = doc(&to, "api/reqs/api.techreq.iter.md");
    assert_eq!(at.id, "99999999-9999-4999-8999-999999999999", "the first source by path keeps its id");
    let (apre, at_items) = nodefile::parse_reqs(&at.body);
    assert!(apre.contains("# api tech") && apre.contains("About retries."), "both preambles: {apre}");
    assert_eq!(at_items.len(), 2);
    assert_eq!((at_items[0].title.as_str(), at_items[0].text.as_str()), ("api tech", "All calls time out after 5s."));
    assert_eq!((at_items[1].key.as_str(), at_items[1].title.as_str()), ("API-T2", "Retries"));
    assert!(at_items[1].text.contains("### Why"));
    assert!(at.children.reqs.contains(&"{topdir}/api/reqs/notes.md".to_string()), "a merged file's references come along: {:?}", at.children.reqs);
    // a folder without a code node
    let dp = doc(&to, "deploy/reqs/deploy.bizreq.iter.md");
    let (_, di) = nodefile::parse_reqs(&dp.body);
    assert_eq!(di.iter().map(|i| i.title.as_str()).collect::<Vec<_>>(), vec!["Deploys are one command", "Rollback is one command too"]);
    assert_eq!(rep.count("reqs files merged into another"), 1);
    assert_eq!(rep.count("requirements (sections) written"), 2 + 1 + 2 + 2 + 2);
    let api = doc(&to, "api/api.code.iter.md");
    assert_eq!(
        api.children.reqs,
        vec!["{thisfiledir}/reqs/api.bizreq.iter.md", "{thisfiledir}/reqs/api.techreq.iter.md"],
        "the owner names its two files"
    );
    assert!(!api.front.contains_key("testgroup") && !api.front.contains_key("test_dir"));
    assert_eq!(api.children.tests, vec!["{thisfiledir}/tests/*.test.iter.md"], "globs get the v5 tag");
    assert!(api.body.contains("## Summary") && api.body.contains("front door"), "simple_description folded");

    // testgroups
    assert!(!to.join("api/tests/api.testgroup.iter.md").exists());
    let t = doc(&to, "api/tests/api.test.iter.md");
    assert_eq!(t.id, "55555555-5555-4555-8555-555555555555");
    assert_eq!(t.children.tests, vec!["{thisfiledir}/dev/*.sh", "{thisfiledir}/t1.sh"], "registered scripts not covered by a glob are added");
    assert!(!t.body.contains("iterapp:testgroups"));
    assert_eq!(t.front["test_tiers"]["qa"], json!(["{thisfiledir}/qa/*.sh"]));
    assert_eq!(t.front["last_result"]["overall_success"], true);
    assert_eq!(t.front["last_result"]["normal"], json!({"total": 3, "pass": 3, "err": 0}));
    assert_eq!(t.timestamps.last_tested, "2026-09-30 10:00:00Z");
    assert_eq!(t.front["coverage"], json!({"normal": 3, "longtail": 1, "failure": 1}));
    assert_eq!(t.front["input_space"], "orders");
    let parsed: iter_core::testresult::TestResult = serde_json::from_value(t.front["last_result"].clone()).unwrap();
    assert_eq!(parsed.id, t.id);
    let buy = doc(&to, "usecases/buy.usecase.iter.md");
    assert_eq!(buy.children.tests, vec!["{thisfiledir}/buy.test.iter.md"], "exact references follow the rename");
    assert!(to.join("usecases/buy.test.iter.md").exists());
    assert_eq!(rep.count("testgroup → test"), 2);

    // interfaces → connections
    assert!(!to.join("interfaces/api-orders.interface.iter.md").exists());
    assert!(!to.join("interfaces/stock-events.interface.iter.md").exists());
    let api_call = doc(&to, "global/connections/api_call.code.iter.md");
    assert_eq!(api_call.level.as_deref(), Some("connection"));
    assert_eq!(api_call.name, "API call");
    assert_eq!(api_call.front["connects"], json!({"from": ["{topdir}/api/api.code.iter.md"], "to": ["{topdir}/web/web.code.iter.md"]}));
    assert!(api_call.body.contains("`api-orders`"));
    let ev = doc(&to, "global/connections/event.code.iter.md");
    assert_eq!(ev.front["connects"], json!({"from": ["{topdir}/api/api.code.iter.md"], "to": ["{topdir}/web/web.code.iter.md"]}));
    assert_eq!(rep.count("interfaces removed"), 2);
    assert_eq!(rep.count("connection nodes created"), 2);
    let web = read(&to, "web/web.code.iter.md");
    assert!(!web.contains("interface"), "no interface reference survives: {web}");

    // actors
    let buyer = doc(&to, "global/usecases/buyer.actor.iter.md");
    assert_eq!(buyer.front["touches"], json!(["{topdir}/api/api.code.iter.md"]));
    assert_eq!(buyer.desc, "Someone who buys.");
    assert!(to.join("actors.yaml").exists());

    // agent memory
    assert!(!to.join("web/web.agentmemory.iter.md").exists());
    assert_eq!(read(&to, "web/web.agentmem.iter.md"), "# memory\nnever touched\n");

    // plain docs untouched
    assert_eq!(read(&to, "notes.iter.md"), "a plain context doc\n");

    // every node file conforms idempotently and no interface remains anywhere
    let now = nodefile::now_ts();
    for f in crate::walk::node_files(&to, &[to.clone()]) {
        let sp = crate::walk::topdir_path(&to, &f).unwrap();
        assert!(!sp.contains(".interface."));
        if nodefile::type_of(&sp).is_some_and(nodefile::is_synced) {
            assert!(!nodefile::type_of(&sp).is_none());
            assert!(!nodefile::is_legacy_filename(&sp), "{sp} still has a legacy name");
            assert!(!nodefile::conform(&sp, &std::fs::read_to_string(&f).unwrap(), &now, "x").changed, "{sp} not conformed");
        }
    }
}

#[test]
fn dry_run_writes_nothing_and_refusals() {
    let base = tmp("dry");
    let from = base.join("from");
    iter4_tree(&from);
    let to = base.join("to");
    let rep = run(&from, &to, true).unwrap();
    assert!(!to.exists(), "dry run writes nothing");
    assert_eq!(rep.count("interfaces removed"), 2);
    assert!(run(&from, &from, false).is_err(), "never in place");
    assert!(run(&from, &from.join("sub"), false).is_err(), "never inside --from");
    std::fs::create_dir_all(&to).unwrap();
    std::fs::write(to.join("x"), "x").unwrap();
    assert!(run(&from, &to, false).is_err(), "a non-empty target is refused");
}

#[test]
fn bullets_and_names() {
    let b = top_level_bullets("intro\n\n# H\n- **A-1** one\n  more\n\n  para\n- two words here\nprose ends it\n* star\n");
    assert_eq!(b.len(), 3);
    assert_eq!(b[0].text, "**A-1** one\nmore\n\npara");
    assert_eq!(b[0].section, "H");
    assert_eq!(bullet_name(&b[0].text), "A-1");
    assert_eq!(bullet_name("**PDY-BIZ-001**: rule"), "PDY-BIZ-001");
    assert_eq!(bullet_name("plain words, one two three four five six seven"), "plain words, one two three four five six");
    let r = requirement_bullets("- intro list item\n- **R1** first\n- a detail of R1\n- **R2** second\n");
    assert_eq!(r.len(), 2);
    assert_eq!(r[0].text, "**R1** first\n- a detail of R1");
    assert!(has_bold_id("**PDY-BIZ-001** x") && !has_bold_id("**Note** x") && !has_bold_id("plain"));
    assert_eq!(requirement_bullets("- a\n- b\n").len(), 2, "no ids: every bullet");
    assert_eq!(v5_file_name("x.testgroup.iter.md"), "x.test.iter.md");
    assert_eq!(v5_file_name("testgroup.iter.md"), "test.iter.md");
    assert_eq!(v5_file_name("a.tests.iter.md"), "a.test.iter.md");
    assert_eq!(v5_file_name("a.agentmemory.iter.md"), "a.agentmem.iter.md");
    assert_eq!(v5_file_name("a.code.iter.md"), "a.code.iter.md");
}

#[test]
fn req_sections_shapes() {
    // empty
    let (p, i) = req_sections("", "x");
    assert!(p.is_empty() && i.is_empty());
    let (p, i) = req_sections("\n# Only a title\n\n", "x");
    assert_eq!((p.as_str(), i.len()), ("# Only a title", 0));
    // prose: one requirement named after the file; # title stays the preamble, ## inside demoted
    let (p, i) = req_sections("# T\n\nProse rule. More.\n\n#### Detail\nd\n", "File name");
    assert_eq!(p, "# T");
    assert_eq!(i.len(), 1);
    assert_eq!((i[0].key.as_str(), i[0].title.as_str(), i[0].status.as_str()), ("", "File name", "agreed"));
    assert_eq!(i[0].text, "Prose rule. More.\n\n#### Detail\nd");
    // prose with ## headings and no bullets: each ## is a requirement (as conform reads it)
    let (p, i) = req_sections("# T\n\nProse.\n\n## Detail\nd\n", "File name");
    assert_eq!((p.as_str(), i.len(), i[0].title.as_str()), ("# T\n\nProse.\n", 1, "Detail"));
    // already sectioned (with or without markers, no bullets): kept as is
    let body = "Pre\n\n## K-1 — t\n<!-- req: id=6f2c4c9e-1111-4aaa-8bbb-000000000001 status=done -->\nx\n- a list inside\n- is not split\n";
    let (p, i) = req_sections(body, "n");
    assert_eq!(p, "Pre\n");
    assert_eq!(i.len(), 1);
    assert_eq!((i[0].id.as_str(), i[0].status.as_str()), ("6f2c4c9e-1111-4aaa-8bbb-000000000001", "done"));
    let (_, i) = req_sections("## A\ntext a\n## B\ntext b\n", "n");
    assert_eq!(i.iter().map(|x| x.title.as_str()).collect::<Vec<_>>(), vec!["A", "B"]);
    // bullets: preamble keeps its # heading, ## demoted; bold keys; long titles cut
    let long = "word ".repeat(40);
    let body = format!("# Rules\n\nIntro.\n\n## Part\n- **R-1.** {long}end.\n  cont\n- **R 2** not a key. Rest\n");
    let (p, i) = req_sections(&body, "n");
    assert_eq!(p, "# Rules\n\nIntro.\n\n### Part");
    assert_eq!(i[0].key, "R-1");
    assert!(i[0].title.chars().count() <= 100 && i[0].title.ends_with('…'));
    assert!(i[0].text.ends_with("end.\ncont"));
    assert_eq!((i[1].key.as_str(), i[1].title.as_str()), ("", "R 2 not a key"));
    assert_eq!(i[1].text, "**R 2** not a key. Rest");
    // the rendered result conforms to a fixed point
    let out = nodefile::render_reqs(&p, &i);
    let (c, _) = nodefile::conform_reqs(&out);
    assert_eq!(c, out);
}
