//! The policy an agent's kernel enforces, for the POLICIES tab.

use crate::mode_command::{ApprovalLevel, PermissionMode};

/// The policy an agent's kernel enforces, as the Kernel panel's POLICIES
/// tab shows it. A read-only snapshot: it changes only when the event loop
/// sends another.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PolicyView {
    /// Work mode, the session ceiling every tool call is checked against
    pub mode: PermissionMode,
    /// Which allowed calls ask first
    pub approval: ApprovalLevel,
    /// BLAKE3 hex digest of the policy configuration in force
    pub policy_hash: String,
    /// Proposal kinds the kernel accepts (`reason`, `delegate`, ...)
    pub action_kinds: Vec<String>,
    /// Most tool calls accepted from one model reply
    pub max_proposals: usize,
    /// Capabilities the agent holds (`SpawnAgent`, `InvokeProcess`, ...)
    pub capabilities: Vec<String>,
    /// Capabilities there are to hold, for `n/total`
    pub capability_total: usize,
    /// Orgs, projects and agents the agent may touch, in words
    pub scope: String,
    /// How each tool offered to the agent resolves now, in name order
    pub tools: Vec<ToolPolicyView>,
    /// The project's "always allow" rules, as their labels
    pub always_allow: Vec<String>,
    /// Answers remembered for this session, as `tool=state`
    pub session_grants: Vec<String>,
    /// Integrations installed for this runtime
    pub integrations: Vec<String>,
    /// Whether commands can be confined to the agent's folder here
    pub commands_confined: bool,
    /// Whether `ask` calls can prompt the user; without it they are denied
    pub live_prompts: bool,
    /// How long a prompt waits before its call is denied, in seconds
    pub prompt_timeout_secs: u64,
    /// `PermissionRequest` plugin hooks consulted before prompting
    pub permission_hooks: usize,
}

impl PolicyView {
    /// How many tools resolve to `on`, `ask` and `off`.
    #[must_use]
    pub fn tool_counts(&self) -> (usize, usize, usize) {
        self.tools
            .iter()
            .fold((0, 0, 0), |(on, ask, off), tool| match tool.state {
                ToolPolicyState::On => (on + 1, ask, off),
                ToolPolicyState::Ask => (on, ask + 1, off),
                ToolPolicyState::Off => (on, ask, off + 1),
            })
    }
}

/// How one tool resolves under an agent's policy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolPolicyView {
    /// Tool name
    pub name: String,
    pub state: ToolPolicyState,
    /// The deciding rule, rendered as `source.rule` (`mode.plan`,
    /// `user_default.per_tool[read_file]=on`)
    pub decided_by: String,
}

/// What the policy does with a call to a tool, before its arguments.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolPolicyState {
    /// Runs without asking
    On,
    /// Asks the user first
    Ask,
    /// Denied
    Off,
}

impl ToolPolicyState {
    /// Wire label: `on`, `ask` or `off`.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::On => "on",
            Self::Ask => "ask",
            Self::Off => "off",
        }
    }
}
