use super::*;

#[tokio::test]
async fn installed_tool_executes_via_trusted_runtime_metadata_rest_path() {
    let (_cat, resolver) = make_catalog_and_resolver();
    let endpoint = spawn_asserting_response_server(
        "POST",
        &[("Authorization", "Bearer gh-test")],
        "200 OK",
        r#"{"number":42,"title":"Ship it","state":"open","html_url":"https://github.com/cypher-asi/aura/issues/42"}"#,
    );
    let resolver = resolver.with_installed_tools(vec![InstalledToolDefinition {
        name: "github_create_issue".into(),
        description: "Create a GitHub issue".into(),
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
        metadata: trusted_runtime_metadata(serde_json::json!({
            "type":"rest_json",
            "method":"post",
            "path":"/repos/{owner}/{repo}/issues",
            "query":[],
            "body":[
                {"argNames":["title"],"target":"title","valueType":"string","required":true},
                {"argNames":["body"],"target":"body","valueType":"string","required":false}
            ],
            "successGuard":"none",
            "result":{
                "type":"project_object",
                "key":"issue",
                "fields":[
                    {"output":"number","pointer":"/number"},
                    {"output":"title","pointer":"/title"},
                    {"output":"state","pointer":"/state"},
                    {"output":"html_url","pointer":"/html_url"}
                ]
            }
        })),
    }]);
    let (ctx, _dir) = test_context();
    let tc = ToolCall::new(
        "github_create_issue",
        serde_json::json!({"owner":"cypher-asi","repo":"aura","title":"Ship it","body":"hello"}),
    );
    let action = Action::delegate_tool(&tc).unwrap();
    let effect = resolver.execute(&ctx, &action).await.unwrap();
    assert_eq!(effect.status, EffectStatus::Committed);
    let result: ToolResult = serde_json::from_slice(&effect.payload).unwrap();
    let stdout = std::str::from_utf8(&result.stdout).unwrap();
    assert!(stdout.contains("\"Ship it\""), "stdout was: {stdout}");
    assert!(stdout.contains("\"number\":42"), "stdout was: {stdout}");
}

#[tokio::test]
async fn installed_tool_trusted_runtime_metadata_supports_boolean_query_args() {
    let (_cat, resolver) = make_catalog_and_resolver();
    let endpoint = spawn_asserting_request_server(
        "GET",
        &[
            "GET /calendar/v3/calendars/primary/events?maxResults=2&singleEvents=true ",
            "authorization: Bearer google-secret",
        ],
        "200 OK",
        r#"{"items":[{"id":"evt_1","summary":"Planning","status":"confirmed"}]}"#,
    );
    let resolver = resolver.with_installed_tools(vec![InstalledToolDefinition {
        name: "google_calendar_list_events".into(),
        description: "List Google Calendar events".into(),
        input_schema: serde_json::json!({"type":"object"}),
        endpoint: "http://unused.local".into(),
        auth: ToolAuth::None,
        timeout_ms: Some(5_000),
        namespace: Some("aura_org_tools".into()),
        required_integration: None,
        runtime_execution: Some(InstalledToolRuntimeExecution::AppProvider(
            InstalledToolRuntimeProviderExecution {
                provider: "google".into(),
                base_url: endpoint,
                static_headers: std::collections::HashMap::new(),
                integrations: vec![InstalledToolRuntimeIntegration {
                    integration_id: "google-default".into(),
                    base_url: None,
                    auth: InstalledToolRuntimeAuth::AuthorizationBearer {
                        token: "google-secret".into(),
                    },
                    provider_config: std::collections::HashMap::new(),
                }],
            },
        )),
        metadata: trusted_runtime_metadata(serde_json::json!({
            "type":"rest_json",
            "method":"get",
            "path":"/calendar/v3/calendars/primary/events",
            "query":[
                {"argNames":["max_results"],"target":"maxResults","valueType":"positive_number","required":false},
                {"argNames":["single_events"],"target":"singleEvents","valueType":"boolean","required":false}
            ],
            "body":[],
            "successGuard":"none",
            "result":{"type":"wrap_pointer","key":"events","pointer":"/items"}
        })),
    }]);
    let (ctx, _dir) = test_context();
    let tc = ToolCall::new(
        "google_calendar_list_events",
        serde_json::json!({"max_results":2,"single_events":true}),
    );
    let action = Action::delegate_tool(&tc).unwrap();
    let effect = resolver.execute(&ctx, &action).await.unwrap();
    assert_eq!(effect.status, EffectStatus::Committed);
    let result: ToolResult = serde_json::from_slice(&effect.payload).unwrap();
    let stdout = std::str::from_utf8(&result.stdout).unwrap();
    assert!(stdout.contains("\"Planning\""), "stdout was: {stdout}");
}

