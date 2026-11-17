//! Persistent list of the agents the terminal UI manages.
//!
//! Stored as `<data_dir>/agents.json`. The legacy single-agent
//! `agent_identity.txt` is folded in as the first entry so existing
//! records stay attached to the agent that wrote them.

use anyhow::Context;
use aura_core_types::{AgentId, Identity};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

const REGISTRY_FILE: &str = "agents.json";
const LEGACY_IDENTITY_FILE: &str = "agent_identity.txt";
const DEFAULT_AGENT_NAME: &str = "Terminal Agent";
/// Name of the agent created with a new project.
const DEFAULT_PROJECT_AGENT: &str = "Agent";
/// ZNS id of the agent `aura -p` runs as. Fixed, so every headless run in
/// a data directory finds the same agent and its saved sessions.
const HEADLESS_ZNS_ID: &str = "0://terminal/headless";
const HEADLESS_AGENT_NAME: &str = "Headless";
/// ZNS id prefix of the agents that hold a `/compare` and its runs.
const COMPARE_ZNS_PREFIX: &str = "0://terminal/compare/";

/// One agent known to the terminal UI.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RegisteredAgent {
    pub zns_id: String,
    pub name: String,
    pub created_at: DateTime<Utc>,
    /// Permanent agent this is an instance of.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub definition: Option<String>,
    /// ZNS id of the agent that spawned this one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent: Option<String>,
    /// Name of the project this root agent heads. Absent on older registries
    /// and on agents spawned under another agent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
    /// Folder this root agent's project works in. Absent on projects
    /// created before folders were picked; they use the launch directory.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub folder: Option<PathBuf>,
}

impl RegisteredAgent {
    #[must_use]
    pub fn identity(&self) -> Identity {
        Identity::new(&self.zns_id, &self.name)
    }

    #[must_use]
    pub fn agent_id(&self) -> AgentId {
        self.identity().agent_id
    }

    /// Whether this agent holds a `/compare` (its record and runs).
    #[must_use]
    pub fn is_compare(&self) -> bool {
        self.zns_id.starts_with(COMPARE_ZNS_PREFIX)
    }
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct RegistryFile {
    /// ZNS id of the agent that was active when the TUI last switched.
    #[serde(default)]
    active: Option<String>,
    #[serde(default)]
    agents: Vec<RegisteredAgent>,
}

/// File-backed agent registry.
#[derive(Debug)]
pub struct AgentRegistry {
    path: PathBuf,
    file: RegistryFile,
}

impl AgentRegistry {
    /// Load `<data_dir>/agents.json`, seeding it from the legacy identity
    /// file (or a fresh identity) when it does not exist yet.
    ///
    /// # Errors
    /// Returns an error if the registry or identity file cannot be read,
    /// parsed, or written.
    pub fn load_or_init(data_dir: &Path) -> anyhow::Result<Self> {
        let path = data_dir.join(REGISTRY_FILE);
        let file = if path.exists() {
            let body =
                fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
            serde_json::from_str(&body).with_context(|| format!("parsing {}", path.display()))?
        } else {
            RegistryFile::default()
        };

        let mut registry = Self { path, file };
        if registry.file.agents.is_empty() {
            let zns_id = legacy_or_new_zns_id(data_dir)?;
            registry.file.agents.push(RegisteredAgent {
                zns_id,
                name: DEFAULT_AGENT_NAME.to_string(),
                created_at: Utc::now(),
                definition: None,
                parent: None,
                project: None,
                folder: None,
            });
            registry.save()?;
        }
        Ok(registry)
    }

    #[must_use]
    pub fn agents(&self) -> &[RegisteredAgent] {
        &self.file.agents
    }

    #[must_use]
    pub fn find(&self, agent_id: AgentId) -> Option<&RegisteredAgent> {
        self.file.agents.iter().find(|a| a.agent_id() == agent_id)
    }

