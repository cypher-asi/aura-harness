use super::*;

#[tokio::test]
async fn installed_tool_executes_linear_via_runtime_provider_path() {
    let (_cat, resolver) = make_catalog_and_resolver();
    let endpoint = spawn_asserting_response_server(
        "POST",
        &[("Authorization", "lin-secret")],
        "200 OK",
        r#"{"data":{"teams":{"nodes":[{"id":"t1","name":"Zero","key":"ZER"}]}}}"#,
    );
    let resolver = resolver.with_installed_tools(vec![InstalledToolDefinition {
        name: "linear_list_teams".into(),
        description: "List Linear teams".into(),
        input_schema: serde_json::json!({"type":"object"}),
        endpoint: "http://unused.local".into(),
        auth: ToolAuth::None,
        timeout_ms: Some(5_000),
        namespace: Some("aura_org_tools".into()),
        required_integration: None,
        runtime_execution: Some(InstalledToolRuntimeExecution::AppProvider(
            InstalledToolRuntimeProviderExecution {
                provider: "linear".into(),
                base_url: endpoint,
                static_headers: std::collections::HashMap::new(),
                integrations: vec![InstalledToolRuntimeIntegration {
                    integration_id: "linear-default".into(),
                    base_url: None,
                    auth: InstalledToolRuntimeAuth::AuthorizationRaw {
                        value: "lin-secret".into(),
                    },
                    provider_config: std::collections::HashMap::new(),
                }],
            },
        )),
        metadata: std::collections::HashMap::new(),
    }]);
    let (ctx, _dir) = test_context();
    let tc = ToolCall::new("linear_list_teams", serde_json::json!({}));
    let action = Action::delegate_tool(&tc).unwrap();
    let effect = resolver.execute(&ctx, &action).await.unwrap();
    assert_eq!(effect.status, EffectStatus::Committed);
    let result: ToolResult = serde_json::from_slice(&effect.payload).unwrap();
    let stdout = std::str::from_utf8(&result.stdout).unwrap();
    assert!(stdout.contains("\"ZER\""), "stdout was: {stdout}");
}

#[tokio::test]
async fn installed_tool_executes_brave_via_runtime_provider_path() {
    let (_cat, resolver) = make_catalog_and_resolver();
    let endpoint = spawn_asserting_response_server(
        "GET",
        &[("X-Subscription-Token", "brave-secret")],
        "200 OK",
        r#"{"web":{"results":[{"title":"Aura","url":"https://zero.tech","description":"desc"}]},"query":{"more_results_available":false}}"#,
    );
    let resolver = resolver.with_installed_tools(vec![InstalledToolDefinition {
        name: "brave_search_web".into(),
        description: "Search the web".into(),
        input_schema: serde_json::json!({"type":"object"}),
        endpoint: "http://unused.local".into(),
        auth: ToolAuth::None,
        timeout_ms: Some(5_000),
        namespace: Some("aura_org_tools".into()),
        required_integration: None,
        runtime_execution: Some(InstalledToolRuntimeExecution::AppProvider(
            InstalledToolRuntimeProviderExecution {
                provider: "brave_search".into(),
                base_url: endpoint,
                static_headers: std::collections::HashMap::new(),
                integrations: vec![InstalledToolRuntimeIntegration {
                    integration_id: "brave-default".into(),
                    base_url: None,
                    auth: InstalledToolRuntimeAuth::Header {
                        name: "X-Subscription-Token".into(),
                        value: "brave-secret".into(),
                    },
                    provider_config: std::collections::HashMap::new(),
                }],
            },
        )),
        metadata: std::collections::HashMap::new(),
    }]);
    let (ctx, _dir) = test_context();
    let tc = ToolCall::new(
        "brave_search_web",
        serde_json::json!({"query":"aura","count":1}),
    );
    let action = Action::delegate_tool(&tc).unwrap();
    let effect = resolver.execute(&ctx, &action).await.unwrap();
    assert_eq!(effect.status, EffectStatus::Committed);
    let result: ToolResult = serde_json::from_slice(&effect.payload).unwrap();
    let stdout = std::str::from_utf8(&result.stdout).unwrap();
    assert!(stdout.contains("\"Aura\""), "stdout was: {stdout}");
}

