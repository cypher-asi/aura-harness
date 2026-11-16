//! `--allowed-tools` / `--disallowed-tools`.
//!
//! A filtered tool is both left out of what the model is offered and
//! denied by the kernel policy, so a model that names it anyway gets a
//! denial instead of a run. Nothing in `aura -p` prompts: a tool the
//! policy would ask about is denied too, because the kernel has no
//! approval prompter here. The filter only narrows the policy of the
//! permission mode the run is in.

use aura_agent_kernel::PolicyConfig;
use aura_core_types::{
    resolve_effective_permission, AgentToolPermissions, ToolState, UserToolDefaults,
};
use aura_exec_tools::jobs::{JOB_KILL_TOOL_NAME, JOB_OUTPUT_TOOL_NAME};
use aura_exec_tools::todo::TODO_WRITE_TOOL_NAME;
use aura_model::ToolDefinition;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct ToolFilter {
    /// `None` allows every tool not disallowed
    allowed: Option<BTreeSet<String>>,
    disallowed: BTreeSet<String>,
}

/// Names from comma- or space-separated flag values.
fn names(values: &[String]) -> BTreeSet<String> {
    values
        .iter()
        .flat_map(|v| v.split([',', ' ', '\t']))
        .map(str::trim)
        .filter(|n| !n.is_empty())
        .map(str::to_string)
        .collect()
}

impl ToolFilter {
    pub(crate) fn new(allowed: &[String], disallowed: &[String]) -> Self {
        let allowed = names(allowed);
        Self {
            allowed: (!allowed.is_empty()).then_some(allowed),
            disallowed: names(disallowed),
        }
    }

    /// Whether `name` may run.
    pub(crate) fn permits(&self, name: &str) -> bool {
        if self.disallowed.contains(name) {
            return false;
        }
        let Some(allowed) = &self.allowed else {
            return true;
        };
        // Planning and job tools only touch the agent's own state, so an
        // allow list never needs to name them (as for permanent agents).
        allowed.contains(name)
            || name == TODO_WRITE_TOOL_NAME
            || ((name == JOB_OUTPUT_TOOL_NAME || name == JOB_KILL_TOOL_NAME)
                && allowed.contains("run_command"))
    }

    /// The tools of `tools` the model may be offered.
    pub(crate) fn filter(&self, tools: Vec<ToolDefinition>) -> Vec<ToolDefinition> {
        tools
            .into_iter()
            .filter(|t| self.permits(&t.name))
            .collect()
    }

    /// Names given in the flags that no tool in `tools` has, for a warning.
    pub(crate) fn unknown(&self, tools: &[ToolDefinition]) -> Vec<String> {
        self.allowed
            .iter()
            .flatten()
            .chain(&self.disallowed)
            .filter(|name| !tools.iter().any(|t| &t.name == *name))
            .cloned()
            .collect()
    }