    /// The agent to open on startup: the last active one, else the first.
    #[must_use]
    pub fn active(&self) -> &RegisteredAgent {
        self.file
            .active
            .as_deref()
            .and_then(|zns| self.file.agents.iter().find(|a| a.zns_id == zns))
            .unwrap_or(&self.file.agents[0])
    }

    /// Remember `agent_id` as the active agent.
    ///
    /// # Errors
    /// Returns an error if the registry cannot be written.
    pub fn set_active(&mut self, agent_id: AgentId) -> anyhow::Result<()> {
        if let Some(agent) = self.find(agent_id) {
            self.file.active = Some(agent.zns_id.clone());
            self.save()?;
        }
        Ok(())
    }

    /// Register a new agent. Blank names fall back to `Agent N`.
    ///
    /// # Errors
    /// Returns an error if the registry cannot be written.
    pub fn create(&mut self, name: Option<String>) -> anyhow::Result<RegisteredAgent> {
        let name = name
            .map(|n| n.trim().to_string())
            .filter(|n| !n.is_empty())
            .unwrap_or_else(|| format!("Agent {}", self.file.agents.len() + 1));
        self.push(Self::new_agent(name))
    }

    /// Register a project: a root agent named [`DEFAULT_PROJECT_AGENT`]
    /// under the project name `name`, working in `folder`.
    ///
    /// # Errors
    /// Returns an error if `name` is blank, `folder` is not a directory, or
    /// the registry cannot be written.
    pub fn create_project(&mut self, name: &str, folder: &Path) -> anyhow::Result<RegisteredAgent> {
        let name = name.trim();
        if name.is_empty() {
            anyhow::bail!("project name cannot be empty");
        }
        let folder = folder
            .canonicalize()
            .with_context(|| format!("opening {}", folder.display()))?;
        if !folder.is_dir() {
            anyhow::bail!("{} is not a folder", folder.display());
        }
        self.push(RegisteredAgent {
            project: Some(name.to_string()),
            folder: Some(folder),
            ..Self::new_agent(DEFAULT_PROJECT_AGENT.to_string())
        })
    }

    /// Folder `agent_id` works in: its own, else the nearest agent's
    /// above it that has one (normally its project's root).
    #[must_use]
    pub fn project_folder(&self, agent_id: AgentId) -> Option<PathBuf> {
        let mut index = self
            .file
            .agents
            .iter()
            .position(|agent| agent.agent_id() == agent_id)?;
        for _ in 0..self.file.agents.len() {
            let agent = &self.file.agents[index];
            if let Some(folder) = &agent.folder {
                return Some(folder.clone());
            }
            index = self.parent_index(index)?;
        }
        None
    }

    fn parent_index(&self, index: usize) -> Option<usize> {
        let parent = self.file.agents[index].parent.as_deref()?;
        self.file
            .agents
            .iter()
            .position(|agent| agent.zns_id == parent)
    }

    /// Every agent below `agent_id`: those it spawned, theirs, and so on.
    #[must_use]
    pub fn descendants(&self, agent_id: AgentId) -> Vec<AgentId> {
        let Some(agent) = self.find(agent_id) else {
            return Vec::new();
        };
        let mut found: Vec<&str> = vec![agent.zns_id.as_str()];
        let mut out = Vec::new();
        let mut i = 0;
        while i < found.len() {
            let parent = found[i];
            for child in self
                .file
                .agents
                .iter()
                .filter(|a| a.parent.as_deref() == Some(parent))
            {
                if !found.contains(&child.zns_id.as_str()) {
                    found.push(&child.zns_id);
                    out.push(child.agent_id());
                }
            }
            i += 1;
        }
        out
    }

    /// `agent_id` and the agents above it, nearest first.
    #[must_use]
    pub fn ancestry(&self, agent_id: AgentId) -> Vec<AgentId> {
        let Some(mut index) = self
            .file
            .agents
            .iter()
            .position(|agent| agent.agent_id() == agent_id)
        else {
            return Vec::new();
        };
        let mut out = vec![agent_id];
        for _ in 0..self.file.agents.len() {
            let Some(parent) = self.parent_index(index) else {
                break;
            };
            index = parent;
            out.push(self.file.agents[index].agent_id());
        }
        out
    }

