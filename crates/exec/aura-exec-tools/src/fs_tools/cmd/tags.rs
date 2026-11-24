//! Environment tags on the commands agents run. Every process a command
//! starts inherits them, so a server an agent launched can be traced back
//! to that agent even after it detaches from the harness. A run with a
//! [`RunScratch`] also points its commands' temp, cache and build folders
//! there.

use crate::tool::ToolContext;
use aura_exec_traits::RunScratch;
use std::ffi::OsString;

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
    /// The run's private folder, which the command's temp, cache and
    /// build variables point beneath.
    pub scratch: Option<RunScratch>,
}

impl ProcessTags {
    /// Tags for a command run by the tool call in `ctx`.
    #[must_use]
    pub fn for_context(ctx: &ToolContext) -> Self {
        Self {
            agent_id: ctx.caller_agent_id.map(|id| id.to_hex()),
            tool_use_id: ctx.current_tool_use_id.clone(),
            scratch: ctx.run_scratch.clone(),
        }
    }

    /// The variables to set: the scratch folders, plus the agent tags
    /// when there is an agent.
    #[must_use]
    pub fn vars(&self) -> Vec<(&'static str, OsString)> {
        let mut vars: Vec<(&'static str, OsString)> = self
            .scratch
            .iter()
            .flat_map(RunScratch::vars)
            .map(|(key, dir)| (key, dir.into_os_string()))
            .collect();
        let Some(agent_id) = &self.agent_id else {
            return vars;
        };
        vars.push((AGENT_ID_VAR, agent_id.into()));
        vars.push((HARNESS_PID_VAR, std::process::id().to_string().into()));
        if let Some(tool_use_id) = &self.tool_use_id {
            vars.push((TOOL_USE_ID_VAR, tool_use_id.into()));
        }
        vars
    }

    pub fn apply(&self, cmd: &mut std::process::Command) {
        for (key, value) in self.vars() {
            cmd.env(key, value);
        }
    }
}
