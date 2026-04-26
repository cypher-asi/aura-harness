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
    std::env::remove_var("ORBIT_URL");
    std::env::remove_var("AURA_STORAGE_URL");
    std::env::remove_var("AURA_NETWORK_URL");
    std::env::remove_var("AURA_OS_SERVER_URL");
    std::env::remove_var("AURA_SERVER_BASE_URL");
    std::env::remove_var("AURA_PROJECT_BASE");
    std::env::remove_var("AURA_NODE_AUTH_TOKEN");
    std::env::remove_var("AURA_NODE_REQUIRE_AUTH");
    std::env::remove_var("AURA_ALLOW_UNRESTRICTED_FULL_ACCESS");
}

#[test]
fn test_default_config() {
    let config = NodeConfig::default();
    let default_data_dir = super::default_data_dir();

    assert_eq!(config.data_dir, default_data_dir);
    assert_eq!(config.bind_addr, "127.0.0.1:8080");
    assert!(!config.sync_writes);
    assert_eq!(config.record_window_size, 50);
    assert_eq!(config.orbit_url, "https://orbit-sfvu.onrender.com");
    assert_eq!(config.aura_storage_url, "https://aura-storage.onrender.com");
    assert_eq!(config.aura_network_url, "https://aura-network.onrender.com");
    assert!(
        config.aura_os_server_url.is_none(),
        "aura-os-server override is additive / opt-in; default must be None so HttpDomainApi falls back to aura_storage_url"
    );
    assert!(config.project_base.is_none());
    assert!(!config.allow_unrestricted_full_access);
}

#[test]
fn test_aura_os_server_url_env() {
    let _lock = ENV_LOCK.lock().unwrap();
    clear_node_env_vars();

    std::env::set_var("AURA_OS_SERVER_URL", "https://os.example.com");
    let config = NodeConfig::from_env();
    assert_eq!(
        config.aura_os_server_url.as_deref(),
        Some("https://os.example.com")
    );

    clear_node_env_vars();
}

#[test]
fn test_aura_server_base_url_legacy_env() {
    let _lock = ENV_LOCK.lock().unwrap();
    clear_node_env_vars();

    std::env::set_var("AURA_SERVER_BASE_URL", "https://legacy-os.example.com");
    let config = NodeConfig::from_env();
    assert_eq!(
        config.aura_os_server_url.as_deref(),
        Some("https://legacy-os.example.com")
    );

    clear_node_env_vars();
}

#[test]
fn test_aura_os_server_url_empty_string_ignored() {
    let _lock = ENV_LOCK.lock().unwrap();
    clear_node_env_vars();

    // Pin a non-loopback bind so the desktop-loopback auto-default
    // doesn't fire and shadow the "empty/whitespace = unset" semantics
    // we're actually testing here.
    std::env::set_var("AURA_LISTEN_ADDR", "0.0.0.0:8080");

    std::env::set_var("AURA_OS_SERVER_URL", "");
    let config = NodeConfig::from_env();
    assert!(config.aura_os_server_url.is_none());

    std::env::set_var("AURA_OS_SERVER_URL", "   ");
    let config = NodeConfig::from_env();
    assert!(
        config.aura_os_server_url.is_none(),
        "whitespace-only should be treated as unset, same as the other URL envs"
    );

    clear_node_env_vars();
}

// === Loopback auto-default for aura_os_server_url ===
//
// The `aura-os-desktop` sidecar spawn path passes
// `AURA_LISTEN_ADDR=127.0.0.1:<port>` but does *not* set
// `AURA_OS_SERVER_URL`. Without an auto-default, every `send_to_agent`
// call returns "runtime hook not wired" because
// `session::helpers::build_kernel` only registers
// `AuraServerAgentHook` when `aura_os_server_url` is `Some(_)`. These
// tests pin the loopback-gated default so that contract can't silently
// regress, and pin the non-loopback skip so swarm pods (which bind
// `0.0.0.0`) keep the historical `None` fallback that
// `HttpDomainApi::specs_tasks_base_url` relies on for direct
// aura-storage posts.

#[test]
fn aura_os_server_url_defaults_to_desktop_loopback_for_127_0_0_1_bind() {
    let _lock = ENV_LOCK.lock().unwrap();
    clear_node_env_vars();

    std::env::set_var("AURA_LISTEN_ADDR", "127.0.0.1:8080");
    let config = NodeConfig::from_env();
    assert_eq!(
        config.aura_os_server_url.as_deref(),
        Some("http://127.0.0.1:19847"),
        "loopback IPv4 bind must wire the desktop-loopback default so send_to_agent's hook registers"
    );

    clear_node_env_vars();
}

#[test]
fn aura_os_server_url_defaults_to_desktop_loopback_for_localhost_bind() {
    let _lock = ENV_LOCK.lock().unwrap();
    clear_node_env_vars();

    std::env::set_var("AURA_LISTEN_ADDR", "localhost:8080");
    let config = NodeConfig::from_env();
    assert_eq!(
        config.aura_os_server_url.as_deref(),
        Some("http://127.0.0.1:19847"),
        "textual `localhost` bind counts as loopback for the auto-default"
    );

    clear_node_env_vars();
}

