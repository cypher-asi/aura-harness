use crate::KernelToolExecutor;
use aura_kernel::ExecutorRouter;
use aura_reasoner::{AnthropicProvider, MockProvider, ModelProvider, ToolDefinition};
use aura_store::RocksStore;
use aura_tools::{DefaultToolRegistry, ToolExecutor, ToolRegistry};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tracing::warn;

/// Resolve the canonical store path, migrating from legacy `store/` if needed.
///
/// Canonical path: `{data_dir}/db`. If a legacy `{data_dir}/store` directory
/// exists and the canonical one does not, performs a one-time rename migration.
pub fn resolve_store_path(data_dir: &Path) -> PathBuf {
    let canonical = data_dir.join("db");
    let legacy = data_dir.join("store");

    if canonical.exists() {
        if legacy.exists() {
            tracing::warn!(
                canonical = %canonical.display(),
                legacy = %legacy.display(),
                "Both 'db' and 'store' directories exist. Using canonical 'db' path. \
                 Please manually reconcile or remove the legacy 'store' directory."
            );
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

#[must_use]
pub fn build_tool_executor(
    agent_id: aura_core::AgentId,
    workspace: PathBuf,
) -> (KernelToolExecutor, Vec<ToolDefinition>) {
    let mut executor_router = ExecutorRouter::new();
    executor_router.add_executor(Arc::new(ToolExecutor::with_defaults()));

    let tool_registry = DefaultToolRegistry::new();
    let tools = tool_registry.list();

    let kernel_executor = KernelToolExecutor::new(executor_router, agent_id, workspace);
    (kernel_executor, tools)
}

#[must_use]
pub fn load_auth_token() -> Option<String> {
    std::env::var("AURA_ROUTER_JWT")
        .ok()
        .or_else(aura_auth::CredentialStore::load_token)
}

pub struct ProviderSelection {
    pub provider: Box<dyn ModelProvider>,
    pub name: String,
}

#[must_use]
pub fn select_provider(name: &str) -> ProviderSelection {
    match name {
        "mock" => {
            let p = MockProvider::simple_response(
                "Mock mode: Set AURA_LLM_ROUTING and required credentials to enable real AI responses.",
            );
            ProviderSelection {
                provider: Box::new(p),
                name: "mock".to_string(),
            }
        }
        _ => match AnthropicProvider::from_env() {
            Ok(p) => ProviderSelection {
                provider: Box::new(p),
                name: "anthropic".to_string(),
            },
            Err(e) => {
                warn!(error = %e, "LLM provider not configured, using mock");
                let p = MockProvider::simple_response(
                    "Mock mode: Set AURA_LLM_ROUTING and required credentials to enable real AI responses.",
                );
                ProviderSelection {
                    provider: Box::new(p),
                    name: "mock (fallback)".to_string(),
                }
            }
        },
    }
}
