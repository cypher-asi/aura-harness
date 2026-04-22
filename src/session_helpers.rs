//! Thin re-export layer over [`aura_agent::session_bootstrap`].
//!
//! Phase 3 consolidated every non-TUI-specific helper into the library
//! crate so `aura-node`, the TUI harness, and any future embedder read
//! the same env-var / policy / executor wiring. This file used to own
//! ~125 lines of that code; it now just re-exports the canonical
//! versions. New helpers should land in
//! [`aura_agent::session_bootstrap`] directly rather than here.

#[allow(unused_imports)]
pub use aura_agent::session_bootstrap::{
    allow_run_command_from_env, build_executor_router_with_config, default_agent_config,
    load_auth_token, open_store, policy_config_from_env, resolve_store_path, tool_config_from_env,
};
