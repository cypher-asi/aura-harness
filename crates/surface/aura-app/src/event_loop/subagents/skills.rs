//! The app's side of `load_skill`: a caller's skills are those of the
//! permanent agent it is an instance of ([`DefinitionSkills`]), and its
//! turn lists them as an index rather than in full
//! ([`AgentHub::push_definition_skills`]).

use super::*;
use crate::agent_definitions::{AgentDefinition, SkillsIn};
use crate::event_loop::inbox::HubLink;
use aura_exec_tools::skills::{load_skill_definition, SkillSource};

/// Where `load_skill` finds a local agent's skills: its permanent
/// agent's, through the hub.
pub struct DefinitionSkills {
    link: HubLink,
}

impl DefinitionSkills {
    #[must_use]
    pub fn new(link: HubLink) -> Self {
        Self { link }
    }
}

impl SkillSource for DefinitionSkills {
    fn load(&self, caller: Option<AgentId>, name: &str) -> Result<String, String> {
        let hub = self
            .link
            .hub()
            .ok_or_else(|| "skills are not available yet".to_string())?;
        let caller = caller.ok_or_else(|| "skills need an agent to load them".to_string())?;
        let definition = lock(&hub.registry)
            .find(caller)
            .and_then(|agent| agent.definition.clone())
            .ok_or_else(|| {
                "you have no skills: you are not a permanent agent's instance".to_string()
            })?;
        let definitions = lock(&hub.definitions);
        let found = definitions
            .get(&definition)
            .ok_or_else(|| format!("your permanent agent `{definition}` no longer exists"))?;
        let skills = lock(&hub.skills);
        found.load_skill(&skills, name)
    }
}

impl AgentHub {
    /// Append `definition`'s skills to `prompt` for a local turn: as an
    /// index, with `load_skill` added to `tools`, when it has any here.
    pub(super) fn push_definition_skills(
        &self,
        definition: &AgentDefinition,
        prompt: &mut String,
        tools: &mut Vec<ToolDefinition>,
    ) {
        let skills = lock(&self.skills);
        let how = if definition.has_skills(&skills) {
            let tool = load_skill_definition();
            if !tools.iter().any(|t| t.name == tool.name) {
                tools.push(tool);
            }
            SkillsIn::Index
        } else {
            SkillsIn::Full
        };
        definition.extend_system_prompt_with(prompt, &skills, how);
    }
}
