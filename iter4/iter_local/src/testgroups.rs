use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

pub const BLOCK_START: &str = "<!-- iterapp:testgroups";
pub const BLOCK_END: &str = "-->";

/// One registered test: a shell script plus its human-facing identity. The script
/// is the entire contract (see features/TDD.md "Test Contract"): exit 0 = green,
/// 1 = red, anything else = the script itself broke (`error`); the last stdout line
/// may be `ITER_RESULT pass=X fail=Y total=Z` for per-test counts.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default, from = "TestEntryDe")]
pub struct TestEntry {
    pub id: String,
    pub name: String,
    pub desc: String,
    pub shell: String,
    /// Whether this entry's result decides the group's colour (2026-09-28,
    /// F20). `false` = run it, log it, show its counts, but leave it out of the
    /// verdict — for inventory-style checks that report on the whole repo and
    /// must not turn a group red. Defaults true and is only written when false,
    /// so existing registries stay byte-identical.
    #[serde(skip_serializing_if = "is_true")]
    pub gates: bool,
    /// Which part of the input space this test covers (2026-09-30): one of
    /// `KINDS` — golden, malformed, longtail, failure. "" = not classified
    /// yet; the sweep asks the test agent to classify it.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub kind: String,
}

/// The four kinds of test a group's coverage is measured in (Stephen,
/// 2026-09-30): the expected / golden path; allowable malformed, incomplete
/// or missing inputs; long-tail inputs; and expected failures.
pub const KINDS: [&str; 4] = ["golden", "malformed", "longtail", "failure"];

/// How many tests of each kind a group should have, set by the test agent
/// from the group's input space: a function taking one boolean needs about
/// two tests in all, one taking an open JSON document needs a collection of
/// each kind. 0 = the kind does not apply (the `input_space` says why).
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(default)]
pub struct Coverage {
    pub golden: u32,
    pub malformed: u32,
    pub longtail: u32,
    pub failure: u32,
}

impl Coverage {
    pub fn target(&self, kind: &str) -> u32 {
        match kind {
            "golden" => self.golden,
            "malformed" => self.malformed,
            "longtail" => self.longtail,
            "failure" => self.failure,
            _ => 0,
        }
    }
}

impl Default for TestEntry {
    fn default() -> TestEntry {
        TestEntry { id: String::new(), name: String::new(), desc: String::new(), shell: String::new(), gates: true, kind: String::new() }
    }
}

fn is_true(b: &bool) -> bool {
    *b
}

fn default_true() -> bool {
    true
}

/// Back-compat: testlists written before the structured schema were bare script
/// names (`"testscript03.sh"`). Those deserialize into a full entry with the id
/// derived from the filename stem.
#[derive(Deserialize)]
#[serde(untagged)]
enum TestEntryDe {
    Script(String),
    Entry {
        #[serde(default)]
        id: String,
        #[serde(default)]
        name: String,
        #[serde(default)]
        desc: String,
        #[serde(default)]
        shell: String,
        #[serde(default = "default_true")]
        gates: bool,
        #[serde(default)]
        kind: String,
    },
}

