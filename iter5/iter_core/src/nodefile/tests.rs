use super::*;
use serde_json::json;
use std::collections::HashSet;

const NOW: &str = "2026-10-02 14:56:11Z";
const LATER: &str = "2026-10-03 09:00:00Z";
const ID: &str = "3f0c1d2e-4a5b-4c6d-8e7f-0123456789ab";

/* ------------------------------------------------------------- fixtures */

/// (path, text) — real iter4 files copied from the iter5 tree plus hand-made
/// messy ones. Every one must conform idempotently.
fn fixtures() -> Vec<(&'static str, String)> {
    vec![
        ("{topdir}/iter_local/src/ids.code.iter.md", include_str!("fixtures/ids.code.iter.md.fixture").to_string()),
        ("{topdir}/map/engine/engine.code.iter.md", include_str!("fixtures/engine.code.iter.md.fixture").to_string()),
        ("{topdir}/iter_data/src/mcp.code.iter.md", include_str!("fixtures/mcp.code.iter.md.fixture").to_string()),
        ("{topdir}/reqs/iter4.bizreq.iter.md", include_str!("fixtures/iter4.bizreq.iter.md.fixture").to_string()),
        ("{topdir}/reqs/iter4.techreq.iter.md", include_str!("fixtures/iter4.techreq.iter.md.fixture").to_string()),
        (
            "{topdir}/usecases/map-the-repo/map-the-repo.usecase.iter.md",
            include_str!("fixtures/map-the-repo.usecase.iter.md.fixture").to_string(),
        ),
        ("{topdir}/main.iter.md", include_str!("fixtures/main.iter.md.fixture").to_string()),
        ("{topdir}/iter_core/test/iter_core.tests.iter.md", include_str!("fixtures/iter_core.tests.iter.md.fixture").to_string()),
        ("{topdir}/x/plain.code.iter.md", "# Just a heading\n\nNo frontmatter at all.\n".to_string()),
        ("{topdir}/x/empty.code.iter.md", String::new()),
        ("{topdir}/x/fenceonly.test.iter.md", "---\n---\n".to_string()),
        (
            "{topdir}/x/broken.code.iter.md",
            "---\nid: not-a-uuid\nname: Broken: has a colon\ndescription: \"ok\"\nlevel: code\nteststate: maybe\n---\nbody\n".to_string(),
        ),
        (
            "{topdir}/x/crlf.bizreq.iter.md",
            "---\r\nid: 9b2e8c1a-0f4d-4e7a-9c3b-5d6e7f8a9b0c\r\nname: \"crlf\"\r\nstatus: weird\r\n---\r\nThe requirement.\r\n".to_string(),
        ),
        ("{topdir}/x/unterminated.philosophy.iter.md", "---\nname: x\nno closing fence\n".to_string()),
        (
            "{topdir}/x/legacy.testgroup.iter.md",
            "---\nid: 1b2c3d4e-5f60-4718-9a0b-1c2d3e4f5a6b\ntest_loop: blocked\ntestpaths: \"{thisfiledir}/*.sh\"\nlong_description: \"The long story.\"\n---\n".to_string(),
        ),
        (
            "{topdir}/x/conn.code.iter.md",
            "---\nid: 2c3d4e5f-6071-4829-8a0b-1c2d3e4f5a6b\nname: API call\nlevel: connection\nconnects: {from: [\"{topdir}/gw/gw.code.iter.md\"]}\nextra_key: {b: 1, a: [x, \"y z\"], c: [{k: v}]}\n---\n".to_string(),
        ),
        (
            "{topdir}/x/weird.actor.iter.md",
            "---\nid: 3d4e5f60-7182-4930-8b1c-2d3e4f5a6b7c\nname: \"Employer \\\"boss\\\" \\u0007\"\ndesc: |\n  multi\n  line\ncreator: 42\ndrives: \"{topdir}/global/usecases/a.usecase.iter.md\"\ntimestamps: {created: \"2020-01-01 00:00:00Z\", custom: x}\n---\nbody without trailing newline".to_string(),
        ),
    ]
}

fn sample_code() -> String {
    format!(
        "---\nid: {}\nname: \"Postgres\"\ndesc: \"The database.\"\ncreator: \"stephen\"\nteststate: inherit\nlevel: container\nowner: oss\nchildren:\n  codedirs:  [\"{{thisfiledir}}/**\"]\n  codenodes: [\"plugins/enforcement.code.iter.md\"]\n  tests:     [\"{{thisfiledir}}/postgres01.test.iter.md\"]\n  reqs:      [\"{{thisfiledir}}/reqs/\"]\ntimestamps: {{create: \"{}\", last_modified: \"{}\", last_tested: \"\"}}\n---\n# Postgres\n\nBody text.\n",
        ID, NOW, NOW
    )
}

fn doc(t: NodeType, path: &str, name: &str) -> NodeDoc {
    let mut d = NodeDoc::new(t, name, "test", NOW);
    d.path = path.to_string();
    d
}

/* ------------------------------------------------------------- type_of */

#[test]
fn type_of_every_type() {
    for t in NodeType::ALL {
        assert_eq!(type_of(&format!("x.{}.iter.md", t.as_str())), Some(t));
        assert_eq!(type_of(&format!("{}.iter.md", t.as_str())), Some(t), "bare {}", t);
    }
}

#[test]
fn type_of_paths_and_dots() {
    assert_eq!(type_of("{topdir}/src/a.b.c.code.iter.md"), Some(NodeType::Code));
    assert_eq!(type_of("/abs/dir.with.dots/x.usecase.iter.md"), Some(NodeType::Usecase));
}

#[test]
fn type_of_legacy_names() {
    assert_eq!(type_of("main.iter.md"), Some(NodeType::Project));
    assert_eq!(type_of("a.tests.iter.md"), Some(NodeType::Test));
    assert_eq!(type_of("a.testgroup.iter.md"), Some(NodeType::Test));
    assert_eq!(type_of("src.agentmemory.iter.md"), Some(NodeType::Agentmem));
    assert!(is_legacy_filename("{topdir}/main.iter.md"));
    assert!(is_legacy_filename("x.tests.iter.md"));
    assert!(!is_legacy_filename("x.test.iter.md"));
}

#[test]
fn type_of_rejects_non_nodes() {
    assert_eq!(type_of("README.md"), None);
    assert_eq!(type_of("x.Code.iter.md"), None, "case-sensitive");
    assert_eq!(type_of("my_thing_code.iter.md"), None);
    assert_eq!(type_of("notes.iter.md"), None, "plain context doc");
    assert_eq!(type_of("x.interface.iter.md"), None, "interfaces are retired");
    assert_eq!(type_of("x.code.iter.txt"), None);
}

#[test]
fn synced_types() {
    for t in NodeType::ALL {
        assert_eq!(is_synced(t), t != NodeType::Agentmem);
    }
}

#[test]
fn nodetype_serde_lowercase() {
    assert_eq!(serde_json::to_string(&NodeType::Techreq).unwrap(), "\"techreq\"");
    let t: NodeType = serde_json::from_str("\"actor\"").unwrap();
    assert_eq!(t, NodeType::Actor);
}

/* ------------------------------------------------------- parse / render */

#[test]
fn parse_sample_code() {
    let d = parse("{topdir}/src/pg/postgres.code.iter.md", &sample_code()).unwrap();
    assert_eq!(d.id, ID);
    assert_eq!(d.nodetype, NodeType::Code);
    assert_eq!(d.name, "Postgres");
    assert_eq!(d.desc, "The database.");
    assert_eq!(d.creator, "stephen");
    assert_eq!(d.teststate, "inherit");
    assert_eq!(d.level.as_deref(), Some("container"));
    assert_eq!(d.children.codedirs, vec!["{thisfiledir}/**"]);
    assert_eq!(d.front.get("owner"), Some(&json!("oss")));
    assert_eq!(d.timestamps.create, NOW);
    assert_eq!(d.body, "# Postgres\n\nBody text.\n");
    assert_eq!(d.path, "{topdir}/src/pg/postgres.code.iter.md");
}

#[test]
fn sample_code_is_already_canonical() {
    let text = sample_code();
    let d = parse("{topdir}/src/pg/postgres.code.iter.md", &text).unwrap();
    assert_eq!(render(&d), text);
    let c = conform("{topdir}/src/pg/postgres.code.iter.md", &text, LATER, "x");
    assert!(!c.changed, "{}", c.text);
    assert!(c.findings.is_empty(), "{:?}", c.findings);
}

