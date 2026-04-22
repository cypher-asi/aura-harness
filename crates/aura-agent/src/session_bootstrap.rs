//! Shared session-bootstrap helpers for `aura-node`, the TUI harness
//! and any future embedder.
//!
//! Phase 3 consolidated the per-binary copies of
//! `default_agent_config`, `tool_config_from_env`,
//! `policy_config_from_env`, and `build_executor_router_with_config`
//! into this module so the TUI and the headless node can't silently
//! drift on env-var semantics or executor wiring. The TUI-side file
//! (`src/session_helpers.rs`) is now a thin `pub use` re-export layer.

use crate::prompts::default_system_prompt;
use crate::AgentLoopConfig;
use aura_kernel::{ExecutorRouter, PermissionLevel, PolicyConfig};
use aura_reasoner::ToolDefinition;
use aura_store::RocksStore;
use aura_tools::{DefaultToolRegistry, ToolConfig, ToolExecutor, ToolRegistry};
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Resolve the canonical store path, migrating from legacy `store/` if needed.
///
/// Canonical path: `{data_dir}/db`. If a legacy `{data_dir}/store` directory
/// exists and the canonical one does not, performs a one-time rename migration.
/// If both exist, the legacy directory is automatically removed.
pub fn resolve_store_path(data_dir: &Path) -> PathBuf {
    let canonical = data_dir.join("db");
    let legacy = data_dir.join("store");

    if canonical.exists() {
        if legacy.exists() {
            match std::fs::remove_dir_all(&legacy) {
                Ok(()) => {
                    tracing::info!(
                        legacy = %legacy.display(),
                        "Removed stale legacy 'store' directory"
                    );
                }
                Err(e) => {
                    tracing::warn!(
                        error = %e,
                        legacy = %legacy.display(),
                        "Failed to remove legacy 'store' directory — please remove it manually"
                    );
                }
            }
        }
        return canonical;
    }
    if legacy.exists() {
        match std::fs::rename(&legacy, &canonical) {
            Ok(()) => {
                tracing::info!(
                    from = %legacy.display(),
                    to = %canonical.display(),
                    "Migrated store from legacy path to canonical path"
                );
                return canonical;
            }
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    legacy = %legacy.display(),
                    "Failed to migrate store — falling back to legacy path"
                );
                return legacy;
            }
        }
    }
    canonical
}

pub fn open_store(path: &Path) -> anyhow::Result<Arc<RocksStore>> {
    Ok(Arc::new(RocksStore::open(path, false)?))
}

/// Build the default executor router used by the terminal harness and
/// embedded tooling.
///
/// **Phase 5 hardening note:** This wires in
/// [`ToolExecutor::with_defaults()`], which — after the Phase 5 flip of
/// [`aura_tools::ToolConfig::default`] — is a *no-shell, no-commands*
/// tool router. Filesystem tools (`read_file`, `write_file`, `list_files`,
/// …) are reachable, but `run_command` is blocked both at the category
/// gate (`enable_commands = false`) and at `CmdRunTool::execute`
/// (empty `binary_allowlist`).
///
/// Production callers that want command execution must *not* rely on
/// this helper. They should construct a custom
/// [`aura_tools::ToolConfig`] with `enable_commands: true` and a
/// populated `binary_allowlist`, feed it into [`ToolExecutor::new`],
/// and register that executor on their own `ExecutorRouter`. The opt-in
/// is deliberately plumbed through user-supplied config rather than a
/// convenience flag on this bootstrap.
#[must_use]
pub fn build_executor_router() -> (ExecutorRouter, Vec<ToolDefinition>) {
    let mut executor_router = ExecutorRouter::new();
    executor_router.add_executor(Arc::new(ToolExecutor::with_defaults()));

    let tool_registry = DefaultToolRegistry::new();
    let tools = tool_registry.list();

    (executor_router, tools)
}

#[must_use]
pub fn load_auth_token() -> Option<String> {
    std::env::var("AURA_ROUTER_JWT")
        .ok()
        .or_else(aura_auth::CredentialStore::load_token)
}

