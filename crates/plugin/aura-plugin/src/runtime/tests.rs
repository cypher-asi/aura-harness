use super::*;
use aura_config::{PluginConfig, PluginsTable};
use std::collections::BTreeMap as Map;
use tempfile::TempDir;

fn write_minimal_plugin(home: &Path, id: &str, version: &str, with_skill: bool) {
    let cache = PluginCache::new(home.join("plugins"));
    let dir = cache.version_dir(id, version);
    std::fs::create_dir_all(&dir).unwrap();
    let mut manifest = format!(
        r#"manifest_version = "v1"
id = "{id}"
version = "{version}"
"#
    );
    if with_skill {
        std::fs::create_dir_all(dir.join("skills")).unwrap();
        std::fs::write(dir.join("skills").join("hello.md"), "skill body").unwrap();
        manifest.push_str(
            r#"
[[contributes.skills]]
id = "hello"
path = "./skills/hello.md"
"#,
        );
    }
    std::fs::write(dir.join(".aura-plugin.toml"), manifest).unwrap();
    cache.set_active(id, version).unwrap();
}

fn enabled_config(plugin_id: &str) -> PluginsConfig {
    let mut table: Map<String, PluginConfig> = Map::new();
    table.insert(
        plugin_id.to_string(),
        PluginConfig {
            enabled: true,
            trusted: true,
            version: None,
        },
    );
    PluginsConfig {
        table: PluginsTable(table),
    }
}

#[test]
fn empty_aura_home_yields_empty_runtime() {
    let home = TempDir::new().unwrap();
    let runtime =
        load_enabled_plugins(home.path(), &PluginsConfig::default()).expect("empty home is OK");
    assert!(runtime.enabled.is_empty());
    assert!(runtime.load_failures.is_empty());
    assert!(runtime.skill_roots.is_empty());
    assert!(runtime.is_empty());
}

#[test]
fn enabled_plugin_with_skill_yields_root() {
    let home = TempDir::new().unwrap();
    write_minimal_plugin(home.path(), "hello-plug", "0.1.0", true);
    let runtime =
        load_enabled_plugins(home.path(), &enabled_config("hello-plug")).expect("load ok");
    assert_eq!(runtime.enabled.len(), 1);
    assert_eq!(runtime.enabled[0].id, "hello-plug");
    assert_eq!(runtime.skill_roots.len(), 1);
    assert!(runtime.skill_roots[0].ends_with("skills"));
}

#[test]
fn disabled_plugin_is_skipped() {
    let home = TempDir::new().unwrap();
    write_minimal_plugin(home.path(), "off", "0.1.0", false);
    let runtime = load_enabled_plugins(home.path(), &PluginsConfig::default()).expect("load ok");
    assert!(runtime.enabled.is_empty());
    assert!(runtime.load_failures.is_empty());
}

#[test]
fn enabled_but_untrusted_plugin_is_skipped() {
    let home = TempDir::new().unwrap();
    write_minimal_plugin(home.path(), "untrusted", "0.1.0", false);
    let mut table: Map<String, PluginConfig> = Map::new();
    table.insert(
        "untrusted".into(),
        PluginConfig {
            enabled: true,
            trusted: false,
            version: None,
        },
    );
    let runtime = load_enabled_plugins(
        home.path(),
        &PluginsConfig {
            table: PluginsTable(table),
        },
    )
    .expect("load ok");

    assert!(runtime.enabled.is_empty());
    assert!(runtime.load_failures.is_empty());
}

#[test]
fn malformed_manifest_becomes_load_failure() {
    let home = TempDir::new().unwrap();
    let cache = PluginCache::new(home.path().join("plugins"));
    let dir = cache.version_dir("broken", "0.1.0");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join(".aura-plugin.toml"), "not = valid\nmanifest").unwrap();
    cache.set_active("broken", "0.1.0").unwrap();
    let runtime = load_enabled_plugins(home.path(), &enabled_config("broken")).expect("ok");
    assert!(runtime.enabled.is_empty());
    assert_eq!(runtime.load_failures.len(), 1);
    assert_eq!(runtime.load_failures[0].plugin_id, "broken");
}

#[test]
fn pascal_case_hook_event_parses() {
    assert_eq!(
        parse_hook_event("PreToolUse"),
        Some(aura_plugin_hooks::HookEvent::PreToolUse)
    );
    assert_eq!(
        parse_hook_event("pre_tool_use"),
        Some(aura_plugin_hooks::HookEvent::PreToolUse)
    );
    assert_eq!(parse_hook_event("definitelyNotAnEvent"), None);
}

#[test]
fn config_lookup_matches_qualified_key() {
    let mut table: Map<String, PluginConfig> = Map::new();
    table.insert(
        "myplugin@official".into(),
        PluginConfig {
            enabled: true,
            trusted: true,
            version: None,
        },
    );
    let cfg = PluginsConfig {
        table: PluginsTable(table),
    };
    assert!(plugin_enabled("myplugin", &cfg));
    assert!(!plugin_enabled("other", &cfg));
}
