//! `iter init` (Phase 2, R15): scaffold a new iter project in a checkout.
//!
//! Writes the head file (`main.iter.md`), the engine's connection config
//! (`.iter/config.json`), `.iter/.gitignore`, one business and one technical
//! requirements node under `reqs/`, and the global `interfaces/` and
//! `usecases/` folders. Every node file's frontmatter starts with a fresh
//! `id: <uuid v4>` (T3). An existing file is left alone unless `force`; the
//! folders are created when missing and never count as written.
//!
//! The webui's new-project wizard renders the same `main.iter.md` and
//! `config.json` from the same inputs, so what the page shows is what this
//! writes.

use std::path::Path;

/// Scaffold a new iter project in `topdir`: main.iter.md, .iter/config.json,
/// .iter/.gitignore, reqs/<name>.bizreq.iter.md, reqs/<name>.techreq.iter.md,
/// interfaces/ and usecases/ dirs. Never overwrites an existing file unless force.
/// Returns the list of files written (paths relative to topdir) or an error string.
pub fn init_project(
    topdir: &Path,
    project: &str,
    data_url: &str,
    engine: &str,
    description: &str,
    force: bool,
) -> Result<Vec<String>, String> {
    let project = project.trim();
    let engine = engine.trim();
    let data_url = data_url.trim();
    if project.is_empty() {
        return Err("init: a project name is required".into());
    }
    if engine.is_empty() {
        return Err("init: an engine name is required".into());
    }
    if data_url.is_empty() {
        return Err("init: a data URL is required (the iter_data server, e.g. http://127.0.0.1:8300)".into());
    }
    if !topdir.is_dir() {
        return Err(format!("init: {} is not a directory", topdir.display()));
    }
    let slug = file_slug(project);
    if slug.is_empty() {
        return Err(format!("init: project name \"{project}\" has no letters or digits to name its files with"));
    }
    let description = one_line(description);

    for dir in [".iter", "reqs", "interfaces", "usecases"] {
        let p = topdir.join(dir);
        std::fs::create_dir_all(&p).map_err(|e| format!("init: cannot create {}: {e}", p.display()))?;
    }

    let biz = format!("reqs/{slug}.bizreq.iter.md");
    let tech = format!("reqs/{slug}.techreq.iter.md");
    let files: Vec<(String, String)> = vec![
        ("main.iter.md".into(), main_file(project, &description, &biz, &tech)),
        (".iter/config.json".into(), config_json(data_url, engine)),
        (".iter/.gitignore".into(), GITIGNORE.to_string()),
        (biz.clone(), req_file(project, "bizreq")),
        (tech.clone(), req_file(project, "techreq")),
    ];

    let mut written = Vec::new();
    for (rel, content) in files {
        let path = topdir.join(&rel);
        if path.exists() && !force {
            continue;
        }
        std::fs::write(&path, content).map_err(|e| format!("init: cannot write {}: {e}", path.display()))?;
        written.push(rel);
    }
    Ok(written)
}

/// What the engine writes under `.iter/` and must never be committed: agent
/// user keys (`iter --adduser`), the `iter` shim and scratch space.
const GITIGNORE: &str = "users/\nbin/\ntemp/\n";

/// `.iter/config.json`: only enough to reach iter_data (everything else lives
/// there). The token itself stays in `.env`, named by `token_envar`.
pub fn config_json(data_url: &str, engine: &str) -> String {
    let v = serde_json::json!({
        "data_url": data_url,
        "token_envar": "ITER_ENGINE_TOKEN",
        "engine_name": engine,
        "env_file": "./.env",
    });
    let mut s = serde_json::to_string_pretty(&v).unwrap_or_default();
    s.push('\n');
    s
}

