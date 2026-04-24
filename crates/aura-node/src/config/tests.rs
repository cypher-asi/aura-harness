use super::*;
use std::sync::Mutex;

// Mutex to serialize env var tests (env vars are process-global)
static ENV_LOCK: Mutex<()> = Mutex::new(());

fn clear_node_env_vars() {
    std::env::remove_var("DATA_DIR");
    std::env::remove_var("AURA_DATA_DIR");
    std::env::remove_var("BIND_ADDR");
    std::env::remove_var("AURA_LISTEN_ADDR");
    std::env::remove_var("SYNC_WRITES");
    std::env::remove_var("RECORD_WINDOW_SIZE");
    std::env::remove_var("ENABLE_FS_TOOLS");
    std::env::remove_var("ENABLE_CMD_TOOLS");
    std::env::remove_var("ALLOWED_COMMANDS");
    std::env::remove_var("ORBIT_URL");
    std::env::remove_var("AURA_STORAGE_URL");
    std::env::remove_var("AURA_NETWORK_URL");
    std::env::remove_var("AURA_PROJECT_BASE");
    std::env::remove_var("AURA_NODE_AUTH_TOKEN");
    std::env::remove_var("AURA_ALLOWED_COMMANDS");
    std::env::remove_var("AURA_ALLOW_SHELL");
    std::env::remove_var("AURA_STRICT_MODE");
    std::env::remove_var("AURA_NODE_REQUIRE_AUTH");
}

#[test]
fn test_default_config() {
    let config = NodeConfig::default();
    let default_data_dir = super::default_data_dir();

    assert_eq!(config.data_dir, default_data_dir);
    assert_eq!(config.bind_addr, "127.0.0.1:8080");
    assert!(!config.sync_writes);
    assert_eq!(config.record_window_size, 50);
    assert!(config.enable_fs_tools);
    assert!(
        config.enable_cmd_tools,
        "cmd tools must default to on; `run_command` is the baseline for aura-os deployments"
    );
    assert!(config.allowed_commands.is_empty());
    assert!(!config.allow_shell);
    assert_eq!(config.orbit_url, "https://orbit-sfvu.onrender.com");
    assert_eq!(config.aura_storage_url, "https://aura-storage.onrender.com");
    assert_eq!(config.aura_network_url, "https://aura-network.onrender.com");
    assert!(config.project_base.is_none());
}

#[test]
fn test_db_path() {
    let config = NodeConfig::default();
    assert_eq!(config.db_path(), super::default_data_dir().join("db"));
}

#[test]
fn test_workspaces_path() {
    let config = NodeConfig::default();
    assert_eq!(
        config.workspaces_path(),
        super::default_data_dir().join("workspaces")
    );
}

#[test]
fn test_custom_data_dir() {
    let config = NodeConfig {
        data_dir: PathBuf::from("/custom/path"),
        ..NodeConfig::default()
    };

    assert_eq!(config.db_path(), PathBuf::from("/custom/path/db"));
    assert_eq!(
        config.workspaces_path(),
        PathBuf::from("/custom/path/workspaces")
    );
}

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
fn test_enable_fs_tools_parsing() {
    let _lock = ENV_LOCK.lock().unwrap();
    clear_node_env_vars();

    // Test disabling with "false"
    std::env::set_var("ENABLE_FS_TOOLS", "false");
    let config = NodeConfig::from_env();
    assert!(!config.enable_fs_tools);

    // Test disabling with "0"
    std::env::set_var("ENABLE_FS_TOOLS", "0");
    let config = NodeConfig::from_env();
    assert!(!config.enable_fs_tools);

    // Test keeping enabled with any other value
    std::env::set_var("ENABLE_FS_TOOLS", "yes");
    let config = NodeConfig::from_env();
    assert!(config.enable_fs_tools);

    clear_node_env_vars();
}

