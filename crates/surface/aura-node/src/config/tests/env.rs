//! [`NodeConfig::from_env`] parsing, and the tool visibility it leaves to
//! the catalog.

use super::super::NodeConfig;
use super::support::{clear_node_env_vars, ENV_LOCK};
use std::path::PathBuf;

#[test]
fn test_from_env_uses_defaults_when_not_set() {
    let _lock = ENV_LOCK.lock().unwrap();
    clear_node_env_vars();

    let config = NodeConfig::from_env();
    let default = NodeConfig::default();

    assert_eq!(config.data_dir, default.data_dir);
    assert_eq!(config.bind_addr, default.bind_addr);
    assert_eq!(config.sync_writes, default.sync_writes);
}

#[test]
fn test_sync_writes_parsing() {
    let _lock = ENV_LOCK.lock().unwrap();
    clear_node_env_vars();

    // Test "true"
    std::env::set_var("SYNC_WRITES", "true");
    let config = NodeConfig::from_env();
    assert!(config.sync_writes);

    // Test "1"
    std::env::set_var("SYNC_WRITES", "1");
    let config = NodeConfig::from_env();
    assert!(config.sync_writes);

    // Test "false"
    std::env::set_var("SYNC_WRITES", "false");
    let config = NodeConfig::from_env();
    assert!(!config.sync_writes);

    clear_node_env_vars();
}

#[test]
fn test_unrestricted_full_access_env() {
    let _lock = ENV_LOCK.lock().unwrap();
    clear_node_env_vars();

    std::env::set_var("AURA_ALLOW_UNRESTRICTED_FULL_ACCESS", "1");
    let config = NodeConfig::from_env();
    assert!(config.allow_unrestricted_full_access);

    std::env::set_var("AURA_ALLOW_UNRESTRICTED_FULL_ACCESS", "true");
    let config = NodeConfig::from_env();
    assert!(config.allow_unrestricted_full_access);

    std::env::set_var("AURA_ALLOW_UNRESTRICTED_FULL_ACCESS", "false");
    let config = NodeConfig::from_env();
    assert!(!config.allow_unrestricted_full_access);

    clear_node_env_vars();
}

#[test]
fn test_record_window_size_parsing() {
    let _lock = ENV_LOCK.lock().unwrap();
    clear_node_env_vars();

    std::env::set_var("RECORD_WINDOW_SIZE", "100");
    let config = NodeConfig::from_env();
    assert_eq!(config.record_window_size, 100);

    // Invalid value should keep default
    std::env::set_var("RECORD_WINDOW_SIZE", "invalid");
    let config = NodeConfig::from_env();
    assert_eq!(config.record_window_size, 50); // default

    clear_node_env_vars();
}

#[test]
fn test_bind_addr_env() {
    let _lock = ENV_LOCK.lock().unwrap();
    clear_node_env_vars();

    std::env::set_var("BIND_ADDR", "0.0.0.0:3000");
    let config = NodeConfig::from_env();
    assert_eq!(config.bind_addr, "0.0.0.0:3000");

    clear_node_env_vars();
}

#[test]
fn test_full_env_override() {
    let _lock = ENV_LOCK.lock().unwrap();
    clear_node_env_vars();

    std::env::set_var("DATA_DIR", "/opt/aura");
    std::env::set_var("BIND_ADDR", "0.0.0.0:4000");
    std::env::set_var("SYNC_WRITES", "true");
    std::env::set_var("RECORD_WINDOW_SIZE", "200");
    std::env::set_var("AURA_STORAGE_URL", "https://storage.example.com");
    std::env::set_var("AURA_NETWORK_URL", "https://network.example.com");

    let config = NodeConfig::from_env();

    assert_eq!(config.data_dir, PathBuf::from("/opt/aura"));
    assert_eq!(config.bind_addr, "0.0.0.0:4000");
    assert!(config.sync_writes);
    assert_eq!(config.record_window_size, 200);
    assert_eq!(config.aura_storage_url, "https://storage.example.com");
    assert_eq!(config.aura_network_url, "https://network.example.com");

    clear_node_env_vars();
}

#[test]
fn test_project_base_env() {
    let _lock = ENV_LOCK.lock().unwrap();
    clear_node_env_vars();

    std::env::set_var("AURA_PROJECT_BASE", "/home/aura");
    let config = NodeConfig::from_env();
    assert_eq!(config.project_base, Some(PathBuf::from("/home/aura")));

    clear_node_env_vars();
}

#[test]
fn test_project_base_empty_string_ignored() {
    let _lock = ENV_LOCK.lock().unwrap();
    clear_node_env_vars();

    std::env::set_var("AURA_PROJECT_BASE", "");
    let config = NodeConfig::from_env();
    assert!(config.project_base.is_none());

    clear_node_env_vars();
}

#[test]
fn run_command_visible_by_default_via_tool_catalog() {
    use aura_exec_tools::{catalog::ToolProfile, ToolCatalog, ToolConfig};

    let _lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    clear_node_env_vars();

    let _config = NodeConfig::from_env();
    let tool_config = ToolConfig::default();

    let catalog = ToolCatalog::new();
    let visible = catalog.visible_tools(ToolProfile::Core, &tool_config);
    let visible_names: Vec<&str> = visible.iter().map(|t| t.name.as_str()).collect();

    assert!(
        visible_names.contains(&"run_command"),
        "run_command visibility is policy/catalog state; got: {visible_names:?}"
    );

    clear_node_env_vars();
}

#[test]
fn command_execution_guardrail_does_not_hide_run_command() {
    use aura_exec_tools::{catalog::ToolProfile, ToolCatalog, ToolConfig};

    let _lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    clear_node_env_vars();

    let tool_config = ToolConfig::default();
    assert!(!tool_config.command.enabled);

    let catalog = ToolCatalog::new();
    let visible = catalog.visible_tools(ToolProfile::Core, &tool_config);
    let visible_names: Vec<&str> = visible.iter().map(|t| t.name.as_str()).collect();
    assert!(
        visible_names.contains(&"run_command"),
        "execution guardrails must not filter catalog visibility; got: {visible_names:?}"
    );

    clear_node_env_vars();
}
