use aura_agent::prompts::default_system_prompt;
use aura_agent::AgentLoopConfig;
use aura_kernel::{ExecutorRouter, PermissionLevel, PolicyConfig};
use aura_reasoner::ToolDefinition;
use aura_tools::{DefaultToolRegistry, ToolConfig, ToolExecutor, ToolRegistry};
use std::sync::Arc;

pub use aura_agent::session_bootstrap::{load_auth_token, open_store, resolve_store_path};

#[must_use]
pub fn default_agent_config() -> AgentLoopConfig {
    AgentLoopConfig {
        system_prompt: default_system_prompt(),
        auth_token: load_auth_token(),
        ..AgentLoopConfig::default()
    }
}

/// Parse a boolean-ish environment variable.
///
/// Accepts `1`, `true`, `yes`, `on` (case-insensitive). Anything else
/// — including an unset var — is treated as `false`. Mirrors the
/// tolerance used elsewhere in the workspace so operators don't have
/// to remember yet another spelling.
fn env_bool(name: &str) -> bool {
    std::env::var(name).ok().is_some_and(|v| {
        let v = v.trim();
        v.eq_ignore_ascii_case("1")
            || v.eq_ignore_ascii_case("true")
            || v.eq_ignore_ascii_case("yes")
            || v.eq_ignore_ascii_case("on")
    })
}

fn env_csv(name: &str) -> Vec<String> {
    std::env::var(name)
        .ok()
        .map(|v| {
            v.split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect()
        })
        .unwrap_or_default()
}

/// Build the terminal-mode `ToolConfig` from environment overrides.
///
/// Starts from [`ToolConfig::default`] — the Phase-5 fail-closed
/// defaults — and only loosens things when the operator opts in:
///
/// * `AURA_ALLOW_RUN_COMMAND=1` — flips `enable_commands = true`. On
///   its own this is still a no-op because `binary_allowlist` is empty
///   (Phase-5 `CmdRunTool::execute` rejects anything not in the list).
/// * `AURA_ALLOWED_COMMANDS=cargo,git,ls` — comma-separated
///   binary allowlist. Additive — supply whatever the agent needs.
/// * `AURA_ALLOW_SHELL=1` — allow `sh`/`bash`/`pwsh` fan-out. Off by
///   default; use sparingly, shell invocations bypass the binary
///   allowlist's argv0 check.
#[must_use]
pub fn tool_config_from_env() -> ToolConfig {
    let mut cfg = ToolConfig::default();
    if env_bool("AURA_ALLOW_RUN_COMMAND") {
        cfg.enable_commands = true;
        let allowlist = env_csv("AURA_ALLOWED_COMMANDS");
        if !allowlist.is_empty() {
            cfg.binary_allowlist = allowlist;
        }
        if env_bool("AURA_ALLOW_SHELL") {
            cfg.allow_shell = true;
        }
    }
    cfg
}

/// True when the operator has opted into `run_command` via
/// `AURA_ALLOW_RUN_COMMAND`. Kept as a thin wrapper so the policy
/// layer and the executor layer reach the same decision from the same
/// env variable.
#[must_use]
pub fn allow_run_command_from_env() -> bool {
    env_bool("AURA_ALLOW_RUN_COMMAND")
}

/// Build the terminal-mode [`PolicyConfig`] with `run_command`
/// elevated when `AURA_ALLOW_RUN_COMMAND=1` is set.
///
/// With the env flag unset this is exactly [`PolicyConfig::default`]
/// (fail-closed, `run_command` denied). With it set, `run_command`
/// joins `allowed_tools` and is mapped to
/// [`PermissionLevel::AlwaysAllow`] so the CLI's lack of an approval
/// pump doesn't permanently deny every shell invocation. The
/// executor-layer `ToolConfig` from [`tool_config_from_env`] still
/// enforces `binary_allowlist`, so this is not an "arbitrary command
/// execution" switch — it's "let `run_command` reach the allowlist
/// filter at all".
#[must_use]
pub fn policy_config_from_env() -> PolicyConfig {
    let mut policy = PolicyConfig::default();
    if allow_run_command_from_env() {
        policy.allowed_tools.insert("run_command".to_string());
        policy
            .tool_permissions
            .insert("run_command".to_string(), PermissionLevel::AlwaysAllow);
    }
    policy
}

