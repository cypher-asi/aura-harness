//! Session management for the CLI.
//!
//! Manages the agent session using `AgentLoop` for multi-step orchestration.

use aura_agent::{AgentLoop, AgentLoopConfig, AgentLoopResult, KernelToolExecutor};
use aura_core::{AgentId, Identity};
use aura_executor::ExecutorRouter;
use aura_kernel::TurnConfig;
use aura_reasoner::{AnthropicProvider, Message, MockProvider, ModelProvider, ToolDefinition};
use aura_store::RocksStore;
use aura_tools::{DefaultToolRegistry, ToolExecutor, ToolRegistry};
use std::path::PathBuf;
use std::sync::Arc;
use tracing::{debug, info};

// ============================================================================
// Configuration
// ============================================================================

/// Session configuration.
#[derive(Debug, Clone)]
pub struct SessionConfig {
    /// Data directory for storage
    pub data_dir: PathBuf,
    /// Workspace root for agent files
    pub workspace_root: PathBuf,
    /// Model provider to use ("anthropic" or "mock")
    pub provider: String,
    /// Agent name
    pub agent_name: String,
    /// Agent loop configuration
    pub loop_config: AgentLoopConfig,
}

impl SessionConfig {
    /// Load configuration from environment variables.
    #[must_use]
    pub fn from_env() -> Self {
        let data_dir = std::env::var("AURA_DATA_DIR")
            .map_or_else(|_| PathBuf::from("./aura_data"), PathBuf::from);

        let workspace_root = std::env::var("AURA_WORKSPACE_ROOT")
            .map_or_else(|_| data_dir.join("workspaces"), PathBuf::from);

        let provider =
            std::env::var("AURA_MODEL_PROVIDER").unwrap_or_else(|_| "anthropic".to_string());

        let agent_name =
            std::env::var("AURA_AGENT_NAME").unwrap_or_else(|_| "CLI Agent".to_string());

        let mut loop_config = AgentLoopConfig {
            system_prompt: TurnConfig::default().system_prompt,
            ..AgentLoopConfig::default()
        };

        if let Ok(v) = std::env::var("AURA_MAX_STEPS_PER_TURN") {
            if let Ok(n) = v.parse() {
                loop_config.max_iterations = n;
            }
        }
        if let Ok(v) = std::env::var("AURA_ANTHROPIC_MODEL") {
            loop_config.model = v;
        }

        Self {
            data_dir,
            workspace_root,
            provider,
            agent_name,
            loop_config,
        }
    }
}

// ============================================================================
// Session
// ============================================================================

/// An interactive CLI session.
pub struct Session {
    identity: Identity,
    #[allow(dead_code)]
    store: Arc<RocksStore>,
    provider_name: String,
    current_seq: u64,
    agent_loop: AgentLoop,
    provider: Box<dyn ModelProvider>,
    executor: KernelToolExecutor,
    tools: Vec<ToolDefinition>,
    messages: Vec<Message>,
}

impl Session {
    /// Create a new session.
    ///
    /// # Errors
    ///
    /// Returns error if initialization fails.
    pub async fn new(config: SessionConfig) -> anyhow::Result<Self> {
        tokio::fs::create_dir_all(&config.data_dir).await?;
        tokio::fs::create_dir_all(&config.workspace_root).await?;

        let zns_id = format!("0://cli/{}", uuid::Uuid::new_v4());
        let identity = Identity::new(&zns_id, &config.agent_name);
        info!(agent_id = %identity.agent_id, name = %identity.name, "Created identity");

        let store_path = config.data_dir.join("store");
        let store = Arc::new(RocksStore::open(&store_path, false)?);
        debug!(?store_path, "Opened store");

        let mut executor_router = ExecutorRouter::new();
        executor_router.add_executor(std::sync::Arc::new(ToolExecutor::with_defaults()));

        let tool_registry = DefaultToolRegistry::new();
        let tools = tool_registry.list();

        let workspace = config.workspace_root.join(identity.agent_id.to_hex());
        let kernel_executor =
            KernelToolExecutor::new(executor_router, identity.agent_id, workspace);

        let (provider, provider_name): (Box<dyn ModelProvider>, &str) =
            match config.provider.as_str() {
                "mock" => {
                    let p = MockProvider::simple_response(
                        "I'm a mock assistant. Real model integration requires ANTHROPIC_API_KEY.",
                    );
                    (Box::new(p), "mock")
                }
                _ => match AnthropicProvider::from_env() {
                    Ok(p) => (Box::new(p), "anthropic"),
                    Err(e) => {
                        tracing::warn!("Failed to create Anthropic provider: {e}. Using mock.");
                        let p = MockProvider::simple_response(
                            "Mock mode: Set ANTHROPIC_API_KEY to use real model.",
                        );
                        (Box::new(p), "mock (fallback)")
                    }
                },
            };

        let agent_loop = AgentLoop::new(config.loop_config);

        info!(provider = provider_name, "Session initialized");

        Ok(Self {
            identity,
            store,
            provider_name: provider_name.to_string(),
            current_seq: 1,
            agent_loop,
            provider,
            executor: kernel_executor,
            tools,
            messages: Vec::new(),
        })
    }

