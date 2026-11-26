//! Creating agents and projects: plain agents, children, compare runs,
//! instances of definitions and the headless agent.

use aura_core_types::AgentId;
use chrono::Utc;
use std::path::{Path, PathBuf};

use super::{
    code, AgentRegistry, AgentRegistryError, IdCache, Opened, Project, RegisteredAgent,
    COMPARE_ARM_ZNS_PREFIX, COMPARE_ZNS_PREFIX, DEFAULT_PROJECT_AGENT, HEADLESS_AGENT_NAME,
    HEADLESS_ZNS_ID,
};

impl AgentRegistry {
    /// Id of the project for the launch directory, made (or put back on
    /// the list) when needed. Not saved.
    pub(super) fn launch_project(&mut self) -> String {
        let folder = self.launch_dir.clone();
        self.project_for(&folder)
    }

    /// Id of the project for the canonical folder `folder`, made (or put
    /// back on the list) when needed. Not saved.
    fn project_for(&mut self, folder: &Path) -> String {
        if let Some(project) = self.file.projects.iter_mut().find(|p| p.folder == folder) {
            project.removed = false;
            return project.id.clone();
        }
        let id = uuid::Uuid::new_v4().simple().to_string();
        self.file.projects.push(Project {
            id: id.clone(),
            folder: folder.to_path_buf(),
            created_at: Utc::now(),
            removed: false,
        });
        id
    }

    /// Id of the project the agent with ZNS id `zns_id` is in.
    fn project_id_of_zns(&self, zns_id: &str) -> Option<String> {
        self.file
            .agents
            .iter()
            .find(|a| a.zns_id == zns_id)
            .and_then(|a| a.project_id.clone())
    }

    /// Register a new agent in the launch directory's project. Blank names
    /// fall back to `Agent N`.
    ///
    /// # Errors
    /// Returns an error if the registry cannot be written.
    pub fn create(&mut self, name: Option<String>) -> Result<RegisteredAgent, AgentRegistryError> {
        let name = name
            .map(|n| n.trim().to_string())
            .filter(|n| !n.is_empty())
            .unwrap_or_else(|| format!("Agent {}", self.file.agents.len() + 1));
        let project_id = self.launch_project();
        self.push(RegisteredAgent {
            project_id: Some(project_id),
            ..Self::new_agent(name)
        })
    }

    /// Open `folder` as a project. A folder is one project: when it is
    /// listed already, that project's first listed agent is returned and
    /// nothing changes. Otherwise the project is made, or put back on the
    /// list with its archived agents left in the Archive, and gets a new
    /// agent, which is returned.
    ///
    /// # Errors
    /// Returns an error if `folder` is not a directory or the registry
    /// cannot be written.
    pub fn open_project(
        &mut self,
        folder: &Path,
    ) -> Result<(RegisteredAgent, Opened), AgentRegistryError> {
        let folder = folder
            .canonicalize()
            .map_err(|source| AgentRegistryError::OpenFolder {
                folder: folder.to_path_buf(),
                source,
            })?;
        if !folder.is_dir() {
            return Err(AgentRegistryError::NotAFolder { folder });
        }
        let known = self
            .file
            .projects
            .iter()
            .find(|p| p.folder == folder)
            .map(|p| p.id.clone());
        let Some(id) = known else {
            let id = self.project_for(&folder);
            let agent = self.push(RegisteredAgent {
                project_id: Some(id),
                ..Self::new_agent(DEFAULT_PROJECT_AGENT.to_string())
            })?;
            return Ok((agent, Opened::Created));
        };
        let listed = self
            .tree()
            .into_iter()
            .find(|(agent, _)| agent.project_id.as_deref() == Some(id.as_str()))
            .map(|(agent, _)| agent.clone());
        if let Some(agent) = listed {
            return Ok((agent, Opened::Existing));
        }
        let agent = self.create_in_project(&id)?;
        Ok((agent, Opened::Revived))
    }

