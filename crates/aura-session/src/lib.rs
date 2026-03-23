//! Shared session wiring for AURA entry points.
//!
//! Provides common setup utilities used by both the TUI binary and the CLI.

use aura_agent::prompts::default_system_prompt;
use aura_agent::{AgentLoopConfig, KernelToolExecutor};
use aura_core::Identity;
use aura_executor::ExecutorRouter;
use aura_reasoner::{AnthropicProvider, MockProvider, ModelProvider, ToolDefinition};
use aura_store::RocksStore;
use aura_tools::{DefaultToolRegistry, ToolExecutor, ToolRegistry};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tracing::warn;

/// Create a new agent identity with a UUID-based ZNS ID.
#[must_use]
pub fn create_identity(prefix: &str, name: &str) -> Identity {
    let zns_id = format!("0://{prefix}/{}", uuid::Uuid::new_v4());
    Identity::new(&zns_id, name)
}

/// Open or create a `RocksDB` store at the given path.
///
/// # Errors
///
/// Returns error if the store cannot be opened.
pub fn open_store(path: &Path) -> anyhow::Result<Arc<RocksStore>> {
    Ok(Arc::new(RocksStore::open(path, false)?))
}

/// Build the standard tool executor stack (`ExecutorRouter` + default tools).
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

/// Load auth token from `AURA_ROUTER_JWT` env var or the credential store.
#[must_use]
pub fn load_auth_token() -> Option<String> {
    std::env::var("AURA_ROUTER_JWT")
        .ok()
        .or_else(aura_auth::CredentialStore::load_token)
}

/// Result of provider selection.
pub struct ProviderSelection {
    pub provider: Box<dyn ModelProvider>,
    pub name: String,
}

/// Select a model provider based on the provider name.
///
/// Falls back to `MockProvider` when the requested provider cannot be
/// initialised from the environment.
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

/// Create a default `AgentLoopConfig` with the standard system prompt and auth.
#[must_use]
pub fn default_agent_config() -> AgentLoopConfig {
    AgentLoopConfig {
        system_prompt: default_system_prompt(),
        auth_token: load_auth_token(),
        ..AgentLoopConfig::default()
    }
}
