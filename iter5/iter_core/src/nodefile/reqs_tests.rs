//! Tests for `nodefile::reqs` (§2.8) and the bizreq/techreq rules of
//! `conform` / `plan_path`.

use super::*;
use std::collections::HashSet;

const NOW: &str = "2026-10-02 14:56:11Z";
const LATER: &str = "2026-10-03 09:00:00Z";
const A: &str = "6f2c4c9e-1111-4aaa-8bbb-000000000001";
const B: &str = "6f2c4c9e-1111-4aaa-8bbb-000000000002";

fn canonical() -> String {
    format!(
        "# Payments — technical requirements\n\nShared rules for the payment path.\n\n\
## PDY-TECH-034 — JWT identifies; only Ed25519 authorizes money\n\
<!-- req: id={A} status=agreed -->\n\
Every caller presents a JSON Web Token. Money moves only on an Ed25519 signature.\n\n\
### Why\n\nTokens leak.\n\n\
## Second requirement title\n\
<!-- req: id={B} status=draft -->\n\
Plain text.\n"
    )
}

#[test]
fn parse_canonical() {
    let (pre, items) = parse_reqs(&canonical());
    assert_eq!(pre, "# Payments — technical requirements\n\nShared rules for the payment path.\n");
    assert_eq!(items.len(), 2);
    assert_eq!(items[0].id, A);
    assert_eq!(items[0].key, "PDY-TECH-034");
    assert_eq!(items[0].title, "JWT identifies; only Ed25519 authorizes money");
    assert_eq!(items[0].status, "agreed");
    assert_eq!(
        items[0].text,
        "Every caller presents a JSON Web Token. Money moves only on an Ed25519 signature.\n\n### Why\n\nTokens leak."
    );
    assert_eq!(items[1].key, "");
    assert_eq!(items[1].title, "Second requirement title");
    assert_eq!(items[1].status, "draft");
    assert_eq!(items[1].text, "Plain text.");
    assert!(items[0].extra.is_empty());
}

#[test]
fn render_round_trip_is_exact_for_canonical_text() {
    let body = canonical();
    let (pre, items) = parse_reqs(&body);
    assert_eq!(render_reqs(&pre, &items), body);
    let (out, findings) = conform_reqs(&body);
    assert_eq!(out, body, "a canonical body is a fixed point");
    assert!(findings.is_empty(), "{findings:?}");
}

#[test]
fn items_round_trip_through_render() {
    let items = vec![
        ReqItem::new("K.1", "title one", "text\n\n### sub\nmore\n", "agreed"),
        ReqItem::new("", "no key — but a dash", "x", ""),
        ReqItem::new("KEY", "", "", "done"),
    ];
    let body = render_reqs("", &items);
    let (pre, back) = parse_reqs(&body);
    assert_eq!(pre, "");
    assert_eq!(back[0], items[0]);
    assert_eq!(back[2], items[2], "key with an empty title survives");
    assert_eq!(back[2].heading(), "KEY —");
    // "no key — but a dash": the part before " — " is not a key (has spaces)
    assert_eq!(back[1], items[1]);
    assert_eq!(items[1].status, "draft", "ReqItem::new defaults the status");
}

#[test]
fn heading_rule() {
    let body = "## PDY-TECH-034 — t\n## a.b_c-1 — t2\n## Not a key — t3\n## Just a title\n## KEY —\n## X—no spaces\n";
    let (_, it) = parse_reqs(body);
    let kt: Vec<(&str, &str)> = it.iter().map(|i| (i.key.as_str(), i.title.as_str())).collect();
    assert_eq!(
        kt,
        vec![
            ("PDY-TECH-034", "t"),
            ("a.b_c-1", "t2"),
            ("", "Not a key — t3"),
            ("", "Just a title"),
            ("KEY", ""),
            ("", "X—no spaces"),
        ]
    );
    assert!(is_req_key("A-1.b_2") && !is_req_key("") && !is_req_key("A B") && !is_req_key("A/B"));
}

#[test]
fn req_node_name_and_first_sentence() {
    let mut i = ReqItem { key: "K-1".into(), title: "Title".into(), ..Default::default() };
    assert_eq!(req_node_name(&i), "K-1 — Title");
    i.key.clear();
    assert_eq!(req_node_name(&i), "Title");
    i.title.clear();
    i.key = "K".into();
    assert_eq!(req_node_name(&i), "K");
    i.key.clear();
    assert_eq!(req_node_name(&i), "untitled requirement");
    assert_eq!(first_sentence("**Every** caller presents a `JWT`. Second one."), "Every caller presents a JWT.");
    assert_eq!(first_sentence("- one\n  two\n\n```\ncode. here\n```\nend"), "one two end");
    assert_eq!(first_sentence("v1.2 is fine. next"), "v1.2 is fine.");
    assert_eq!(first_sentence(""), "");
    assert!(first_sentence(&"word ".repeat(200)).ends_with("..."));
}