    /// Register a new agent in project `project_id`, named `Agent N` after
    /// how many agents the project already has.
    ///
    /// # Errors
    /// Returns an error if the project is unknown or the registry cannot be
    /// written.
    pub fn create_in_project(
        &mut self,
        project_id: &str,
    ) -> Result<RegisteredAgent, AgentRegistryError> {
        let Some(project) = self.file.projects.iter_mut().find(|p| p.id == project_id) else {
            return Err(AgentRegistryError::NoSuchProject {
                project_id: project_id.to_string(),
            });
        };
        project.removed = false;
        let count = self
            .file
            .agents
            .iter()
            .filter(|a| a.project_id.as_deref() == Some(project_id))
            .count();
        let name = if count == 0 {
            DEFAULT_PROJECT_AGENT.to_string()
        } else {
            format!("{DEFAULT_PROJECT_AGENT} {}", count + 1)
        };
        self.push(RegisteredAgent {
            project_id: Some(project_id.to_string()),
            ..Self::new_agent(name)
        })
    }

    /// Register an agent named `name` under the agent with ZNS id
    /// `parent`, in its project, working in `folder` when given.
    ///
    /// # Errors
    /// Returns an error if the registry cannot be written.
    pub fn create_child(
        &mut self,
        name: &str,
        parent: &str,
        folder: Option<PathBuf>,
    ) -> Result<RegisteredAgent, AgentRegistryError> {
        self.push(RegisteredAgent {
            parent: Some(parent.to_string()),
            project_id: self.project_id_of_zns(parent),
            folder,
            ..Self::new_agent(name.to_string())
        })
    }

    /// Register the agent that holds a new `/compare` asked for by the
    /// agent with ZNS id `parent`, named `Compare #<n>`.
    ///
    /// # Errors
    /// Returns an error if the registry cannot be written.
    pub fn create_compare(&mut self, parent: &str) -> Result<RegisteredAgent, AgentRegistryError> {
        let n = self.compares().len() + 1;
        self.push(RegisteredAgent {
            zns_id: format!("{COMPARE_ZNS_PREFIX}{}", uuid::Uuid::new_v4()),
            parent: Some(parent.to_string()),
            project_id: self.project_id_of_zns(parent),
            ..Self::new_agent(format!("Compare #{n}"))
        })
    }

    /// Register the agent named `name` that groups one arm's runs of the
    /// comparison with ZNS id `compare`.
    ///
    /// # Errors
    /// Returns an error if the registry cannot be written.
    pub fn create_compare_arm(
        &mut self,
        name: &str,
        compare: &str,
    ) -> Result<RegisteredAgent, AgentRegistryError> {
        self.push(RegisteredAgent {
            zns_id: format!("{COMPARE_ARM_ZNS_PREFIX}{}", uuid::Uuid::new_v4()),
            parent: Some(compare.to_string()),
            project_id: self.project_id_of_zns(compare),
            ..Self::new_agent(name.to_string())
        })
    }

    /// [`Self::is_top_level`] for the agent `agent_id`; one that isn't
    /// registered, like a headless run's, is top level too.
    #[must_use]
    pub fn is_top_level_agent(&self, agent_id: AgentId) -> bool {
        self.find(agent_id)
            .is_none_or(|agent| self.is_top_level(agent))
    }

    /// Register an instance of the permanent agent `definition`, named
    /// `<definition> #<n>`, spawned by the agent with ZNS id `parent` (in
    /// its project), or else in the launch directory's project.
    ///
    /// # Errors
    /// Returns an error if the registry cannot be written.
    pub fn create_instance(
        &mut self,
        definition: &str,
        parent: Option<String>,
    ) -> Result<RegisteredAgent, AgentRegistryError> {
        let project_id = match parent.as_deref() {
            Some(parent) => self.project_id_of_zns(parent),
            None => Some(self.launch_project()),
        };
        self.push(RegisteredAgent {
            parent,
            project_id,
            ..self.new_instance(definition)
        })
    }

