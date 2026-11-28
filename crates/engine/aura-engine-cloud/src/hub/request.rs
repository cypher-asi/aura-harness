//! The chat run a cloud agent's turn asks its node for, and the policy
//! its node holds the turn to.

use crate::machine_env::WORKSPACE_DIR;
use aura_core_protocol::{
    AgentCapabilities, AgentIdentity, AgentPermissionsWire, AgentToolPermissionsWire,
    ModelSelection, RuntimeRequest, RuntimeRequestType, ToolStateWire, WorkspaceLocation,
};
use aura_core_types::AgentId;

/// What one turn of a cloud agent asks its node for.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TurnAsk {
    /// What the agent is asked.
    pub prompt: String,
    /// The model to run, else the node's own.
    pub model: Option<String>,
    /// The model that titles the chat from the prompt, when it wants a
    /// title.
    pub title_model: Option<String>,
    /// The zOS session's token, for models behind its proxy.
    pub auth_jwt: Option<String>,
    /// What the agent's turns add to the node's prompt, as they would
    /// here: its project's instructions, its permanent agent, plan mode,
    /// its channel. `None` leaves the node's prompt as it is.
    pub system_prompt: Option<String>,
    /// Tools the node offers that the agent may not call, as its
    /// permanent agent leaves them out.
    pub tools_off: Vec<String>,
    /// The agent's skills, sent to its node before the turn when they
    /// changed; `system_prompt` then carries none of their text.
    pub skills: Option<TurnSkills>,
}

/// An agent's skills for its cloud turn ([`CloudHub::run_turn`](super::CloudHub::run_turn)).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TurnSkills {
    /// Their folders, for the node to list them and serve `load_skill`
    pub bundle: aura_core_protocol::cloud::SkillBundle,
    /// The turn's system prompt with every skill's text in it, for a node
    /// that can't take the folders
    pub full_system_prompt: String,
}

/// The chat run request for `ask`, a turn of `agent`.
pub(super) fn chat_request(agent: AgentId, ask: &TurnAsk) -> RuntimeRequest {
    RuntimeRequest {
        r#type: RuntimeRequestType::Chat {
            conversation_messages: Vec::new(),
        },
        agent_identity: AgentIdentity {
            partition_id: Some(agent.to_hex()),
            system_prompt: ask.system_prompt.as_deref().map(with_workspace),
            ..AgentIdentity::default()
        },
        model: ModelSelection {
            id: ask.model.clone(),
            title_model: ask.title_model.clone(),
            ..ModelSelection::default()
        },
        workspace: WorkspaceLocation {
            project_path: Some(WORKSPACE_DIR.to_string()),
            ..WorkspaceLocation::default()
        },
        project: None,
        agent_permissions: cloud_agent_permissions(),
        tool_permissions: tools_off(&ask.tools_off),
        agent_capabilities: AgentCapabilities::default(),
        auth_jwt: ask.auth_jwt.clone(),
        user_id: "hub".to_string(),
        mode: None,
    }
}

/// `prompt` with where the workspace is: a node given a prompt of its
/// own no longer adds that to its own.
fn with_workspace(prompt: &str) -> String {
    format!(
        "{prompt}\n\nYour workspace root is `{WORKSPACE_DIR}`. All relative file paths are \
         resolved against this directory."
    )
}

/// `tools` turned off for the turn; `None` leaves every tool to the
/// node's policy.
fn tools_off(tools: &[String]) -> Option<AgentToolPermissionsWire> {
    (!tools.is_empty()).then(|| AgentToolPermissionsWire {
        per_tool: tools
            .iter()
            .map(|tool| (tool.clone(), ToolStateWire::Off))
            .collect(),
    })
}

/// What a cloud agent may do on its machine. Sent explicitly: an empty
/// bundle means full access on a node, which includes computer use, and
/// a cloud machine has no screen (and OpenRouter refuses the computer
/// tool's server-tool entry).
fn cloud_agent_permissions() -> AgentPermissionsWire {
    use aura_core_protocol::CapabilityWire;
    AgentPermissionsWire {
        capabilities: vec![
            CapabilityWire::SpawnAgent,
            CapabilityWire::ControlAgent,
            CapabilityWire::ReadAgent,
            CapabilityWire::ListAgents,
            CapabilityWire::InvokeProcess,
            CapabilityWire::ReadAllProjects,
            CapabilityWire::WriteAllProjects,
        ],
        ..AgentPermissionsWire::default()
    }
}

/// The policy a cloud agent's node enforces on its turns: the permissions
/// [`chat_request`] sends, over the node's defaults (the `Agent` mode, no
/// per-tool settings), as the node builds it for a chat run. The tools a
/// permanent agent leaves out are off on top of it.
#[must_use]
pub fn cloud_policy() -> aura_agent_kernel::PolicyConfig {
    aura_agent_kernel::PolicyConfig {
        agent_permissions: aura_core_protocol::agent_permissions_from_wire(
            cloud_agent_permissions(),
        ),
        ..aura_agent_kernel::PolicyConfig::default()
    }
    .with_user_default(aura_core_types::UserToolDefaults::default())
    .with_agent_override(None)
    .with_mode(aura_core_types::AgentMode::Agent)
}
