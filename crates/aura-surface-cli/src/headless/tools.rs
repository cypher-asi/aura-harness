//! `--allowed-tools` / `--disallowed-tools`.
//!
//! A filtered tool is both left out of what the model is offered and
//! denied by the kernel policy, so a model that names it anyway gets a
//! denial instead of a run. Nothing in `aura -p` prompts: a tool the
//! policy would ask about is denied too, because the kernel has no
//! approval prompter here.

use aura_agent_kernel::PolicyConfig;
use aura_core_types::{ToolState, UserToolDefaults};
use aura_model_reasoner::ToolDefinition;
use aura_tools::jobs::{JOB_KILL_TOOL_NAME, JOB_OUTPUT_TOOL_NAME};
use aura_tools::todo::TODO_WRITE_TOOL_NAME;
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

    /// `policy` with the filter applied to its tool permissions.
    pub(crate) fn apply(&self, policy: PolicyConfig) -> PolicyConfig {
        if self.allowed.is_none() && self.disallowed.is_empty() {
            return policy;
        }
        let mut per_tool = BTreeMap::new();
        if let Some(allowed) = &self.allowed {
            let harness = [
                TODO_WRITE_TOOL_NAME,
                JOB_OUTPUT_TOOL_NAME,
                JOB_KILL_TOOL_NAME,
            ];
            for name in allowed.iter().map(String::as_str).chain(harness) {
                if self.permits(name) {
                    per_tool.insert(name.to_string(), ToolState::Allow);
                }
            }
        }
        for name in &self.disallowed {
            per_tool.insert(name.clone(), ToolState::Deny);
        }
        let fallback = if self.allowed.is_some() {
            ToolState::Deny
        } else {
            ToolState::Allow
        };
        policy.with_user_default(UserToolDefaults::default_permissions(per_tool, fallback))
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

    fn state(filter: &ToolFilter, name: &str) -> ToolState {
        let policy = filter.apply(PolicyConfig::default());
        resolve_effective_permission(&policy.user_default, None, name)
    }

    #[test]
    fn no_flags_change_nothing() {
        let filter = ToolFilter::new(&[], &[]);
        assert_eq!(filter, ToolFilter::default());
        assert_eq!(
            filter.apply(PolicyConfig::default()).user_default,
            UserToolDefaults::full_access()
        );
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
