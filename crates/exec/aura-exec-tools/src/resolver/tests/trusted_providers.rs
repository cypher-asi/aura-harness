use super::*;

#[tokio::test]
async fn installed_tool_executes_via_trusted_runtime_metadata_gmail_send_email() {
    let (_cat, resolver) = make_catalog_and_resolver();
    let endpoint = spawn_asserting_request_server(
        "POST",
        &[
            "POST /gmail/v1/users/me/messages/send ",
            "authorization: Bearer google-access-token",
            "\"raw\":\"",
            "\"threadId\":\"thread-1\"",
        ],
        "200 OK",
        r#"{"id":"msg-123","threadId":"thread-1","labelIds":["SENT"]}"#,
    );
    let resolver = resolver.with_installed_tools(vec![InstalledToolDefinition {
        name: "gmail_send_email".into(),
        description: "Send Gmail".into(),
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
                        token: "google-access-token".into(),
                    },
                    provider_config: std::collections::HashMap::new(),
                }],
            },
        )),
        metadata: trusted_runtime_metadata(serde_json::json!({
            "type":"gmail_send_email"
        })),
    }]);
    let (ctx, _dir) = test_context();
    let tc = ToolCall::new(
        "gmail_send_email",
        serde_json::json!({
            "from":"shahroz@wilderworld.com",
            "to":"n30@wilderworld.com",
            "subject":"Google integration working smoothly",
            "text":"hello",
            "thread_id":"thread-1"
        }),
    );
    let action = Action::delegate_tool(&tc).unwrap();
    let effect = resolver.execute(&ctx, &action).await.unwrap();
    assert_eq!(effect.status, EffectStatus::Committed);
    let result: ToolResult = serde_json::from_slice(&effect.payload).unwrap();
    assert!(result.ok, "trusted runtime Gmail send should succeed");
    let stdout = std::str::from_utf8(&result.stdout).unwrap();
    assert!(stdout.contains("\"msg-123\""), "stdout was: {stdout}");
    assert!(stdout.contains("\"SENT\""), "stdout was: {stdout}");
}

#[tokio::test]
async fn installed_tool_executes_via_trusted_runtime_metadata_google_calendar_create_event() {
    let (_cat, resolver) = make_catalog_and_resolver();
    let endpoint = spawn_asserting_request_server(
        "POST",
        &[
            "POST /calendar/v3/calendars/primary/events?sendUpdates=all&conferenceDataVersion=1 ",
            "authorization: Bearer google-access-token",
            "\"summary\":\"Planning\"",
            "\"dateTime\":\"2026-06-08T10:00:00-04:00\"",
            "\"conferenceData\"",
        ],
        "200 OK",
        r#"{"id":"event-123","summary":"Planning","htmlLink":"https://calendar.google.com/event","status":"confirmed","start":{"dateTime":"2026-06-08T10:00:00-04:00"},"end":{"dateTime":"2026-06-08T10:30:00-04:00"},"attendees":[{"email":"n30@wilderworld.com"}],"conferenceData":{"conferenceId":"meet-123"}}"#,
    );
    let resolver = resolver.with_installed_tools(vec![InstalledToolDefinition {
        name: "google_calendar_create_event".into(),
        description: "Create Google Calendar event".into(),
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
                        token: "google-access-token".into(),
                    },
                    provider_config: std::collections::HashMap::new(),
                }],
            },
        )),
        metadata: trusted_runtime_metadata(serde_json::json!({
            "type":"google_calendar_create_event"
        })),
    }]);
    let (ctx, _dir) = test_context();
    let tc = ToolCall::new(
        "google_calendar_create_event",
        serde_json::json!({
            "calendar_id":"primary",
            "summary":"Planning",
            "start":"2026-06-08T10:00:00-04:00",
            "end":"2026-06-08T10:30:00-04:00",
            "attendees":["n30@wilderworld.com"],
            "send_updates":"all",
            "create_google_meet":true
        }),
    );
    let action = Action::delegate_tool(&tc).unwrap();
    let effect = resolver.execute(&ctx, &action).await.unwrap();
    assert_eq!(effect.status, EffectStatus::Committed);
    let result: ToolResult = serde_json::from_slice(&effect.payload).unwrap();
    assert!(result.ok, "trusted runtime Calendar create should succeed");
    let stdout = std::str::from_utf8(&result.stdout).unwrap();
    assert!(stdout.contains("\"event-123\""), "stdout was: {stdout}");
    assert!(
        stdout.contains("\"conference_data\""),
        "stdout was: {stdout}"
    );
}