#[test]
fn aura_os_server_url_defaults_to_desktop_loopback_for_ipv6_bind() {
    let _lock = ENV_LOCK.lock().unwrap();
    clear_node_env_vars();

    std::env::set_var("AURA_LISTEN_ADDR", "[::1]:8080");
    let config = NodeConfig::from_env();
    assert_eq!(
        config.aura_os_server_url.as_deref(),
        Some("http://127.0.0.1:19847"),
        "bracketed IPv6 loopback bind must also wire the auto-default"
    );

    clear_node_env_vars();
}

#[test]
fn aura_os_server_url_default_skipped_for_wildcard_bind() {
    let _lock = ENV_LOCK.lock().unwrap();
    clear_node_env_vars();

    // Wildcard bind == swarm/Docker deployment. The historical
    // `HttpDomainApi` fallback to `aura-storage` direct must stay,
    // otherwise spec/task CRUD inside swarm pods starts timing out
    // against an unreachable 127.0.0.1.
    std::env::set_var("AURA_LISTEN_ADDR", "0.0.0.0:8080");
    let config = NodeConfig::from_env();
    assert!(
        config.aura_os_server_url.is_none(),
        "non-loopback bind must not auto-fill aura_os_server_url; \
         got {:?}",
        config.aura_os_server_url
    );

    clear_node_env_vars();
}

#[test]
fn aura_os_server_url_explicit_env_wins_over_loopback_default() {
    let _lock = ENV_LOCK.lock().unwrap();
    clear_node_env_vars();

    std::env::set_var("AURA_LISTEN_ADDR", "127.0.0.1:8080");
    std::env::set_var("AURA_OS_SERVER_URL", "https://os.example.com");
    let config = NodeConfig::from_env();
    assert_eq!(
        config.aura_os_server_url.as_deref(),
        Some("https://os.example.com"),
        "explicit env var must win over the loopback auto-default"
    );

    clear_node_env_vars();
}

#[test]
fn aura_os_server_url_default_uses_struct_default_bind_when_listen_unset() {
    let _lock = ENV_LOCK.lock().unwrap();
    clear_node_env_vars();

    // No bind override at all. `NodeConfig::default().bind_addr` is
    // `127.0.0.1:8080`, which is loopback, so the auto-default still
    // fires. This is the path the local desktop sidecar actually hits
    // when it runs the harness without any explicit listen-addr env
    // (aura-os-desktop currently does pass AURA_LISTEN_ADDR, but
    // pinning the struct-default behavior protects against future
    // sidecar wiring changes).
    let config = NodeConfig::from_env();
    assert_eq!(
        config.aura_os_server_url.as_deref(),
        Some("http://127.0.0.1:19847")
    );

    clear_node_env_vars();
}

#[test]
fn aura_os_server_url_default_does_not_pin_struct_default() {
    // `NodeConfig::default()` is what hand-built configs / tests use
    // when constructing a `NodeConfig` literal. Auto-defaults belong
    // strictly to `from_env`, otherwise every test that builds a
    // `NodeConfig` by hand would suddenly have a loopback URL pinned
    // that they didn't ask for.
    let config = NodeConfig::default();
    assert!(
        config.aura_os_server_url.is_none(),
        "Default::default() must stay None; loopback auto-default lives only in from_env"
    );
}

#[test]
fn aura_server_base_url_legacy_env_still_wins_over_loopback_default() {
    let _lock = ENV_LOCK.lock().unwrap();
    clear_node_env_vars();

    std::env::set_var("AURA_LISTEN_ADDR", "127.0.0.1:8080");
    std::env::set_var("AURA_SERVER_BASE_URL", "https://legacy-os.example.com");
    let config = NodeConfig::from_env();
    assert_eq!(
        config.aura_os_server_url.as_deref(),
        Some("https://legacy-os.example.com"),
        "legacy AURA_SERVER_BASE_URL must keep priority over the loopback auto-default"
    );

    clear_node_env_vars();
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
    std::env::set_var("ORBIT_URL", "https://orbit.example.com");
    std::env::set_var("AURA_STORAGE_URL", "https://storage.example.com");
    std::env::set_var("AURA_NETWORK_URL", "https://network.example.com");

    let config = NodeConfig::from_env();

    assert_eq!(config.data_dir, PathBuf::from("/opt/aura"));
    assert_eq!(config.bind_addr, "0.0.0.0:4000");
    assert!(config.sync_writes);
    assert_eq!(config.record_window_size, 200);
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
fn run_command_visible_by_default_via_tool_catalog() {
    use aura_tools::{catalog::ToolProfile, ToolCatalog, ToolConfig};

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
    use aura_tools::{catalog::ToolProfile, ToolCatalog, ToolConfig};

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