#[test]
fn parse_errors() {
    assert_eq!(parse("README.md", "x"), Err(NodeErr::NotANode("README.md".into())));
    assert!(matches!(parse("a.code.iter.md", "---\nname: x\n---\n"), Err(NodeErr::BadId(_))));
    assert!(matches!(parse("a.code.iter.md", "---\nid: 1234\n---\n"), Err(NodeErr::BadId(_))));
    // agentmem has no id and that's fine
    let d = parse("a.agentmem.iter.md", "# memory\n").unwrap();
    assert_eq!(d.body, "# memory\n");
}

#[test]
fn round_trip_every_type() {
    for t in NodeType::ALL {
        if t == NodeType::Agentmem {
            continue;
        }
        let path = format!("{{topdir}}/a/thing.{}.iter.md", t.as_str());
        let mut d = doc(t, &path, "Thing \"quoted\" & co: yes");
        d.desc = "Multi\nline desc with: colons, #hash and 'quotes'".into();
        d.body = "# Thing\n\n- a\n- b\n".into();
        d.children.reqs = vec!["{topdir}/global/requirements/".into()];
        d.children.extra.insert("docs".into(), vec!["{thisfiledir}/README.md".into()]);
        d.timestamps.extra.insert("last_built".into(), NOW.into());
        let text = render(&d);
        let back = parse(&path, &text).unwrap();
        assert_eq!(back, d, "type {} text:\n{}", t, text);
        assert_eq!(render(&back), text);
    }
}

#[test]
fn round_trip_rich_front_values() {
    let path = "{topdir}/global/usecases/u.usecase.iter.md";
    let mut d = doc(NodeType::Usecase, path, "U");
    d.front.insert(
        "flowmap".into(),
        json!({
            "summary": "s: with colon",
            "sequence": ["actor:developer", "{topdir}/a.code.iter.md"],
            "process_flow": [{"step": 1, "from": "actor:developer", "to": "{topdir}/a.code.iter.md", "what": "x", "plain": "y", "evidence": "z"}],
            "data_flow": [{"step": 2, "stored": true, "data": "d"}, {}],
        }),
    );
    d.front.insert(
        "misc".into(),
        json!({"n": null, "f": 1.5, "neg": -3, "b": false, "empty": {}, "nested": [[1, 2], [[3]], "s"], "yes": "no", "1": "true"}),
    );
    let text = render(&d);
    let back = parse(path, &text).unwrap();
    assert_eq!(back.front, d.front, "{}", text);
    assert_eq!(render(&back), text);
}

#[test]
fn render_key_order() {
    let mut d = doc(NodeType::Code, "{topdir}/a/a.code.iter.md", "A");
    d.front.insert("zeta".into(), json!("z"));
    d.front.insert("alpha".into(), json!("a"));
    d.front.insert("owner".into(), json!("bespoke"));
    d.children.extra.insert("aaa".into(), vec![]);
    let text = render(&d);
    let keys: Vec<&str> = text
        .lines()
        .skip(1)
        .take_while(|l| *l != "---")
        .filter(|l| !l.starts_with(' '))
        .map(|l| l.split(':').next().unwrap())
        .collect();
    assert_eq!(
        keys,
        vec!["id", "name", "desc", "creator", "teststate", "alpha", "level", "owner", "zeta", "children", "timestamps"]
    );
    let child_keys: Vec<&str> = text
        .lines()
        .filter(|l| l.starts_with("  "))
        .map(|l| l.trim().split(':').next().unwrap())
        .collect();
    assert_eq!(child_keys, vec!["codedirs", "codenodes", "tests", "reqs", "aaa"]);
}

#[test]
fn render_style_matches_sample() {
    let text = sample_code();
    assert!(text.contains("  codedirs:  [\"{thisfiledir}/**\"]\n"));
    let mut d = parse("{topdir}/a.code.iter.md", &text).unwrap();
    d.children.codenodes = vec!["a".into(), "b".into()];
    let r = render(&d);
    assert!(r.contains("  codenodes: [\"a\", \"b\"]\n"), "{}", r);
    assert!(r.contains(&format!("timestamps: {{create: \"{}\", last_modified: \"{}\", last_tested: \"\"}}\n", NOW, NOW)));
    assert!(r.contains(&format!("id: {}\n", ID)), "uuid is plain");
    assert!(r.contains("name: \"Postgres\"\n"), "names always quoted");
    assert!(r.contains("teststate: inherit\n"));
}

#[test]
fn render_never_duplicates_common_keys_from_front() {
    let mut d = doc(NodeType::Code, "{topdir}/a/a.code.iter.md", "A");
    d.front.insert("name".into(), json!("shadow"));
    d.front.insert("children".into(), json!({"x": 1}));
    let text = render(&d);
    assert_eq!(text.matches("\nname:").count(), 1);
    let back = parse(&d.path, &text).unwrap();
    assert_eq!(back.name, "A");
}

#[test]
fn render_parses_with_serde_yaml() {
    for (path, text) in fixtures() {
        let c = conform(path, &text, NOW, "agent.test");
        let Some(d) = c.doc else { continue };
        let (front, _, _) = yaml::split(&render(&d));
        let v: serde_yaml::Value = serde_yaml::from_str(&front.unwrap()).expect(path);
        assert!(v.is_mapping(), "{}", path);
    }
}

#[test]
fn body_kept_verbatim() {
    let body = "\n\n# T\n\n```yaml\n---\nkey: v\n---\n```\ntrailing spaces   \n\n\n";
    let mut d = doc(NodeType::Philosophy, "{topdir}/p.philosophy.iter.md", "P");
    d.body = body.into();
    let back = parse(&d.path, &render(&d)).unwrap();
    assert_eq!(back.body, body);
}

#[test]
fn agentmem_renders_as_body() {
    let text = include_str!("fixtures/src.agentmemory.iter.md.fixture");
    let d = parse("{topdir}/iter_core/src/src.agentmemory.iter.md", text).unwrap();
    assert_eq!(d.nodetype, NodeType::Agentmem);
    assert_eq!(render(&d), text);
}

#[test]
fn nodedoc_json_round_trip() {
    let d = parse("{topdir}/src/pg/postgres.code.iter.md", &sample_code()).unwrap();
    let j = serde_json::to_value(&d).unwrap();
    assert_eq!(j["nodetype"], "code");
    assert_eq!(j["children"]["codedirs"][0], "{thisfiledir}/**");
    let back: NodeDoc = serde_json::from_value(j).unwrap();
    assert_eq!(back, d);
}

#[test]
fn nodedoc_json_minimal() {
    let d: NodeDoc = serde_json::from_value(json!({"nodetype": "bizreq", "id": ID, "children": {"reqs": ["x"], "more": ["y"]}})).unwrap();
    assert_eq!(d.children.reqs, vec!["x"]);
    assert_eq!(d.children.extra["more"], vec!["y"]);
    assert!(d.level.is_none());
}

/* ------------------------------------------------------------ legacy keys */

#[test]
fn legacy_code_file_migrates() {
    let text = include_str!("fixtures/mcp.code.iter.md.fixture");
    let (d, findings) = parse_tolerant("{topdir}/iter_data/src/mcp.code.iter.md", text).unwrap();
    assert!(d.desc.starts_with("Serves every agent-facing"));
    assert!(d.body.contains("## Summary\n\nLets AI assistants"), "{}", d.body);
    assert!(d.children.extra.is_empty(), "inputs/outputs/bizreqs/techreqs gone: {:?}", d.children.extra);
    assert!(!d.front.contains_key("simple_description"));
    assert!(!d.front.contains_key("description"));
    assert!(findings.iter().any(|f| f.code == "dropped-interface"));
    assert!(findings.iter().any(|f| f.code == "legacy-folded"));
    assert!(findings.iter().any(|f| f.code == "legacy-key" && f.msg.starts_with("description")));
}

#[test]
fn summary_goes_after_title_heading() {
    let text = "---\nid: 3f0c1d2e-4a5b-4c6d-8e7f-0123456789ab\nsimple_description: \"Short.\"\n---\n\n# Title\n\nBody.\n";
    let d = parse("a.code.iter.md", text).unwrap();
    assert_eq!(d.body, "\n# Title\n\n## Summary\n\nShort.\n\nBody.\n");
    let text = "---\nid: 3f0c1d2e-4a5b-4c6d-8e7f-0123456789ab\nsimple_description: \"Short.\"\nlong_description: \"Long.\"\n---\nBody.\n";
    let d = parse("a.code.iter.md", text).unwrap();
    assert_eq!(d.body, "## Summary\n\nShort.\n\nBody.\n\n## Long description\n\nLong.\n");
}