#[test]
fn no_sections_is_all_preamble_and_untouched() {
    for body in ["", "\n", "# Title\n\n- **B1.** a bullet\n- **B2.** another\r\n", "### only a sub heading\n"] {
        let (pre, items) = parse_reqs(body);
        assert_eq!(pre, body);
        assert!(items.is_empty());
        assert_eq!(render_reqs(&pre, &items), body);
        let (out, f) = conform_reqs(body);
        assert_eq!(out, body);
        assert!(f.is_empty());
    }
}

#[test]
fn conform_adds_missing_markers_and_keeps_text() {
    let body = "Intro.\n\n## R-1 — first\nThe text of R-1.\n\n## second\n\nText two.   \n\n\n";
    let (out, f) = conform_reqs(body);
    let (pre, items) = parse_reqs(&out);
    assert_eq!(pre, "Intro.\n");
    assert_eq!(items.len(), 2);
    for it in &items {
        assert!(is_valid_id(&it.id));
        assert_eq!(it.status, "draft");
    }
    assert_eq!(items[0].text, "The text of R-1.");
    assert_eq!(items[1].text, "Text two.");
    assert_eq!(f.iter().filter(|x| x.code == "req-marker-missing").count(), 2);
    assert!(out.contains(&format!("## R-1 — first\n<!-- req: id={} status=draft -->\nThe text of R-1.\n\n## second\n", items[0].id)));
    // idempotent
    let (again, f2) = conform_reqs(&out);
    assert_eq!(again, out);
    assert!(f2.is_empty(), "{f2:?}");
}

#[test]
fn conform_repairs_markers() {
    let body = format!(
        "## a\n<!--req:   status=Agreed    id={A}-->\nx\n\
## b\n<!-- req: id=not-a-uuid -->\ny\n\
## c\n<!-- req: status=done -->\nz\n\
## d\n<!-- req: id={A} status=finished owner=stephen note=\"two words\" stray -->\nw\n"
    );
    let (out, f) = conform_reqs(&body);
    let codes: Vec<&str> = f.iter().map(|x| x.code.as_str()).collect();
    for c in ["req-marker-normalised", "req-id-malformed", "req-id-missing", "req-status-missing", "req-id-duplicate", "req-status-invalid", "req-marker-junk"] {
        assert!(codes.contains(&c), "{c} missing from {codes:?}");
    }
    let (_, it) = parse_reqs(&out);
    assert_eq!(it[0].id, A, "the first holder keeps a duplicated id");
    assert_eq!(it[0].status, "agreed", "status lower-cased");
    assert!(is_valid_id(&it[1].id) && is_valid_id(&it[2].id) && is_valid_id(&it[3].id));
    assert_ne!(it[3].id, A, "the later duplicate gets a new id");
    assert_eq!(it[1].status, "draft");
    assert_eq!(it[2].status, "done");
    assert_eq!(it[3].status, "finished", "an invalid status is reported, not replaced");
    assert_eq!(it[3].extra.get("owner").map(String::as_str), Some("stephen"));
    assert_eq!(it[3].extra.get("note").map(String::as_str), Some("two words"));
    assert!(out.contains(&format!("<!-- req: id={} status=finished note=\"two words\" owner=stephen -->", it[3].id)));
    let ids: HashSet<&str> = it.iter().map(|i| i.id.as_str()).collect();
    assert_eq!(ids.len(), 4);
    let (again, _) = conform_reqs(&out);
    assert_eq!(again, out);
}

#[test]
fn marker_found_below_text_moves_to_the_top() {
    let body = format!("## a\nsome text first\n<!-- req: id={A} status=agreed -->\nmore\n");
    let (_, it) = parse_reqs(&body);
    assert_eq!(it[0].id, A);
    assert_eq!(it[0].text, "some text first\nmore");
    let (out, _) = conform_reqs(&body);
    assert_eq!(out, format!("## a\n<!-- req: id={A} status=agreed -->\nsome text first\nmore\n"));
}