#[tokio::test]
async fn installed_tool_executes_via_trusted_runtime_metadata_graphql_path() {
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
        metadata: trusted_runtime_metadata(serde_json::json!({
            "type":"graphql",
            "query":"query AuraLinearTeams { teams { nodes { id name key } } }",
            "variables":[],
            "successGuard":"graphql_errors",
            "result":{"type":"wrap_pointer","key":"teams","pointer":"/data/teams/nodes"}
        })),
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
async fn installed_tool_executes_via_trusted_runtime_metadata_brave_path() {
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
        metadata: trusted_runtime_metadata(serde_json::json!({
            "type":"brave_search",
            "vertical":"web"
        })),
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
async fn installed_tool_executes_via_trusted_runtime_metadata_resend_send_email() {
    let (_cat, resolver) = make_catalog_and_resolver();
    let endpoint = spawn_asserting_response_server(
        "POST",
        &[("Authorization", "Bearer resend-secret")],
        "200 OK",
        r#"{"id":"email_123"}"#,
    );
    let resolver = resolver.with_installed_tools(vec![InstalledToolDefinition {
        name: "resend_send_email".into(),
        description: "Send email".into(),
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
        metadata: trusted_runtime_metadata(serde_json::json!({
            "type":"resend_send_email"
        })),
    }]);
    let (ctx, _dir) = test_context();
    let tc = ToolCall::new(
        "resend_send_email",
        serde_json::json!({"from":"Aura <onboarding@resend.dev>","to":"shahroz@wilderworld.com","subject":"Hello","text":"hello"}),
    );
    let action = Action::delegate_tool(&tc).unwrap();
    let effect = resolver.execute(&ctx, &action).await.unwrap();
    assert_eq!(effect.status, EffectStatus::Committed);
    let result: ToolResult = serde_json::from_slice(&effect.payload).unwrap();
    let stdout = std::str::from_utf8(&result.stdout).unwrap();
    assert!(stdout.contains("\"email_123\""), "stdout was: {stdout}");
}

#[test]
fn trusted_runtime_metadata_accepts_provider_specific_variants() {
    for spec in [
        serde_json::json!({"type":"brave_search","vertical":"web"}),
        serde_json::json!({"type":"resend_send_email"}),
        serde_json::json!({"type":"gmail_send_email"}),
        serde_json::json!({"type":"gmail_create_draft"}),
        serde_json::json!({"type":"google_calendar_create_event"}),
        serde_json::json!({"type":"google_calendar_update_event"}),
        serde_json::json!({"type":"google_calendar_delete_event"}),
    ] {
        let variant = spec["type"].as_str().unwrap().to_string();
        let tool = InstalledToolDefinition {
            name: variant.clone(),
            description: "Provider-specific trusted runtime".into(),
            input_schema: serde_json::json!({"type":"object"}),
            endpoint: "http://unused.local".into(),
            auth: ToolAuth::None,
            timeout_ms: Some(5_000),
            namespace: Some("aura_org_tools".into()),
            required_integration: None,
            runtime_execution: None,
            metadata: trusted_runtime_metadata(spec),
        };
        assert!(
            super::trusted::trusted_runtime_spec(&tool)
                .unwrap()
                .is_some(),
            "expected trusted runtime metadata variant `{variant}` to parse"
        );
    }
}
