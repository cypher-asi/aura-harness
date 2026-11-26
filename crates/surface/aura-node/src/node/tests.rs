use super::listener::{
    format_addr_in_use_error, is_aura_node_image, self_heal_disabled, DISABLE_SELF_HEAL_ENV,
};
use super::*;
use std::io;

#[test]
fn test_node_new() {
    let config = NodeConfig::default();
    let node = Node::new(config.clone());
    assert_eq!(node.config.bind_addr, config.bind_addr);
}

#[test]
fn test_node_with_defaults() {
    let node = Node::with_defaults();
    assert_eq!(node.config.bind_addr, "127.0.0.1:8080");
}

#[test]
fn test_node_custom_config() {
    let config = NodeConfig {
        bind_addr: "0.0.0.0:9090".to_string(),
        sync_writes: true,
        record_window_size: 100,
        ..NodeConfig::default()
    };
    let node = Node::new(config);
    assert_eq!(node.config.bind_addr, "0.0.0.0:9090");
    assert!(node.config.sync_writes);
    assert_eq!(node.config.record_window_size, 100);
}

#[test]
fn test_node_config_propagation() {
    let config = NodeConfig {
        data_dir: std::path::PathBuf::from("/custom/data"),
        ..NodeConfig::default()
    };
    let node = Node::new(config);
    assert_eq!(
        node.config.data_dir,
        std::path::PathBuf::from("/custom/data")
    );
}

#[test]
fn test_create_model_provider_returns_something() {
    let _provider = aura_model::default_provider_from_env().expect("default provider should build");
}

#[test]
fn memory_model_defaults_to_low_cost_anthropic_route() {
    assert_eq!(configured_memory_model(None), DEFAULT_MEMORY_MODEL);
    assert_eq!(configured_memory_model(Some("  ")), DEFAULT_MEMORY_MODEL);
}

#[test]
fn memory_model_override_is_trimmed_and_isolates_fallbacks() {
    let model = configured_memory_model(Some("  aura-claude-sonnet-4-6  "));
    assert_eq!(model, "aura-claude-sonnet-4-6");

    let overrides = memory_provider_overrides(&model);
    assert_eq!(overrides.default_model.as_deref(), Some(model.as_str()));
    assert_eq!(overrides.fallback_model.as_deref(), Some(model.as_str()));
}

/// `AddrInUse` errors must be reformatted into an actionable
/// message that names the conflict, points the operator at the
/// stale-`aura-node` killer command, and offers the
/// `AURA_NODE_BIND_ADDR` escape hatch. Pinned because the
/// recurring "stop didn't release the port" symptom is the whole
/// reason this fail-fast path exists, and a regression that
/// dropped the hint back to the raw `std::io::Error` would put
/// us back in the world where the operator had to find the PID
/// by hand every time.
#[test]
fn addr_in_use_error_includes_killer_and_bind_override() {
    let addr: SocketAddr = "127.0.0.1:8080".parse().expect("valid loopback addr");
    let err = io::Error::new(io::ErrorKind::AddrInUse, "address already in use (mock)");
    let message = format_addr_in_use_error(addr, &err);

    assert!(
        message.contains("127.0.0.1:8080"),
        "must name the conflicting address so the operator can find the right process; got `{message}`"
    );
    assert!(
        message.contains("AURA_NODE_BIND_ADDR"),
        "must surface the bind-override escape hatch; got `{message}`"
    );
    assert!(
        message.contains("aura-node"),
        "must mention `aura-node` so the killer command is recognisable; got `{message}`"
    );

    if cfg!(windows) {
        assert!(
            message.contains("Stop-Process"),
            "Windows hint must use PowerShell `Stop-Process`; got `{message}`"
        );
    } else {
        assert!(
            message.contains("pkill"),
            "non-Windows hint must use `pkill`; got `{message}`"
        );
    }
}

/// The self-heal is allowed to kill any process whose image name
/// starts with `aura-node` — covering both the installed binary
/// (`aura-node.exe`) and cargo's scratch builds
/// (`aura-node-0.1.0-<hash>.exe`) that orphan most often. Pinned
/// because narrowing this predicate (e.g. requiring an exact
/// `aura-node.exe` match) would silently disable self-heal for
/// the most common orphan source and put operators back in the
/// "find PID by hand on every dev-run" loop.
#[test]
fn is_aura_node_image_matches_both_release_and_cargo_builds() {
    assert!(is_aura_node_image("aura-node.exe"));
    assert!(is_aura_node_image("aura-node-0.1.0-23616512-abcdef.exe"));
    assert!(is_aura_node_image("aura-node"));
    assert!(is_aura_node_image("AURA-NODE.EXE"));
    assert!(is_aura_node_image("  aura-node.exe  "));
}

/// Self-heal must NOT kill an unrelated server that happens to
/// own port 8080 (nginx, another dev server the user actually
/// wants, a sibling Aura repo's process from a different build,
/// etc.). Anything that does not start with `aura-node` after
/// case folding falls through to the actionable
/// `format_addr_in_use_error` instead.
#[test]
fn is_aura_node_image_rejects_unrelated_processes() {
    assert!(!is_aura_node_image("nginx.exe"));
    assert!(!is_aura_node_image("node.exe"));
    assert!(!is_aura_node_image("aura-os-server.exe"));
    assert!(!is_aura_node_image("aura-runtime.exe"));
    assert!(!is_aura_node_image(""));
    assert!(!is_aura_node_image("python.exe"));
}

/// The `AURA_NODE_DISABLE_PORT_SELF_HEAL` escape hatch must
/// accept the standard truthy spellings so an operator running
/// two intentional aura-node instances can opt out of the
/// auto-kill without having to remember a single magic value.
/// Pinned because forgetting to handle one of these would silently
/// re-enable self-heal for that operator and surprise-kill their
/// other instance.
#[test]
fn self_heal_disabled_recognises_truthy_env_spellings() {
    let key = DISABLE_SELF_HEAL_ENV;
    let prev = std::env::var(key).ok();
    let with_env = |val: Option<&str>, f: &dyn Fn()| {
        match val {
            Some(v) => std::env::set_var(key, v),
            None => std::env::remove_var(key),
        }
        f();
    };

    with_env(None, &|| assert!(!self_heal_disabled()));
    with_env(Some(""), &|| assert!(!self_heal_disabled()));
    with_env(Some("0"), &|| assert!(!self_heal_disabled()));
    with_env(Some("false"), &|| assert!(!self_heal_disabled()));

    for truthy in ["1", "true", "True", "TRUE", "yes", "on"] {
        with_env(Some(truthy), &|| {
            assert!(
                self_heal_disabled(),
                "`{truthy}` must be recognised as a truthy disable signal"
            );
        });
    }

    match prev {
        Some(v) => std::env::set_var(key, v),
        None => std::env::remove_var(key),
    }
}