// `ProviderSelection` / `select_provider` were removed in Wave 4. The
// canonical factory now lives in
// [`aura_reasoner::provider_factory`]. Callers use
// `aura_reasoner::provider_from_name` / `provider_from_session_config` /
// `default_provider_from_env`.

// ---------------------------------------------------------------------
// Phase 3 consolidation: moved from `src/session_helpers.rs`.
//
// These helpers used to live next to the TUI binary but were needed by
// `aura-node` and future embedders too. Centralising them here keeps
// the env-var contract single-sourced. The TUI re-exports them
// verbatim; new helpers should land here directly.
// ---------------------------------------------------------------------

/// Default [`AgentLoopConfig`] used by the TUI and other CLI-shaped
/// embedders — pulls the canonical system prompt and the harness auth
/// token, leaves everything else at `AgentLoopConfig::default()`.
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
/// — including an unset var — is treated as `false`.
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

/// Build a [`ToolConfig`] from environment overrides.
///
/// Starts from [`ToolConfig::default`] — the Phase-5 fail-closed
/// defaults — and only loosens things when the operator opts in:
///
/// * `AURA_ALLOW_RUN_COMMAND=1` — flips `enable_commands = true`.
/// * `AURA_ALLOWED_COMMANDS=cargo,git,ls` — comma-separated binary
///   allowlist. Additive.
/// * `AURA_ALLOW_SHELL=1` — allow `sh`/`bash`/`pwsh` fan-out.
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
/// `AURA_ALLOW_RUN_COMMAND`.
#[must_use]
pub fn allow_run_command_from_env() -> bool {
    env_bool("AURA_ALLOW_RUN_COMMAND")
}

/// Build a [`PolicyConfig`] with `run_command` elevated when
/// `AURA_ALLOW_RUN_COMMAND=1` is set.
///
/// With the env flag unset this is exactly [`PolicyConfig::default`]
/// (fail-closed, `run_command` denied). With it set, `run_command`
/// joins `allowed_tools` and is mapped to
/// [`PermissionLevel::AlwaysAllow`] so embedders without an approval
/// pump don't permanently deny every shell invocation. The
/// executor-layer [`ToolConfig`] from [`tool_config_from_env`] still
/// enforces `binary_allowlist`.
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

/// Build an executor router honoring a caller-supplied [`ToolConfig`].
///
/// The plain [`build_executor_router`] hard-codes
/// `ToolExecutor::with_defaults()` which ignores env overrides; this
/// variant threads the config through [`ToolExecutor::new`] instead so
/// `AURA_ALLOW_RUN_COMMAND` / `AURA_ALLOWED_COMMANDS` actually take
/// effect.
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
mod env_tests {
    use super::*;
    use std::sync::Mutex;

    static ENV_LOCK: Mutex<()> = Mutex::new(());

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

    #[test]
    fn default_policy_allows_find_files_and_delete_file() {
        let policy = PolicyConfig::default();
        assert!(policy.allowed_tools.contains("find_files"));
        assert!(policy.allowed_tools.contains("delete_file"));
    }

    #[test]
    fn default_policy_still_denies_run_command() {
        let policy = PolicyConfig::default();
        assert!(!policy.allowed_tools.contains("run_command"));
    }

    #[test]
    fn env_toggles_behave_correctly() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());

        clear_env();
        let policy = policy_config_from_env();
        assert!(!policy.allowed_tools.contains("run_command"));
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
        assert!(!cfg.allow_shell);

        set_env(&[("AURA_ALLOW_SHELL", "1")]);
        let cfg = tool_config_from_env();
        assert!(cfg.allow_shell);

        for v in ["1", "true", "TRUE", "Yes", "on"] {
            clear_env();
            set_env(&[("AURA_ALLOW_RUN_COMMAND", v)]);
            assert!(allow_run_command_from_env());
        }

        clear_env();
        set_env(&[("AURA_ALLOW_RUN_COMMAND", "1")]);
        let cfg = tool_config_from_env();
        assert!(cfg.enable_commands);
        assert!(cfg.binary_allowlist.is_empty());

        clear_env();
    }
}