#[tokio::test]
async fn installed_tool_executes_via_trusted_runtime_metadata_buffer_query_and_form_path() {
    let (_cat, resolver) = make_catalog_and_resolver();
    let endpoint = spawn_asserting_request_server(
        "POST",
        &[
            "POST /updates/create.json?access_token=buffer-secret ",
            "content-type: application/x-www-form-urlencoded",
            "text=Ship+it",
            "profile_ids%5B%5D=profile-1",
        ],
        "200 OK",
        r#"{"success":true,"updates":[{"id":"update-1","status":"buffer","text":"Ship it","service":"twitter"}]}"#,
    );
    let resolver = resolver.with_installed_tools(vec![InstalledToolDefinition {
        name: "buffer_create_update".into(),
        description: "Create a Buffer update".into(),
        input_schema: serde_json::json!({"type":"object"}),
        endpoint: "http://unused.local".into(),
        auth: ToolAuth::None,
        timeout_ms: Some(5_000),
        namespace: Some("aura_org_tools".into()),
        required_integration: None,
        runtime_execution: Some(InstalledToolRuntimeExecution::AppProvider(
            InstalledToolRuntimeProviderExecution {
                provider: "buffer".into(),
                base_url: endpoint,
                static_headers: std::collections::HashMap::new(),
                integrations: vec![InstalledToolRuntimeIntegration {
                    integration_id: "buffer-default".into(),
                    base_url: None,
                    auth: InstalledToolRuntimeAuth::QueryParam {
                        name: "access_token".into(),
                        value: "buffer-secret".into(),
                    },
                    provider_config: std::collections::HashMap::new(),
                }],
            },
        )),
        metadata: trusted_runtime_metadata(serde_json::json!({
            "type":"rest_form",
            "method":"post",
            "path":"/updates/create.json",
            "body":[
                {"argNames":["text"],"target":"text","valueType":"string","required":true},
                {"argNames":["profile_id","profileId"],"target":"profile_ids[]","valueType":"string","required":true}
            ],
            "result":{
                "type":"project_array",
                "key":"updates",
                "pointer":"/updates",
                "fields":[
                    {"output":"id","pointer":"/id"},
                    {"output":"status","pointer":"/status"},
                    {"output":"text","pointer":"/text"},
                    {"output":"service","pointer":"/service"}
                ],
                "extras":[
                    {"output":"success","pointer":"/success","defaultValue":false}
                ]
            }
        })),
    }]);
    let (ctx, _dir) = test_context();

    let tc = ToolCall::new(
        "buffer_create_update",
        serde_json::json!({"profile_id":"profile-1","text":"Ship it"}),
    );
    let action = Action::delegate_tool(&tc).unwrap();
    let effect = resolver.execute(&ctx, &action).await.unwrap();
    assert_eq!(effect.status, EffectStatus::Committed);
    let result: ToolResult = serde_json::from_slice(&effect.payload).unwrap();
    assert!(result.ok, "trusted runtime buffer tool should succeed");
    let stdout = std::str::from_utf8(&result.stdout).unwrap();
    assert!(stdout.contains("\"update-1\""), "stdout was: {stdout}");
    assert!(stdout.contains("\"success\":true"), "stdout was: {stdout}");
}