#[test]
fn test_allowed_commands_parsing() {
    let _lock = ENV_LOCK.lock().unwrap();
    clear_node_env_vars();

    std::env::set_var("ALLOWED_COMMANDS", "ls,cat,echo");
    let config = NodeConfig::from_env();
    assert_eq!(config.allowed_commands, vec!["ls", "cat", "echo"]);

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
fn test_enable_cmd_tools_parsing() {
    // `enable_cmd_tools` defaults to `true` now, so `ENABLE_CMD_TOOLS`
    // is effectively a runtime kill-switch: setting it to `false` /
    // `0` force-disables the command tool catalog for callers who want
    // a narrower surface without going full `AURA_STRICT_MODE=1`.
    let _lock = ENV_LOCK.lock().unwrap();
    clear_node_env_vars();

    std::env::set_var("ENABLE_CMD_TOOLS", "true");
    let config = NodeConfig::from_env();
    assert!(config.enable_cmd_tools);

    std::env::set_var("ENABLE_CMD_TOOLS", "1");
    let config = NodeConfig::from_env();
    assert!(config.enable_cmd_tools);

    std::env::set_var("ENABLE_CMD_TOOLS", "false");
    let config = NodeConfig::from_env();
    assert!(!config.enable_cmd_tools);

    std::env::set_var("ENABLE_CMD_TOOLS", "anything_else");
    let config = NodeConfig::from_env();
    assert!(
        !config.enable_cmd_tools,
        "unrecognized values must be treated as an explicit opt-out, matching the pre-existing semantics"
    );

    clear_node_env_vars();
}

#[test]
fn test_allowed_commands_empty_string() {
    let _lock = ENV_LOCK.lock().unwrap();
    clear_node_env_vars();

    std::env::set_var("ALLOWED_COMMANDS", "");
    let config = NodeConfig::from_env();
    assert_eq!(config.allowed_commands, vec![""]);

    clear_node_env_vars();
}

#[test]
fn test_allowed_commands_single_command() {
    let _lock = ENV_LOCK.lock().unwrap();
    clear_node_env_vars();

    std::env::set_var("ALLOWED_COMMANDS", "cargo");
    let config = NodeConfig::from_env();
    assert_eq!(config.allowed_commands, vec!["cargo"]);

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
    std::env::set_var("ENABLE_FS_TOOLS", "false");
    std::env::set_var("ENABLE_CMD_TOOLS", "true");
    std::env::set_var("ALLOWED_COMMANDS", "git,cargo,npm");
    std::env::set_var("ORBIT_URL", "https://orbit.example.com");
    std::env::set_var("AURA_STORAGE_URL", "https://storage.example.com");
    std::env::set_var("AURA_NETWORK_URL", "https://network.example.com");

    let config = NodeConfig::from_env();

    assert_eq!(config.data_dir, PathBuf::from("/opt/aura"));
    assert_eq!(config.bind_addr, "0.0.0.0:4000");
    assert!(config.sync_writes);
    assert_eq!(config.record_window_size, 200);
    assert!(!config.enable_fs_tools);
    assert!(config.enable_cmd_tools);
    assert_eq!(config.allowed_commands, vec!["git", "cargo", "npm"]);
    assert_eq!(config.orbit_url, "https://orbit.example.com");
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
fn test_resolve_project_path_with_base() {
    let config = NodeConfig {
        project_base: Some(PathBuf::from("/home/aura")),
        ..NodeConfig::default()
    };
    let incoming = std::path::Path::new("/state/workspaces/my-app");
    assert_eq!(
        config.resolve_project_path(incoming),
        PathBuf::from("/home/aura/my-app")
    );
}

#[test]
fn test_resolve_project_path_without_base() {
    let config = NodeConfig::default();
    let incoming = std::path::Path::new("/state/workspaces/my-app");
    assert_eq!(
        config.resolve_project_path(incoming),
        PathBuf::from("/state/workspaces/my-app")
    );
}

#[test]
fn test_allowed_commands_and_shell_envs_still_apply() {
    // Fine-grained runtime knobs survive the removal of the master
    // switches: operators who want a narrow allowlist or the shell
    // fan-out interpreter can still opt in.
    let _lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    clear_node_env_vars();

    std::env::set_var("AURA_ALLOWED_COMMANDS", "cargo, git ,ls");
    std::env::set_var("AURA_ALLOW_SHELL", "1");
    let config = NodeConfig::from_env();
    assert!(config.enable_cmd_tools);
    assert_eq!(config.allowed_commands, vec!["cargo", "git", "ls"]);
    assert!(config.allow_shell);

    clear_node_env_vars();
}

/// End-to-end verification that a bare `NodeConfig::from_env()` (no
/// env flags set) is already permissive enough to expose `run_command`
/// in the executor catalog. This is the regression gate for the
/// "Tool 'run_command' is not allowed" failure mode that used to
/// require `AURA_AUTONOMOUS_DEV_LOOP=1` / `AURA_ALLOW_RUN_COMMAND=1`
/// on the sidecar.
#[test]
fn run_command_exposed_by_default_via_tool_catalog() {
    use aura_tools::{catalog::ToolProfile, ToolCatalog, ToolConfig};

    let _lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    clear_node_env_vars();

    let config = NodeConfig::from_env();

    // Mirror the `ToolConfig` literal in `crate::node::Node::run`
    // (crates/aura-node/src/node.rs ~L86). Keep this in sync —
    // divergence is the failure mode this test is gating.
    let tool_config = ToolConfig {
        enable_fs: config.enable_fs_tools,
        enable_commands: config.enable_cmd_tools,
        command_allowlist: config.allowed_commands.clone(),
        allow_shell: config.allow_shell,
        ..Default::default()
    };

    assert!(tool_config.enable_commands, "commands must be enabled by default");

    let catalog = ToolCatalog::new();
    let visible = catalog.visible_tools(ToolProfile::Core, &tool_config);
    let visible_names: Vec<&str> = visible.iter().map(|t| t.name.as_str()).collect();

    assert!(
        visible_names.contains(&"run_command"),
        "run_command must be visible by default; got: {visible_names:?}"
    );

    clear_node_env_vars();
}

/// Inverse: `ENABLE_CMD_TOOLS=false` explicitly force-disables the
/// command tool catalog without needing `AURA_STRICT_MODE=1`.
#[test]
fn run_command_hidden_when_enable_cmd_tools_force_disabled() {
    use aura_tools::{catalog::ToolProfile, ToolCatalog, ToolConfig};

    let _lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    clear_node_env_vars();

    std::env::set_var("ENABLE_CMD_TOOLS", "false");
    let config = NodeConfig::from_env();
    let tool_config = ToolConfig {
        enable_fs: config.enable_fs_tools,
        enable_commands: config.enable_cmd_tools,
        command_allowlist: config.allowed_commands.clone(),
        allow_shell: config.allow_shell,
        ..Default::default()
    };

    assert!(!tool_config.enable_commands);

    let catalog = ToolCatalog::new();
    let visible = catalog.visible_tools(ToolProfile::Core, &tool_config);
    let visible_names: Vec<&str> = visible.iter().map(|t| t.name.as_str()).collect();
    assert!(
        !visible_names.contains(&"run_command"),
        "run_command must be hidden when ENABLE_CMD_TOOLS=false; got: {visible_names:?}"
    );

    clear_node_env_vars();
}

#[test]
fn test_resolve_project_path_local_absolute() {
    let config = NodeConfig {
        project_base: Some(PathBuf::from("/home/aura")),
        ..NodeConfig::default()
    };
    let incoming = std::path::Path::new("/some/deep/nested/cool-project");
    assert_eq!(
        config.resolve_project_path(incoming),
        PathBuf::from("/home/aura/cool-project")
    );
}
