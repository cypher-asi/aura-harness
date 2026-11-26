
use super::*;
use aura_core_types::{InstalledToolDefinition, ToolAuth};

#[test]
fn catalog_has_entries() {
    let cat = ToolCatalog::new();
    assert!(cat.static_count() > 0);
}

#[test]
fn core_profile_contains_fs_and_cmd() {
    let cat = ToolCatalog::new();
    let tools = cat.tools_for_profile(ToolProfile::Core);
    let names: HashSet<_> = tools.iter().map(|t| t.name.as_str()).collect();
    assert!(names.contains("read_file"));
    assert!(names.contains("write_file"));
    assert!(names.contains("run_command"));
    assert!(names.contains("search_code"));
}

#[test]
fn agent_profile_includes_core_and_management() {
    let cat = ToolCatalog::new();
    let tools = cat.tools_for_profile(ToolProfile::Agent);
    let names: HashSet<_> = tools.iter().map(|t| t.name.as_str()).collect();
    assert!(names.contains("read_file"), "agent should include core");
    assert!(
        names.contains("list_specs"),
        "agent should include management"
    );
    assert!(
        !names.contains("task_done"),
        "agent should not include engine"
    );
}

#[test]
fn engine_profile_includes_core_and_engine() {
    let cat = ToolCatalog::new();
    let tools = cat.tools_for_profile(ToolProfile::Engine);
    let names: HashSet<_> = tools.iter().map(|t| t.name.as_str()).collect();
    assert!(names.contains("read_file"), "engine should include core");
    assert!(
        names.contains("task_done"),
        "engine should include engine tools"
    );
    assert!(
        !names.contains("list_specs"),
        "engine should not include management"
    );
}

#[test]
fn visible_tools_do_not_apply_tool_config_category_gates() {
    let cat = ToolCatalog::new();
    let config = ToolConfig::default();

    let tools = cat.visible_tools(ToolProfile::Core, &config);
    let names: HashSet<_> = tools.iter().map(|t| t.name.as_str()).collect();
    assert!(names.contains("run_command"));
    assert!(names.contains("read_file"));
}

#[test]
fn owner_of_reports_correctly() {
    let cat = ToolCatalog::new();
    assert_eq!(cat.owner_of("read_file"), Some(ToolOwner::Internal));
    assert_eq!(cat.owner_of("nonexistent"), None);
}

#[test]
fn multi_project_adds_project_id() {
    let cat = ToolCatalog::new();
    let config = ToolConfig::default();
    let tools = cat.visible_tools_multi_project(&config);
    for tool in &tools {
        let has_project_id = tool
            .input_schema
            .get("properties")
            .and_then(|p| p.get("project_id"))
            .is_some();
        assert!(
            has_project_id,
            "multi-project tool '{}' must have project_id",
            tool.name
        );
    }
}

#[test]
fn no_duplicate_names_in_any_profile() {
    let cat = ToolCatalog::new();
    for profile in [ToolProfile::Core, ToolProfile::Agent, ToolProfile::Engine] {
        let tools = cat.tools_for_profile(profile);
        let mut seen = HashSet::new();
        for t in &tools {
            assert!(seen.insert(&t.name), "duplicate: {} in {profile:?}", t.name);
        }
    }
}

#[test]
fn every_builtin_has_catalog_entry() {
    let cat = ToolCatalog::new();
    let core = cat.tools_for_profile(ToolProfile::Core);
    let names: HashSet<_> = core.iter().map(|t| t.name.as_str()).collect();
    for tool in builtin_tools() {
        assert!(
            names.contains(tool.name()),
            "builtin '{}' missing from core profile",
            tool.name()
        );
    }
}

#[test]
fn executor_builtin_tools_preserve_builtin_surface() {
    let cat = ToolCatalog::new();
    let tools = cat.executor_builtin_tools();
    let expected = builtin_tools();

    assert_eq!(tools.len(), expected.len());
    for (definition, tool) in tools.iter().zip(expected) {
        assert_eq!(definition.name, tool.name());
    }
}