#[test]
fn legacy_main_file_migrates() {
    let text = include_str!("fixtures/main.iter.md.fixture");
    let d = parse("{topdir}/main.iter.md", text).unwrap();
    assert_eq!(d.nodetype, NodeType::Project);
    assert_eq!(d.name, "iter4");
    assert!(d.desc.starts_with("The iter harness"));
    assert_eq!(d.front.get("scandirs"), Some(&json!(["{topdir}/"])));
    assert!(!d.front.contains_key("globalinterfacedir"));
    assert_eq!(d.children.reqs.len(), 2, "globalcontextfiles → children.reqs");
    assert_eq!(d.children.codenodes.len(), 4);
}

#[test]
fn legacy_reqpaths_and_testpaths() {
    let d = parse("{topdir}/reqs/iter4.bizreq.iter.md", include_str!("fixtures/iter4.bizreq.iter.md.fixture")).unwrap();
    assert_eq!(d.children.reqs, vec!["{topdir}/requirements.md"]);
    let d = parse("{topdir}/t/iter_core.tests.iter.md", include_str!("fixtures/iter_core.tests.iter.md.fixture")).unwrap();
    assert_eq!(d.nodetype, NodeType::Test);
    assert_eq!(d.children.tests, vec!["{thisfiledir}/*.sh"]);
}

#[test]
fn legacy_reqs_merge_and_dedupe() {
    let text = "---\nid: 3f0c1d2e-4a5b-4c6d-8e7f-0123456789ab\nchildren:\n  reqs: [\"a\"]\n  bizreqs: [\"a\", \"b\"]\n  techreqs: c\n  testgroups: [\"t\"]\n  tests: [\"t\", \"u\"]\nbizreqs: [\"d\"]\n---\n";
    let d = parse("x.code.iter.md", text).unwrap();
    assert_eq!(d.children.reqs, vec!["a", "b", "c", "d"]);
    assert_eq!(d.children.tests, vec!["t", "u"]);
}

#[test]
fn legacy_test_loop_and_desc_precedence() {
    let text = "---\nid: 3f0c1d2e-4a5b-4c6d-8e7f-0123456789ab\ntest_loop: blocked\ndesc: \"new\"\ndescription: \"old\"\n---\n";
    let d = parse("x.code.iter.md", text).unwrap();
    assert_eq!(d.teststate, "block");
    assert_eq!(d.desc, "new");
    let text = "---\nid: 3f0c1d2e-4a5b-4c6d-8e7f-0123456789ab\nteststate: omit\ntest_loop: include\n---\n";
    assert_eq!(parse("x.code.iter.md", text).unwrap().teststate, "omit");
}

#[test]
fn legacy_interface_file_keys_dropped_by_conform() {
    // the file itself is not a node in iter5 (the converter removes it), but
    // renamed to a node type its keys conform cleanly
    let text = include_str!("fixtures/datasync-claim.interface.iter.md.fixture");
    assert_eq!(type_of("datasync-claim.interface.iter.md"), None);
    let c = conform("datasync-claim.interface.iter.md", text, NOW, "x");
    assert!(!c.changed);
    assert_eq!(c.findings[0].code, "not-a-node");
    let c = conform("{topdir}/x/datasync-claim.code.iter.md", text, NOW, "x");
    assert!(c.changed);
    let d = c.doc.unwrap();
    assert_eq!(d.front.get("kind"), Some(&json!("request-reply")), "unknown keys survive");
    assert_eq!(d.front.get("label"), Some(&json!("Claims a waiting graph edit")));
}

#[test]
fn timestamps_legacy_aliases() {
    let text = "---\nid: 3f0c1d2e-4a5b-4c6d-8e7f-0123456789ab\ntimestamps: {created: \"2020-01-01 00:00:00Z\", modified: \"2020-02-01 00:00:00Z\", other: x}\n---\n";
    let d = parse("x.code.iter.md", text).unwrap();
    assert_eq!(d.timestamps.create, "2020-01-01 00:00:00Z");
    assert_eq!(d.timestamps.last_modified, "2020-02-01 00:00:00Z");
    assert_eq!(d.timestamps.extra["other"], "x");
}

/* ------------------------------------------------------------- conform */

#[test]
fn conform_idempotent_over_fixtures() {
    for (path, text) in fixtures() {
        let once = conform(path, &text, NOW, "agent.test");
        let twice = conform(path, &once.text, LATER, "someone.else");
        assert_eq!(twice.text, once.text, "not idempotent: {}\n--- once:\n{}", path, once.text);
        assert!(!twice.changed, "{}", path);
        let thrice = conform(path, &twice.text, LATER, "x");
        assert_eq!(thrice.text, once.text);
        if is_synced(type_of(path).unwrap()) {
            let d = parse(path, &once.text).unwrap_or_else(|e| panic!("{}: {}", path, e));
            assert!(is_valid_id(&d.id));
            assert_eq!(render(&d), once.text, "{}", path);
        }
    }
}

#[test]
fn conform_idempotent_over_repo_tree() {
    // every *.iter.md in the iter5 checkout (iter4-era files) conforms to a
    // fixed point — the property the engine's filescan relies on
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap().to_path_buf();
    let mut stack = vec![root.clone()];
    let mut n = 0;
    while let Some(dir) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&dir) else { continue };
        for e in rd.flatten() {
            let p = e.path();
            let name = e.file_name().to_string_lossy().to_string();
            if p.is_dir() {
                if !matches!(name.as_str(), "target" | ".git" | "node_modules") && !name.starts_with('.') {
                    stack.push(p);
                }
            } else if name.ends_with(".iter.md") {
                let Ok(text) = std::fs::read_to_string(&p) else { continue };
                let rel = format!("{{topdir}}/{}", p.strip_prefix(&root).unwrap().to_string_lossy());
                let once = conform(&rel, &text, NOW, "t");
                let twice = conform(&rel, &once.text, LATER, "t");
                assert_eq!(twice.text, once.text, "{}", rel);
                n += 1;
            }
        }
    }
    assert!(n > 20, "only {} node files found", n);
}

#[test]
fn conform_no_frontmatter() {
    let c = conform("{topdir}/x/plain.code.iter.md", "# Just a heading\n\nNo frontmatter.\n", NOW, "agent.code");
    assert!(c.changed);
    let d = c.doc.unwrap();
    assert!(is_valid_id(&d.id));
    assert_eq!(d.name, "Just a heading", "name derived from the heading");
    assert_eq!(d.creator, "agent.code");
    assert_eq!(d.teststate, "inherit");
    assert_eq!(d.level.as_deref(), Some("component"));
    assert_eq!(d.timestamps.create, NOW);
    assert_eq!(d.timestamps.last_modified, NOW);
    assert_eq!(d.timestamps.last_tested, "");
    assert_eq!(d.body, "# Just a heading\n\nNo frontmatter.\n");
    assert!(c.findings.iter().any(|f| f.code == "frontmatter-added"));
    assert!(c.findings.iter().any(|f| f.code == "id-missing"));
    assert!(c.findings.iter().any(|f| f.code == "level-missing"));
}

#[test]
fn conform_name_from_filename_when_no_heading() {
    let c = conform("{topdir}/x/billing.code.iter.md", "", NOW, "");
    assert_eq!(c.doc.unwrap().name, "billing");
    let c = conform("{topdir}/x/code.iter.md", "", NOW, "");
    assert_eq!(c.doc.unwrap().name, "code");
}

#[test]
fn conform_empty_present_keys_stay_empty() {
    let text = format!("---\nid: {}\nname: \"\"\ndesc: \"\"\ncreator: \"\"\n---\n# Heading\n", ID);
    let d = conform("x.techreq.iter.md", &text, NOW, "who").doc.unwrap();
    assert_eq!(d.name, "", "empty is OK, only missing is filled");
    assert_eq!(d.creator, "");
    assert_eq!(d.front.get("status"), Some(&json!("draft")));
}

#[test]
fn conform_all_common_keys_present() {
    for (path, text) in fixtures() {
        let c = conform(path, &text, NOW, "t");
        if c.doc.is_none() {
            continue;
        }
        let front = yaml::split(&c.text).0.unwrap();
        for k in ["id:", "name:", "desc:", "creator:", "teststate:", "children:", "timestamps:"] {
            assert!(front.lines().any(|l| l.starts_with(k)), "{} missing {}", path, k);
        }
        for k in ["codedirs:", "codenodes:", "tests:", "reqs:"] {
            assert!(front.lines().any(|l| l.trim_start().starts_with(k)), "{} missing children.{}", path, k);
        }
    }
}

