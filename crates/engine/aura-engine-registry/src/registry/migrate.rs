//! Bringing an older registry up to date, and importing the legacy
//! `agents.json` and identity files.

use std::fs;
use std::path::Path;

use super::{
    canonical, AgentRegistry, AgentRegistryError, Project, RegisteredAgent, RegistryFile,
    DESKTOP_TABS_FILE, LEGACY_IDENTITY_FILE, MIGRATED_REGISTRY_FILE, REGISTRY_FILE,
};
use crate::definitions::COORDINATOR_DEFINITION;

impl AgentRegistry {
    /// Turn a version 0 registry, where the agent heading a project held
    /// its folder and the agents added to it pointed at it, into projects
    /// of their own. A project takes the hex id of the agent that headed
    /// it, so what saved that id still finds it. Projects in one folder
    /// become one. Projects are ordered as the desktop app's tabs had
    /// them, then as the registry listed them.
    pub(super) fn migrate(&mut self, data_dir: &Path) {
        let agents = &mut self.file.agents;
        relink_legacy_members(agents);
        let roots = ordered_roots(agents, data_dir);
        let mut projects: Vec<Project> = self.file.projects.drain(..).collect();
        let project_of_root = project_per_root(agents, &roots, &mut projects, &self.launch_dir);
        assign_projects(agents, &roots, &project_of_root);
        self.file.projects = projects;
    }

    /// Before version 3 a project showed only while it had an agent
    /// listed. Take those with none off the list, as removed, so the list
    /// looks as it did; restoring one of their agents brings them back.
    pub(super) fn hide_empty_projects(&mut self) {
        let shown: Vec<String> = self
            .tree()
            .iter()
            .filter_map(|(agent, _)| agent.project_id.clone())
            .collect();
        for project in &mut self.file.projects {
            if !shown.contains(&project.id) {
                project.removed = true;
            }
        }
    }

    /// Before version 4 a channel's coordinator was named
    /// `#<slug> coordinator`; now it is just `coordinator`. Only instances
    /// of the coordinator definition with a name of the old form are
    /// renamed, so a name the user chose stays. Codes come from the
    /// definition, not the name, so they do not change.
    pub(super) fn rename_channel_coordinators(&mut self) {
        for agent in &mut self.file.agents {
            if is_old_coordinator_name(agent) {
                COORDINATOR_DEFINITION.clone_into(&mut agent.name);
            }
        }
    }
}

/// Whether `agent` is a coordinator still named `#<slug> coordinator`.
fn is_old_coordinator_name(agent: &RegisteredAgent) -> bool {
    agent.definition.as_deref() == Some(COORDINATOR_DEFINITION)
        && agent
            .name
            .strip_prefix('#')
            .and_then(|rest| rest.strip_suffix(" coordinator"))
            .is_some_and(|slug| !slug.is_empty())
}

/// Registries written before `member_of` existed linked agents added to a
/// project to its root through `parent`. Only those agents have a parent
/// and no definition: spawned agents are always instances.
fn relink_legacy_members(agents: &mut [RegisteredAgent]) {
    for agent in agents.iter_mut() {
        if agent.definition.is_none() && agent.legacy_member_of.is_none() {
            if let Some(root) = agent.parent.take() {
                agent.legacy_member_of = Some(root);
            }
        }
    }
}

/// Position of the agent above `index`: its parent, else the agent it was
/// added to.
fn up(agents: &[RegisteredAgent], index: usize) -> Option<usize> {
    let agent = &agents[index];
    let above = agent
        .parent
        .as_deref()
        .or(agent.legacy_member_of.as_deref())?;
    agents.iter().position(|a| a.zns_id == above)
}

/// Position of the root above `index`, giving up after as many steps as
/// there are agents, in case of a cycle.
fn root_of(agents: &[RegisteredAgent], mut index: usize) -> usize {
    for _ in 0..agents.len() {
        match up(agents, index) {
            Some(above) => index = above,
            None => break,
        }
    }
    index
}

/// Positions of the agents with nothing above them, in the order of the
/// desktop app's saved tabs, then in registry order.
fn ordered_roots(agents: &[RegisteredAgent], data_dir: &Path) -> Vec<usize> {
    let mut roots: Vec<usize> = (0..agents.len())
        .filter(|&i| up(agents, i).is_none())
        .collect();
    let tabs = fs::read_to_string(data_dir.join(DESKTOP_TABS_FILE)).unwrap_or_default();
    let saved: Vec<&str> = tabs.split_whitespace().collect();
    roots.sort_by_key(|&i| {
        let hex = agents[i].agent_id().to_hex();
        saved
            .iter()
            .position(|id| *id == hex)
            .unwrap_or(saved.len())
    });
    roots
}

