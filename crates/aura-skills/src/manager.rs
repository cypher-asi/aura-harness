//! High-level skill manager — façade over loader, registry, activation, and prompt injection.

use crate::activation;
use crate::error::SkillError;
use crate::install::{SkillInstallStore, SkillInstallation};
use crate::loader::SkillLoader;
use crate::parser::validate_name;
use crate::prompt;
use crate::registry::SkillRegistry;
use crate::types::{Skill, SkillActivation, SkillMeta};
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
        agent_id: &str,
        skill_name: &str,
        source_url: Option<String>,
    ) -> Result<SkillInstallation, SkillError> {
        let store = self.require_install_store()?;
        let installation = SkillInstallation {
            agent_id: agent_id.to_string(),
            skill_name: skill_name.to_string(),
            source_url,
            installed_at: Utc::now(),
            version: None,
        };
        store.install(&installation)?;
        info!(agent_id, skill_name, "skill installed for agent");
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
        agent_id: &str,
        skill_name: &str,
    ) -> Result<(), SkillError> {
        let store = self.require_install_store()?;
        store.uninstall(agent_id, skill_name)?;
        info!(agent_id, skill_name, "skill uninstalled from agent");
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
        agent_id: &str,
    ) -> Result<Vec<SkillInstallation>, SkillError> {
        let store = self.require_install_store()?;
        store.list_for_agent(agent_id)
    }
}
