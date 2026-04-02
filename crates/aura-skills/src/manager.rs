//! High-level skill manager — façade over loader, registry, activation, and prompt injection.

use crate::activation;
use crate::error::SkillError;
use crate::loader::SkillLoader;
use crate::prompt;
use crate::registry::SkillRegistry;
use crate::types::{Skill, SkillActivation, SkillMeta};
use tracing::info;

/// Top-level entry point for the skill system.
///
/// Owns a [`SkillLoader`] and [`SkillRegistry`], and exposes methods for
/// listing, activating, and injecting skills into agent prompts.
pub struct SkillManager {
    registry: SkillRegistry,
    loader: SkillLoader,
}

impl SkillManager {
    /// Create a new manager and immediately load all discoverable skills.
    #[must_use]
    pub fn new(loader: SkillLoader) -> Self {
        let mut registry = SkillRegistry::new();
        registry.reload(&loader);
        info!("skill manager initialized with {} skills", registry.len());
        Self { registry, loader }
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

    /// Access the inner registry (e.g. for path-based matching).
    #[must_use]
    pub const fn registry(&self) -> &SkillRegistry {
        &self.registry
    }
}