#[tokio::test]
async fn installed_tool_executes_via_trusted_runtime_metadata_metricool_provider_config_path() {
    let (_cat, resolver) = make_catalog_and_resolver();
    let endpoint = spawn_asserting_request_server(
        "GET",
        &[
            "GET /admin/simpleProfiles?userId=user-123&blogId=blog-456 ",
            "x-mc-auth: metricool-secret",
        ],
        "200 OK",
        r#"[{"id":654321,"userId":123456,"label":"Aura Brand"}]"#,
    );
    let resolver = resolver.with_installed_tools(vec![InstalledToolDefinition {
        name: "metricool_list_brands".into(),
        description: "List Metricool brands".into(),
        input_schema: serde_json::json!({"type":"object"}),
        endpoint: "http://unused.local".into(),
        auth: ToolAuth::None,
        timeout_ms: Some(5_000),
        namespace: Some("aura_org_tools".into()),
        required_integration: None,
        runtime_execution: Some(InstalledToolRuntimeExecution::AppProvider(
            InstalledToolRuntimeProviderExecution {
                provider: "metricool".into(),
                base_url: endpoint,
                static_headers: std::collections::HashMap::new(),
                integrations: vec![InstalledToolRuntimeIntegration {
                    integration_id: "metricool-default".into(),
                    base_url: None,
                    auth: InstalledToolRuntimeAuth::Header {
                        name: "X-Mc-Auth".into(),
                        value: "metricool-secret".into(),
                    },
                    provider_config: std::collections::HashMap::from([
                        ("userId".into(), serde_json::json!("user-123")),
                        ("blogId".into(), serde_json::json!("blog-456")),
                    ]),
                }],
            },
        )),
        metadata: trusted_runtime_metadata(serde_json::json!({
            "type":"rest_json",
            "method":"get",
            "path":"/admin/simpleProfiles",
            "query":[
                {"argNames":["userId"],"target":"userId","source":"provider_config","valueType":"string","required":true},
                {"argNames":["blogId"],"target":"blogId","source":"provider_config","valueType":"string","required":true}
            ],
            "result":{
                "type":"project_array",
                "key":"brands",
                "fields":[
                    {"output":"id","pointer":"/id"},
                    {"output":"user_id","pointer":"/userId"},
                    {"output":"label","pointer":"/label"}
                ]
            }
        })),
    }]);
    let (ctx, _dir) = test_context();

    let tc = ToolCall::new("metricool_list_brands", serde_json::json!({}));
    let action = Action::delegate_tool(&tc).unwrap();
    let effect = resolver.execute(&ctx, &action).await.unwrap();
    assert_eq!(effect.status, EffectStatus::Committed);
    let result: ToolResult = serde_json::from_slice(&effect.payload).unwrap();
    assert!(result.ok, "trusted runtime metricool tool should succeed");
    let stdout = std::str::from_utf8(&result.stdout).unwrap();
    assert!(stdout.contains("\"Aura Brand\""), "stdout was: {stdout}");
}

#[tokio::test]
async fn installed_tool_executes_via_trusted_runtime_metadata_apify_root_json_body() {
    let (_cat, resolver) = make_catalog_and_resolver();
    let endpoint = spawn_asserting_request_server(
        "POST",
        &[
            "POST /acts/my-actor/runs ",
            "authorization: Bearer apify-secret",
            "{\"query\":\"aura\"}",
        ],
        "200 OK",
        r#"{"data":{"id":"run-1","status":"READY","actId":"actor-1"}}"#,
    );
    let resolver = resolver.with_installed_tools(vec![InstalledToolDefinition {
        name: "apify_run_actor".into(),
        description: "Run an Apify actor".into(),
        input_schema: serde_json::json!({"type":"object"}),
        endpoint: "http://unused.local".into(),
        auth: ToolAuth::None,
        timeout_ms: Some(5_000),
        namespace: Some("aura_org_tools".into()),
        required_integration: None,
        runtime_execution: Some(InstalledToolRuntimeExecution::AppProvider(
            InstalledToolRuntimeProviderExecution {
                provider: "apify".into(),
                base_url: endpoint,
                static_headers: std::collections::HashMap::new(),
                integrations: vec![InstalledToolRuntimeIntegration {
                    integration_id: "apify-default".into(),
                    base_url: None,
                    auth: InstalledToolRuntimeAuth::AuthorizationBearer {
                        token: "apify-secret".into(),
                    },
                    provider_config: std::collections::HashMap::new(),
                }],
            },
        )),
        metadata: trusted_runtime_metadata(serde_json::json!({
            "type":"rest_json",
            "method":"post",
            "path":"/acts/{actor_id}/runs",
            "body":[
                {"argNames":["input"],"target":"$","valueType":"json","defaultValue":{}}
            ],
            "result":{
                "type":"project_object",
                "key":"run",
                "pointer":"/data",
                "fields":[
                    {"output":"id","pointer":"/id"},
                    {"output":"status","pointer":"/status"},
                    {"output":"act_id","pointer":"/actId"}
                ]
            }
        })),
    }]);
    let (ctx, _dir) = test_context();

    let tc = ToolCall::new(
        "apify_run_actor",
        serde_json::json!({"actor_id":"my-actor","input":{"query":"aura"}}),
    );
    let action = Action::delegate_tool(&tc).unwrap();
    let effect = resolver.execute(&ctx, &action).await.unwrap();
    assert_eq!(effect.status, EffectStatus::Committed);
    let result: ToolResult = serde_json::from_slice(&effect.payload).unwrap();
    assert!(result.ok, "trusted runtime apify tool should succeed");
    let stdout = std::str::from_utf8(&result.stdout).unwrap();
    assert!(stdout.contains("\"run-1\""), "stdout was: {stdout}");
    assert!(stdout.contains("\"READY\""), "stdout was: {stdout}");
}

