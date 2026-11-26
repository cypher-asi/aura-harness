use super::*;

#[tokio::test]
async fn installed_tool_executes_via_http_callback() {
    let (_cat, resolver) = make_catalog_and_resolver();
    let endpoint = spawn_single_response_server(
        Some("Bearer test-token"),
        "200 OK",
        r#"{"ok":true,"title":"Aura OS"}"#,
    );
    let resolver = resolver.with_installed_tools(vec![InstalledToolDefinition {
        name: "brave_search_web".into(),
        description: "Search the web".into(),
        input_schema: serde_json::json!({"type":"object"}),
        endpoint,
        auth: ToolAuth::Bearer {
            token: "test-token".into(),
        },
        timeout_ms: Some(5_000),
        namespace: Some("aura_org_tools".into()),
        required_integration: None,
        runtime_execution: None,
        metadata: std::collections::HashMap::new(),
    }]);
    let (ctx, _dir) = test_context();

    let tc = ToolCall::new(
        "brave_search_web",
        serde_json::json!({"query":"aura os","count":1}),
    );
    let action = Action::delegate_tool(&tc).unwrap();
    let effect = resolver.execute(&ctx, &action).await.unwrap();
    assert_eq!(effect.status, EffectStatus::Committed);
    let result: ToolResult = serde_json::from_slice(&effect.payload).unwrap();
    assert!(result.ok, "installed tool should succeed");
    let stdout = std::str::from_utf8(&result.stdout).unwrap();
    assert!(stdout.contains("\"Aura OS\""), "stdout was: {stdout}");
}

#[tokio::test]
async fn installed_tool_http_failure_returns_failed_effect() {
    let (_cat, resolver) = make_catalog_and_resolver();
    let endpoint = spawn_single_response_server(None, "404 Not Found", r#"{"error":"missing"}"#);
    let resolver = resolver.with_installed_tools(vec![InstalledToolDefinition {
        name: "brave_search_web".into(),
        description: "Search the web".into(),
        input_schema: serde_json::json!({"type":"object"}),
        endpoint,
        auth: ToolAuth::None,
        timeout_ms: Some(5_000),
        namespace: Some("aura_org_tools".into()),
        required_integration: None,
        runtime_execution: None,
        metadata: std::collections::HashMap::new(),
    }]);
    let (ctx, _dir) = test_context();

    let tc = ToolCall::new("brave_search_web", serde_json::json!({"query":"aura os"}));
    let action = Action::delegate_tool(&tc).unwrap();
    let effect = resolver.execute(&ctx, &action).await.unwrap();
    assert_eq!(effect.status, EffectStatus::Failed);
    let result: ToolResult = serde_json::from_slice(&effect.payload).unwrap();
    let stderr = std::str::from_utf8(&result.stderr).unwrap();
    assert!(
        stderr.contains("returned status 404"),
        "stderr was: {stderr}"
    );
}

#[tokio::test]
async fn installed_tool_executes_via_runtime_provider_path() {
    let (_cat, resolver) = make_catalog_and_resolver();
    let endpoint = spawn_single_response_server(
        Some("Bearer gh-test"),
        "200 OK",
        r#"[{"name":"aura","full_name":"cypher-asi/aura","private":false,"html_url":"https://github.com/cypher-asi/aura","default_branch":"main","description":"Aura"}]"#,
    );
    let resolver = resolver.with_installed_tools(vec![InstalledToolDefinition {
        name: "github_list_repos".into(),
        description: "List GitHub repos".into(),
        input_schema: serde_json::json!({"type":"object"}),
        endpoint: "http://unused.local".into(),
        auth: ToolAuth::None,
        timeout_ms: Some(5_000),
        namespace: Some("aura_org_tools".into()),
        required_integration: None,
        runtime_execution: Some(InstalledToolRuntimeExecution::AppProvider(
            InstalledToolRuntimeProviderExecution {
                provider: "github".into(),
                base_url: endpoint,
                static_headers: std::collections::HashMap::new(),
                integrations: vec![InstalledToolRuntimeIntegration {
                    integration_id: "github-default".into(),
                    base_url: None,
                    auth: InstalledToolRuntimeAuth::AuthorizationBearer {
                        token: "gh-test".into(),
                    },
                    provider_config: std::collections::HashMap::new(),
                }],
            },
        )),
        metadata: std::collections::HashMap::new(),
    }]);
    let (ctx, _dir) = test_context();

    let tc = ToolCall::new("github_list_repos", serde_json::json!({}));
    let action = Action::delegate_tool(&tc).unwrap();
    let effect = resolver.execute(&ctx, &action).await.unwrap();
    assert_eq!(effect.status, EffectStatus::Committed);
    let result: ToolResult = serde_json::from_slice(&effect.payload).unwrap();
    assert!(result.ok, "runtime-installed tool should succeed");
    let stdout = std::str::from_utf8(&result.stdout).unwrap();
    assert!(
        stdout.contains("\"cypher-asi/aura\""),
        "stdout was: {stdout}"
    );
}