    /// Register an agent named `name` under the agent with ZNS id
    /// `parent`, working in `folder` when given.
    ///
    /// # Errors
    /// Returns an error if the registry cannot be written.
    pub fn create_child(
        &mut self,
        name: &str,
        parent: &str,
        folder: Option<PathBuf>,
    ) -> anyhow::Result<RegisteredAgent> {
        self.push(RegisteredAgent {
            parent: Some(parent.to_string()),
            folder,
            ..Self::new_agent(name.to_string())
        })
    }

    /// Register the agent that holds a new `/compare` asked for by the
    /// agent with ZNS id `parent`, named `Compare #<n>`.
    ///
    /// # Errors
    /// Returns an error if the registry cannot be written.
    pub fn create_compare(&mut self, parent: &str) -> anyhow::Result<RegisteredAgent> {
        let n = self.compares().len() + 1;
        self.push(RegisteredAgent {
            zns_id: format!("{COMPARE_ZNS_PREFIX}{}", uuid::Uuid::new_v4()),
            parent: Some(parent.to_string()),
            ..Self::new_agent(format!("Compare #{n}"))
        })
    }

    /// The agents holding comparisons, oldest first.
    #[must_use]
    pub fn compares(&self) -> Vec<&RegisteredAgent> {
        self.file.agents.iter().filter(|a| a.is_compare()).collect()
    }

    /// Root agent heading the project that contains `agent_id`.
    #[must_use]
    pub fn project_root(&self, agent_id: AgentId) -> Option<&RegisteredAgent> {
        let index = self
            .file
            .agents
            .iter()
            .position(|agent| agent.agent_id() == agent_id)?;
        Some(&self.file.agents[self.root_of(index)])
    }

    /// Set the project name on the root agent that contains `agent_id`.
    ///
    /// # Errors
    /// Returns an error if the name is blank, the agent is unknown, or the
    /// registry cannot be written.
    pub fn rename_project(&mut self, agent_id: AgentId, name: &str) -> anyhow::Result<()> {
        let name = name.trim();
        if name.is_empty() {
            anyhow::bail!("project name cannot be empty");
        }
        let Some(start) = self
            .file
            .agents
            .iter()
            .position(|agent| agent.agent_id() == agent_id)
        else {
            anyhow::bail!("no such agent");
        };
        let root = self.root_of(start);
        self.file.agents[root].project = Some(name.to_string());
        self.save()
    }

    /// Walk `parent` links to the agent that heads a project.
    fn root_of(&self, mut index: usize) -> usize {
        for _ in 0..self.file.agents.len() {
            let Some(parent) = self.file.agents[index].parent.as_deref() else {
                break;
            };
            let Some(parent_index) = self
                .file
                .agents
                .iter()
                .position(|agent| agent.zns_id == parent)
            else {
                break;
            };
            index = parent_index;
        }
        index
    }

    /// Register an instance of the permanent agent `definition`, named
    /// `<definition> #<n>`, optionally spawned by the agent with ZNS id
    /// `parent`.
    ///
    /// # Errors
    /// Returns an error if the registry cannot be written.
    pub fn create_instance(
        &mut self,
        definition: &str,
        parent: Option<String>,
    ) -> anyhow::Result<RegisteredAgent> {
        let count = self
            .file
            .agents
            .iter()
            .filter(|a| a.definition.as_deref() == Some(definition))
            .count();
        let name = format!("{definition} #{}", count + 1);
        self.push(RegisteredAgent {
            definition: Some(definition.to_string()),
            parent,
            ..Self::new_agent(name)
        })
    }

    /// The agent `aura -p` runs as, registered on first use. It is a root
    /// agent of its own, so a headless run never replaces the current
    /// session of an agent the terminal shows.
    ///
    /// # Errors
    /// Returns an error if the registry cannot be written.
    pub fn headless(&mut self) -> anyhow::Result<RegisteredAgent> {
        if let Some(agent) = self
            .file
            .agents
            .iter()
            .find(|a| a.zns_id == HEADLESS_ZNS_ID)
        {
            return Ok(agent.clone());
        }
        self.push(RegisteredAgent {
            zns_id: HEADLESS_ZNS_ID.to_string(),
            ..Self::new_agent(HEADLESS_AGENT_NAME.to_string())
        })
    }