#[test]
fn conform_malformed_id_replaced() {
    let c = conform("{topdir}/x/broken.code.iter.md", "---\nid: not-a-uuid\nname: \"b\"\nlevel: component\n---\n", NOW, "t");
    let d = c.doc.unwrap();
    assert!(is_valid_id(&d.id));
    assert!(c.findings.iter().any(|f| f.code == "id-malformed"));
}

#[test]
fn conform_keeps_valid_uppercase_id() {
    let up = ID.to_uppercase();
    let c = conform("x.code.iter.md", &format!("---\nid: {}\nlevel: context\n---\n", up), NOW, "t");
    assert_eq!(c.doc.unwrap().id, up, "a parseable id is never replaced");
}

#[test]
fn conform_invalid_level_and_teststate_are_findings_only() {
    let text = format!("---\nid: {}\nlevel: code\nteststate: maybe\nowner: mine\n---\n", ID);
    let c = conform("x.code.iter.md", &text, NOW, "t");
    let d = c.doc.unwrap();
    assert_eq!(d.level.as_deref(), Some("code"), "not auto-set");
    assert_eq!(d.teststate, "maybe");
    let codes: Vec<&str> = c.findings.iter().map(|f| f.code.as_str()).collect();
    assert!(codes.contains(&"level-invalid"));
    assert!(codes.contains(&"teststate-invalid"));
    assert!(codes.contains(&"owner-invalid"));
}

#[test]
fn conform_broken_yaml_is_repaired() {
    let text = "---\nid: 3f0c1d2e-4a5b-4c6d-8e7f-0123456789ab\nname: Broken: has a colon\ndescription: \"ok\"\n---\nbody\n";
    let c = conform("{topdir}/x/broken.code.iter.md", text, NOW, "t");
    let d = c.doc.unwrap();
    assert_eq!(d.name, "Broken: has a colon");
    assert_eq!(d.desc, "ok");
    assert_eq!(d.id, "3f0c1d2e-4a5b-4c6d-8e7f-0123456789ab");
    assert!(c.findings.iter().any(|f| f.code == "yaml-repaired"));
}

#[test]
fn conform_crlf_and_bom() {
    let text = format!("\u{feff}---\r\nid: {}\r\nname: \"crlf\"\r\nlevel: context\r\n---\r\nBody\r\n", ID);
    let c = conform("x.code.iter.md", &text, NOW, "t");
    let d = c.doc.unwrap();
    assert_eq!(d.id, ID);
    assert_eq!(d.name, "crlf");
    assert_eq!(d.body, "Body\r\n");
}

#[test]
fn conform_unterminated_fence_reads_as_body() {
    let c = conform("x.philosophy.iter.md", "---\nname: x\n", NOW, "t");
    assert!(c.findings.iter().any(|f| f.code == "frontmatter-unterminated"));
    let d = c.doc.unwrap();
    assert_eq!(d.body, "---\nname: x\n");
}

#[test]
fn conform_type_defaults() {
    let p = conform("{topdir}/global/p.project.iter.md", "", NOW, "t").doc.unwrap();
    assert_eq!(p.front["scandirs"], json!(["{topdir}/"]));
    assert_eq!(p.front["file_naming"], json!("sequence"));
    assert_eq!(p.front["gitrepo"], json!(""));
    let a = conform("a.actor.iter.md", "", NOW, "t").doc.unwrap();
    assert_eq!(a.front["drives"], json!([]));
    assert_eq!(a.front["touches"], json!([]));
    let u = conform("a.usecase.iter.md", "", NOW, "t").doc.unwrap();
    assert_eq!(u.front["actors"], json!([]));
    let c = conform("a.code.iter.md", "---\nlevel: connection\n---\n", NOW, "t").doc.unwrap();
    assert_eq!(c.front["connects"], json!({"from": [], "to": []}));
    let r = conform("a.bizreq.iter.md", "", NOW, "t").doc.unwrap();
    assert_eq!(r.front["status"], json!("draft"));
    let ph = conform("a.philosophy.iter.md", "", NOW, "t").doc.unwrap();
    assert!(ph.front.is_empty());
}

#[test]
fn conform_coerces_single_string_lists() {
    let c = conform("a.actor.iter.md", "---\ndrives: \"u.usecase.iter.md\"\ntouches: null\n---\n", NOW, "t");
    let d = c.doc.unwrap();
    assert_eq!(d.front["drives"], json!(["u.usecase.iter.md"]));
    assert_eq!(d.front["touches"], json!([]));
    assert!(c.findings.iter().any(|f| f.code == "list-coerced"));
    let d = conform("a.code.iter.md", "---\nlevel: connection\nconnects: {from: x, to: [y]}\n---\n", NOW, "t").doc.unwrap();
    assert_eq!(d.front["connects"], json!({"from": ["x"], "to": ["y"]}));
}

#[test]
fn conform_connects_partial_and_misplaced() {
    let c = conform("a.code.iter.md", "---\nlevel: connection\nconnects: {from: [\"x\"]}\n---\n", NOW, "t").doc.unwrap();
    assert_eq!(c.front["connects"], json!({"from": ["x"], "to": []}));
    let c = conform("a.code.iter.md", "---\nlevel: component\nconnects: {from: [\"x\"]}\n---\n", NOW, "t");
    assert!(c.findings.iter().any(|f| f.code == "connects-on-non-connection"));
}

#[test]
fn conform_agentmem_and_plain_untouched() {
    let text = include_str!("fixtures/src.agentmemory.iter.md.fixture");
    let c = conform("{topdir}/iter_core/src/src.agentmemory.iter.md", text, NOW, "t");
    assert!(!c.changed);
    assert_eq!(c.text, text);
    assert!(c.doc.is_none());
    let c = conform("{topdir}/notes.iter.md", "hello", NOW, "t");
    assert!(!c.changed);
    assert_eq!(c.findings[0].code, "not-a-node");
}

#[test]
fn conform_bumps_last_modified_on_migration_only() {
    let canonical = sample_code();
    // formatting-only difference (key order / spacing): no bump
    let reordered = canonical.replace("name: \"Postgres\"\n", "").replace("teststate: inherit\n", "teststate: inherit\nname:    Postgres\n");
    let c = conform("{topdir}/a.code.iter.md", &reordered, LATER, "t");
    assert!(c.changed);
    assert_eq!(c.text, canonical);
    assert_eq!(c.doc.unwrap().timestamps.last_modified, NOW);
    // a legacy key: bump
    let legacy = canonical.replace("desc: \"The database.\"", "description: \"The database.\"");
    let c = conform("{topdir}/a.code.iter.md", &legacy, LATER, "t");
    let d = c.doc.unwrap();
    assert_eq!(d.timestamps.last_modified, LATER);
    assert_eq!(d.timestamps.create, NOW, "create never moves");
}

#[test]
fn conform_against_prev_hash_bumps_hand_edits() {
    let canonical = sample_code();
    let d0 = parse("{topdir}/a.code.iter.md", &canonical).unwrap();
    let prev = semantic_hash(&d0);
    let edited = canonical.replace("Body text.", "Body text, edited by hand.");
    let c = conform_against("{topdir}/a.code.iter.md", &edited, LATER, "t", Some(&prev));
    assert_eq!(c.doc.as_ref().unwrap().timestamps.last_modified, LATER);
    // same content as prev: no bump
    let c = conform_against("{topdir}/a.code.iter.md", &canonical, LATER, "t", Some(&prev));
    assert!(!c.changed);
    // without prev a hand edit to the body is not detected (documented)
    let c = conform("{topdir}/a.code.iter.md", &edited, LATER, "t");
    assert_eq!(c.doc.unwrap().timestamps.last_modified, NOW);
}

#[test]
fn conform_creator_only_fills_missing() {
    let c = conform("a.code.iter.md", &format!("---\nid: {}\nlevel: context\n---\n", ID), NOW, "agent.code");
    assert_eq!(c.doc.unwrap().creator, "agent.code");
    let c = conform("a.code.iter.md", &format!("---\nid: {}\ncreator: stephen\nlevel: context\n---\n", ID), NOW, "agent.code");
    assert_eq!(c.doc.unwrap().creator, "stephen");
}

