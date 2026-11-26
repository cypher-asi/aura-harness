//! `aura_os_server_url`: its env vars and the desktop loopback default.

use super::super::NodeConfig;
use super::support::{clear_node_env_vars, ENV_LOCK};

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