    /// A fresh agent named `name`, not yet registered.
    fn new_agent(name: String) -> RegisteredAgent {
        RegisteredAgent {
            zns_id: format!("0://terminal/{}", uuid::Uuid::new_v4()),
            name,
            created_at: Utc::now(),
            definition: None,
            parent: None,
            project: None,
            folder: None,
        }
    }

    fn push(&mut self, agent: RegisteredAgent) -> anyhow::Result<RegisteredAgent> {
        self.file.agents.push(agent.clone());
        self.save()?;
        Ok(agent)
    }

    /// Agents in display order: each agent is followed by the agents it
    /// spawned, paired with how deeply it is nested.
    #[must_use]
    pub fn tree(&self) -> Vec<(&RegisteredAgent, usize)> {
        fn visit<'a>(
            agents: &'a [RegisteredAgent],
            agent: &'a RegisteredAgent,
            depth: usize,
            out: &mut Vec<(&'a RegisteredAgent, usize)>,
        ) {
            out.push((agent, depth));
            for child in agents
                .iter()
                .filter(|a| a.parent.as_deref() == Some(agent.zns_id.as_str()))
            {
                visit(agents, child, depth + 1, out);
            }
        }
        let agents = &self.file.agents;
        let mut out = Vec::with_capacity(agents.len());
        for root in agents.iter().filter(|a| {
            a.parent
                .as_deref()
                .is_none_or(|parent| !agents.iter().any(|p| p.zns_id == parent))
        }) {
            visit(agents, root, 0, &mut out);
        }
        out
    }

    fn save(&self) -> anyhow::Result<()> {
        let body =
            serde_json::to_string_pretty(&self.file).context("serialising agent registry")?;
        let tmp = self.path.with_extension("json.tmp");
        fs::write(&tmp, body).with_context(|| format!("writing {}", tmp.display()))?;
        if self.path.exists() {
            // Windows fs::rename refuses to overwrite; remove first.
            let _ = fs::remove_file(&self.path);
        }
        fs::rename(&tmp, &self.path)
            .with_context(|| format!("renaming {} -> {}", tmp.display(), self.path.display()))?;
        Ok(())
    }
}

fn legacy_or_new_zns_id(data_dir: &Path) -> anyhow::Result<String> {
    let identity_file = data_dir.join(LEGACY_IDENTITY_FILE);
    if identity_file.exists() {
        let zns_id = fs::read_to_string(&identity_file)
            .with_context(|| format!("reading {}", identity_file.display()))?;
        return Ok(zns_id);
    }
    let zns_id = format!("0://terminal/{}", uuid::Uuid::new_v4());
    fs::write(&identity_file, &zns_id)
        .with_context(|| format!("writing {}", identity_file.display()))?;
    Ok(zns_id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seeds_from_legacy_identity_file() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join(LEGACY_IDENTITY_FILE), "0://terminal/legacy").unwrap();

        let registry = AgentRegistry::load_or_init(dir.path()).unwrap();

        assert_eq!(registry.agents().len(), 1);
        assert_eq!(registry.active().zns_id, "0://terminal/legacy");
        assert_eq!(registry.active().name, DEFAULT_AGENT_NAME);
        assert_eq!(
            registry.active().agent_id(),
            Identity::new("0://terminal/legacy", "x").agent_id
        );
        assert!(dir.path().join(REGISTRY_FILE).exists());
    }

    #[test]
    fn create_and_set_active_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let mut registry = AgentRegistry::load_or_init(dir.path()).unwrap();
        let first = registry.active().clone();

        let named = registry.create(Some("  Researcher ".to_string())).unwrap();
        let unnamed = registry.create(None).unwrap();
        assert_eq!(named.name, "Researcher");
        assert_eq!(unnamed.name, "Agent 3");
        registry.set_active(named.agent_id()).unwrap();