#[test]
fn conform_create_falls_back_to_last_modified() {
    let text = format!("---\nid: {}\nlevel: context\ntimestamps: {{last_modified: \"2020-01-01 00:00:00Z\"}}\n---\n", ID);
    let d = conform("a.code.iter.md", &text, NOW, "t").doc.unwrap();
    assert_eq!(d.timestamps.create, "2020-01-01 00:00:00Z");
}

#[test]
fn conform_findings_stable_codes() {
    let known = [
        "not-a-node",
        "frontmatter-added",
        "frontmatter-unterminated",
        "id-missing",
        "id-malformed",
        "keys-added",
        "legacy-key",
        "legacy-folded",
        "dropped-interface",
        "level-missing",
        "level-invalid",
        "teststate-invalid",
        "owner-invalid",
        "status-invalid",
        "file-naming-invalid",
        "connects-invalid",
        "connects-on-non-connection",
        "yaml-repaired",
        "children-invalid",
        "timestamps-invalid",
        "list-coerced",
    ];
    for (path, text) in fixtures() {
        for f in conform(path, &text, NOW, "t").findings {
            assert!(known.contains(&f.code.as_str()), "unexpected code {} ({})", f.code, path);
        }
    }
}

#[test]
fn conformed_doc_matches_text() {
    for (path, text) in fixtures() {
        let c = conform(path, &text, NOW, "t");
        if let Some(d) = c.doc {
            assert_eq!(render(&d), c.text);
        }
    }
}

/* ------------------------------------------------------------- hashes */

#[test]
fn content_hash_is_sha256_hex() {
    assert_eq!(content_hash(""), "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855");
    assert_eq!(content_hash("abc").len(), 64);
}

#[test]
fn semantic_hash_ignores_timestamps_and_path() {
    let a = parse("{topdir}/a.code.iter.md", &sample_code()).unwrap();
    let mut b = a.clone();
    b.timestamps.last_modified = LATER.into();
    b.timestamps.last_tested = LATER.into();
    b.path = "{topdir}/moved/a.code.iter.md".into();
    assert_eq!(semantic_hash(&a), semantic_hash(&b));
    b.body.push('x');
    assert_ne!(semantic_hash(&a), semantic_hash(&b));
    let mut c = a.clone();
    c.front.insert("last_result".into(), json!({"overall_success": true}));
    assert_ne!(semantic_hash(&a), semantic_hash(&c));
}

#[test]
fn now_ts_format() {
    let t = now_ts();
    assert_eq!(t.len(), 20);
    assert!(t.ends_with('Z'));
    assert!(chrono::NaiveDateTime::parse_from_str(&t, "%Y-%m-%d %H:%M:%SZ").is_ok());
}

/* ---------------------------------------------------------------- slug */

#[test]
fn slug_rules() {
    assert_eq!(slug("API call over HTTP"), "api_call_over_http");
    assert_eq!(slug("  Hello,  World!! "), "hello_world");
    assert_eq!(slug("already-ok_name"), "already-ok_name");
    assert_eq!(slug("Café / Ünïcode"), "caf_n_code");
    assert_eq!(slug("***"), "");
    assert_eq!(slug(""), "");
    assert_eq!(slug("--x--"), "x");
    let long = "a".repeat(100);
    assert_eq!(slug(&long).len(), 60);
    let s = slug(&format!("{} b", "a".repeat(59)));
    assert!(s.len() <= 60 && !s.ends_with('_'), "{}", s);
}

/* ----------------------------------------------------------- plan_path */

fn set(paths: &[&str]) -> HashSet<String> {
    paths.iter().map(|s| s.to_string()).collect()
}

#[test]
fn plan_path_folder_rules() {
    let none = HashSet::new();
    let n = Naming::Sequence;
    let project = doc(NodeType::Project, "{topdir}/global/my_repo.project.iter.md", "My Repo");
    assert_eq!(plan_path(&project, None, Attach::Root, &none, n), "{topdir}/global/my_repo.project.iter.md");
    let uc = doc(NodeType::Usecase, "", "Environ");
    assert_eq!(plan_path(&uc, None, Attach::Root, &none, n), "{topdir}/global/usecases/environ.usecase.iter.md");
    let actor = doc(NodeType::Actor, "", "Employer");
    assert_eq!(plan_path(&actor, None, Attach::Root, &none, n), "{topdir}/global/usecases/employer.actor.iter.md");
    let req = doc(NodeType::Techreq, "", "Global");
    assert_eq!(
        plan_path(&req, Some(&project), Attach::Under(EdgeKind::Reqs), &none, n),
        "{topdir}/global/requirements/my_repo.techreq.iter.md",
        "a project's requirement file is named after the project (§2.8)"
    );
    assert_eq!(plan_path(&req, None, Attach::Root, &none, n), "{topdir}/global/requirements/global.techreq.iter.md");
    let ctx = doc(NodeType::Code, "{topdir}/src/data/data.code.iter.md", "Data");
    let new_ctx = doc(NodeType::Code, "", "Data");
    assert_eq!(plan_path(&new_ctx, None, Attach::Root, &none, n), "{topdir}/src/data/data.code.iter.md");
    let pg = doc(NodeType::Code, "", "Postgres");
    assert_eq!(
        plan_path(&pg, Some(&ctx), Attach::Under(EdgeKind::Codenodes), &none, n),
        "{topdir}/src/data/postgres/postgres.code.iter.md"
    );
    let ph = doc(NodeType::Philosophy, "", "Keep it small");
    assert_eq!(
        plan_path(&ph, Some(&ctx), Attach::Under(EdgeKind::Reqs), &none, n),
        "{topdir}/src/data/reqs/keep_it_small.philosophy.iter.md"
    );
    let test = doc(NodeType::Test, "", "Data");
    assert_eq!(plan_path(&test, Some(&ctx), Attach::Under(EdgeKind::Tests), &none, n), "{topdir}/src/data/data.test.iter.md");
    let mut conn = doc(NodeType::Code, "", "API call over HTTP");
    conn.level = Some("connection".into());
    assert_eq!(
        plan_path(&conn, Some(&ctx), Attach::Under(EdgeKind::Codenodes), &none, n),
        "{topdir}/global/connections/api_call_over_http.code.iter.md"
    );
}

#[test]
fn plan_path_code_under_usecase_goes_to_src() {
    let uc = doc(NodeType::Usecase, "{topdir}/global/usecases/u.usecase.iter.md", "U");
    let c = doc(NodeType::Code, "", "Worker");
    assert_eq!(
        plan_path(&c, Some(&uc), Attach::Under(EdgeKind::Uses), &HashSet::new(), Naming::Sequence),
        "{topdir}/src/worker/worker.code.iter.md"
    );
}

#[test]
fn plan_path_empty_slug_uses_type() {
    let c = doc(NodeType::Bizreq, "", "!!!");
    assert_eq!(plan_path(&c, None, Attach::Root, &HashSet::new(), Naming::Sequence), "{topdir}/global/requirements/bizreq.bizreq.iter.md");
}

#[test]
fn plan_path_collision_sequence() {
    let req = doc(NodeType::Bizreq, "", "Global");
    let existing = set(&[
        "{topdir}/global/requirements/global.bizreq.iter.md",
        "{topdir}/global/requirements/global01.bizreq.iter.md",
    ]);
    assert_eq!(
        plan_path(&req, None, Attach::Root, &existing, Naming::Sequence),
        "{topdir}/global/requirements/global02.bizreq.iter.md"
    );
    // a different type with the same slug is no collision
    let tr = doc(NodeType::Techreq, "", "Global");
    assert_eq!(plan_path(&tr, None, Attach::Root, &existing, Naming::Sequence), "{topdir}/global/requirements/global.techreq.iter.md");
}

#[test]
fn plan_path_collision_uuid12() {
    let mut req = doc(NodeType::Bizreq, "", "Global");
    req.id = ID.into();
    let existing = set(&["{topdir}/global/requirements/global.bizreq.iter.md"]);
    assert_eq!(
        plan_path(&req, None, Attach::Root, &existing, Naming::Uuid12),
        "{topdir}/global/requirements/global_0123456789ab.bizreq.iter.md"
    );
    let existing = set(&[
        "{topdir}/global/requirements/global.bizreq.iter.md",
        "{topdir}/global/requirements/global_0123456789ab.bizreq.iter.md",
    ]);
    assert_eq!(
        plan_path(&req, None, Attach::Root, &existing, Naming::Uuid12),
        "{topdir}/global/requirements/global_0123456789ab01.bizreq.iter.md"
    );
}

