//! Session management for the CLI.
//!
//! Manages the agent session, including the turn processor and state.

use aura_core::{AgentId, Identity, Transaction};
use aura_executor::ExecutorRouter;
use aura_kernel::{TurnConfig, TurnProcessor, TurnResult};
use aura_reasoner::{AnthropicProvider, MockProvider};
use aura_store::RocksStore;
use aura_tools::{DefaultToolRegistry, ToolExecutor};
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
    /// Turn processor config
    pub turn_config: TurnConfig,
}

impl SessionConfig {
    /// Load configuration from environment variables.
    ///
    /// # Errors
    ///
    /// Returns error if required configuration is missing.
    pub fn from_env() -> anyhow::Result<Self> {
        let data_dir = std::env::var("AURA_DATA_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|_| PathBuf::from("./aura_data"));

        let workspace_root = std::env::var("AURA_WORKSPACE_ROOT")
            .map(PathBuf::from)
            .unwrap_or_else(|_| data_dir.join("workspaces"));

        let provider =
            std::env::var("AURA_MODEL_PROVIDER").unwrap_or_else(|_| "anthropic".to_string());

        let agent_name =
            std::env::var("AURA_AGENT_NAME").unwrap_or_else(|_| "CLI Agent".to_string());

        let mut turn_config = TurnConfig::default();
        turn_config.workspace_base = workspace_root.clone();

        // Override turn config from env
        if let Ok(v) = std::env::var("AURA_MAX_STEPS_PER_TURN") {
            if let Ok(n) = v.parse() {
                turn_config.max_steps = n;
            }
        }
        if let Ok(v) = std::env::var("AURA_MAX_TOOL_CALLS_PER_STEP") {
            if let Ok(n) = v.parse() {
                turn_config.max_tool_calls_per_step = n;
            }
        }
        if let Ok(v) = std::env::var("AURA_MODEL_TIMEOUT_MS") {
            if let Ok(n) = v.parse() {
                turn_config.model_timeout_ms = n;
            }
        }
        if let Ok(v) = std::env::var("AURA_ANTHROPIC_MODEL") {
            turn_config.model = v;
        }

        Ok(Self {
            data_dir,
            workspace_root,
            provider,
            agent_name,
            turn_config,
        })
    }
}

// ============================================================================
// Session
// ============================================================================

/// An interactive CLI session.
pub struct Session {
    identity: Identity,
    /// Store for persistence (reserved for state persistence)
    #[allow(dead_code)]
    store: Arc<RocksStore>,
    provider_name: String,
    current_seq: u64,
    // We need to box the processor since it's generic
    processor: SessionProcessor,
}

/// Boxed processor to handle different provider types.
enum SessionProcessor {
    Anthropic(Box<TurnProcessor<AnthropicProvider, RocksStore, DefaultToolRegistry>>),
    Mock(Box<TurnProcessor<MockProvider, RocksStore, DefaultToolRegistry>>),
}

impl Session {
    /// Create a new session.
    ///
    /// # Errors
    ///
    /// Returns error if initialization fails.
    pub async fn new(config: SessionConfig) -> anyhow::Result<Self> {
        // Ensure directories exist
        tokio::fs::create_dir_all(&config.data_dir).await?;
        tokio::fs::create_dir_all(&config.workspace_root).await?;

        // Create identity
        let zns_id = format!("0://cli/{}", uuid::Uuid::new_v4());
        let identity = Identity::new(&zns_id, &config.agent_name);
        info!(agent_id = %identity.agent_id, name = %identity.name, "Created identity");

        // Open store
        let store_path = config.data_dir.join("store");
        let store = Arc::new(RocksStore::open(&store_path, false)?);
        debug!(?store_path, "Opened store");

        // Create executor with tool support
        let mut executor = ExecutorRouter::new();
        executor.add_executor(std::sync::Arc::new(ToolExecutor::with_defaults()));

        // Create tool registry
        let tool_registry = Arc::new(DefaultToolRegistry::new());

        // Create provider and processor
        let (processor, provider_name) = match config.provider.as_str() {
            "mock" => {
                let provider = Arc::new(MockProvider::simple_response(
                    "I'm a mock assistant. Real model integration requires ANTHROPIC_API_KEY.",
                ));
                let processor = TurnProcessor::new(
                    provider,
                    store.clone(),
                    executor,
                    tool_registry,
                    config.turn_config.clone(),
                );
                (SessionProcessor::Mock(Box::new(processor)), "mock")
            }
            "anthropic" | _ => {
                // Try to create Anthropic provider, fall back to mock
                match AnthropicProvider::from_env() {
                    Ok(provider) => {
                        let provider = Arc::new(provider);
                        let processor = TurnProcessor::new(
                            provider,
                            store.clone(),
                            executor,
                            tool_registry,
                            config.turn_config.clone(),
                        );
                        (
                            SessionProcessor::Anthropic(Box::new(processor)),
                            "anthropic",
                        )
                    }
                    Err(e) => {
                        tracing::warn!("Failed to create Anthropic provider: {}. Using mock.", e);
                        let provider = Arc::new(MockProvider::simple_response(
                            "Mock mode: Set ANTHROPIC_API_KEY to use real model.",
                        ));
                        let processor = TurnProcessor::new(
                            provider,
                            store.clone(),
                            executor,
                            tool_registry,
                            config.turn_config.clone(),
                        );
                        (
                            SessionProcessor::Mock(Box::new(processor)),
                            "mock (fallback)",
                        )
                    }
                }
            }
        };

        info!(provider = provider_name, "Session initialized");

        Ok(Self {
            identity,
            store,
            provider_name: provider_name.to_string(),
            current_seq: 1,
            processor,
        })
    }

