//! A permanent agent's skills: written into its instances' system
//! prompt in full, or as an index the instance loads from with
//! `load_skill` ([`aura_context_skills::on_demand`]).

use super::AgentDefinition;
use aura_context_skills::on_demand::{inject_skill_index, load_skill_text};
use aura_context_skills::prompt::{inject_full_skills, SkillPromptEntry};
use aura_context_skills::{Skill, SkillManager};
use tracing::warn;

/// How a definition's skills go into the system prompt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SkillsIn {
    /// Each skill's whole SKILL.md: for a turn that can't call
    /// `load_skill` (a cloud machine without this computer's skills)
    Full,
    /// Each skill's name, description and path; the instance loads a
    /// body with `load_skill`
    Index,
    /// None: the node the turn runs on lists the skills it was sent
    Omit,
}

impl AgentDefinition {
    /// The definition's skills that `skills` has, warning about the rest.
    fn found_skills<'a>(&self, skills: &'a SkillManager) -> Vec<&'a Skill> {
        self.skills
            .iter()
            .filter_map(|name| match skills.get(name) {
                Ok(skill) => Some(skill),
                Err(e) => {
                    warn!(definition = %self.name, skill = %name, error = %e, "Skill not found");
                    None
                }
            })
            .collect()
    }

    /// Append the definition's skills to `prompt`, as `how` says.
    pub(super) fn push_skills(&self, prompt: &mut String, skills: &SkillManager, how: SkillsIn) {
        let found = self.found_skills(skills);
        let entries: Vec<SkillPromptEntry<'_>> = found.iter().map(|skill| entry(skill)).collect();
        match how {
            SkillsIn::Full => inject_full_skills(prompt, &entries),
            SkillsIn::Index => inject_skill_index(prompt, &entries),
            SkillsIn::Omit => {}
        }
    }

    /// Whether any of the definition's skills is installed here: its
    /// instances are offered `load_skill`.
    #[must_use]
    pub fn has_skills(&self, skills: &SkillManager) -> bool {
        self.skills.iter().any(|name| skills.get(name).is_ok())
    }

    /// The definition's skills as folders, to send a cloud node.
    #[must_use]
    pub fn skill_bundle(&self, skills: &SkillManager) -> aura_core_protocol::cloud::SkillBundle {
        aura_context_skills::bundle::bundle(&self.found_skills(skills))
    }

    /// The full instructions of the definition's skill `name`, or one
    /// `file` of its folder.
    ///
    /// # Errors
    /// When the definition has no such skill, or it isn't installed here:
    /// the message names the skills it can load. When `file` can't be.
    pub fn load_skill(
        &self,
        skills: &SkillManager,
        name: &str,
        file: Option<&str>,
    ) -> Result<String, String> {
        let found = self.found_skills(skills);
        if let Some(skill) = found.iter().find(|skill| skill.frontmatter.name == name) {
            return load_skill_text(&entry(skill), file);
        }
        let names: Vec<&str> = found.iter().map(|s| s.frontmatter.name.as_str()).collect();
        Err(if names.is_empty() {
            format!("no skill `{name}`: you have no skills")
        } else {
            format!("no skill `{name}`; your skills: {}", names.join(", "))
        })
    }
}

/// `skill` as the prompt renderers take it. A definition's skills carry
/// no delegation target.
fn entry(skill: &Skill) -> SkillPromptEntry<'_> {
    SkillPromptEntry {
        name: &skill.frontmatter.name,
        description: &skill.frontmatter.description,
        body: &skill.body,
        dir_path: &skill.dir_path,
        agent_target_id: None,
        agent_target_name: None,
    }
}
