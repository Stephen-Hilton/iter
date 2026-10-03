//! `iter init` (iter5): scaffold a v5 project in a checkout — the project
//! node `global/<slug>.project.iter.md` and the three global requirement
//! nodes `global/requirements/{philosophy,bizreq,techreq}` (one requirement
//! per file; empty bodies to fill in). Every file is built with
//! `iter_core::nodefile` so it is already conformed. No engine config is
//! written: an engine is started with `iter_engine --data-url URL --env-file
//! PATH` and learns its projects from iter_data (the `serves` edge).
//! An existing file is left alone unless `force`.

use iter_core::nodefile::{self, NodeDoc, NodeType};
use std::path::Path;

/// Returns the files written (relative to `topdir`).
pub fn init_project(topdir: &Path, project: &str, description: &str, creator: &str, force: bool) -> Result<Vec<String>, String> {
    let project = project.trim();
    if project.is_empty() {
        return Err("init: a project name is required".into());
    }
    if !topdir.is_dir() {
        return Err(format!("init: {} is not a directory", topdir.display()));
    }
    let slug = nodefile::slug(project);
    let now = nodefile::now_ts();
    let creator = if creator.trim().is_empty() { "iter init" } else { creator.trim() };
    let desc = description.split_whitespace().collect::<Vec<_>>().join(" ");

    let req = |t: NodeType, name: &str, desc: &str, body: &str| -> (String, String) {
        let mut d = NodeDoc::new(t, name, creator, &now);
        let rel = format!("global/requirements/{}.{}.iter.md", nodefile::slug(name), t.as_str());
        d.path = format!("{{topdir}}/{rel}");
        d.desc = desc.to_string();
        d.body = body.to_string();
        (rel, nodefile::render(&d))
    };
    let philosophy = req(
        NodeType::Philosophy,
        &format!("{project} philosophy"),
        "What the people behind this project want, in plain prose: agents use it to infer missing requirements and settle conflicting ones.",
        &format!("# {project} — philosophy\n\n<What this project is for, who it serves and what matters most when two goals pull apart. Free prose.>\n"),
    );
    let bizreq = req(
        NodeType::Bizreq,
        &format!("{project} first business requirement"),
        "",
        "# First business requirement\n\n<One requirement per file: a statement someone could test, then why it matters.>\n",
    );
    let techreq = req(
        NodeType::Techreq,
        &format!("{project} first technical requirement"),
        "",
        "# First technical requirement\n\n<One engineering rule every part of the project follows, then why.>\n",
    );

    let mut p = NodeDoc::new(NodeType::Project, project, creator, &now);
    let prel = format!("global/{slug}.project.iter.md");
    p.path = format!("{{topdir}}/{prel}");
    p.desc = if desc.is_empty() { format!("The {project} project.") } else { desc.clone() };
    p.children.reqs = vec!["{topdir}/global/requirements/".to_string()];
    p.body = format!(
        "# {project}\n\n{}\n\nThis body is the first thing every agent reads about the project: keep it current. \
         Add one `*.code.iter.md` file per part of the code (context, container, component; connection types in \
         `global/connections/`) and list the top ones under `children.codenodes`; global requirements live in \
         `global/requirements/` (one per file), use cases and actors in `global/usecases/`.\n",
        p.desc
    );
    let files = vec![(prel, nodefile::render(&p)), philosophy, bizreq, techreq];

    let mut written = Vec::new();
    for (rel, content) in files {
        let path = topdir.join(&rel);
        if path.exists() && !force {
            continue;
        }
        if let Some(d) = path.parent() {
            std::fs::create_dir_all(d).map_err(|e| format!("init: cannot create {}: {e}", d.display()))?;
        }
        std::fs::write(&path, content).map_err(|e| format!("init: cannot write {}: {e}", path.display()))?;
        written.push(rel);
    }
    Ok(written)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scaffolds_a_conformed_v5_project_and_never_overwrites() {
        let top = std::env::temp_dir().join(format!("iter5_init_{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&top).unwrap();
        let w = init_project(&top, "Demo Shop", "Sells\n things.", "stephen", false).unwrap();
        assert_eq!(w, vec![
            "global/demo_shop.project.iter.md",
            "global/requirements/demo_shop_philosophy.philosophy.iter.md",
            "global/requirements/demo_shop_first_business_requirement.bizreq.iter.md",
            "global/requirements/demo_shop_first_technical_requirement.techreq.iter.md",
        ]);
        let now = nodefile::now_ts();
        for rel in &w {
            let sp = format!("{{topdir}}/{rel}");
            let t = std::fs::read_to_string(top.join(rel)).unwrap();
            assert!(!nodefile::conform(&sp, &t, &now, "x").changed, "{rel} is conformed");
            nodefile::parse(&sp, &t).unwrap();
        }
        let p = nodefile::parse("{topdir}/global/demo_shop.project.iter.md", &std::fs::read_to_string(top.join(&w[0])).unwrap()).unwrap();
        assert_eq!((p.name.as_str(), p.desc.as_str(), p.creator.as_str()), ("Demo Shop", "Sells things.", "stephen"));
        assert_eq!(p.children.reqs, vec!["{topdir}/global/requirements/"]);
        assert!(!top.join(".iter/config.json").exists());
        assert!(init_project(&top, "Demo Shop", "", "", false).unwrap().is_empty(), "nothing overwritten");
        assert_eq!(init_project(&top, "Demo Shop", "", "", true).unwrap().len(), 4);
        // the validator agrees
        let rep = iter_local::validate::run(&top, None, false).unwrap();
        assert_eq!(rep.exit_code(), 0, "{:?}", rep.files);
    }
}