fn main_file(project: &str, description: &str, biz: &str, tech: &str) -> String {
    let desc = if description.is_empty() { format!("The {project} project.") } else { description.to_string() };
    format!(
        "---\n\
id: {id}\n\
projectname: {name}\n\
projectdescription: {desc_q}\n\
globalscandirs: [\"{{topdir}}/\"]\n\
globalinterfacedir: \"{{topdir}}/interfaces/\"\n\
globalusecasedir: \"{{topdir}}/usecases/\"\n\
globalcontextfiles: [\"{{topdir}}/{biz}\", \"{{topdir}}/{tech}\"]\n\
children:\n  codenodes: []\n\
---\n\n\
# {project}\n\n\
{desc}\n\n\
This body is the first thing every agent reads about the project: keep it\n\
current. Say what the project is, who it serves and the shape of the build.\n\
Add one `*.code.iter.md` file per part of the code (a context, its\n\
containers, their components) and list the top ones under\n\
`children.codenodes` above; put shared contracts in `interfaces/` and the\n\
journeys people take through the product in `usecases/`.\n",
        id = uuid::Uuid::new_v4(),
        name = yaml_str(project),
        desc_q = yaml_str(&desc),
    )
}

fn req_file(project: &str, nodetype: &str) -> String {
    let (title, what, tag) = if nodetype == "bizreq" {
        ("business requirements", "What the project must do for the people who use it, project-wide.", "B")
    } else {
        ("technical requirements", "Engineering rules every part of the project follows.", "T")
    };
    format!(
        "---\n\
id: {id}\n\
name: {name}\n\
description: {desc}\n\
children:\n  reqpaths: []\n\
---\n\n\
# {project} — {title}\n\n\
- **{tag}1.** <one requirement per bullet: a stable id that is never renumbered,\n  and a statement someone could test.>\n",
        id = uuid::Uuid::new_v4(),
        name = yaml_str(&format!("{project} {title}")),
        desc = yaml_str(what),
    )
}

/// A double-quoted frontmatter scalar. The node-file reader strips the quotes
/// but does not unescape, so an inner `"` becomes `'` rather than `\"`.
fn yaml_str(s: &str) -> String {
    format!("\"{}\"", s.replace('"', "'"))
}

