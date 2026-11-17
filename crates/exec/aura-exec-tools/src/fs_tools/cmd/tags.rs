//! Environment tags on the commands agents run. Every process a command
//! starts inherits them, so a server an agent launched can be traced back
//! to that agent even after it detaches from the harness.

use crate::tool::ToolContext;

/// Hex id of the agent whose tool call started the process.
pub const AGENT_ID_VAR: &str = "AURA_AGENT_ID";
/// Model `tool_use` id of that call.
pub const TOOL_USE_ID_VAR: &str = "AURA_TOOL_USE_ID";
/// Pid of the harness that ran the call.
pub const HARNESS_PID_VAR: &str = "AURA_HARNESS_PID";

/// Who started a command, written into its environment.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProcessTags {
    pub agent_id: Option<String>,
    pub tool_use_id: Option<String>,
}

impl ProcessTags {
    /// Tags for a command run by the tool call in `ctx`.
    #[must_use]
    pub fn for_context(ctx: &ToolContext) -> Self {
        Self {
            agent_id: ctx.caller_agent_id.map(|id| id.to_hex()),
            tool_use_id: ctx.current_tool_use_id.clone(),
        }
    }

    /// The variables to set; none without an agent.
    #[must_use]
    pub fn vars(&self) -> Vec<(&'static str, String)> {
        let Some(agent_id) = &self.agent_id else {
            return Vec::new();
        };
        let mut vars = vec![
            (AGENT_ID_VAR, agent_id.clone()),
            (HARNESS_PID_VAR, std::process::id().to_string()),
        ];
        if let Some(tool_use_id) = &self.tool_use_id {
            vars.push((TOOL_USE_ID_VAR, tool_use_id.clone()));
        }
        vars
    }

    pub fn apply(&self, cmd: &mut std::process::Command) {
        for (key, value) in self.vars() {
            cmd.env(key, value);
        }
    }
}