#[test]
fn sub_headings_and_fences_stay_inside_text() {
    let body = format!(
        "## a\n<!-- req: id={A} status=draft -->\n### Rationale\n#### deeper\n```md\n## not a heading\n<!-- req: id={B} status=done -->\n```\n~~~\n## nor this\n~~~\nend\n"
    );
    let (_, it) = parse_reqs(&body);
    assert_eq!(it.len(), 1);
    assert_eq!(it[0].id, A);
    assert!(it[0].text.contains("## not a heading") && it[0].text.contains("## nor this"));
    assert!(it[0].text.contains(&format!("<!-- req: id={B} status=done -->")), "a marker inside a fence is text");
    let (out, f) = conform_reqs(&body);
    assert_eq!(out, body);
    assert!(f.is_empty());
}

#[test]
fn crlf_and_trailing_whitespace() {
    let body = format!("Pre  \r\n\r\n## K-1 — t  \r\n<!-- req: id={A} status=agreed -->\r\nline one  \r\nline two\r\n\r\n   \r\n");
    let (out, _) = conform_reqs(&body);
    assert_eq!(out, format!("Pre\n\n## K-1 — t\n<!-- req: id={A} status=agreed -->\nline one  \nline two\n"));
    assert!(!out.contains('\r'));
    assert_eq!(conform_reqs(&out).0, out);
}

#[test]
fn empty_heading_and_empty_text() {
    let body = "##\n\n## only title\n";
    let (out, _) = conform_reqs(body);
    let (_, it) = parse_reqs(&out);
    assert_eq!(it.len(), 2);
    assert_eq!((it[0].key.as_str(), it[0].title.as_str(), it[0].text.as_str()), ("", "", ""));
    assert!(out.starts_with("##\n<!-- req: id="));
    assert_eq!(conform_reqs(&out).0, out);
}

#[test]
fn order_is_preserved_and_serde_shape() {
    let items: Vec<ReqItem> = (0..5).map(|n| ReqItem::new(&format!("R{n}"), &format!("t{n}"), "x", "")).collect();
    let body = render_reqs("p", &items);
    let (_, back) = parse_reqs(&body);
    assert_eq!(back, items);
    let j = serde_json::to_value(&items[0]).unwrap();
    assert_eq!(j["key"], "R0");
    assert!(j.get("extra").is_none(), "empty extra is not serialised");
    let r: ReqItem = serde_json::from_value(serde_json::json!({"id": A, "title": "t"})).unwrap();
    assert_eq!(r.status, "");
}

/* ------------------------------------------------- conform / file level */

#[test]
fn conform_file_runs_conform_reqs_for_bizreq_and_techreq_only() {
    let text = "---\nname: \"Payments\"\n---\n# Payments\n\n## R-1 — one\nText.\n";
    for path in ["{topdir}/src/p/reqs/p.bizreq.iter.md", "{topdir}/src/p/reqs/p.techreq.iter.md"] {
        let c = conform(path, text, NOW, "t");
        assert!(c.findings.iter().any(|f| f.code == "req-marker-missing"), "{path}");
        let d = c.doc.unwrap();
        let (_, it) = parse_reqs(&d.body);
        assert!(is_valid_id(&it[0].id));
        let twice = conform(path, &c.text, LATER, "x");
        assert_eq!(twice.text, c.text, "idempotent");
        assert!(!twice.changed);
        assert_eq!(req_edges(&d), vec![(d.id.clone(), it[0].id.clone())]);
    }
    let ph = conform("{topdir}/x.philosophy.iter.md", text, NOW, "t");
    assert!(!ph.text.contains("<!-- req:"), "philosophy bodies are free prose");
    assert!(req_edges(&ph.doc.unwrap()).is_empty());
}

#[test]
fn conform_bumps_last_modified_when_markers_added() {
    let text = format!(
        "---\nid: {A}\nname: \"x\"\ndesc: \"\"\ncreator: \"s\"\nteststate: inherit\nstatus: draft\nchildren:\n  codedirs:  []\n  codenodes: []\n  tests:     []\n  reqs:      []\ntimestamps: {{create: \"{NOW}\", last_modified: \"{NOW}\", last_tested: \"\"}}\n---\n## a\ntext\n"
    );
    let c = conform("{topdir}/r.bizreq.iter.md", &text, LATER, "s");
    assert_eq!(c.doc.as_ref().unwrap().timestamps.last_modified, LATER);
    let c2 = conform("{topdir}/r.bizreq.iter.md", &c.text, "2026-12-01 00:00:00Z", "s");
    assert!(!c2.changed);
}

/* ------------------------------------------------------------ plan_path */

fn doc(t: NodeType, path: &str, name: &str) -> NodeDoc {
    let mut d = NodeDoc::new(t, name, "test", NOW);
    d.path = path.to_string();
    d
}

