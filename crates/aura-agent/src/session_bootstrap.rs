use aura_kernel::ExecutorRouter;
use aura_reasoner::ToolDefinition;
use aura_store::RocksStore;
use aura_tools::{DefaultToolRegistry, ToolExecutor, ToolRegistry};
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