#[test]
fn plan_path_nested_code_dir_collision() {
    let c = doc(NodeType::Code, "", "Data");
    // the dir src/data/ already holds another node: the new one must not share it
    let existing = set(&["{topdir}/src/data/sub/sub.code.iter.md"]);
    assert_eq!(plan_path(&c, None, Attach::Root, &existing, Naming::Sequence), "{topdir}/src/data01/data01.code.iter.md");
    let existing = set(&["{topdir}/src/data/data.code.iter.md"]);
    let mut c2 = c.clone();
    c2.id = ID.into();
    assert_eq!(
        plan_path(&c2, None, Attach::Root, &existing, Naming::Uuid12),
        "{topdir}/src/data_0123456789ab/data_0123456789ab.code.iter.md"
    );
}

#[test]
fn naming_from_str() {
    assert_eq!(Naming::from_setting("uuid12"), Naming::Uuid12);
    assert_eq!(Naming::from_setting("sequence"), Naming::Sequence);
    assert_eq!(Naming::from_setting(""), Naming::Sequence);
}

/* ------------------------------------------------------------- resolve */

fn node_paths() -> Vec<String> {
    [
        "{topdir}/global/my.project.iter.md",
        "{topdir}/global/requirements/global.bizreq.iter.md",
        "{topdir}/global/requirements/global.techreq.iter.md",
        "{topdir}/global/requirements/global.philosophy.iter.md",
        "{topdir}/src/data/data.code.iter.md",
        "{topdir}/src/data/data.test.iter.md",
        "{topdir}/src/data/postgres/postgres.code.iter.md",
        "{topdir}/src/data/postgres/postgres01.test.iter.md",
        "{topdir}/src/data/postgres/plugins/enforcement.code.iter.md",
        "{topdir}/src/data/reqs/r1.bizreq.iter.md",
        "{topdir}/src/webui/webui.code.iter.md",
        "{topdir}/global/usecases/environ.usecase.iter.md",
        "{topdir}/global/usecases/employer.actor.iter.md",
        "{topdir}/global/connections/api.code.iter.md",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect()
}

const DATA: &str = "{topdir}/src/data/data.code.iter.md";

#[test]
fn resolve_exact_and_missing() {
    let np = node_paths();
    assert_eq!(resolve("{topdir}/src/webui/webui.code.iter.md", DATA, &np), vec!["{topdir}/src/webui/webui.code.iter.md"]);
    assert!(resolve("{topdir}/src/nope.code.iter.md", DATA, &np).is_empty());
    assert!(resolve("", DATA, &np).is_empty());
    assert!(resolve("{topdir}/src/data/dockerfile", DATA, &np).is_empty(), "non-node files never resolve");
}

#[test]
fn resolve_placeholders() {
    let np = node_paths();
    assert_eq!(resolve("{thisfiledir}/data.test.iter.md", DATA, &np), vec!["{topdir}/src/data/data.test.iter.md"]);
    assert_eq!(resolve("{thisfiledir}/{thisfilestem}.test.iter.md", DATA, &np), vec!["{topdir}/src/data/data.test.iter.md"]);
    assert_eq!(resolve("{thisfiledir}/{thisfilename}", DATA, &np), vec![DATA.to_string()]);
    let np2 = vec!["{topdir}/docs/acme/x.bizreq.iter.md".to_string()];
    assert_eq!(resolve_in("{topdir}/docs/{projectname}/", DATA, &np2, Some("acme")), np2);
    assert!(resolve("{topdir}/docs/{projectname}/", DATA, &np2).is_empty());
}

#[test]
fn resolve_relative_paths() {
    let np = node_paths();
    assert_eq!(resolve("postgres/postgres.code.iter.md", DATA, &np), vec!["{topdir}/src/data/postgres/postgres.code.iter.md"]);
    assert_eq!(resolve("./data.test.iter.md", DATA, &np), vec!["{topdir}/src/data/data.test.iter.md"]);
    assert_eq!(resolve("../webui/webui.code.iter.md", DATA, &np), vec!["{topdir}/src/webui/webui.code.iter.md"]);
    assert_eq!(
        resolve("../../../../global/connections/api.code.iter.md", DATA, &np),
        vec!["{topdir}/global/connections/api.code.iter.md"],
        "never climbs above topdir"
    );
}

#[test]
fn resolve_directory_matches_everything_under_it() {
    let np = node_paths();
    let r = resolve("{thisfiledir}/postgres", DATA, &np);
    assert_eq!(r.len(), 3, "{:?}", r);
    let r2 = resolve("{thisfiledir}/postgres/", DATA, &np);
    assert_eq!(r, r2);
    let r = resolve("{topdir}/global/requirements/", DATA, &np);
    assert_eq!(r.len(), 3);
    // a dir prefix must end at a segment boundary
    assert!(resolve("{topdir}/src/dat", DATA, &np).is_empty());
}

#[test]
fn resolve_globs() {
    let np = node_paths();
    let r = resolve("{thisfiledir}/*.test.iter.md", DATA, &np);
    assert_eq!(r, vec!["{topdir}/src/data/data.test.iter.md"], "* stays inside one directory");
    let r = resolve("{thisfiledir}/**/*.test.iter.md", DATA, &np);
    assert_eq!(r.len(), 2, "** spans directories and zero dirs: {:?}", r);
    let r = resolve("{thisfiledir}/**", DATA, &np);
    assert_eq!(r.len(), 6);
    let r = resolve("{topdir}/**/*.code.iter.md", DATA, &np);
    assert_eq!(r.len(), 5);
    let r = resolve("{topdir}/global/requirements/*.?izreq.iter.md", DATA, &np);
    assert_eq!(r.len(), 1);
    let r = resolve("{topdir}/global/requirements/*.[bt]*req.iter.md", DATA, &np);
    assert_eq!(r.len(), 2);
    let r = resolve("reqs/*bizreq.iter.md", DATA, &np);
    assert_eq!(r, vec!["{topdir}/src/data/reqs/r1.bizreq.iter.md"]);
}

#[test]
fn expand_entry_rules() {
    assert_eq!(expand_entry("{thisfiledir}/tests/{thisfilestem}*.sh", "{topdir}/a/b.test.iter.md", None), "{topdir}/a/tests/b*.sh");
    assert_eq!(expand_entry("x//y/./z/", "{topdir}/a/b.code.iter.md", None), "{topdir}/a/x/y/z/");
    assert_eq!(expand_entry("/abs/path", "{topdir}/a/b.code.iter.md", None), "/abs/path");
    assert_eq!(expand_entry("{topdir}/", "{topdir}/a/b.code.iter.md", None), "{topdir}/");
}

#[test]
fn stem_and_dir_helpers() {
    assert_eq!(stem_of("mylib.code.iter.md"), "mylib");
    assert_eq!(stem_of("{topdir}/a/b.c.test.iter.md"), "b.c");
    assert_eq!(stem_of("code.iter.md"), "");
    assert_eq!(stem_of("notes.iter.md"), "notes");
    assert_eq!(dir_of("{topdir}/a/b.code.iter.md"), "{topdir}/a");
    assert_eq!(dir_of("b.code.iter.md"), "");
    assert_eq!(file_name_of("{topdir}/a/b.code.iter.md"), "b.code.iter.md");
}

/* --------------------------------------------------------------- edges */

fn ids_for(paths: &[String]) -> Vec<(String, String)> {
    paths.iter().enumerate().map(|(i, p)| (format!("id{:02}", i), p.clone())).collect()
}

fn id_of(by_id: &[(String, String)], path: &str) -> String {
    by_id.iter().find(|(_, p)| p == path).unwrap().0.clone()
}

#[test]
fn edges_codenodes_tests_reqs() {
    let np = node_paths();
    let by_id = ids_for(&np);
    let mut d = doc(NodeType::Code, DATA, "Data");
    d.id = id_of(&by_id, DATA);
    d.level = Some("context".into());
    d.children.codenodes = vec!["{thisfiledir}/**/*.code.iter.md".into()];
    d.children.tests = vec!["{thisfiledir}/*.test.iter.md".into()];
    d.children.reqs = vec!["{thisfiledir}/reqs/".into(), "{topdir}/global/requirements/global.philosophy.iter.md".into()];
    d.children.codedirs = vec!["{thisfiledir}/**".into()];
    let e = edges_of(&d, &by_id);
    let kinds = |k: EdgeKind| e.iter().filter(|x| x.kind == k).count();
    assert_eq!(kinds(EdgeKind::Codenodes), 2, "self excluded: {:?}", e);
    assert_eq!(kinds(EdgeKind::Tests), 1);
    assert_eq!(kinds(EdgeKind::Reqs), 2);
    assert_eq!(e.len(), 5, "codedirs make no edges");
    assert!(e.iter().all(|x| x.from == d.id));
}

#[test]
fn edges_wrong_target_types_ignored() {
    let np = node_paths();
    let by_id = ids_for(&np);
    let mut d = doc(NodeType::Code, DATA, "Data");
    d.id = id_of(&by_id, DATA);
    d.children.codenodes = vec!["{thisfiledir}/".into()]; // also matches tests & reqs
    d.children.reqs = vec!["{topdir}/src/webui/webui.code.iter.md".into()];
    d.children.tests = vec!["{topdir}/src/data/reqs/".into()];
    let e = edges_of(&d, &by_id);
    assert!(e.iter().all(|x| x.kind == EdgeKind::Codenodes), "{:?}", e);
    assert_eq!(e.len(), 2);
}

#[test]
fn edges_test_node_scripts_are_not_edges() {
    let np = node_paths();
    let by_id = ids_for(&np);
    let p = "{topdir}/src/data/data.test.iter.md";
    let mut t = doc(NodeType::Test, p, "Data tests");
    t.id = id_of(&by_id, p);
    t.children.tests = vec!["{topdir}/src/data/postgres/postgres01.test.iter.md".into()];
    assert!(edges_of(&t, &by_id).is_empty());
}

#[test]
fn edges_connection_supplies_and_connects() {
    let np = node_paths();
    let by_id = ids_for(&np);
    let p = "{topdir}/global/connections/api.code.iter.md";
    let mut c = doc(NodeType::Code, p, "API");
    c.id = id_of(&by_id, p);
    c.level = Some("connection".into());
    c.front.insert(
        "connects".into(),
        json!({"from": ["{topdir}/src/webui/webui.code.iter.md"], "to": ["{topdir}/src/data/", "{topdir}/global/usecases/environ.usecase.iter.md"]}),
    );
    let e = edges_of(&c, &by_id);
    let webui = id_of(&by_id, "{topdir}/src/webui/webui.code.iter.md");
    assert!(e.contains(&Edge { from: webui, kind: EdgeKind::Supplies, to: c.id.clone() }));
    let connects: Vec<_> = e.iter().filter(|x| x.kind == EdgeKind::Connects).collect();
    assert_eq!(connects.len(), 3, "data, postgres, enforcement; usecase ignored: {:?}", connects);
    assert!(connects.iter().all(|x| x.from == c.id));
    // the same key on a non-connection makes no edges
    c.level = Some("component".into());
    assert!(edges_of(&c, &by_id).is_empty());
}

#[test]
fn edges_actor_drives_touches() {
    let np = node_paths();
    let by_id = ids_for(&np);
    let p = "{topdir}/global/usecases/employer.actor.iter.md";
    let mut a = doc(NodeType::Actor, p, "Employer");
    a.id = id_of(&by_id, p);
    a.front.insert("drives".into(), json!(["environ.usecase.iter.md"]));
    a.front.insert("touches".into(), json!("{topdir}/src/webui/webui.code.iter.md"));
    let e = edges_of(&a, &by_id);
    let uc = id_of(&by_id, "{topdir}/global/usecases/environ.usecase.iter.md");
    let webui = id_of(&by_id, "{topdir}/src/webui/webui.code.iter.md");
    assert_eq!(
        e,
        {
            let mut v = vec![
                Edge { from: a.id.clone(), kind: EdgeKind::Drives, to: uc },
                Edge { from: a.id.clone(), kind: EdgeKind::Touches, to: webui },
            ];
            v.sort();
            v
        }
    );
}

#[test]
fn edges_usecase_uses_and_actors() {
    let np = node_paths();
    let by_id = ids_for(&np);
    let p = "{topdir}/global/usecases/environ.usecase.iter.md";
    let mut u = doc(NodeType::Usecase, p, "Environ");
    u.id = id_of(&by_id, p);
    u.children.codenodes = vec!["{topdir}/src/webui/webui.code.iter.md".into(), DATA.into()];
    u.front.insert("actors".into(), json!(["{thisfiledir}/*.actor.iter.md"]));
    let e = edges_of(&u, &by_id);
    assert_eq!(e.iter().filter(|x| x.kind == EdgeKind::Uses && x.from == u.id).count(), 2);
    assert_eq!(e.iter().filter(|x| x.kind == EdgeKind::Codenodes).count(), 0, "usecase codenodes are `uses`");
    let actor = id_of(&by_id, "{topdir}/global/usecases/employer.actor.iter.md");
    assert!(e.contains(&Edge { from: actor, kind: EdgeKind::Drives, to: u.id.clone() }));
}

#[test]
fn edges_project_roots_and_global_reqs() {
    let np = node_paths();
    let by_id = ids_for(&np);
    let p = "{topdir}/global/my.project.iter.md";
    let mut pr = doc(NodeType::Project, p, "My");
    pr.id = id_of(&by_id, p);
    pr.children.codenodes = vec!["{topdir}/src/*/*.code.iter.md".into()];
    pr.children.reqs = vec!["requirements/".into()];
    let e = edges_of(&pr, &by_id);
    assert_eq!(e.iter().filter(|x| x.kind == EdgeKind::Codenodes).count(), 2);
    assert_eq!(e.iter().filter(|x| x.kind == EdgeKind::Reqs).count(), 3);
}

#[test]
fn edges_dedup_and_unknown_paths() {
    let np = node_paths();
    let by_id = ids_for(&np);
    let mut d = doc(NodeType::Code, DATA, "Data");
    d.id = id_of(&by_id, DATA);
    d.children.tests = vec!["data.test.iter.md".into(), "{thisfiledir}/data.test.iter.md".into(), "{thisfiledir}/missing.test.iter.md".into()];
    assert_eq!(edges_of(&d, &by_id).len(), 1);
    let agentmem = doc(NodeType::Agentmem, "{topdir}/src/x.agentmem.iter.md", "m");
    assert!(edges_of(&agentmem, &by_id).is_empty());
}

#[test]
fn edge_kind_strings_and_owner() {
    for k in EdgeKind::ALL {
        assert_eq!(EdgeKind::from_name(k.as_str()), Some(k));
        assert_eq!(serde_json::to_value(k).unwrap(), json!(k.as_str()));
    }
    assert!(owner_is_target(EdgeKind::Supplies));
    assert!(!owner_is_target(EdgeKind::Connects));
}

/* ------------------------------------------------- add / remove child */

#[test]
fn add_child_writes_right_key() {
    let mut code = doc(NodeType::Code, DATA, "Data");
    add_child(&mut code, EdgeKind::Codenodes, "{topdir}/src/data/pg/pg.code.iter.md").unwrap();
    add_child(&mut code, EdgeKind::Tests, "{topdir}/src/data/data.test.iter.md").unwrap();
    add_child(&mut code, EdgeKind::Reqs, "{topdir}/global/requirements/x.bizreq.iter.md").unwrap();
    assert_eq!(code.children.codenodes, vec!["{topdir}/src/data/pg/pg.code.iter.md"]);
    assert_eq!(code.children.tests.len(), 1);
    assert_eq!(code.children.reqs.len(), 1);
    assert!(matches!(add_child(&mut code, EdgeKind::Drives, "x"), Err(NodeErr::WrongKind { .. })));
    assert!(matches!(add_child(&mut code, EdgeKind::Supplies, "x"), Err(NodeErr::WrongKind { .. })));
    assert!(matches!(add_child(&mut code, EdgeKind::Uses, "x"), Err(NodeErr::WrongKind { .. })));

    let mut conn = doc(NodeType::Code, "{topdir}/global/connections/api.code.iter.md", "API");
    conn.level = Some("connection".into());
    add_child(&mut conn, EdgeKind::Supplies, "{topdir}/gw/gw.code.iter.md").unwrap();
    add_child(&mut conn, EdgeKind::Connects, "{topdir}/app/app.code.iter.md").unwrap();
    assert_eq!(conn.front["connects"], json!({"from": ["{topdir}/gw/gw.code.iter.md"], "to": ["{topdir}/app/app.code.iter.md"]}));

    let mut actor = doc(NodeType::Actor, "{topdir}/global/usecases/a.actor.iter.md", "A");
    add_child(&mut actor, EdgeKind::Drives, "{topdir}/global/usecases/u.usecase.iter.md").unwrap();
    add_child(&mut actor, EdgeKind::Touches, "{topdir}/src/webui/webui.code.iter.md").unwrap();
    assert_eq!(actor.front["drives"], json!(["{topdir}/global/usecases/u.usecase.iter.md"]));
    assert_eq!(actor.front["touches"], json!(["{topdir}/src/webui/webui.code.iter.md"]));

    let mut uc = doc(NodeType::Usecase, "{topdir}/global/usecases/u.usecase.iter.md", "U");
    add_child(&mut uc, EdgeKind::Uses, DATA).unwrap();
    add_child(&mut uc, EdgeKind::Drives, "{topdir}/global/usecases/a.actor.iter.md").unwrap();
    assert_eq!(uc.children.codenodes, vec![DATA]);
    assert_eq!(uc.front["actors"], json!(["{topdir}/global/usecases/a.actor.iter.md"]));
}

#[test]
fn add_child_round_trips_into_edges() {
    let np = node_paths();
    let by_id = ids_for(&np);
    let p = "{topdir}/global/connections/api.code.iter.md";
    let mut c = doc(NodeType::Code, p, "API");
    c.id = id_of(&by_id, p);
    c.level = Some("connection".into());
    add_child(&mut c, EdgeKind::Supplies, "{topdir}/src/webui/webui.code.iter.md").unwrap();
    let text = render(&c);
    let (back, _) = parse_tolerant(p, &text).unwrap();
    let e = edges_of(&back, &by_id);
    assert_eq!(e.len(), 1);
    assert_eq!(e[0].kind, EdgeKind::Supplies);
    assert_eq!(e[0].to, c.id);
}

#[test]
fn add_child_noop_when_already_covered() {
    let mut code = doc(NodeType::Code, DATA, "Data");
    code.children.codenodes = vec!["{thisfiledir}/**/*.code.iter.md".into()];
    add_child(&mut code, EdgeKind::Codenodes, "{topdir}/src/data/pg/pg.code.iter.md").unwrap();
    assert_eq!(code.children.codenodes.len(), 1);
    add_child(&mut code, EdgeKind::Codenodes, "{topdir}/src/other/o.code.iter.md").unwrap();
    add_child(&mut code, EdgeKind::Codenodes, "{topdir}/src/other/o.code.iter.md").unwrap();
    assert_eq!(code.children.codenodes.len(), 2);
}

#[test]
fn remove_child_exact() {
    let mut code = doc(NodeType::Code, DATA, "Data");
    code.children.tests = vec!["{topdir}/src/data/data.test.iter.md".into(), "other.test.iter.md".into()];
    remove_child(&mut code, EdgeKind::Tests, "{topdir}/src/data/data.test.iter.md").unwrap();
    assert_eq!(code.children.tests, vec!["other.test.iter.md"]);
    // a relative entry is removed by its resolved path
    remove_child(&mut code, EdgeKind::Tests, "{topdir}/src/data/other.test.iter.md").unwrap();
    assert!(code.children.tests.is_empty());
    assert!(matches!(remove_child(&mut code, EdgeKind::Tests, "{topdir}/x.test.iter.md"), Err(NodeErr::NoSuchChild(_))));
}

#[test]
fn remove_child_refuses_glob_and_dir() {
    let mut code = doc(NodeType::Code, DATA, "Data");
    code.children.codenodes = vec!["{thisfiledir}/**/*.code.iter.md".into()];
    let before = code.clone();
    let err = remove_child(&mut code, EdgeKind::Codenodes, "{topdir}/src/data/pg/pg.code.iter.md").unwrap_err();
    assert!(matches!(err, NodeErr::GlobOnly { .. }), "{}", err);
    assert_eq!(code, before, "nothing changed");
    // exact + glob both match: still refused (the edge would survive)
    code.children.codenodes.push("{topdir}/src/data/pg/pg.code.iter.md".into());
    assert!(matches!(remove_child(&mut code, EdgeKind::Codenodes, "{topdir}/src/data/pg/pg.code.iter.md"), Err(NodeErr::GlobOnly { .. })));
    // a directory entry counts as a pattern
    code.children.reqs = vec!["{thisfiledir}/reqs/".into()];
    assert!(matches!(remove_child(&mut code, EdgeKind::Reqs, "{topdir}/src/data/reqs/r1.bizreq.iter.md"), Err(NodeErr::GlobOnly { .. })));
}

#[test]
fn remove_child_connection_actor_usecase() {
    let mut conn = doc(NodeType::Code, "{topdir}/global/connections/api.code.iter.md", "API");
    conn.level = Some("connection".into());
    add_child(&mut conn, EdgeKind::Supplies, "{topdir}/gw/gw.code.iter.md").unwrap();
    add_child(&mut conn, EdgeKind::Connects, "{topdir}/app/app.code.iter.md").unwrap();
    remove_child(&mut conn, EdgeKind::Supplies, "{topdir}/gw/gw.code.iter.md").unwrap();
    assert_eq!(conn.front["connects"], json!({"from": [], "to": ["{topdir}/app/app.code.iter.md"]}));
    let mut uc = doc(NodeType::Usecase, "{topdir}/global/usecases/u.usecase.iter.md", "U");
    add_child(&mut uc, EdgeKind::Drives, "{topdir}/global/usecases/a.actor.iter.md").unwrap();
    remove_child(&mut uc, EdgeKind::Drives, "{topdir}/global/usecases/a.actor.iter.md").unwrap();
    assert_eq!(uc.front["actors"], json!([]));
}

/* --------------------------------------------------------------- misc */

#[test]
fn new_doc_defaults() {
    let t = NodeDoc::new(NodeType::Test, "T", "stephen", NOW);
    assert!(is_valid_id(&t.id));
    assert_eq!(t.children.tests, vec!["{thisfiledir}/tests/{thisfilestem}*.sh"]);
    assert_eq!(t.timestamps.create, NOW);
    let c = NodeDoc::new(NodeType::Code, "C", "s", NOW);
    assert_eq!(c.level.as_deref(), Some("component"));
    // a new doc is already conformed
    let mut c = c;
    c.path = "{topdir}/src/c/c.code.iter.md".into();
    let text = render(&c);
    assert!(!conform(&c.path, &text, LATER, "x").changed);
}

#[test]
fn quoting_survives_hostile_strings() {
    let mut d = doc(NodeType::Bizreq, "{topdir}/r.bizreq.iter.md", "x");
    for s in ["", "true", "null", "123", "1e3", "- dash", "#hash", "a: b", "{curly}", "[sq]", "'single'", "tab\there", "\u{7f}del", "émoji 🚀", "~", "@at", "`tick`", "%pct", "yes", "No"] {
        d.name = s.into();
        d.desc = s.into();
        d.creator = s.into();
        d.front.insert("custom".into(), json!([s, {"k": s}]));
        d.children.reqs = vec![s.into()];
        let back = parse(&d.path, &render(&d)).unwrap();
        assert_eq!(back.name, s);
        assert_eq!(back.front["custom"], json!([s, {"k": s}]), "{:?}", s);
        assert_eq!(back.children.reqs, if s.is_empty() { vec![] } else { vec![s.to_string()] });
    }
}

#[test]
fn unquoted_placeholders_are_repaired() {
    let text = "---\nid: 3f0c1d2e-4a5b-4c6d-8e7f-0123456789ab\nlevel: context\nscandirs: {topdir}/\nchildren:\n  codedirs: [{thisfiledir}/**, \"{topdir}/x\"]\n  codenodes:\n    - {topdir}/a/a.code.iter.md  # trailing comment\n  reqs: [{topdir}/r/, 'q']\n---\nbody\n";
    let c = conform("{topdir}/a.code.iter.md", text, NOW, "t");
    let d = c.doc.unwrap();
    assert_eq!(d.children.codedirs, vec!["{thisfiledir}/**", "{topdir}/x"]);
    assert_eq!(d.children.codenodes, vec!["{topdir}/a/a.code.iter.md"]);
    assert_eq!(d.children.reqs, vec!["{topdir}/r/", "q"]);
    assert_eq!(d.front["scandirs"], json!(["{topdir}/"]), "coerced to a list");
    assert!(c.findings.iter().any(|f| f.code == "yaml-repaired"));
    assert_eq!(conform("{topdir}/a.code.iter.md", &c.text, LATER, "t").text, c.text);
}

#[test]
fn quote_placeholders_leaves_quoted_and_non_placeholder_braces() {
    let s = "a: \"{topdir}/x\"\nb: {k: v}\nc: '{thisfiledir}'\n";
    assert_eq!(yaml::quote_placeholders(s), s);
}
