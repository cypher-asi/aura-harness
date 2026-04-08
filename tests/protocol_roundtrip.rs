//! Round-trip tests asserting serde compatibility between
//! aura-core and aura-protocol duplicate types.

use std::collections::HashMap;

/// ToolAuth: serialize from aura-core, deserialize as aura-protocol, and vice versa.
#[test]
fn tool_auth_roundtrip_core_to_protocol() {
    let variants: Vec<aura_core::ToolAuth> = vec![
        aura_core::ToolAuth::None,
        aura_core::ToolAuth::Bearer {
            token: "sk-test-123".into(),
        },
        aura_core::ToolAuth::ApiKey {
            header: "X-Api-Key".into(),
            key: "key-456".into(),
        },
        aura_core::ToolAuth::Headers {
            headers: {
                let mut m = HashMap::new();
                m.insert("Authorization".into(), "Bearer tok".into());
                m.insert("X-Custom".into(), "val".into());
                m
            },
        },
    ];

    for core_val in &variants {
        let json = serde_json::to_string(core_val).expect("serialize core ToolAuth");
        let proto_val: aura_protocol::ToolAuth =
            serde_json::from_str(&json).expect("deserialize as protocol ToolAuth");
        let back_json = serde_json::to_string(&proto_val).expect("re-serialize protocol ToolAuth");
        let roundtrip: aura_core::ToolAuth =
            serde_json::from_str(&back_json).expect("deserialize back to core ToolAuth");
        assert_eq!(core_val, &roundtrip, "ToolAuth roundtrip failed for {json}");
    }
}

#[test]
fn tool_auth_roundtrip_protocol_to_core() {
    let variants: Vec<aura_protocol::ToolAuth> = vec![
        aura_protocol::ToolAuth::None,
        aura_protocol::ToolAuth::Bearer {
            token: "sk-test-789".into(),
        },
        aura_protocol::ToolAuth::ApiKey {
            header: "X-Key".into(),
            key: "abc".into(),
        },
        aura_protocol::ToolAuth::Headers {
            headers: {
                let mut m = HashMap::new();
                m.insert("H1".into(), "V1".into());
                m
            },
        },
    ];

    for proto_val in &variants {
        let json = serde_json::to_string(proto_val).expect("serialize protocol ToolAuth");
        let core_val: aura_core::ToolAuth =
            serde_json::from_str(&json).expect("deserialize as core ToolAuth");
        let back_json = serde_json::to_string(&core_val).expect("re-serialize core ToolAuth");
        let roundtrip: aura_protocol::ToolAuth =
            serde_json::from_str(&back_json).expect("deserialize back to protocol ToolAuth");
        assert_eq!(
            proto_val, &roundtrip,
            "ToolAuth roundtrip failed for {json}"
        );
    }
}

