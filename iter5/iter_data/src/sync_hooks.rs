//! Hooks between the server core (heartbeat, project create) and the project
//! graph ⇄ repo sync (spec §3; bodies in filesync.rs / nodes.rs).

use crate::storage::Storage;

/// Projects (of `projects`) with node changes waiting for an engine to write
/// to files (`file_state` pending_write / pending_delete) — the heartbeat
/// reply's `files_waiting`.
pub async fn files_waiting(store: &dyn Storage, projects: &[String]) -> Vec<String> {
    crate::filesync::files_waiting(store, projects).await
}

/// Projects (of `projects`) with a build requested (`project.build.state ==
/// requested`) — the heartbeat reply's `build_waiting`.
pub async fn build_waiting(store: &dyn Storage, projects: &[String]) -> Vec<String> {
    crate::filesync::build_waiting(store, projects).await
}

/// A project was just created (any path: settings graph, PUT, wizard): its
/// project node and default global reqs, `designed` until it is built
/// (spec §3.4). A failure is logged; GET graph retries lazily.
pub async fn project_created(store: &dyn Storage, project: &str) {
    if let Err(e) = crate::nodes::ensure_project_node(store, project).await {
        let msg = match e {
            crate::api::ApiError::Status(c, m) => format!("{c}: {m}"),
            _ => "refused".into(),
        };
        eprintln!("[iter_data] project {project}: could not create its project node: {msg}");
    }
}