    /// Register an instance of the permanent agent `definition` at the top
    /// of project `project_id`.
    ///
    /// # Errors
    /// Returns an error if the project is unknown or the registry cannot be
    /// written.
    pub fn add_instance_to_project(
        &mut self,
        definition: &str,
        project_id: &str,
    ) -> Result<RegisteredAgent, AgentRegistryError> {
        let Some(project) = self.file.projects.iter_mut().find(|p| p.id == project_id) else {
            return Err(AgentRegistryError::NoSuchProject {
                project_id: project_id.to_string(),
            });
        };
        project.removed = false;
        self.push(RegisteredAgent {
            project_id: Some(project_id.to_string()),
            ..self.new_instance(definition)
        })
    }

    /// A fresh instance of `definition`, named `<definition> #<n>`, not yet
    /// registered. `n` is one past both the highest number any instance of
    /// `definition` has in its name and the highest number its code prefix
    /// was ever given, so neither a name nor a code is reused after an
    /// instance is dropped, even the highest-numbered one, and [`code::assign`]
    /// gives the instance the code with the same number.
    pub(super) fn new_instance(&self, definition: &str) -> RegisteredAgent {
        let highest_named = self
            .file
            .agents
            .iter()
            .filter(|a| a.definition.as_deref() == Some(definition))
            .filter_map(|a| code::number_in_name(&a.name, definition))
            .max()
            .unwrap_or(0);
        let highest_coded = self
            .file
            .prefixes
            .get(definition)
            .map_or(0, |prefix| self.file.last_numbers.last(prefix));
        let number = highest_named.max(highest_coded).saturating_add(1);
        RegisteredAgent {
            definition: Some(definition.to_string()),
            ..Self::new_agent(format!("{definition} #{number}"))
        }
    }

    /// The agent `aura -p` runs as, registered on first use in the launch
    /// directory's project. It is an agent of its own, so a headless run
    /// never replaces the current session of an agent the terminal shows.
    ///
    /// # Errors
    /// Returns an error if the registry cannot be written.
    pub fn headless(&mut self) -> Result<RegisteredAgent, AgentRegistryError> {
        if let Some(agent) = self
            .file
            .agents
            .iter()
            .find(|a| a.zns_id == HEADLESS_ZNS_ID)
        {
            return Ok(agent.clone());
        }
        let project_id = self.launch_project();
        self.push(RegisteredAgent {
            zns_id: HEADLESS_ZNS_ID.to_string(),
            project_id: Some(project_id),
            ..Self::new_agent(HEADLESS_AGENT_NAME.to_string())
        })
    }

    /// A fresh agent named `name`, not yet registered.
    pub(super) fn new_agent(name: String) -> RegisteredAgent {
        RegisteredAgent {
            zns_id: format!("0://terminal/{}", uuid::Uuid::new_v4()),
            name,
            created_at: Utc::now(),
            definition: None,
            parent: None,
            project_id: None,
            folder: None,
            archived: false,
            archived_at: None,
            removed: false,
            title: None,
            code: None,
            subtitle: None,
            legacy_member_of: None,
            id: IdCache::default(),
        }
    }

    /// Register `agent`, giving it its code, and save.
    pub(super) fn push(
        &mut self,
        mut agent: RegisteredAgent,
    ) -> Result<RegisteredAgent, AgentRegistryError> {
        if agent.code.is_none() {
            agent.code = Some(code::assign(
                &agent,
                &self.file.agents,
                &mut self.file.prefixes,
                &mut self.file.last_numbers,
            ));
        }
        self.file.agents.push(agent.clone());
        self.persist()?;
        Ok(agent)
    }
}

/// `folder` made canonical when it exists, else as given.
pub(super) fn canonical(folder: &Path) -> PathBuf {
    folder
        .canonicalize()
        .unwrap_or_else(|_| folder.to_path_buf())
}