/// Build an executor router honoring the env-driven `ToolConfig`.
///
/// Replaces the bare `aura_agent::session_bootstrap::build_executor_router`
/// call at the CLI seam — that helper hard-codes
/// `ToolExecutor::with_defaults()` which ignores `AURA_ALLOW_RUN_COMMAND`.
#[must_use]
pub fn build_executor_router_with_config(
    tool_config: &ToolConfig,
) -> (ExecutorRouter, Vec<ToolDefinition>) {
    let mut executor_router = ExecutorRouter::new();
    executor_router.add_executor(Arc::new(ToolExecutor::new(tool_config.clone())));

    let tool_registry = DefaultToolRegistry::new();
    let tools = tool_registry.list();

    (executor_router, tools)
}

#[cfg(test)]
mod tests {
    use super::*;
    use aura_kernel::PermissionLevel;
    use std::sync::Mutex;

    /// Env-var tests mutate process-global state and therefore cannot
    /// run in parallel. We serialize them through a single mutex
    /// rather than pulling in `serial_test` as a dev-dep.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    /// Keys we touch; cleaned up after every env-var test so ordering
    /// within the mutex still matters less than in truly isolated
    /// processes.
    const ENV_KEYS: &[&str] = &[
        "AURA_ALLOW_RUN_COMMAND",
        "AURA_ALLOWED_COMMANDS",
        "AURA_ALLOW_SHELL",
    ];

    fn clear_env() {
        for k in ENV_KEYS {
            std::env::remove_var(k);
        }
    }

    fn set_env(pairs: &[(&str, &str)]) {
        for (k, v) in pairs {
            std::env::set_var(k, v);
        }
    }

    /// Kernel-default asserts don't touch env vars and are safe to run
    /// in parallel with each other, so they live outside the mutex.
    #[test]
    fn default_policy_allows_find_files_and_delete_file() {
        let policy = PolicyConfig::default();
        assert!(
            policy.allowed_tools.contains("find_files"),
            "find_files must be in the default allowed_tools; the CLI relies on \
             `PolicyConfig::default()` and find_files was a Phase-5 oversight"
        );
        assert!(
            policy.allowed_tools.contains("delete_file"),
            "delete_file must be in the default allowed_tools to match its \
             default_tool_permission() of AlwaysAllow"
        );
    }

    #[test]
    fn default_policy_still_denies_run_command() {
        let policy = PolicyConfig::default();
        assert!(
            !policy.allowed_tools.contains("run_command"),
            "run_command must stay out of default allowed_tools; it is the \
             highest blast-radius tool and requires explicit opt-in"
        );
    }

    #[test]
    fn env_toggles_behave_correctly() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());

        clear_env();
        let policy = policy_config_from_env();
        assert!(
            !policy.allowed_tools.contains("run_command"),
            "run_command must stay denied when AURA_ALLOW_RUN_COMMAND is unset"
        );
        assert!(!policy.tool_permissions.contains_key("run_command"));

        set_env(&[
            ("AURA_ALLOW_RUN_COMMAND", "1"),
            ("AURA_ALLOWED_COMMANDS", "cargo, git ,ls"),
        ]);
        let policy = policy_config_from_env();
        assert!(policy.allowed_tools.contains("run_command"));
        assert_eq!(
            policy.tool_permissions.get("run_command"),
            Some(&PermissionLevel::AlwaysAllow)
        );
        let cfg = tool_config_from_env();
        assert!(cfg.enable_commands);
        assert_eq!(
            cfg.binary_allowlist,
            vec!["cargo".to_string(), "git".to_string(), "ls".to_string()]
        );
        assert!(
            !cfg.allow_shell,
            "shell must stay off unless AURA_ALLOW_SHELL is also set"
        );

        set_env(&[("AURA_ALLOW_SHELL", "1")]);
        let cfg = tool_config_from_env();
        assert!(cfg.allow_shell);

        for v in ["1", "true", "TRUE", "Yes", "on"] {
            clear_env();
            set_env(&[("AURA_ALLOW_RUN_COMMAND", v)]);
            assert!(
                allow_run_command_from_env(),
                "expected `{v}` to count as a truthy env value"
            );
        }

        clear_env();
        set_env(&[("AURA_ALLOW_RUN_COMMAND", "1")]);
        let cfg = tool_config_from_env();
        assert!(cfg.enable_commands);
        assert!(
            cfg.binary_allowlist.is_empty(),
            "no allowlist means empty, not a stealth default"
        );

        clear_env();
    }
}
