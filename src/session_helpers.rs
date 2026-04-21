use aura_agent::prompts::default_system_prompt;
use aura_agent::AgentLoopConfig;

pub use aura_agent::session_bootstrap::{
    build_executor_router, load_auth_token, open_store, resolve_store_path,
};

#[must_use]
pub fn default_agent_config() -> AgentLoopConfig {
    AgentLoopConfig {
        system_prompt: default_system_prompt(),
        auth_token: load_auth_token(),
        ..AgentLoopConfig::default()
    }
}