        let reloaded = AgentRegistry::load_or_init(dir.path()).unwrap();
        assert_eq!(reloaded.agents(), &[first, named.clone(), unnamed]);
        assert_eq!(reloaded.active(), &named);
    }

    #[test]
    fn headless_agent_is_registered_once_and_never_made_active() {
        let dir = tempfile::tempdir().unwrap();
        let mut registry = AgentRegistry::load_or_init(dir.path()).unwrap();
        let active = registry.active().clone();

        let headless = registry.headless().unwrap();
        assert_eq!(headless.name, HEADLESS_AGENT_NAME);
        assert_eq!(registry.headless().unwrap(), headless);

        let reloaded = AgentRegistry::load_or_init(dir.path()).unwrap();
        assert_eq!(reloaded.agents().len(), 2);
        assert_eq!(reloaded.active(), &active);
    }

    #[test]
    fn loads_registry_written_before_definitions_existed() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join(REGISTRY_FILE),
            r#"{"agents":[{"zns_id":"0://terminal/a","name":"A","created_at":"2026-10-31T01:48:50Z"}]}"#,
        )
        .unwrap();

        let registry = AgentRegistry::load_or_init(dir.path()).unwrap();
        assert_eq!(registry.agents()[0].definition, None);
        assert_eq!(registry.agents()[0].parent, None);
    }

    #[test]
    fn instances_are_numbered_per_definition_and_nested_under_parents() {
        let dir = tempfile::tempdir().unwrap();
        let mut registry = AgentRegistry::load_or_init(dir.path()).unwrap();
        let root = registry.active().clone();
        let other = registry.create(Some("Other".to_string())).unwrap();
        let child = registry
            .create_instance("reviewer", Some(root.zns_id.clone()))
            .unwrap();
        let grandchild = registry
            .create_instance("explorer", Some(child.zns_id.clone()))
            .unwrap();
        let second = registry.create_instance("reviewer", None).unwrap();
        assert_eq!(child.name, "reviewer #1");
        assert_eq!(second.name, "reviewer #2");
        assert_eq!(child.definition.as_deref(), Some("reviewer"));

        let reloaded = AgentRegistry::load_or_init(dir.path()).unwrap();
        let tree: Vec<(&str, usize)> = reloaded
            .tree()
            .into_iter()
            .map(|(a, depth)| (a.name.as_str(), depth))
            .collect();
        assert_eq!(
            tree,
            [
                (root.name.as_str(), 0),
                (child.name.as_str(), 1),
                (grandchild.name.as_str(), 2),
                (other.name.as_str(), 0),
                (second.name.as_str(), 0),
            ]
        );
    }

    #[test]
    fn create_project_names_it_and_adds_a_default_agent() {
        let dir = tempfile::tempdir().unwrap();
        let folder = tempfile::tempdir().unwrap();
        let mut registry = AgentRegistry::load_or_init(dir.path()).unwrap();
        let created = registry
            .create_project("  Website  ", folder.path())
            .unwrap();
        assert_eq!(created.name, "Agent");
        assert_eq!(created.project.as_deref(), Some("Website"));
        assert!(created.parent.is_none());
        let canonical = folder.path().canonicalize().unwrap();
        assert_eq!(created.folder.as_deref(), Some(canonical.as_path()));

        let reloaded = AgentRegistry::load_or_init(dir.path()).unwrap();
        let stored = reloaded
            .agents()
            .iter()
            .find(|agent| agent.zns_id == created.zns_id)
            .unwrap();
        assert_eq!(stored.project.as_deref(), Some("Website"));
        assert_eq!(stored.folder.as_deref(), Some(canonical.as_path()));
        assert!(registry.create_project("   ", folder.path()).is_err());
    }

    #[test]
    fn create_project_rejects_a_missing_folder_or_a_file() {
        let dir = tempfile::tempdir().unwrap();
        let mut registry = AgentRegistry::load_or_init(dir.path()).unwrap();
        let before = registry.agents().len();
        assert!(registry
            .create_project("Gone", &dir.path().join("missing"))
            .is_err());
        let file = dir.path().join("notes.txt");
        fs::write(&file, "").unwrap();
        assert!(registry.create_project("File", &file).is_err());
        assert_eq!(registry.agents().len(), before);
    }

    #[test]
    fn spawned_agents_use_their_projects_folder() {
        let dir = tempfile::tempdir().unwrap();
        let folder = tempfile::tempdir().unwrap();
        let mut registry = AgentRegistry::load_or_init(dir.path()).unwrap();
        let legacy = registry.active().clone();
        let root = registry.create_project("Site", folder.path()).unwrap();
        let child = registry
            .create_instance("reviewer", Some(root.zns_id.clone()))
            .unwrap();
        let grandchild = registry
            .create_instance("explorer", Some(child.zns_id.clone()))
            .unwrap();

        let canonical = folder.path().canonicalize().unwrap();
        assert_eq!(
            registry.project_folder(grandchild.agent_id()),
            Some(canonical)
        );
        assert_eq!(registry.project_root(grandchild.agent_id()), Some(&root));
        assert!(child.folder.is_none());
        assert_eq!(registry.project_folder(legacy.agent_id()), None);
    }

    #[test]
    fn compare_runs_work_in_their_own_folder_under_the_comparison() {
        let dir = tempfile::tempdir().unwrap();
        let project = tempfile::tempdir().unwrap();
        let copy = tempfile::tempdir().unwrap();
        let mut registry = AgentRegistry::load_or_init(dir.path()).unwrap();
        let root = registry.create_project("Site", project.path()).unwrap();
        let compare = registry.create_compare(&root.zns_id).unwrap();
        let second = registry.create_compare(&root.zns_id).unwrap();
        let run = registry
            .create_child("gate-1", &compare.zns_id, Some(copy.path().to_path_buf()))
            .unwrap();
        let subtask = registry
            .create_instance("explorer", Some(run.zns_id.clone()))
            .unwrap();

        assert_eq!(compare.name, "Compare #1");
        assert_eq!(second.name, "Compare #2");
        assert!(compare.is_compare() && !run.is_compare());
        let canonical = project.path().canonicalize().unwrap();
        assert_eq!(registry.project_folder(compare.agent_id()), Some(canonical));
        assert_eq!(
            registry.project_folder(subtask.agent_id()).as_deref(),
            Some(copy.path()),
            "a run's subtasks work in its copy"
        );
        assert_eq!(
            registry.descendants(compare.agent_id()),
            [run.agent_id(), subtask.agent_id()]
        );
        assert_eq!(
            registry.ancestry(subtask.agent_id()),
            [
                subtask.agent_id(),
                run.agent_id(),
                compare.agent_id(),
                root.agent_id()
            ]
        );

        let reloaded = AgentRegistry::load_or_init(dir.path()).unwrap();
        let names: Vec<&str> = reloaded
            .compares()
            .iter()
            .map(|a| a.name.as_str())
            .collect();
        assert_eq!(names, ["Compare #1", "Compare #2"]);
    }

    #[test]
    fn rename_project_updates_the_root_even_from_a_child() {
        let dir = tempfile::tempdir().unwrap();
        let mut registry = AgentRegistry::load_or_init(dir.path()).unwrap();
        let root = registry.active().clone();
        let child = registry
            .create_instance("reviewer", Some(root.zns_id.clone()))
            .unwrap();
        registry
            .rename_project(child.agent_id(), "Renamed")
            .unwrap();
        let reloaded = AgentRegistry::load_or_init(dir.path()).unwrap();
        assert_eq!(
            reloaded
                .agents()
                .iter()
                .find(|agent| agent.zns_id == root.zns_id)
                .unwrap()
                .project
                .as_deref(),
            Some("Renamed")
        );
        assert!(reloaded
            .agents()
            .iter()
            .find(|agent| agent.zns_id == child.zns_id)
            .unwrap()
            .project
            .is_none());
    }
}
