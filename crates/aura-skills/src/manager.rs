//! High-level skill manager — façade over loader, registry, activation, and prompt injection.

use crate::activation;
use crate::error::SkillError;
use crate::install::{SkillInstallStore, SkillInstallStoreApi, SkillInstallation};
use crate::loader::SkillLoader;
use crate::parser::validate_name;
use crate::prompt;
use crate::registry::SkillRegistry;
use crate::types::{Skill, SkillActivation, SkillMeta};
use aura_core::AgentId;
use chrono::Utc;
use std::sync::Arc;
use tracing::info;

/// Top-level entry point for the skill system.
///
/// Owns a [`SkillLoader`] and [`SkillRegistry`], and exposes methods for
/// listing, activating, and injecting skills into agent prompts.
/// Optionally holds a [`SkillInstallStore`] for per-agent installation tracking.
pub struct SkillManager {
    registry: SkillRegistry,
    loader: SkillLoader,
    /// Optional RocksDB-backed per-agent installation store.
    install_store: Option<Arc<SkillInstallStore>>,
}

impl SkillManager {
    /// Create a new manager and immediately load all discoverable skills.
    #[must_use]
    pub fn new(loader: SkillLoader) -> Self {
        let mut registry = SkillRegistry::new();
        registry.reload(&loader);
        info!("skill manager initialized with {} skills", registry.len());
        Self {
            registry,
            loader,
            install_store: None,
        }
    }

    /// Create a new manager with a RocksDB-backed installation store.
    #[must_use]
    pub fn with_install_store(loader: SkillLoader, store: Arc<SkillInstallStore>) -> Self {
        let mut registry = SkillRegistry::new();
        registry.reload(&loader);
        info!("skill manager initialized with {} skills", registry.len());
        Self {
            registry,
            loader,
            install_store: Some(store),
        }
    }

    /// Access the underlying install store, if configured.
    #[must_use]
    pub const fn install_store(&self) -> Option<&Arc<SkillInstallStore>> {
        self.install_store.as_ref()
    }

    /// Inject model-invocable skill metadata into the given system prompt.
    pub fn inject_skills(&self, system_prompt: &mut String) {
        let meta = self.registry.model_invocable_metadata();
        prompt::inject_into_prompt(system_prompt, &meta);
    }

    /// Inject only the skills installed for `agent_id` into the system prompt.
    ///
    /// Looks up installed skill names from the persistent store, filters the
    /// registry to those that are both installed *and* model-invocable, then
    /// appends full skill content (description + body) so the agent can follow
    /// the instructions directly. Returns the metadata for the skills that
    /// were injected (useful for surfacing in `SessionReady`).
    ///
    /// Accepts the agent ID as a hex string (64-char BLAKE3 hash) and converts
    /// it to `AgentId`. Returns an empty vec if the ID is invalid, the install
    /// store is not configured, or the agent has no installed skills.
    pub fn inject_agent_skills(
        &self,
        agent_id_hex: &str,
        system_prompt: &mut String,
    ) -> Vec<SkillMeta> {
        let skills = self.agent_skills_full(agent_id_hex);
        if skills.is_empty() {
            return Vec::new();
        }
        let entries: Vec<prompt::SkillPromptEntry<'_>> = skills
            .iter()
            .map(|s| prompt::SkillPromptEntry {
                name: &s.frontmatter.name,
                description: &s.frontmatter.description,
                body: &s.body,
                dir_path: &s.dir_path,
            })
            .collect();
        prompt::inject_full_skills(system_prompt, &entries);
        skills
            .iter()
            .map(|s| crate::registry::skill_to_meta(s))
            .collect()
    }

    /// Return model-invocable [`SkillMeta`] for only the skills installed for
    /// `agent_id`, without modifying a prompt.
    ///
    /// Accepts the agent ID as a hex string (64-char BLAKE3 hash).
    pub fn agent_skill_meta(&self, agent_id_hex: &str) -> Vec<SkillMeta> {
        self.agent_skills_full(agent_id_hex)
            .iter()
            .map(|s| crate::registry::skill_to_meta(s))
            .collect()
    }

    /// Return full [`Skill`] objects (with body) for skills installed for
    /// `agent_id` that are also model-invocable.
    fn agent_skills_full(&self, agent_id_hex: &str) -> Vec<Skill> {
        let agent_id = match AgentId::from_hex(agent_id_hex) {
            Ok(id) => id,
            Err(_) => {
                tracing::warn!(agent_id_hex, "invalid agent ID hex for skill lookup");
                return Vec::new();
            }
        };
        let store = match self.install_store.as_deref() {
            Some(s) => s,
            None => return Vec::new(),
        };
        let installed = match store.list_for_agent(agent_id) {
            Ok(list) => list,
            Err(e) => {
                tracing::warn!(%agent_id, error = %e, "failed to list agent skills");
                return Vec::new();
            }
        };
        if installed.is_empty() {
            return Vec::new();
        }
        let installed_names: std::collections::HashSet<&str> = installed
            .iter()
            .map(|i| i.skill_name.as_str())
            .collect();
        self.registry
            .all_skills()
            .into_iter()
            .filter(|s| {
                !s.frontmatter.disable_model_invocation.unwrap_or(false)
                    && installed_names.contains(s.frontmatter.name.as_str())
            })
            .cloned()
            .collect()
    }