    /// Get the agent ID.
    #[must_use]
    pub fn agent_id(&self) -> AgentId {
        self.identity.agent_id
    }

    /// Get the current sequence number.
    #[must_use]
    pub fn current_seq(&self) -> u64 {
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
    pub async fn submit_prompt(&mut self, text: &str) -> anyhow::Result<TurnResult> {
        let tx = Transaction::user_prompt(self.identity.agent_id, text.to_string());
        let seq = self.current_seq;
        self.current_seq += 1;

        let result = match &self.processor {
            SessionProcessor::Anthropic(p) => {
                p.process_turn(self.identity.agent_id, tx, seq).await?
            }
            SessionProcessor::Mock(p) => p.process_turn(self.identity.agent_id, tx, seq).await?,
        };

        Ok(result)
    }

    /// Approve the pending tool request.
    ///
    /// # Errors
    ///
    /// Returns error if no pending request or approval fails.
    pub async fn approve_pending(&mut self) -> anyhow::Result<()> {
        // TODO: Implement approval queue
        anyhow::bail!("No pending approval requests")
    }

    /// Deny the pending tool request.
    ///
    /// # Errors
    ///
    /// Returns error if no pending request.
    pub async fn deny_pending(&mut self) -> anyhow::Result<()> {
        // TODO: Implement approval queue
        anyhow::bail!("No pending approval requests")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    // Mutex to serialize env var tests (env vars are process-global)
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

        let config = SessionConfig::from_env().unwrap();

        assert_eq!(config.data_dir, PathBuf::from("./aura_data"));
        assert_eq!(config.provider, "anthropic");
        assert_eq!(config.agent_name, "CLI Agent");
    }

    #[test]
    fn test_session_config_custom_data_dir() {
        let _lock = ENV_LOCK.lock().unwrap();
        clear_all_env_vars();
        
        std::env::set_var("AURA_DATA_DIR", "/custom/data");
        
        let config = SessionConfig::from_env().unwrap();
        
        assert_eq!(config.data_dir, PathBuf::from("/custom/data"));
        // Workspace root should be relative to data dir when not set
        assert_eq!(config.workspace_root, PathBuf::from("/custom/data/workspaces"));

        clear_all_env_vars();
    }

    #[test]
    fn test_session_config_custom_workspace() {
        let _lock = ENV_LOCK.lock().unwrap();
        clear_all_env_vars();
        
        std::env::set_var("AURA_WORKSPACE_ROOT", "/my/workspaces");
        
        let config = SessionConfig::from_env().unwrap();
        
        assert_eq!(config.workspace_root, PathBuf::from("/my/workspaces"));

        clear_all_env_vars();
    }

    #[test]
    fn test_session_config_provider() {
        let _lock = ENV_LOCK.lock().unwrap();
        clear_all_env_vars();
        
        std::env::set_var("AURA_MODEL_PROVIDER", "mock");
        
        let config = SessionConfig::from_env().unwrap();
        
        assert_eq!(config.provider, "mock");

        clear_all_env_vars();
    }

    #[test]
    fn test_session_config_agent_name() {
        let _lock = ENV_LOCK.lock().unwrap();
        clear_all_env_vars();
        
        std::env::set_var("AURA_AGENT_NAME", "Test Agent");
        
        let config = SessionConfig::from_env().unwrap();
        
        assert_eq!(config.agent_name, "Test Agent");

        clear_all_env_vars();
    }

    #[test]
    fn test_session_config_turn_config_overrides() {
        let _lock = ENV_LOCK.lock().unwrap();
        clear_all_env_vars();
        
        std::env::set_var("AURA_MAX_STEPS_PER_TURN", "20");
        std::env::set_var("AURA_MAX_TOOL_CALLS_PER_STEP", "5");
        std::env::set_var("AURA_MODEL_TIMEOUT_MS", "60000");
        std::env::set_var("AURA_ANTHROPIC_MODEL", "claude-sonnet-4-20250514");
        
        let config = SessionConfig::from_env().unwrap();
        
        assert_eq!(config.turn_config.max_steps, 20);
        assert_eq!(config.turn_config.max_tool_calls_per_step, 5);
        assert_eq!(config.turn_config.model_timeout_ms, 60000);
        assert_eq!(config.turn_config.model, "claude-sonnet-4-20250514");

        clear_all_env_vars();
    }

    #[test]
    fn test_session_config_invalid_number_uses_default() {
        let _lock = ENV_LOCK.lock().unwrap();
        clear_all_env_vars();
        
        std::env::set_var("AURA_MAX_STEPS_PER_TURN", "not_a_number");
        
        let config = SessionConfig::from_env().unwrap();
        
        // Should use default value since parsing failed
        let default_config = TurnConfig::default();
        assert_eq!(config.turn_config.max_steps, default_config.max_steps);

        clear_all_env_vars();
    }
}