#[tokio::test]
async fn installed_tool_executes_slack_post_message_via_runtime_provider_path() {
    let (_cat, resolver) = make_catalog_and_resolver();
    let endpoint = spawn_asserting_response_server(
        "POST",
        &[("Authorization", "Bearer slack-secret")],
        "200 OK",
        r#"{"ok":true,"channel":"C0AR7PWBX4P","ts":"1775678730.921229"}"#,
    );
    let resolver = resolver.with_installed_tools(vec![InstalledToolDefinition {
        name: "slack_post_message".into(),
        description: "Post to Slack".into(),
        input_schema: serde_json::json!({"type":"object"}),
        endpoint: "http://unused.local".into(),
        auth: ToolAuth::None,
        timeout_ms: Some(5_000),
        namespace: Some("aura_org_tools".into()),
        required_integration: None,
        runtime_execution: Some(InstalledToolRuntimeExecution::AppProvider(
            InstalledToolRuntimeProviderExecution {
                provider: "slack".into(),
                base_url: endpoint,
                static_headers: std::collections::HashMap::new(),
                integrations: vec![InstalledToolRuntimeIntegration {
                    integration_id: "slack-default".into(),
                    base_url: None,
                    auth: InstalledToolRuntimeAuth::AuthorizationBearer {
                        token: "slack-secret".into(),
                    },
                    provider_config: std::collections::HashMap::new(),
                }],
            },
        )),
        metadata: std::collections::HashMap::new(),
    }]);
    let (ctx, _dir) = test_context();
    let tc = ToolCall::new(
        "slack_post_message",
        serde_json::json!({"channel_id":"C0AR7PWBX4P","text":"hello from aura"}),
    );
    let action = Action::delegate_tool(&tc).unwrap();
    let effect = resolver.execute(&ctx, &action).await.unwrap();
    assert_eq!(effect.status, EffectStatus::Committed);
    let result: ToolResult = serde_json::from_slice(&effect.payload).unwrap();
    let stdout = std::str::from_utf8(&result.stdout).unwrap();
    assert!(
        stdout.contains("\"1775678730.921229\""),
        "stdout was: {stdout}"
    );
}

#[tokio::test]
async fn installed_tool_surfaces_slack_api_errors_via_runtime_provider_path() {
    let (_cat, resolver) = make_catalog_and_resolver();
    let endpoint = spawn_asserting_response_server(
        "GET",
        &[("Authorization", "Bearer slack-secret")],
        "200 OK",
        r#"{"ok":false,"error":"missing_scope"}"#,
    );
    let resolver = resolver.with_installed_tools(vec![InstalledToolDefinition {
        name: "slack_list_channels".into(),
        description: "List Slack channels".into(),
        input_schema: serde_json::json!({"type":"object"}),
        endpoint: "http://unused.local".into(),
        auth: ToolAuth::None,
        timeout_ms: Some(5_000),
        namespace: Some("aura_org_tools".into()),
        required_integration: None,
        runtime_execution: Some(InstalledToolRuntimeExecution::AppProvider(
            InstalledToolRuntimeProviderExecution {
                provider: "slack".into(),
                base_url: endpoint,
                static_headers: std::collections::HashMap::new(),
                integrations: vec![InstalledToolRuntimeIntegration {
                    integration_id: "slack-default".into(),
                    base_url: None,
                    auth: InstalledToolRuntimeAuth::AuthorizationBearer {
                        token: "slack-secret".into(),
                    },
                    provider_config: std::collections::HashMap::new(),
                }],
            },
        )),
        metadata: std::collections::HashMap::new(),
    }]);
    let (ctx, _dir) = test_context();
    let tc = ToolCall::new("slack_list_channels", serde_json::json!({}));
    let action = Action::delegate_tool(&tc).unwrap();
    let effect = resolver.execute(&ctx, &action).await.unwrap();
    assert_eq!(effect.status, EffectStatus::Failed);
    let result: ToolResult = serde_json::from_slice(&effect.payload).unwrap();
    let stderr = std::str::from_utf8(&result.stderr).unwrap();
    assert!(
        stderr.contains("slack api error: missing_scope"),
        "stderr was: {stderr}"
    );
}

#[tokio::test]
async fn installed_tool_executes_resend_via_runtime_provider_path() {
    let (_cat, resolver) = make_catalog_and_resolver();
    let endpoint = spawn_asserting_response_server(
        "GET",
        &[("Authorization", "Bearer resend-secret")],
        "200 OK",
        r#"{"data":[{"id":"dom_1","name":"example.com","status":"verified"}],"has_more":false}"#,
    );
    let resolver = resolver.with_installed_tools(vec![InstalledToolDefinition {
        name: "resend_list_domains".into(),
        description: "List resend domains".into(),
        input_schema: serde_json::json!({"type":"object"}),
        endpoint: "http://unused.local".into(),
        auth: ToolAuth::None,
        timeout_ms: Some(5_000),
        namespace: Some("aura_org_tools".into()),
        required_integration: None,
        runtime_execution: Some(InstalledToolRuntimeExecution::AppProvider(
            InstalledToolRuntimeProviderExecution {
                provider: "resend".into(),
                base_url: endpoint,
                static_headers: std::collections::HashMap::new(),
                integrations: vec![InstalledToolRuntimeIntegration {
                    integration_id: "resend-default".into(),
                    base_url: None,
                    auth: InstalledToolRuntimeAuth::AuthorizationBearer {
                        token: "resend-secret".into(),
                    },
                    provider_config: std::collections::HashMap::new(),
                }],
            },
        )),
        metadata: std::collections::HashMap::new(),
    }]);
    let (ctx, _dir) = test_context();
    let tc = ToolCall::new("resend_list_domains", serde_json::json!({}));
    let action = Action::delegate_tool(&tc).unwrap();
    let effect = resolver.execute(&ctx, &action).await.unwrap();
    assert_eq!(effect.status, EffectStatus::Committed);
    let result: ToolResult = serde_json::from_slice(&effect.payload).unwrap();
    let stdout = std::str::from_utf8(&result.stdout).unwrap();
    assert!(stdout.contains("\"example.com\""), "stdout was: {stdout}");
}