    /// Activate a skill by name with the given argument string.
    ///
    /// # Errors
    ///
    /// Returns [`SkillError::NotFound`] if the skill does not exist, or
    /// [`SkillError::Activation`] if argument substitution fails.
    pub fn activate(&self, name: &str, arguments: &str) -> Result<SkillActivation, SkillError> {
        let skill = self.registry.get(name)?;
        activation::activate(skill, arguments)
    }

    /// List all user-invocable skills.
    #[must_use]
    pub fn list_user_invocable(&self) -> Vec<SkillMeta> {
        self.registry.user_invocable_metadata()
    }

    /// List all registered skills.
    #[must_use]
    pub fn list_all(&self) -> Vec<SkillMeta> {
        self.registry.all_metadata()
    }

    /// Look up a skill by name.
    ///
    /// # Errors
    ///
    /// Returns [`SkillError::NotFound`] if no skill with the given name is registered.
    pub fn get(&self, name: &str) -> Result<&Skill, SkillError> {
        self.registry.get(name)
    }

    /// Reload all skills from disk.
    pub fn reload(&mut self) {
        self.registry.reload(&self.loader);
        info!("skills reloaded — {} skills available", self.registry.len());
    }

    /// Create a new skill by writing a SKILL.md to the personal skills directory,
    /// then reload the registry so it's immediately available.
    ///
    /// # Errors
    ///
    /// Returns [`SkillError`] if the name is invalid, the target directory cannot
    /// be resolved, or the filesystem write fails.
    pub fn create(
        &mut self,
        name: &str,
        description: &str,
        body: &str,
        user_invocable: bool,
    ) -> Result<Skill, SkillError> {
        validate_name(name)?;

        let target_dir = self
            .loader
            .config()
            .personal_dir
            .clone()
            .ok_or_else(|| {
                SkillError::Io(std::io::Error::new(
                    std::io::ErrorKind::NotFound,
                    "personal skills directory not configured",
                ))
            })?;

        let skill_dir = target_dir.join(name);
        std::fs::create_dir_all(&skill_dir)?;

        let mut yaml = format!("name: {name}\ndescription: {description}\n");
        if user_invocable {
            yaml.push_str("user-invocable: true\n");
        }

        let content = format!("---\n{yaml}---\n{body}");
        std::fs::write(skill_dir.join("SKILL.md"), &content)?;

        info!(name, "skill created on disk");
        self.reload();
        self.registry.get(name).map(|s| s.clone())
    }

    /// Access the inner registry (e.g. for path-based matching).
    #[must_use]
    pub const fn registry(&self) -> &SkillRegistry {
        &self.registry
    }

    // -- Per-agent installation tracking --

    fn require_install_store(&self) -> Result<&SkillInstallStore, SkillError> {
        self.install_store
            .as_deref()
            .ok_or_else(|| SkillError::Activation("install store not configured".to_string()))
    }

    /// Install a skill for a specific agent, recording it in the persistent store.
    ///
    /// # Errors
    ///
    /// Returns [`SkillError`] if the install store is not configured or the
    /// write fails.
    pub fn install_for_agent(
        &self,
        agent_id: AgentId,
        skill_name: &str,
        source_url: Option<String>,
    ) -> Result<SkillInstallation, SkillError> {
        let store = self.require_install_store()?;
        let installation = SkillInstallation {
            agent_id,
            skill_name: skill_name.to_string(),
            source_url,
            installed_at: Utc::now(),
            version: None,
        };
        store.install(&installation)?;
        info!(%agent_id, skill_name, "skill installed for agent");
        Ok(installation)
    }

    /// Uninstall a skill from a specific agent.
    ///
    /// # Errors
    ///
    /// Returns [`SkillError`] if the install store is not configured or the
    /// delete fails.
    pub fn uninstall_from_agent(
        &self,
        agent_id: AgentId,
        skill_name: &str,
    ) -> Result<(), SkillError> {
        let store = self.require_install_store()?;
        store.uninstall(agent_id, skill_name)?;
        info!(%agent_id, skill_name, "skill uninstalled from agent");
        Ok(())
    }

    /// List all skills installed for a specific agent.
    ///
    /// # Errors
    ///
    /// Returns [`SkillError`] if the install store is not configured or the
    /// read fails.
    pub fn list_agent_skills(
        &self,
        agent_id: AgentId,
    ) -> Result<Vec<SkillInstallation>, SkillError> {
        let store = self.require_install_store()?;
        store.list_for_agent(agent_id)
    }
}