    /// Get the agent ID.
    #[must_use]
    pub const fn agent_id(&self) -> AgentId {
        self.identity.agent_id
    }

    /// Get the current sequence number.
    #[must_use]
    pub const fn current_seq(&self) -> u64 {
        self.current_seq
    }

    /// Get the provider name.
    #[must_use]
    pub fn provider_name(&self) -> &str {
        &self.provider_name
    }

    /// Submit a prompt to the agent.
    ///
    /// # Errors
    ///
    /// Returns error if processing fails.
    pub async fn submit_prompt(&mut self, text: &str) -> anyhow::Result<AgentLoopResult> {
        self.messages.push(Message::user(text));
        self.current_seq += 1;

        let result = self
            .agent_loop
            .run(
                self.provider.as_ref(),
                &self.executor,
                self.messages.clone(),
                self.tools.clone(),
            )
            .await?;

        self.messages.clone_from(&result.messages);

        Ok(result)
    }

    /// Approve the pending tool request.
    ///
    /// # Errors
    ///
    /// Returns error if no pending request or approval fails.
    #[allow(clippy::unused_self)]
    pub fn approve_pending(&self) -> anyhow::Result<()> {
        anyhow::bail!("No pending approval requests")
    }

    /// Deny the pending tool request.
    ///
    /// # Errors
    ///
    /// Returns error if no pending request.
    #[allow(clippy::unused_self)]
    pub fn deny_pending(&self) -> anyhow::Result<()> {
        anyhow::bail!("No pending approval requests")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    static ENV_LOCK: Mutex<()> = Mutex::new(());

    fn clear_all_env_vars() {
        std::env::remove_var("AURA_DATA_DIR");
        std::env::remove_var("AURA_WORKSPACE_ROOT");
        std::env::remove_var("AURA_MODEL_PROVIDER");
        std::env::remove_var("AURA_AGENT_NAME");
        std::env::remove_var("AURA_MAX_STEPS_PER_TURN");
        std::env::remove_var("AURA_MAX_TOOL_CALLS_PER_STEP");
        std::env::remove_var("AURA_MODEL_TIMEOUT_MS");
        std::env::remove_var("AURA_ANTHROPIC_MODEL");
    }

    #[test]
    fn test_session_config_from_env_defaults() {
        let _lock = ENV_LOCK.lock().unwrap();
        clear_all_env_vars();

        let config = SessionConfig::from_env();

        assert_eq!(config.data_dir, PathBuf::from("./aura_data"));
        assert_eq!(config.provider, "anthropic");
        assert_eq!(config.agent_name, "CLI Agent");
    }

    #[test]
    fn test_session_config_custom_data_dir() {
        let _lock = ENV_LOCK.lock().unwrap();
        clear_all_env_vars();

        std::env::set_var("AURA_DATA_DIR", "/custom/data");

        let config = SessionConfig::from_env();

        assert_eq!(config.data_dir, PathBuf::from("/custom/data"));
        assert_eq!(
            config.workspace_root,
            PathBuf::from("/custom/data/workspaces")
        );

        clear_all_env_vars();
    }

    #[test]
    fn test_session_config_custom_workspace() {
        let _lock = ENV_LOCK.lock().unwrap();
        clear_all_env_vars();

        std::env::set_var("AURA_WORKSPACE_ROOT", "/my/workspaces");

        let config = SessionConfig::from_env();

        assert_eq!(config.workspace_root, PathBuf::from("/my/workspaces"));

        clear_all_env_vars();
    }

    #[test]
    fn test_session_config_provider() {
        let _lock = ENV_LOCK.lock().unwrap();
        clear_all_env_vars();

        std::env::set_var("AURA_MODEL_PROVIDER", "mock");

        let config = SessionConfig::from_env();

        assert_eq!(config.provider, "mock");

        clear_all_env_vars();
    }

    #[test]
    fn test_session_config_agent_name() {
        let _lock = ENV_LOCK.lock().unwrap();
        clear_all_env_vars();

        std::env::set_var("AURA_AGENT_NAME", "Test Agent");

        let config = SessionConfig::from_env();

        assert_eq!(config.agent_name, "Test Agent");

        clear_all_env_vars();
    }

    #[test]
    fn test_session_config_loop_config_overrides() {
        let _lock = ENV_LOCK.lock().unwrap();
        clear_all_env_vars();

        std::env::set_var("AURA_MAX_STEPS_PER_TURN", "20");
        std::env::set_var("AURA_ANTHROPIC_MODEL", "claude-sonnet-4-20250514");

        let config = SessionConfig::from_env();

        assert_eq!(config.loop_config.max_iterations, 20);
        assert_eq!(config.loop_config.model, "claude-sonnet-4-20250514");

        clear_all_env_vars();
    }

    #[test]
    fn test_session_config_invalid_number_uses_default() {
        let _lock = ENV_LOCK.lock().unwrap();
        clear_all_env_vars();

        std::env::set_var("AURA_MAX_STEPS_PER_TURN", "not_a_number");

        let config = SessionConfig::from_env();

        let default_config = AgentLoopConfig::default();
        assert_eq!(
            config.loop_config.max_iterations,
            default_config.max_iterations
        );

        clear_all_env_vars();
    }
}
