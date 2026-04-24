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
/// `run_command` is on by default — `enable_commands = true`, empty
/// `binary_allowlist` (= all binaries allowed per the [`ToolConfig`]
/// contract). The remaining env knobs just narrow or widen from there:
///
/// * `AURA_ALLOWED_COMMANDS=cargo,git,ls` — comma-separated binary
///   allowlist. Replaces the "all allowed" default with a narrower set.
/// * `AURA_ALLOW_SHELL=1` — allow `sh`/`bash`/`pwsh` fan-out. Stays
///   opt-in; the shell interpreter is a separate blast-radius concern.
///
/// Callers that want a fully locked-down surface should use
/// [`ToolConfig::default`] directly instead of this helper, or set
/// `AURA_STRICT_MODE=1` on the aura-node side so the kernel policy
/// rejects `run_command` independently of what this executor config
/// permits.
#[must_use]
pub fn tool_config_from_env() -> ToolConfig {
    let mut cfg = ToolConfig {
        enable_commands: true,
        ..ToolConfig::default()
    };
    let allowlist = env_csv("AURA_ALLOWED_COMMANDS");
    if !allowlist.is_empty() {
        cfg.binary_allowlist = allowlist;
    }
    if env_bool("AURA_ALLOW_SHELL") {
        cfg.allow_shell = true;
    }
    cfg
}

/// Build a [`PolicyConfig`] for env-driven embedders (the standalone
/// TUI harness, ad-hoc CLI tests).
///
/// Non-strict mode unconditionally elevates `run_command` to
/// [`PermissionLevel::AlwaysAllow`] so agents spawned through this
/// helper can invoke shell commands without per-call approval.
/// `AURA_STRICT_MODE=1` keeps the fail-closed [`PolicyConfig::default`]
/// and denies `run_command` until the caller wires in an approval
/// pump or a per-agent permission override.
///
/// The executor-layer [`ToolConfig`] from [`tool_config_from_env`]
/// still enforces `binary_allowlist` and `allow_shell` independently.
#[must_use]
pub fn policy_config_from_env() -> PolicyConfig {
    let mut policy = PolicyConfig::default();
    if !env_bool("AURA_STRICT_MODE") {
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
/// `AURA_ALLOWED_COMMANDS` / `AURA_ALLOW_SHELL` actually take effect.
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

    const ENV_KEYS: &[&str] = &["AURA_ALLOWED_COMMANDS", "AURA_ALLOW_SHELL", "AURA_STRICT_MODE"];

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
    fn default_kernel_policy_still_denies_run_command() {
        // The kernel baseline stays fail-closed; only the
        // `policy_config_from_env` / `fetch_agent_permissions_with_default`
        // wrappers unlock `run_command` in non-strict mode.
        let policy = PolicyConfig::default();
        assert!(!policy.allowed_tools.contains("run_command"));
    }

    #[test]
    fn default_env_policy_allows_run_command() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        clear_env();

        let policy = policy_config_from_env();
        assert!(policy.allowed_tools.contains("run_command"));
        assert_eq!(
            policy.tool_permissions.get("run_command"),
            Some(&PermissionLevel::AlwaysAllow)
        );

        clear_env();
    }

    #[test]
    fn strict_mode_env_denies_run_command() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        clear_env();

        set_env(&[("AURA_STRICT_MODE", "1")]);
        let policy = policy_config_from_env();
        assert!(!policy.allowed_tools.contains("run_command"));
        assert!(!policy.tool_permissions.contains_key("run_command"));

        clear_env();
    }

    #[test]
    fn tool_config_defaults_allow_commands_with_narrowable_allowlist() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        clear_env();

        let cfg = tool_config_from_env();
        assert!(cfg.enable_commands);
        assert!(cfg.binary_allowlist.is_empty());
        assert!(!cfg.allow_shell);

        set_env(&[("AURA_ALLOWED_COMMANDS", "cargo, git ,ls")]);
        let cfg = tool_config_from_env();
        assert_eq!(
            cfg.binary_allowlist,
            vec!["cargo".to_string(), "git".to_string(), "ls".to_string()]
        );

        set_env(&[("AURA_ALLOW_SHELL", "1")]);
        let cfg = tool_config_from_env();
        assert!(cfg.allow_shell);

        clear_env();
    }
}
