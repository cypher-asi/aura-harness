//! A session's skills on the node: its turns list the skills installed
//! for its agent as an index ([`inject_skills`]), and `load_skill`
//! returns one in full ([`SessionSkills`]).

use std::sync::{Arc, RwLock};

use aura_agent::AgentLoopConfig;
use aura_context_skills::SkillManager;
use aura_core_types::AgentId;
use aura_exec_tools::skills::SkillSource;
use tracing::debug;

use super::{Session, WsContext};

/// Where `load_skill` finds the skills installed for a session's agent.
pub(in crate::gateway::session) struct SessionSkills {
    manager: Arc<RwLock<SkillManager>>,
    agent_id: String,
}

impl SessionSkills {
    /// The session's skills, when it has any: `None` for a session with
    /// no skill manager, no skill agent, or no skills installed for it.
    pub(in crate::gateway::session) fn of(session: &Session, ctx: &WsContext) -> Option<Self> {
        let manager = ctx.skill_manager.clone()?;
        let agent_id = session.skill_agent_id.clone()?;
        let has_any = manager
            .read()
            .is_ok_and(|mgr| !mgr.agent_skill_meta(&agent_id).is_empty());
        has_any.then_some(Self { manager, agent_id })
    }
}

impl SkillSource for SessionSkills {
    fn load(
        &self,
        _caller: Option<AgentId>,
        name: &str,
        file: Option<&str>,
    ) -> Result<String, String> {
        let mgr = self
            .manager
            .read()
            .map_err(|_| "the skills are unavailable".to_string())?;
        mgr.load_agent_skill(&self.agent_id, name, file)
            .map_err(|error| {
                if let aura_context_skills::SkillError::Activation(why) = error {
                    return why;
                }
                let names: Vec<String> = mgr
                    .agent_skill_meta(&self.agent_id)
                    .into_iter()
                    .map(|meta| meta.name)
                    .collect();
                format!("no skill `{name}`; your skills: {}", names.join(", "))
            })
    }
}

/// List the session agent's skills in the system prompt, as an index its
/// turn loads from with `load_skill`; returns the names of the listed
/// skills.
pub(in crate::gateway::session) fn inject_skills(
    session: &Session,
    ctx: &WsContext,
    config: &mut AgentLoopConfig,
) -> Vec<String> {
    let (Some(sm), Some(agent_id)) = (&ctx.skill_manager, &session.skill_agent_id) else {
        return Vec::new();
    };
    let Ok(mgr) = sm.read() else {
        return Vec::new();
    };
    // The "Skills" bucket is the text this step appends, measured as a
    // delta so `recompute_breakdown` doesn't also count it under "System
    // prompt".
    let pre_inject_chars = config.system_prompt.len();
    let injected = mgr.inject_agent_skill_index(agent_id, &mut config.system_prompt);
    config.skills_chars = config.system_prompt.len().saturating_sub(pre_inject_chars);
    // Each skill's name and summary for the context-contents viewer.
    config.skills_segments = injected
        .iter()
        .map(|s| (s.name.clone(), s.description.clone()))
        .collect();
    let names: Vec<String> = injected.iter().map(|s| s.name.clone()).collect();
    if !names.is_empty() {
        debug!(
            session_id = %session.session_id,
            skill_count = names.len(),
            skills_chars = config.skills_chars,
            "Listed agent skills in the prompt"
        );
    }
    names
}