/// The project each root heads: the one already in its folder, else a
/// new one in `projects` with the root's hex id.
fn project_per_root(
    agents: &[RegisteredAgent],
    roots: &[usize],
    projects: &mut Vec<Project>,
    launch_dir: &Path,
) -> Vec<(usize, String)> {
    let mut project_of_root: Vec<(usize, String)> = Vec::with_capacity(roots.len());
    for &root in roots {
        let agent = &agents[root];
        let folder = agent
            .folder
            .as_deref()
            .map_or_else(|| launch_dir.to_path_buf(), canonical);
        let id = match projects.iter_mut().find(|p| p.folder == folder) {
            Some(project) => {
                project.removed &= agent.removed;
                project.id.clone()
            }
            None => {
                let id = agent.agent_id().to_hex();
                projects.push(Project {
                    id: id.clone(),
                    folder,
                    created_at: agent.created_at,
                    removed: agent.removed,
                });
                id
            }
        };
        project_of_root.push((root, id));
    }
    project_of_root
}

/// Put every agent in its root's project; roots hand their folder over.
fn assign_projects(
    agents: &mut [RegisteredAgent],
    roots: &[usize],
    project_of_root: &[(usize, String)],
) {
    let assigned: Vec<Option<String>> = (0..agents.len())
        .map(|i| {
            let root = root_of(agents, i);
            project_of_root
                .iter()
                .find(|(r, _)| *r == root)
                .map(|(_, id)| id.clone())
        })
        .collect();
    for (index, project_id) in assigned.into_iter().enumerate() {
        let agent = &mut agents[index];
        if agent.project_id.is_none() {
            agent.project_id = project_id;
        }
        if roots.contains(&index) {
            // The folder is the project's now.
            agent.folder = None;
        }
        agent.legacy_member_of = None;
    }
}

/// Where [`read_registry_file`] found the registry to import.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ImportSource {
    /// Nothing to import: the registry is seeded fresh.
    None,
    /// `agents.json`, imported for the first time.
    File,
    /// `agents.json.migrated`: an earlier import was renamed away but its
    /// batch is not in the store, as after a power loss.
    Migrated,
}

/// The registry to import into an empty store: `<data_dir>/agents.json`,
/// or, when only `agents.json.migrated` is left, that file again, since an
/// empty store next to it means the store lost the earlier import. Empty
/// when there is neither.
pub(super) fn read_registry_file(
    data_dir: &Path,
) -> Result<(RegistryFile, ImportSource), AgentRegistryError> {
    let path = data_dir.join(REGISTRY_FILE);
    let migrated = data_dir.join(MIGRATED_REGISTRY_FILE);
    let (path, source) = if path.exists() {
        (path, ImportSource::File)
    } else if migrated.exists() {
        tracing::warn!(
            path = %migrated.display(),
            "the agent store is empty but agents.json was imported before; \
             the store lost the import, importing the renamed file again"
        );
        (migrated, ImportSource::Migrated)
    } else {
        return Ok((RegistryFile::default(), ImportSource::None));
    };
    let body = fs::read_to_string(&path).map_err(|source| AgentRegistryError::ReadFile {
        path: path.clone(),
        source,
    })?;
    let file = serde_json::from_str(&body)
        .map_err(|source| AgentRegistryError::ParseFile { path, source })?;
    Ok((file, source))
}

/// Rename `<data_dir>/agents.json` to `agents.json.migrated` once its
/// contents are durably in the store, so it is never imported again and
/// the original stays on disk, then fsync the directory so the rename
/// itself is durable. A failed rename or sync is only logged: the store
/// is not empty any more, so the file is not read again either way.
pub(super) fn retire_registry_file(data_dir: &Path) {
    let path = data_dir.join(REGISTRY_FILE);
    if !path.exists() {
        return;
    }
    let target = data_dir.join(MIGRATED_REGISTRY_FILE);
    if let Err(error) = fs::rename(&path, &target) {
        tracing::warn!(
            from = %path.display(),
            to = %target.display(),
            %error,
            "could not rename the imported agent registry"
        );
        return;
    }
    tracing::info!(to = %target.display(), "imported agents.json into the store");
    sync_dir(data_dir);
}

/// Fsync directory `dir` so a rename in it survives a power loss. Best
/// effort: a failure is logged, as the data the rename guards is already
/// durable in the store.
#[cfg(unix)]
fn sync_dir(dir: &Path) {
    if let Err(error) = fs::File::open(dir).and_then(|dir| dir.sync_all()) {
        tracing::warn!(dir = %dir.display(), %error, "could not sync the data directory");
    }
}

/// `std::fs::File` cannot open a directory for syncing here, so the rename
/// is left to the file system.
#[cfg(not(unix))]
fn sync_dir(_dir: &Path) {}

pub(super) fn legacy_or_new_zns_id(data_dir: &Path) -> Result<String, AgentRegistryError> {
    let identity_file = data_dir.join(LEGACY_IDENTITY_FILE);
    if identity_file.exists() {
        return fs::read_to_string(&identity_file).map_err(|source| AgentRegistryError::ReadFile {
            path: identity_file,
            source,
        });
    }
    let zns_id = format!("0://terminal/{}", uuid::Uuid::new_v4());
    fs::write(&identity_file, &zns_id).map_err(|source| AgentRegistryError::WriteFile {
        path: identity_file,
        source,
    })?;
    Ok(zns_id)
}