impl From<TestEntryDe> for TestEntry {
    fn from(de: TestEntryDe) -> TestEntry {
        match de {
            TestEntryDe::Script(shell) => {
                let id = shell.trim_end_matches(".sh").to_string();
                TestEntry { id: id.clone(), name: id, desc: String::new(), shell, gates: true, kind: String::new() }
            }
            TestEntryDe::Entry { id, name, desc, shell, gates, kind } => {
                let id = if id.is_empty() { shell.trim_end_matches(".sh").to_string() } else { id };
                TestEntry { id, name, desc, shell, gates, kind }
            }
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct TestGroup {
    pub label: String,
    /// What this group is supposed to prove — surfaced in the testing UI.
    pub desc: String,
    /// Gates the STATE of sweep-born fix items, never their existence: red run →
    /// fix item `queued` when true (work proceeds next pick), `todo` when false
    /// (sits for human review). Defaults false.
    pub auto_fix: bool,
    pub lastrun: String,
    pub result: String,
    pub counts: String,
    /// What the code under test accepts and how many practical permutations
    /// that allows, written by the test agent (2026-09-30); it justifies the
    /// `coverage` targets, including any kind set to 0. "" = not assessed.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub input_space: String,
    /// Target number of tests per kind; absent = not assessed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub coverage: Option<Coverage>,
    pub testlist: Vec<TestEntry>,
}

impl TestGroup {
    /// What keeps this group short of its coverage (2026-09-30), plain
    /// phrases for the top-up item; empty = covered. Golden paths always
    /// need at least one test, whatever the target says.
    pub fn coverage_gaps(&self) -> Vec<String> {
        let mut gaps = Vec::new();
        let unclassified = self.testlist.iter().filter(|t| !KINDS.contains(&t.kind.as_str())).count();
        if unclassified > 0 {
            gaps.push(format!("{unclassified} test(s) have no kind (golden, malformed, longtail or failure)"));
        }
        let Some(cov) = self.coverage.as_ref().filter(|_| !self.input_space.trim().is_empty()) else {
            gaps.push("input space not assessed: no `input_space` and `coverage` targets".into());
            return gaps;
        };
        for kind in KINDS {
            let want = if kind == "golden" { cov.target(kind).max(1) } else { cov.target(kind) };
            let have = self.testlist.iter().filter(|t| t.kind == kind).count() as u32;
            if have < want {
                gaps.push(format!("{kind}: {have} of {want}"));
            }
        }
        gaps
    }

    /// "Provably green right now": the last recorded run passed.
    pub fn is_green(&self) -> bool {
        self.result == "passed"
    }
}

/// Parse the `iterapp:testgroups` JSONL block from a testgroups.iter.md document.
/// A missing block means "never tested": returns an empty list.
pub fn parse(content: &str) -> Vec<TestGroup> {
    let Some(start) = content.find(BLOCK_START) else { return Vec::new() };
    let after = &content[start + BLOCK_START.len()..];
    let Some(end) = after.find(BLOCK_END) else { return Vec::new() };
    let mut groups = Vec::new();
    for line in after[..end].lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        match serde_json::from_str::<TestGroup>(line) {
            Ok(g) => groups.push(g),
            Err(e) => eprintln!("warning: bad testgroups line ({}): {}", e, line),
        }
    }
    groups
}

/// Replace (or append) the `iterapp:testgroups` block with the given groups,
/// leaving all human-facing markdown untouched.
pub fn update(content: &str, groups: &[TestGroup]) -> String {
    let mut block = String::from(BLOCK_START);
    block.push('\n');
    for g in groups {
        block.push_str(&serde_json::to_string(g).expect("testgroup serializes"));
        block.push('\n');
    }
    block.push_str(BLOCK_END);

    if let Some(start) = content.find(BLOCK_START) {
        if let Some(end_rel) = content[start..].find(BLOCK_END) {
            let end = start + end_rel + BLOCK_END.len();
            return format!("{}{}{}", &content[..start], block, &content[end..]);
        }
    }
    format!("{}\n\n{}\n", content.trim_end(), block)
}

/// Every testgroup file under `code_root` (skipping VCS/build noise), identified
/// by FILENAME role: any `*testgroup.iter.md` (see markers::role_of). Scripts and
/// the `runs/` history resolve relative to the file's directory; which C4 object
/// OWNS a file is declared by that object's marker (`testgroup:` key), never
/// inferred from position.
pub fn find_files(code_root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    collect_files(code_root, &mut out);
    let mut out = crate::drop_git_ignored(code_root, out);
    out.sort();
    out
}

fn collect_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().into_owned();
        if path.is_dir() {
            if crate::is_skip_dir(&name) {
                continue;
            }
            collect_files(&path, out);
        } else if crate::markers::role_of(&name) == Some(crate::markers::Role::Testgroup) {
            out.push(path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Two groups in one file — the shape that exposed the sweep's same-file
    /// write race (V2 read this from a since-deleted sample project).
    #[test]
    fn parses_two_groups_in_one_file() {
        let doc = "# parser tests\n\n<!-- iterapp:testgroups\n\
            {\"label\":\"parser decisions\",\"result\":\"passed\",\"counts\":\"3/3\",\"testlist\":[{\"id\":\"t1\",\"name\":\"decisions\",\"desc\":\"\",\"shell\":\"t1-decisions.sh\"}]}\n\
            {\"label\":\"parser refusals\",\"testlist\":[{\"id\":\"t2\",\"name\":\"refusals\",\"desc\":\"\",\"shell\":\"t2-refusals.sh\"}]}\n\
            -->\n";
        let groups = parse(doc);
        assert_eq!(groups.len(), 2);
        assert_eq!(groups[0].label, "parser decisions");
        assert_eq!(groups[0].testlist[0].shell, "t1-decisions.sh");
        assert_eq!(groups[1].label, "parser refusals");
        assert_eq!(groups[1].testlist[0].shell, "t2-refusals.sh");
        assert!(groups[0].is_green());
        assert!(!groups[1].is_green(), "never run = not provably green");
    }

    #[test]
    fn coverage_survives_a_rewrite_and_names_its_gaps() {
        let doc = "<!-- iterapp:testgroups\n{\"label\":\"g\",\"input_space\":\"one bool\",\"coverage\":{\"golden\":2,\"failure\":1},\"testlist\":[{\"id\":\"t1\",\"shell\":\"t1.sh\",\"kind\":\"golden\"},{\"id\":\"t2\",\"shell\":\"t2.sh\"}]}\n-->";
        let groups = parse(doc);
        let again = parse(&update(doc, &groups));
        assert_eq!(again[0].coverage, Some(Coverage { golden: 2, malformed: 0, longtail: 0, failure: 1 }));
        assert_eq!(again[0].input_space, "one bool");
        assert_eq!(again[0].testlist[0].kind, "golden");
        assert_eq!(again[0].coverage_gaps(), vec![
            "1 test(s) have no kind (golden, malformed, longtail or failure)".to_string(),
            "golden: 1 of 2".to_string(),
            "failure: 0 of 1".to_string(),
        ]);
        let bare = parse("<!-- iterapp:testgroups\n{\"label\":\"b\",\"testlist\":[{\"id\":\"t\",\"shell\":\"t.sh\",\"kind\":\"golden\"}]}\n-->");
        assert_eq!(bare[0].coverage_gaps(), vec!["input space not assessed: no `input_space` and `coverage` targets".to_string()]);
    }

    #[test]
    fn bare_string_testlist_still_parses() {
        let doc = "<!-- iterapp:testgroups\n{\"label\":\"legacy\",\"testlist\":[\"testscript03.sh\",{\"id\":\"t2\",\"name\":\"named\",\"desc\":\"d\",\"shell\":\"t2.sh\"}]}\n-->";
        let groups = parse(doc);
        assert_eq!(groups[0].testlist.len(), 2);
        assert_eq!(groups[0].testlist[0].id, "testscript03");
        assert_eq!(groups[0].testlist[0].shell, "testscript03.sh");
        assert_eq!(groups[0].testlist[1].id, "t2");
        assert_eq!(groups[0].testlist[1].name, "named");
        assert!(!groups[0].auto_fix, "auto_fix defaults false");
    }

    #[test]
    fn gates_defaults_true_and_is_written_only_when_false() {
        let doc = "<!-- iterapp:testgroups\n{\"label\":\"g\",\"testlist\":[{\"id\":\"a\",\"shell\":\"a.sh\"},{\"id\":\"b\",\"shell\":\"b.sh\",\"gates\":false}]}\n-->";
        let groups = parse(doc);
        assert!(groups[0].testlist[0].gates, "an entry without the key gates");
        assert!(!groups[0].testlist[1].gates);
        let out = update("", &groups);
        assert_eq!(out.matches("\"gates\"").count(), 1, "only the non-gating entry writes the key: {out}");
        assert!(!parse(&out)[0].testlist[1].gates);
    }

    #[test]
    fn update_roundtrip_preserves_prose() {
        let doc = "# My tests\n\nprose stays\n\n<!-- iterapp:testgroups\n{\"label\":\"a\",\"lastrun\":\"\",\"result\":\"\",\"counts\":\"\",\"testlist\":[]}\n-->\n";
        let mut groups = parse(doc);
        groups[0].result = "passed".into();
        groups[0].counts = "5/5".into();
        let updated = update(doc, &groups);
        assert!(updated.contains("prose stays"));
        let reparsed = parse(&updated);
        assert_eq!(reparsed[0].result, "passed");
        assert_eq!(reparsed[0].counts, "5/5");
        assert!(reparsed[0].is_green());
    }

    #[test]
    fn update_appends_block_when_missing() {
        let doc = "# no block here\n";
        let groups = vec![TestGroup { label: "g".into(), ..Default::default() }];
        let updated = update(doc, &groups);
        assert_eq!(parse(&updated).len(), 1);
        assert!(updated.starts_with("# no block here"));
    }

    #[test]
    fn finds_testgroup_files_by_filename_role() {
        let root = std::env::temp_dir().join(format!("iter-tgfind-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("comp/test")).unwrap();
        std::fs::create_dir_all(root.join("target/skip")).unwrap();
        std::fs::write(root.join("comp/test/testgroup.iter.md"), "x").unwrap();
        std::fs::write(root.join("comp/extra.testgroup.iter.md"), "x").unwrap();
        std::fs::write(root.join("comp/testgroups.iter.md"), "x").unwrap(); // old plural: NOT the role
        std::fs::write(root.join("target/skip/testgroup.iter.md"), "x").unwrap();
        let found = find_files(&root);
        assert_eq!(found.len(), 2, "singular-suffix files only, target/ skipped: {:?}", found);
        let _ = std::fs::remove_dir_all(&root);
    }
}
