//! A cloud agent's files, read on its machine for the panels that show
//! them (SANDBOX, the plans). Only an awake machine is read, and reading
//! neither wakes it nor keeps it awake.

use std::path::{Component, Path};

use aura_core_types::AgentId;
use aura_engine_cloud::machine_env::WORKSPACE_DIR;
use aura_engine_cloud::{CloudError, CloudResult};
use aura_exec_tools::plan_tools::{PLANS_DIR, PLAN_SUFFIX};

use super::CloudHub;
use crate::cloud::node_client::{FileEntry, NodeClient};

/// Plans read from one machine at most.
const MAX_PLANS: usize = 20;

impl CloudHub {
    /// What directory `dir`, under the machine's workspace, holds on
    /// `agent`'s machine.
    ///
    /// # Errors
    ///
    /// When the machine is not awake, `dir` is outside its workspace, or
    /// the node can't list it.
    pub async fn list_files(&self, agent: AgentId, dir: &Path) -> CloudResult<Vec<FileEntry>> {
        let dir = workspace_relative(dir)?;
        let entries = self.reading_node(agent)?.list_files(&dir).await?;
        Ok(entries
            .into_iter()
            .filter(|entry| is_one_name(&entry.name))
            .collect())
    }

    /// The text of file `path`, under the machine's workspace, on
    /// `agent`'s machine.
    ///
    /// # Errors
    ///
    /// When the machine is not awake, `path` is outside its workspace, or
    /// the node can't read it.
    pub async fn read_file(&self, agent: AgentId, path: &Path) -> CloudResult<String> {
        let path = workspace_relative(path)?;
        self.reading_node(agent)?.read_file(&path).await
    }

    /// The plans on `agent`'s machine, each its workspace-relative path
    /// and text: `first` (the agent's own plan) ahead of the rest, which
    /// go by name. A plan that can't be read is left out.
    ///
    /// # Errors
    ///
    /// When the machine is not awake or its plans can't be listed.
    pub async fn machine_plans(
        &self,
        agent: AgentId,
        first: Option<&str>,
    ) -> CloudResult<Vec<(String, String)>> {
        let node = self.reading_node(agent)?;
        let mut paths: Vec<String> = node
            .list_files(PLANS_DIR)
            .await?
            .into_iter()
            .filter(|entry| {
                !entry.is_dir && entry.name.ends_with(PLAN_SUFFIX) && is_one_name(&entry.name)
            })
            .map(|entry| format!("{PLANS_DIR}/{}", entry.name))
            .collect();
        paths.sort_by(|a, b| {
            (Some(a.as_str()) != first)
                .cmp(&(Some(b.as_str()) != first))
                .then_with(|| a.cmp(b))
        });
        let mut plans = Vec::new();
        for path in paths.into_iter().take(MAX_PLANS) {
            match node.read_file(&path).await {
                Ok(text) => plans.push((path, text)),
                Err(err) => tracing::debug!(%agent, %path, error = %err, "could not read a plan"),
            }
        }
        Ok(plans)
    }

    /// A client for `agent`'s node, while its machine is awake.
    fn reading_node(&self, agent: AgentId) -> CloudResult<NodeClient> {
        let hex = agent.to_hex();
        let view = self.fleet.view(&hex).ok_or_else(|| CloudError::NotFound {
            what: "machine",
            name: hex.clone(),
        })?;
        if !view.state.is_awake() {
            return Err(CloudError::BadState {
                machine: view.machine.name,
                state: view.state.label().to_string(),
                op: "read its files while it is not awake",
            });
        }
        self.node(&hex)
    }
}

/// `path` relative to the machine's workspace, `.` for the workspace
/// itself; refused outside it, or with a `..` that could climb out.
fn workspace_relative(path: &Path) -> CloudResult<String> {
    let outside = || CloudError::BadState {
        machine: WORKSPACE_DIR.to_string(),
        state: path.display().to_string(),
        op: "read outside the machine's workspace",
    };
    let rest = path.strip_prefix(WORKSPACE_DIR).map_err(|_| outside())?;
    let mut parts = Vec::new();
    for component in rest.components() {
        match component {
            Component::Normal(part) => parts.push(part.to_str().ok_or_else(outside)?),
            _ => return Err(outside()),
        }
    }
    if parts.is_empty() {
        return Ok(".".to_string());
    }
    Ok(parts.join("/"))
}

/// Whether `name` is one file name, as a directory listing holds.
fn is_one_name(name: &str) -> bool {
    let mut components = Path::new(name).components();
    matches!(
        (components.next(), components.next()),
        (Some(Component::Normal(_)), None)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_paths_inside_the_machine_s_workspace_are_read() {
        let at = |path: &str| workspace_relative(Path::new(path)).ok();
        assert_eq!(at("/workspace").as_deref(), Some("."));
        assert_eq!(at("/workspace/src/main.rs").as_deref(), Some("src/main.rs"));
        assert_eq!(at("/workspace/../etc/passwd"), None);
        assert_eq!(at("/workspace/src/../../etc"), None);
        assert_eq!(at("/etc/passwd"), None);
        assert_eq!(at("/workspaces/x"), None);
        assert_eq!(at("src/main.rs"), None);
    }

    #[test]
    fn a_listing_names_entries_not_paths() {
        assert!(is_one_name("main.rs"));
        assert!(is_one_name(".env"));
        assert!(!is_one_name("../etc"));
        assert!(!is_one_name("src/main.rs"));
        assert!(!is_one_name("/etc"));
        assert!(!is_one_name(""));
        assert!(!is_one_name(".."));
    }
}