#[test]
fn cross_agent_tools_hidden_without_permissions() {
    let cat = ToolCatalog::new();
    let tools =
        cat.visible_tools_with_permissions(ToolProfile::Agent, &ToolConfig::default(), None);
    let names: HashSet<_> = tools.iter().map(|t| t.name.as_str()).collect();
    assert!(!names.contains("spawn_agent"));
    assert!(!names.contains("send_to_agent"));
    assert!(!names.contains("agent_lifecycle"));
    assert!(!names.contains("get_agent_state"));
    assert!(!names.contains("list_agents"));
    assert!(!names.contains("delegate_task"));
    assert!(!names.contains("task"));
}

#[test]
fn cross_agent_tools_hidden_when_capability_missing() {
    let cat = ToolCatalog::new();
    let perms = aura_core_types::AgentPermissions {
        scope: aura_core_types::AgentScope::default(),
        capabilities: vec![aura_core_types::Capability::ReadAgent],
    };
    let tools = cat.visible_tools_with_permissions(
        ToolProfile::Agent,
        &ToolConfig::default(),
        Some(&perms),
    );
    let names: HashSet<_> = tools.iter().map(|t| t.name.as_str()).collect();
    // ReadAgent is held → get_agent_state visible.
    assert!(names.contains("get_agent_state"));
    assert!(!names.contains("list_agents"));
    // ControlAgent / SpawnAgent not held → hidden.
    assert!(!names.contains("spawn_agent"));
    assert!(!names.contains("send_to_agent"));
    assert!(!names.contains("agent_lifecycle"));
    assert!(!names.contains("delegate_task"));
    assert!(!names.contains("task"));
}

#[test]
fn cross_agent_tools_visible_to_ceo_preset() {
    let cat = ToolCatalog::new();
    let perms = aura_core_types::AgentPermissions::ceo_preset();
    let tools = cat.visible_tools_with_permissions(
        ToolProfile::Agent,
        &ToolConfig::default(),
        Some(&perms),
    );
    let names: HashSet<_> = tools.iter().map(|t| t.name.as_str()).collect();
    assert!(names.contains("spawn_agent"));
    assert!(names.contains("send_to_agent"));
    assert!(names.contains("agent_lifecycle"));
    assert!(names.contains("get_agent_state"));
    assert!(names.contains("list_agents"));
    assert!(names.contains("delegate_task"));
    assert!(names.contains("task"));
}

#[test]
fn registry_trait_read_only_view() {
    let mut cat = ToolCatalog::new();

    // Snapshot length matches inherent count.
    let len_via_trait = <ToolCatalog as Registry>::len(&cat);
    assert_eq!(len_via_trait, cat.static_count());
    assert!(!Registry::is_empty(&cat));

    // get() via trait returns a CatalogEntry whose definition matches the name.
    let name = "read_file".to_string();
    let entry = <ToolCatalog as Registry>::get(&cat, &name).expect("read_file must be in catalog");
    assert_eq!(entry.definition.name, name);

    // iter() yields all entries keyed by definition name.
    let pairs = <ToolCatalog as Registry>::iter(&cat);
    assert_eq!(pairs.len(), cat.static_count());
    assert!(pairs.iter().any(|(k, _)| k == "read_file"));

    // register/remove are intentionally unsupported.
    let err = Registry::register(&mut cat, "demo".to_string(), entry.clone())
        .expect_err("register must be unsupported");
    assert!(matches!(err, RegistryError::Unsupported(_)));
    assert!(Registry::remove(&mut cat, &name).is_none());
}

#[test]
fn with_installed_tools_adds_model_visible_tools_for_profile() {
    let cat = ToolCatalog::new();
    let extended = cat.with_installed_tools(
        ToolProfile::Engine,
        &[InstalledToolDefinition {
            name: "brave_search_web".to_string(),
            description: "Search the web using Brave".to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": { "query": { "type": "string" } },
                "required": ["query"]
            }),
            endpoint: "https://example.com/tool".to_string(),
            auth: ToolAuth::None,
            timeout_ms: None,
            namespace: None,
            required_integration: None,
            runtime_execution: None,
            metadata: Default::default(),
        }],
    );

    let tools = extended.tools_for_profile(ToolProfile::Engine);
    let names: HashSet<_> = tools.iter().map(|tool| tool.name.as_str()).collect();
    assert!(names.contains("brave_search_web"));
}