#[test]
fn installed_tool_roundtrip_core_to_protocol() {
    let core_tool = aura_core::InstalledToolDefinition {
        name: "my_tool".into(),
        description: "A test tool".into(),
        input_schema: serde_json::json!({"type": "object", "properties": {"x": {"type": "string"}}}),
        endpoint: "http://localhost:8080/tool".into(),
        auth: aura_core::ToolAuth::Bearer {
            token: "tok".into(),
        },
        timeout_ms: Some(5000),
        namespace: Some("ns".into()),
        required_integration: Some(aura_core::InstalledToolIntegrationRequirement {
            integration_id: None,
            provider: Some("brave_search".into()),
            kind: Some("workspace_integration".into()),
        }),
        runtime_execution: Some(aura_core::InstalledToolRuntimeExecution::AppProvider(
            aura_core::InstalledToolRuntimeProviderExecution {
                provider: "brave_search".into(),
                base_url: "https://api.search.brave.com".into(),
                static_headers: HashMap::new(),
                integrations: vec![aura_core::InstalledToolRuntimeIntegration {
                    integration_id: "int-1".into(),
                    auth: aura_core::InstalledToolRuntimeAuth::Header {
                        name: "X-Subscription-Token".into(),
                        value: "secret".into(),
                    },
                    provider_config: HashMap::new(),
                }],
            },
        )),
        metadata: {
            let mut m = HashMap::new();
            m.insert("key".into(), serde_json::json!("value"));
            m
        },
    };

    let json = serde_json::to_string(&core_tool).expect("serialize core InstalledToolDefinition");
    let proto_tool: aura_protocol::InstalledTool =
        serde_json::from_str(&json).expect("deserialize as protocol InstalledTool");

    assert_eq!(core_tool.name, proto_tool.name);
    assert_eq!(core_tool.description, proto_tool.description);
    assert_eq!(core_tool.endpoint, proto_tool.endpoint);
    assert_eq!(core_tool.timeout_ms, proto_tool.timeout_ms);
    assert_eq!(core_tool.namespace, proto_tool.namespace);
    assert!(proto_tool.runtime_execution.is_some());
    assert_eq!(
        core_tool.required_integration.as_ref().and_then(|req| req.provider.as_deref()),
        proto_tool
            .required_integration
            .as_ref()
            .and_then(|req| req.provider.as_deref())
    );

    let back_json =
        serde_json::to_string(&proto_tool).expect("re-serialize protocol InstalledTool");
    let roundtrip: aura_core::InstalledToolDefinition =
        serde_json::from_str(&back_json).expect("deserialize back to core InstalledToolDefinition");

    assert_eq!(core_tool.name, roundtrip.name);
    assert_eq!(core_tool.description, roundtrip.description);
    assert_eq!(core_tool.endpoint, roundtrip.endpoint);
    assert_eq!(core_tool.auth, roundtrip.auth);
    assert_eq!(core_tool.timeout_ms, roundtrip.timeout_ms);
    assert_eq!(core_tool.namespace, roundtrip.namespace);
    assert!(roundtrip.runtime_execution.is_some());
    assert_eq!(
        core_tool.required_integration.as_ref().and_then(|req| req.kind.as_deref()),
        roundtrip
            .required_integration
            .as_ref()
            .and_then(|req| req.kind.as_deref())
    );
}

#[test]
fn installed_tool_roundtrip_protocol_to_core() {
    let proto_tool = aura_protocol::InstalledTool {
        name: "proto_tool".into(),
        description: "Protocol tool".into(),
        input_schema: serde_json::json!({"type": "object"}),
        endpoint: "http://example.com/api".into(),
        auth: aura_protocol::ToolAuth::ApiKey {
            header: "X-Key".into(),
            key: "secret".into(),
        },
        timeout_ms: None,
        namespace: None,
        required_integration: Some(aura_protocol::InstalledToolIntegrationRequirement {
            integration_id: None,
            provider: Some("github".into()),
            kind: Some("workspace_integration".into()),
        }),
        runtime_execution: Some(aura_protocol::InstalledToolRuntimeExecution::AppProvider(
            aura_protocol::InstalledToolRuntimeProviderExecution {
                provider: "github".into(),
                base_url: "https://api.github.com".into(),
                static_headers: HashMap::new(),
                integrations: vec![aura_protocol::InstalledToolRuntimeIntegration {
                    integration_id: "int-2".into(),
                    auth: aura_protocol::InstalledToolRuntimeAuth::AuthorizationBearer {
                        token: "secret".into(),
                    },
                    provider_config: HashMap::new(),
                }],
            },
        )),
        metadata: HashMap::new(),
    };

    let json = serde_json::to_string(&proto_tool).expect("serialize protocol InstalledTool");
    let core_tool: aura_core::InstalledToolDefinition =
        serde_json::from_str(&json).expect("deserialize as core InstalledToolDefinition");

    assert_eq!(proto_tool.name, core_tool.name);
    assert_eq!(proto_tool.description, core_tool.description);
    assert_eq!(proto_tool.endpoint, core_tool.endpoint);
    assert!(core_tool.runtime_execution.is_some());
    assert_eq!(
        proto_tool
            .required_integration
            .as_ref()
            .and_then(|req| req.provider.as_deref()),
        core_tool
            .required_integration
            .as_ref()
            .and_then(|req| req.provider.as_deref())
    );

    let back_json =
        serde_json::to_string(&core_tool).expect("re-serialize core InstalledToolDefinition");
    let roundtrip: aura_protocol::InstalledTool =
        serde_json::from_str(&back_json).expect("deserialize back to protocol InstalledTool");

    assert_eq!(proto_tool.name, roundtrip.name);
    assert_eq!(proto_tool.endpoint, roundtrip.endpoint);
    assert!(roundtrip.runtime_execution.is_some());
    assert_eq!(
        proto_tool
            .required_integration
            .as_ref()
            .and_then(|req| req.kind.as_deref()),
        roundtrip
            .required_integration
            .as_ref()
            .and_then(|req| req.kind.as_deref())
    );
}