#[test]
fn plan_path_one_req_file_per_node() {
    let none = HashSet::new();
    let n = Naming::Sequence;
    let api = doc(NodeType::Code, "{topdir}/src/api/api.code.iter.md", "The API");
    let r = doc(NodeType::Bizreq, "", "Any name at all");
    assert_eq!(plan_path(&r, Some(&api), Attach::Under(EdgeKind::Reqs), &none, n), "{topdir}/src/api/reqs/api.bizreq.iter.md");
    let t = doc(NodeType::Techreq, "", "x");
    assert_eq!(plan_path(&t, Some(&api), Attach::Under(EdgeKind::Reqs), &none, n), "{topdir}/src/api/reqs/api.techreq.iter.md");
    // it exists already: same path (one per node), never api01
    let existing: HashSet<String> = ["{topdir}/src/api/reqs/api.bizreq.iter.md".to_string()].into();
    assert_eq!(plan_path(&r, Some(&api), Attach::Under(EdgeKind::Reqs), &existing, Naming::Uuid12), "{topdir}/src/api/reqs/api.bizreq.iter.md");
    // an attached file of the type at another path wins
    let mut api2 = api.clone();
    api2.children.reqs = vec!["{thisfiledir}/old/".into(), "{topdir}/docs/rules.md".into()];
    let existing: HashSet<String> = [
        "{topdir}/src/api/old/legacy.bizreq.iter.md".to_string(),
        "{topdir}/src/api/old/legacy.philosophy.iter.md".to_string(),
    ]
    .into();
    assert_eq!(plan_path(&r, Some(&api2), Attach::Under(EdgeKind::Reqs), &existing, n), "{topdir}/src/api/old/legacy.bizreq.iter.md");
    assert_eq!(plan_path(&t, Some(&api2), Attach::Under(EdgeKind::Reqs), &existing, n), "{topdir}/src/api/reqs/api.techreq.iter.md");
    // a connection node too
    let mut conn = doc(NodeType::Code, "{topdir}/global/connections/api_call.code.iter.md", "API call");
    conn.level = Some("connection".into());
    assert_eq!(
        plan_path(&r, Some(&conn), Attach::Under(EdgeKind::Reqs), &none, n),
        "{topdir}/global/connections/reqs/api_call.bizreq.iter.md"
    );
    // a code file with no stem: the node's name
    let bare = doc(NodeType::Code, "{topdir}/src/w/code.iter.md", "Web Front");
    assert_eq!(plan_path(&r, Some(&bare), Attach::Under(EdgeKind::Reqs), &none, n), "{topdir}/src/w/reqs/web_front.bizreq.iter.md");
    // philosophy keeps one file per doc
    let ph = doc(NodeType::Philosophy, "", "Keep it small");
    assert_eq!(plan_path(&ph, Some(&api), Attach::Under(EdgeKind::Reqs), &none, n), "{topdir}/src/api/reqs/keep_it_small.philosophy.iter.md");
}

#[test]
fn plan_path_project_pair() {
    let none = HashSet::new();
    let n = Naming::Sequence;
    let p = doc(NodeType::Project, "{topdir}/global/demo_shop.project.iter.md", "Demo Shop");
    let r = doc(NodeType::Bizreq, "", "whatever");
    assert_eq!(plan_path(&r, Some(&p), Attach::Under(EdgeKind::Reqs), &none, n), "{topdir}/global/requirements/demo_shop.bizreq.iter.md");
    let existing: HashSet<String> = ["{topdir}/global/requirements/demo_shop.bizreq.iter.md".to_string()].into();
    assert_eq!(plan_path(&r, Some(&p), Attach::Under(EdgeKind::Reqs), &existing, n), "{topdir}/global/requirements/demo_shop.bizreq.iter.md");
    let mut p2 = p.clone();
    p2.children.reqs = vec!["{topdir}/global/requirements/".into()];
    let existing: HashSet<String> = ["{topdir}/global/requirements/aaa.techreq.iter.md".to_string(), "{topdir}/global/requirements/b.bizreq.iter.md".to_string()].into();
    assert_eq!(plan_path(&r, Some(&p2), Attach::Under(EdgeKind::Reqs), &existing, n), "{topdir}/global/requirements/b.bizreq.iter.md");
    // no parent: named after the doc, collisions numbered (unchanged)
    assert_eq!(plan_path(&r, None, Attach::Root, &none, n), "{topdir}/global/requirements/whatever.bizreq.iter.md");
}