    /// `policy` narrowed by the filter; a tool the filter permits keeps
    /// the state `policy` gives it. `tools` are the names agents may be
    /// offered.
    ///
    /// The denials go in the per-agent override, because the hub replaces
    /// the user defaults with its mode's table when it spawns a permanent
    /// agent; the override survives that, so spawns stay filtered. An
    /// allow list also denies, in the user defaults, any tool not named.
    pub(crate) fn apply(&self, policy: PolicyConfig, tools: &[String]) -> PolicyConfig {
        if self.allowed.is_none() && self.disallowed.is_empty() {
            return policy;
        }
        let denied: BTreeMap<String, ToolState> = tools
            .iter()
            .chain(self.allowed.iter().flatten())
            .chain(&self.disallowed)
            .filter(|name| !self.permits(name))
            .map(|name| (name.clone(), ToolState::Deny))
            .collect();
        let user_default = match &self.allowed {
            None => policy.user_default.clone(),
            Some(allowed) => {
                let harness = [
                    TODO_WRITE_TOOL_NAME,
                    JOB_OUTPUT_TOOL_NAME,
                    JOB_KILL_TOOL_NAME,
                ];
                let per_tool = allowed
                    .iter()
                    .map(String::as_str)
                    .chain(harness)
                    .filter(|name| self.permits(name))
                    .map(|name| {
                        let state = resolve_effective_permission(&policy.user_default, None, name);
                        (name.to_string(), state)
                    })
                    .collect();
                UserToolDefaults::default_permissions(per_tool, ToolState::Deny)
            }
        };
        policy
            .with_user_default(user_default)
            .with_agent_override(Some(AgentToolPermissions { per_tool: denied }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aura_core_types::resolve_effective_permission;

    fn tool(name: &str) -> ToolDefinition {
        ToolDefinition::new(name, "", serde_json::json!({"type": "object"}))
    }

    fn strings(values: &[&str]) -> Vec<String> {
        values.iter().map(|v| (*v).to_string()).collect()
    }

    const TOOLS: [&str; 6] = [
        "read_file",
        "write_file",
        "run_command",
        "todo_write",
        "job_output",
        "job_kill",
    ];

    fn state(filter: &ToolFilter, name: &str) -> ToolState {
        let policy = filter.apply(PolicyConfig::default(), &strings(&TOOLS));
        resolve_effective_permission(&policy.user_default, policy.agent_override.as_ref(), name)
    }

    #[test]
    fn no_flags_change_nothing() {
        let filter = ToolFilter::new(&[], &[]);
        assert_eq!(filter, ToolFilter::default());
        let policy = filter.apply(PolicyConfig::default(), &strings(&TOOLS));
        assert_eq!(policy.user_default, UserToolDefaults::full_access());
        assert!(policy.agent_override.is_none());
    }

    #[test]
    fn denials_survive_new_user_defaults_and_asks_are_kept() {
        let filter = ToolFilter::new(&strings(&["read_file", "write_file"]), &[]);
        let asking =
            PolicyConfig::default().with_user_default(UserToolDefaults::default_permissions(
                BTreeMap::from([("read_file".to_string(), ToolState::Allow)]),
                ToolState::Ask,
            ));
        let mut policy = filter.apply(asking, &strings(&TOOLS));
        let resolve = |policy: &PolicyConfig, name: &str| {
            resolve_effective_permission(&policy.user_default, policy.agent_override.as_ref(), name)
        };
        assert_eq!(resolve(&policy, "read_file"), ToolState::Allow);
        assert_eq!(resolve(&policy, "write_file"), ToolState::Ask);
        assert_eq!(resolve(&policy, "run_command"), ToolState::Deny);
        assert_eq!(resolve(&policy, "installed_later"), ToolState::Deny);

        // What the hub does when it spawns a permanent agent.
        policy.user_default = UserToolDefaults::full_access();
        assert_eq!(resolve(&policy, "run_command"), ToolState::Deny);
        assert_eq!(resolve(&policy, "write_file"), ToolState::Allow);
    }

    #[test]
    fn allow_list_keeps_harness_tools_and_denies_the_rest() {
        let filter = ToolFilter::new(&strings(&["read_file, run_command"]), &[]);
        let kept: Vec<String> = filter
            .filter(
                [
                    "read_file",
                    "write_file",
                    "run_command",
                    "todo_write",
                    "job_output",
                ]
                .map(tool)
                .to_vec(),
            )
            .into_iter()
            .map(|t| t.name)
            .collect();
        assert_eq!(
            kept,
            ["read_file", "run_command", "todo_write", "job_output"]
        );
        assert_eq!(state(&filter, "read_file"), ToolState::Allow);
        assert_eq!(state(&filter, "job_kill"), ToolState::Allow);
        assert_eq!(state(&filter, "write_file"), ToolState::Deny);
    }

    #[test]
    fn job_tools_need_run_command() {
        let filter = ToolFilter::new(&strings(&["read_file"]), &[]);
        assert!(!filter.permits(JOB_OUTPUT_TOOL_NAME));
        assert!(filter.permits(TODO_WRITE_TOOL_NAME));
    }

    #[test]
    fn deny_list_wins() {
        let filter = ToolFilter::new(
            &strings(&["read_file"]),
            &strings(&["read_file", "todo_write"]),
        );
        assert!(!filter.permits("read_file"));
        assert!(!filter.permits(TODO_WRITE_TOOL_NAME));
        assert_eq!(state(&filter, "read_file"), ToolState::Deny);

        let only_deny = ToolFilter::new(&[], &strings(&["run_command"]));
        assert_eq!(state(&only_deny, "run_command"), ToolState::Deny);
        assert_eq!(state(&only_deny, "write_file"), ToolState::Allow);
    }

    #[test]
    fn unknown_names_are_reported() {
        let filter = ToolFilter::new(&strings(&["read_file", "Bash"]), &[]);
        assert_eq!(filter.unknown(&[tool("read_file")]), ["Bash"]);
    }
}