/// Descriptions live on one frontmatter line: fold whitespace runs (newlines
/// included) into single spaces.
fn one_line(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// A file-name stem for the project: letters, digits, `-` and `_` kept, every
/// other run of characters (dots included: they would break the dot rule)
/// becomes one `-`.
fn file_slug(project: &str) -> String {
    let mut out = String::new();
    for c in project.chars() {
        if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
            out.push(c);
        } else if !out.ends_with('-') {
            out.push('-');
        }
    }
    out.trim_matches('-').to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use iter_local::ids::{is_uuid, read_id};
    use iter_local::markers::parse_front;

    fn tmp() -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("iter4-init-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn writes_every_file_and_dir() {
        let top = tmp();
        let got = init_project(&top, "demo app", "http://127.0.0.1:8300", "Engine01", "A tiny\ndemo.", false).unwrap();
        assert_eq!(
            got,
            vec![
                "main.iter.md",
                ".iter/config.json",
                ".iter/.gitignore",
                "reqs/demo-app.bizreq.iter.md",
                "reqs/demo-app.techreq.iter.md"
            ]
        );
        for f in &got {
            assert!(top.join(f).is_file(), "{f} missing");
        }
        assert!(top.join("interfaces").is_dir());
        assert!(top.join("usecases").is_dir());
        let gi = std::fs::read_to_string(top.join(".iter/.gitignore")).unwrap();
        assert!(gi.lines().any(|l| l == "users/"));
        std::fs::remove_dir_all(&top).ok();
    }

    #[test]
    fn config_json_shape() {
        let top = tmp();
        init_project(&top, "p", "http://h:8300", "E1", "", false).unwrap();
        let v: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(top.join(".iter/config.json")).unwrap()).unwrap();
        assert_eq!(v["data_url"], "http://h:8300");
        assert_eq!(v["token_envar"], "ITER_ENGINE_TOKEN");
        assert_eq!(v["engine_name"], "E1");
        assert_eq!(v["env_file"], "./.env");
        std::fs::remove_dir_all(&top).ok();
    }

    #[test]
    fn frontmatter_parses_with_id_first() {
        let top = tmp();
        init_project(&top, "demo", "http://x", "E", "Says \"hi\" to people.", false).unwrap();
        let main = std::fs::read_to_string(top.join("main.iter.md")).unwrap();
        assert!(main.starts_with("---\nid: "), "id must be the first key");
        let f = parse_front(&main);
        assert!(f.has_frontmatter);
        assert!(is_uuid(&read_id(&main)));
        assert_eq!(f.scalar("projectname"), "demo");
        assert_eq!(f.scalar("projectdescription"), "Says 'hi' to people.");
        assert_eq!(f.list("globalscandirs"), vec!["{topdir}/"]);
        assert_eq!(
            f.list("globalcontextfiles"),
            vec!["{topdir}/reqs/demo.bizreq.iter.md", "{topdir}/reqs/demo.techreq.iter.md"]
        );
        assert_eq!(f.child("codenodes"), Some(vec![]));
        let mut ids = vec![read_id(&main)];
        for nt in ["bizreq", "techreq"] {
            let body = std::fs::read_to_string(top.join(format!("reqs/demo.{nt}.iter.md"))).unwrap();
            assert!(body.starts_with("---\nid: "));
            let id = read_id(&body);
            assert!(is_uuid(&id));
            ids.push(id);
            let f = parse_front(&body);
            assert!(!f.scalar("name").is_empty());
            assert!(!f.scalar("description").is_empty());
            assert_eq!(f.child("reqpaths"), Some(vec![]));
        }
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), 3, "every node gets its own id");
        std::fs::remove_dir_all(&top).ok();
    }

    #[test]
    fn scaffold_validates_clean() {
        let top = tmp();
        init_project(&top, "demo", "http://x", "E", "d", false).unwrap();
        let rep = iter_local::validate::run(&[top.clone()], None, false).unwrap();
        assert_eq!(rep.files_checked, 3);
        assert!(rep.findings.is_empty(), "{:?}", rep.findings);
        std::fs::remove_dir_all(&top).ok();
    }

    #[test]
    fn never_overwrites_without_force() {
        let top = tmp();
        init_project(&top, "demo", "http://x", "E", "d", false).unwrap();
        std::fs::write(top.join("main.iter.md"), "mine").unwrap();
        let again = init_project(&top, "demo", "http://y", "E2", "d", false).unwrap();
        assert!(again.is_empty(), "{again:?}");
        assert_eq!(std::fs::read_to_string(top.join("main.iter.md")).unwrap(), "mine");
        assert!(std::fs::read_to_string(top.join(".iter/config.json")).unwrap().contains("http://x"));

        // a missing file is filled in, the rest left alone
        std::fs::remove_file(top.join(".iter/.gitignore")).unwrap();
        assert_eq!(init_project(&top, "demo", "http://y", "E2", "d", false).unwrap(), vec![".iter/.gitignore"]);

        let forced = init_project(&top, "demo", "http://y", "E2", "d", true).unwrap();
        assert_eq!(forced.len(), 5);
        assert!(std::fs::read_to_string(top.join("main.iter.md")).unwrap().starts_with("---\nid: "));
        assert!(std::fs::read_to_string(top.join(".iter/config.json")).unwrap().contains("http://y"));
        std::fs::remove_dir_all(&top).ok();
    }

    #[test]
    fn refuses_bad_input() {
        let top = tmp();
        assert!(init_project(&top, " ", "http://x", "E", "", false).is_err());
        assert!(init_project(&top, "p", "", "E", "", false).is_err());
        assert!(init_project(&top, "p", "http://x", "", "", false).is_err());
        assert!(init_project(&top, "...", "http://x", "E", "", false).is_err());
        assert!(init_project(&top.join("nope"), "p", "http://x", "E", "", false).is_err());
        std::fs::remove_dir_all(&top).ok();
    }

    #[test]
    fn slug_keeps_the_dot_rule() {
        assert_eq!(file_slug("My App.v2"), "My-App-v2");
        assert_eq!(file_slug("pdy-dev"), "pdy-dev");
    }
}
