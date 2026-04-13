//! Session management for the CLI.
//!
//! Manages the agent session using `AgentLoop` for multi-step orchestration.

use aura_agent::{
    prompts::default_system_prompt, AgentLoop, AgentLoopConfig, AgentLoopResult,
    KernelModelGateway, KernelToolGateway,
};
use aura_core::{AgentId, Identity};
use aura_kernel::{ExecutorRouter, Kernel, KernelConfig};
use aura_reasoner::{Message, ModelProvider, ToolDefinition};
use aura_store::{RocksStore, Store};
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
            system_prompt: default_system_prompt(),
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

        loop_config.auth_token = crate::session_helpers::load_auth_token();

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
    store: Arc<RocksStore>,
    provider_name: String,
    current_seq: u64,
    agent_loop: AgentLoop,
    provider: Arc<dyn ModelProvider + Send + Sync>,
    executor_router: ExecutorRouter,
    workspace: PathBuf,
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
        let store = crate::session_helpers::open_store(&store_path)?;
        debug!(?store_path, "Opened store");

        let workspace = config.workspace_root.join(identity.agent_id.to_hex());
        let (executor_router, tools) = crate::session_helpers::build_executor_router();

        let selection = crate::session_helpers::select_provider(&config.provider);
        let provider: Arc<dyn ModelProvider + Send + Sync> = Arc::from(selection.provider);
        let provider_name = selection.name;

        let agent_loop = AgentLoop::new(config.loop_config);

        info!(provider = %provider_name, "Session initialized");

        Ok(Self {
            identity,
            store,
            provider_name,
            current_seq: 1,
            agent_loop,
            provider,
            executor_router,
            workspace,
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

        let kernel = self.build_kernel()?;
        let model_gateway = KernelModelGateway::new(kernel.clone());
        let tool_gateway = KernelToolGateway::new(kernel);

        let result = self
            .agent_loop
            .run(
                &model_gateway,
                &tool_gateway,
                self.messages.clone(),
                self.tools.clone(),
            )
            .await?;

        self.messages.clone_from(&result.messages);

        Ok(result)
    }

    fn build_kernel(&self) -> anyhow::Result<Arc<Kernel>> {
        let config = KernelConfig {
            workspace_base: self.workspace.clone(),
            ..KernelConfig::default()
        };

        let store: Arc<dyn Store> = self.store.clone();
        let kernel = Kernel::new(
            store,
            self.provider.clone(),
            self.executor_router.clone(),
            config,
            self.identity.agent_id,
        )?;

        Ok(Arc::new(kernel))
    }

    /// Update the auth token for subsequent model requests.
    ///
    /// Called after `/login` or `/logout` to apply the new token without
    /// restarting the session.
    pub fn set_auth_token(&mut self, token: Option<String>) {
        self.agent_loop.set_auth_token(token);
    }

    /// TODO: Implement real approve path; currently always bails with "No pending approval requests".
    ///
    /// Approve the pending tool request.
    ///
    /// # Errors
    ///
    /// Returns error if no pending request or approval fails.
    #[allow(clippy::unused_self)]
    pub fn approve_pending(&self) -> anyhow::Result<()> {
        anyhow::bail!("No pending approval requests")
    }

    /// TODO: Implement real deny path; currently always bails with "No pending approval requests".
    ///
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

    fn lock_env() -> std::sync::MutexGuard<'static, ()> {
        ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn clear_all_env_vars() {
        std::env::remove_var("AURA_DATA_DIR");
        std::env::remove_var("AURA_WORKSPACE_ROOT");
        std::env::remove_var("AURA_MODEL_PROVIDER");
        std::env::remove_var("AURA_AGENT_NAME");
        std::env::remove_var("AURA_MAX_STEPS_PER_TURN");
        std::env::remove_var("AURA_MAX_TOOL_CALLS_PER_STEP");
        std::env::remove_var("AURA_MODEL_TIMEOUT_MS");
        std::env::remove_var("AURA_ANTHROPIC_MODEL");
        std::env::remove_var("AURA_ROUTER_JWT");
    }

    #[test]
    fn test_session_config_from_env_defaults() {
        let _lock = lock_env();
        clear_all_env_vars();

        let config = SessionConfig::from_env();

        assert_eq!(config.data_dir, PathBuf::from("./aura_data"));
        assert_eq!(config.provider, "anthropic");
        assert_eq!(config.agent_name, "CLI Agent");
    }

    #[test]
    fn test_session_config_custom_data_dir() {
        let _lock = lock_env();
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
        let _lock = lock_env();
        clear_all_env_vars();

        std::env::set_var("AURA_WORKSPACE_ROOT", "/my/workspaces");

        let config = SessionConfig::from_env();

        assert_eq!(config.workspace_root, PathBuf::from("/my/workspaces"));

        clear_all_env_vars();
    }

    #[test]
    fn test_session_config_provider() {
        let _lock = lock_env();
        clear_all_env_vars();

        std::env::set_var("AURA_MODEL_PROVIDER", "mock");

        let config = SessionConfig::from_env();

        assert_eq!(config.provider, "mock");

        clear_all_env_vars();
    }

    #[test]
    fn test_session_config_agent_name() {
        let _lock = lock_env();
        clear_all_env_vars();

        std::env::set_var("AURA_AGENT_NAME", "Test Agent");

        let config = SessionConfig::from_env();

        assert_eq!(config.agent_name, "Test Agent");

        clear_all_env_vars();
    }

    #[test]
    fn test_session_config_loop_config_overrides() {
        let _lock = lock_env();
        clear_all_env_vars();

        std::env::set_var("AURA_MAX_STEPS_PER_TURN", "20");
        std::env::set_var("AURA_ANTHROPIC_MODEL", aura_agent::DEFAULT_MODEL);

        let config = SessionConfig::from_env();

        assert_eq!(config.loop_config.max_iterations, 20);
        assert_eq!(config.loop_config.model, aura_agent::DEFAULT_MODEL);

        clear_all_env_vars();
    }

    #[test]
    fn test_session_config_invalid_number_uses_default() {
        let _lock = lock_env();
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

    #[test]
    fn test_session_config_jwt_token() {
        let _lock = lock_env();
        clear_all_env_vars();

        std::env::set_var("AURA_ROUTER_JWT", "my-secret-jwt");

        let config = SessionConfig::from_env();
        assert_eq!(
            config.loop_config.auth_token.as_deref(),
            Some("my-secret-jwt")
        );

        clear_all_env_vars();
    }

    #[test]
    fn test_session_config_no_jwt_by_default() {
        let _lock = lock_env();
        clear_all_env_vars();

        let config = SessionConfig::from_env();
        // Without AURA_ROUTER_JWT the token falls back to the credential store.
        let stored = aura_auth::CredentialStore::load_token();
        assert_eq!(config.loop_config.auth_token, stored);

        clear_all_env_vars();
    }

    #[test]
    fn test_session_config_workspace_inherits_data_dir() {
        let _lock = lock_env();
        clear_all_env_vars();

        std::env::set_var("AURA_DATA_DIR", "/mydata");

        let config = SessionConfig::from_env();
        assert_eq!(config.workspace_root, PathBuf::from("/mydata/workspaces"));

        clear_all_env_vars();
    }

    #[test]
    fn test_session_config_workspace_override_independent() {
        let _lock = lock_env();
        clear_all_env_vars();

        std::env::set_var("AURA_DATA_DIR", "/mydata");
        std::env::set_var("AURA_WORKSPACE_ROOT", "/other/ws");

        let config = SessionConfig::from_env();
        assert_eq!(config.data_dir, PathBuf::from("/mydata"));
        assert_eq!(config.workspace_root, PathBuf::from("/other/ws"));

        clear_all_env_vars();
    }
}
