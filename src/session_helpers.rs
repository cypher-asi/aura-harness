#[path = "session_bootstrap_shared.rs"]
mod session_bootstrap_shared;

use aura_agent::prompts::default_system_prompt;
use aura_agent::AgentLoopConfig;

pub use session_bootstrap_shared::{
    build_tool_executor, load_auth_token, open_store, select_provider,
};

#[must_use]
pub fn default_agent_config() -> AgentLoopConfig {
    AgentLoopConfig {
        system_prompt: default_system_prompt(),
        auth_token: load_auth_token(),
        ..AgentLoopConfig::default()
    }
}
