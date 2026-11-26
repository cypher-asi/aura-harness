//! Shared fixtures for the config tests: the env lock and the env reset.

use std::sync::Mutex;

// Mutex to serialize env var tests (env vars are process-global)
pub(super) static ENV_LOCK: Mutex<()> = Mutex::new(());

pub(super) fn clear_node_env_vars() {
    std::env::remove_var("DATA_DIR");
    std::env::remove_var("AURA_DATA_DIR");
    std::env::remove_var("BIND_ADDR");
    std::env::remove_var("AURA_LISTEN_ADDR");
    std::env::remove_var("SYNC_WRITES");
    std::env::remove_var("RECORD_WINDOW_SIZE");
    std::env::remove_var("AURA_STORAGE_URL");
    std::env::remove_var("AURA_NETWORK_URL");
    std::env::remove_var("AURA_OS_SERVER_URL");
    std::env::remove_var("AURA_SERVER_BASE_URL");
    std::env::remove_var("AURA_PROJECT_BASE");
    std::env::remove_var("AURA_NODE_AUTH_TOKEN");
    std::env::remove_var("AURA_NODE_REQUIRE_AUTH");
    std::env::remove_var("AURA_ALLOW_UNRESTRICTED_FULL_ACCESS");
}
