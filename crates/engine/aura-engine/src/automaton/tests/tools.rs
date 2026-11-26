use super::*;

#[test]
fn prepare_installed_tools_filters_by_required_integration() {
    let tools = AutomatonBridge::prepare_installed_tools(
        Some(vec![
            aura_core_protocol::InstalledTool {
                name: "brave_search_web".to_string(),
                description: "Search the web using Brave".to_string(),
                input_schema: serde_json::json!({
                    "type": "object",
                    "properties": { "query": { "type": "string" } },
                    "required": ["query"]
                }),
                endpoint: "https://example.com/brave".to_string(),
                auth: aura_core_protocol::ToolAuth::None,
                timeout_ms: None,
                namespace: None,
                required_integration: Some(
                    aura_core_protocol::InstalledToolIntegrationRequirement {
                        integration_id: None,
                        provider: Some("brave_search".to_string()),
                        kind: Some("workspace_integration".to_string()),
                    },
                ),
                runtime_execution: None,
                metadata: Default::default(),
            },
            aura_core_protocol::InstalledTool {
                name: "list_org_integrations".to_string(),
                description: "List org integrations".to_string(),
                input_schema: serde_json::json!({
                    "type": "object",
                    "properties": {},
                }),
                endpoint: "https://example.com/list".to_string(),
                auth: aura_core_protocol::ToolAuth::None,
                timeout_ms: None,
                namespace: None,
                required_integration: None,
                runtime_execution: None,
                metadata: Default::default(),
            },
        ]),
        &[InstalledIntegrationDefinition {
            integration_id: "brave-1".to_string(),
            name: "Brave Search".to_string(),
            provider: "brave_search".to_string(),
            kind: "workspace_integration".to_string(),
            metadata: Default::default(),
        }],
    );

    let names = tools
        .iter()
        .map(|tool| tool.name.as_str())
        .collect::<Vec<_>>();
    assert!(names.contains(&"brave_search_web"));
    assert!(names.contains(&"list_org_integrations"));

    let filtered = AutomatonBridge::prepare_installed_tools(
        Some(vec![aura_core_protocol::InstalledTool {
            name: "brave_search_web".to_string(),
            description: "Search the web using Brave".to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": { "query": { "type": "string" } },
                "required": ["query"]
            }),
            endpoint: "https://example.com/brave".to_string(),
            auth: aura_core_protocol::ToolAuth::None,
            timeout_ms: None,
            namespace: None,
            required_integration: Some(aura_core_protocol::InstalledToolIntegrationRequirement {
                integration_id: None,
                provider: Some("brave_search".to_string()),
                kind: Some("workspace_integration".to_string()),
            }),
            runtime_execution: None,
            metadata: Default::default(),
        }]),
        &[],
    );

    assert!(filtered.is_empty());
}
