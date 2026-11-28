//! The app's side of `load_skill`: a caller's skills are those of the
//! permanent agent it is an instance of ([`DefinitionSkills`]), and its
//! turn lists them as an index rather than in full
//! ([`AgentHub::push_definition_skills`]).

use super::setup::{plan_mode, MACHINE_PEERS};
use super::*;
use crate::agent_definitions::{AgentDefinition, SkillsIn};
use crate::cloud::hub::TurnSkills;
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
    fn load(
        &self,
        caller: Option<AgentId>,
        name: &str,
        file: Option<&str>,
    ) -> Result<String, String> {
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
        found.load_skill(&skills, name, file)
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

impl AgentHub {
    /// The system prompt of a cloud turn of `agent`, an instance of
    /// `definition`, working in `folder`, and its skills to send its
    /// node. With skills, the prompt leaves their text to the node
    /// ([`SkillsIn::Omit`]), and the skills carry the prompt with it for a
    /// node that can't take them.
    pub(super) fn cloud_prompts(
        &self,
        agent: AgentId,
        definition: Option<&str>,
        folder: Option<&Path>,
    ) -> (Option<String>, Option<TurnSkills>) {
        let bundle = definition.and_then(|name| {
            let definitions = lock(&self.definitions);
            let found = definitions.get(name)?;
            let skills = lock(&self.skills);
            found
                .has_skills(&skills)
                .then(|| found.skill_bundle(&skills))
        });
        let full = self.cloud_prompt(agent, definition, folder, SkillsIn::Full);
        let Some(bundle) = bundle else {
            return (full, None);
        };
        let skills = TurnSkills {
            bundle,
            full_system_prompt: full.unwrap_or_default(),
        };
        let omitted = self.cloud_prompt(agent, definition, folder, SkillsIn::Omit);
        (omitted, Some(skills))
    }

    /// The system prompt of a cloud turn, its skills as `how` says:
    /// project instructions, permanent agent, plan mode and channel.
    fn cloud_prompt(
        &self,
        agent: AgentId,
        definition: Option<&str>,
        folder: Option<&Path>,
        how: SkillsIn,
    ) -> Option<String> {
        let mut prompt = folder
            .map(|folder| self.instructions.for_workspace(folder).text.clone())
            .unwrap_or_default();
        if let Some(found) = definition.and_then(|name| lock(&self.definitions).get(name).cloned())
        {
            found.extend_system_prompt_with(&mut prompt, &lock(&self.skills), how);
        }
        if self.mode_of(agent) == PermissionMode::Plan {
            prompt.push_str("\n\n");
            prompt.push_str(&plan_mode::render(self.plan_build(agent)));
        }
        if let Some(channel) = crate::event_loop::channels::channel_prompt(self, agent) {
            prompt.push_str("\n\n");
            prompt.push_str(&channel);
            prompt.push_str(MACHINE_PEERS);
        }
        let prompt = prompt.trim();
        (!prompt.is_empty()).then(|| prompt.to_string())
    }
}