#[tokio::test]
async fn installed_tool_executes_via_trusted_runtime_metadata_mailchimp_base_url_override() {
    let (_cat, resolver) = make_catalog_and_resolver();
    let endpoint = spawn_asserting_request_server(
        "GET",
        &["GET /lists "],
        "200 OK",
        r#"{"lists":[{"id":"list-1","name":"Players","stats":{"member_count":128}}]}"#,
    );
    let resolver = resolver.with_installed_tools(vec![InstalledToolDefinition {
        name: "mailchimp_list_audiences".into(),
        description: "List Mailchimp audiences".into(),
        input_schema: serde_json::json!({"type":"object"}),
        endpoint: "http://unused.local".into(),
        auth: ToolAuth::None,
        timeout_ms: Some(5_000),
        namespace: Some("aura_org_tools".into()),
        required_integration: None,
        runtime_execution: Some(InstalledToolRuntimeExecution::AppProvider(
            InstalledToolRuntimeProviderExecution {
                provider: "mailchimp".into(),
                base_url: String::new(),
                static_headers: std::collections::HashMap::new(),
                integrations: vec![InstalledToolRuntimeIntegration {
                    integration_id: "mailchimp-default".into(),
                    base_url: Some(endpoint),
                    auth: InstalledToolRuntimeAuth::Basic {
                        username: "anystring".into(),
                        password: "mailchimp-secret-us19".into(),
                    },
                    provider_config: std::collections::HashMap::new(),
                }],
            },
        )),
        metadata: trusted_runtime_metadata(serde_json::json!({
            "type":"rest_json",
            "method":"get",
            "path":"/lists",
            "result":{
                "type":"project_array",
                "key":"audiences",
                "pointer":"/lists",
                "fields":[
                    {"output":"id","pointer":"/id"},
                    {"output":"name","pointer":"/name"},
                    {"output":"member_count","pointer":"/stats/member_count"}
                ]
            }
        })),
    }]);
    let (ctx, _dir) = test_context();

    let tc = ToolCall::new("mailchimp_list_audiences", serde_json::json!({}));
    let action = Action::delegate_tool(&tc).unwrap();
    let effect = resolver.execute(&ctx, &action).await.unwrap();
    assert_eq!(effect.status, EffectStatus::Committed);
    let result: ToolResult = serde_json::from_slice(&effect.payload).unwrap();
    assert!(result.ok, "trusted runtime mailchimp tool should succeed");
    let stdout = std::str::from_utf8(&result.stdout).unwrap();
    assert!(stdout.contains("\"list-1\""), "stdout was: {stdout}");
    assert!(stdout.contains("\"Players\""), "stdout was: {stdout}");
}
