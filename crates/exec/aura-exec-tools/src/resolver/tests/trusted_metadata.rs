use super::*;

fn github_create_issue_spec() -> Value {
    serde_json::json!({
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
    })
}

#[tokio::test]
async fn installed_tool_executes_via_trusted_runtime_metadata_rest_path() {
    let endpoint = spawn_asserting_response_server(
        "POST",
        &[("Authorization", "Bearer gh-test")],
        "200 OK",
        r#"{"number":42,"title":"Ship it","state":"open","html_url":"https://github.com/cypher-asi/aura/issues/42"}"#,
    );
    let runtime = provider_runtime(
        "github",
        endpoint,
        runtime_integration(
            "github-default",
            InstalledToolRuntimeAuth::AuthorizationBearer {
                token: "gh-test".into(),
            },
        ),
    );
    let tool = trusted_installed_tool(
        "github_create_issue",
        "Create a GitHub issue",
        runtime,
        github_create_issue_spec(),
    );
    let args =
        serde_json::json!({"owner":"cypher-asi","repo":"aura","title":"Ship it","body":"hello"});
    let result = execute_installed_tool(tool, args).await;
    let stdout = std::str::from_utf8(&result.stdout).unwrap();
    assert!(stdout.contains("\"Ship it\""), "stdout was: {stdout}");
    assert!(stdout.contains("\"number\":42"), "stdout was: {stdout}");
}

#[tokio::test]
async fn installed_tool_trusted_runtime_metadata_supports_boolean_query_args() {
    let endpoint = spawn_asserting_request_server(
        "GET",
        &[
            "GET /calendar/v3/calendars/primary/events?maxResults=2&singleEvents=true ",
            "authorization: Bearer google-secret",
        ],
        "200 OK",
        r#"{"items":[{"id":"evt_1","summary":"Planning","status":"confirmed"}]}"#,
    );
    let runtime = provider_runtime(
        "google",
        endpoint,
        runtime_integration(
            "google-default",
            InstalledToolRuntimeAuth::AuthorizationBearer {
                token: "google-secret".into(),
            },
        ),
    );
    let spec = serde_json::json!({
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
    });
    let tool = trusted_installed_tool(
        "google_calendar_list_events",
        "List Google Calendar events",
        runtime,
        spec,
    );
    let args = serde_json::json!({"max_results":2,"single_events":true});
    let result = execute_installed_tool(tool, args).await;
    let stdout = std::str::from_utf8(&result.stdout).unwrap();
    assert!(stdout.contains("\"Planning\""), "stdout was: {stdout}");
}

#[tokio::test]
async fn installed_tool_executes_via_trusted_runtime_metadata_graphql_path() {
    let endpoint = spawn_asserting_response_server(
        "POST",
        &[("Authorization", "lin-secret")],
        "200 OK",
        r#"{"data":{"teams":{"nodes":[{"id":"t1","name":"Zero","key":"ZER"}]}}}"#,
    );
    let runtime = provider_runtime(
        "linear",
        endpoint,
        runtime_integration(
            "linear-default",
            InstalledToolRuntimeAuth::AuthorizationRaw {
                value: "lin-secret".into(),
            },
        ),
    );
    let spec = serde_json::json!({
        "type":"graphql",
        "query":"query AuraLinearTeams { teams { nodes { id name key } } }",
        "variables":[],
        "successGuard":"graphql_errors",
        "result":{"type":"wrap_pointer","key":"teams","pointer":"/data/teams/nodes"}
    });
    let tool = trusted_installed_tool("linear_list_teams", "List Linear teams", runtime, spec);
    let result = execute_installed_tool(tool, serde_json::json!({})).await;
    let stdout = std::str::from_utf8(&result.stdout).unwrap();
    assert!(stdout.contains("\"ZER\""), "stdout was: {stdout}");
}

#[tokio::test]
async fn installed_tool_executes_via_trusted_runtime_metadata_brave_path() {
    let endpoint = spawn_asserting_response_server(
        "GET",
        &[("X-Subscription-Token", "brave-secret")],
        "200 OK",
        r#"{"web":{"results":[{"title":"Aura","url":"https://zero.tech","description":"desc"}]},"query":{"more_results_available":false}}"#,
    );
    let runtime = provider_runtime(
        "brave_search",
        endpoint,
        runtime_integration(
            "brave-default",
            InstalledToolRuntimeAuth::Header {
                name: "X-Subscription-Token".into(),
                value: "brave-secret".into(),
            },
        ),
    );
    let spec = serde_json::json!({
        "type":"brave_search",
        "vertical":"web"
    });
    let tool = trusted_installed_tool("brave_search_web", "Search the web", runtime, spec);
    let args = serde_json::json!({"query":"aura","count":1});
    let result = execute_installed_tool(tool, args).await;
    let stdout = std::str::from_utf8(&result.stdout).unwrap();
    assert!(stdout.contains("\"Aura\""), "stdout was: {stdout}");
}

#[tokio::test]
async fn installed_tool_executes_via_trusted_runtime_metadata_resend_send_email() {
    let endpoint = spawn_asserting_response_server(
        "POST",
        &[("Authorization", "Bearer resend-secret")],
        "200 OK",
        r#"{"id":"email_123"}"#,
    );
    let runtime = provider_runtime(
        "resend",
        endpoint,
        runtime_integration(
            "resend-default",
            InstalledToolRuntimeAuth::AuthorizationBearer {
                token: "resend-secret".into(),
            },
        ),
    );
    let spec = serde_json::json!({
        "type":"resend_send_email"
    });
    let tool = trusted_installed_tool("resend_send_email", "Send email", runtime, spec);
    let args = serde_json::json!({"from":"Aura <onboarding@resend.dev>","to":"shahroz@wilderworld.com","subject":"Hello","text":"hello"});
    let result = execute_installed_tool(tool, args).await;
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
