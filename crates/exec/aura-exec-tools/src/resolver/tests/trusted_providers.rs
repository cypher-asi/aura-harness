use super::*;

fn google_bearer_runtime(endpoint: String) -> InstalledToolRuntimeExecution {
    provider_runtime(
        "google",
        endpoint,
        runtime_integration(
            "google-default",
            InstalledToolRuntimeAuth::AuthorizationBearer {
                token: "google-access-token".into(),
            },
        ),
    )
}

fn buffer_create_update_spec() -> Value {
    serde_json::json!({
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
    })
}

fn metricool_list_brands_spec() -> Value {
    serde_json::json!({
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
    })
}

#[tokio::test]
async fn installed_tool_executes_via_trusted_runtime_metadata_gmail_send_email() {
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
    let tool = trusted_installed_tool(
        "gmail_send_email",
        "Send Gmail",
        google_bearer_runtime(endpoint),
        serde_json::json!({
            "type":"gmail_send_email"
        }),
    );
    let args = serde_json::json!({
        "from":"shahroz@wilderworld.com",
        "to":"n30@wilderworld.com",
        "subject":"Google integration working smoothly",
        "text":"hello",
        "thread_id":"thread-1"
    });
    let result = execute_installed_tool(tool, args).await;
    assert!(result.ok, "trusted runtime Gmail send should succeed");
    let stdout = std::str::from_utf8(&result.stdout).unwrap();
    assert!(stdout.contains("\"msg-123\""), "stdout was: {stdout}");
    assert!(stdout.contains("\"SENT\""), "stdout was: {stdout}");
}

#[tokio::test]
async fn installed_tool_executes_via_trusted_runtime_metadata_google_calendar_create_event() {
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
    let tool = trusted_installed_tool(
        "google_calendar_create_event",
        "Create Google Calendar event",
        google_bearer_runtime(endpoint),
        serde_json::json!({
            "type":"google_calendar_create_event"
        }),
    );
    let args = serde_json::json!({
        "calendar_id":"primary",
        "summary":"Planning",
        "start":"2026-06-08T10:00:00-04:00",
        "end":"2026-06-08T10:30:00-04:00",
        "attendees":["n30@wilderworld.com"],
        "send_updates":"all",
        "create_google_meet":true
    });
    let result = execute_installed_tool(tool, args).await;
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
    let runtime = provider_runtime(
        "buffer",
        endpoint,
        runtime_integration(
            "buffer-default",
            InstalledToolRuntimeAuth::QueryParam {
                name: "access_token".into(),
                value: "buffer-secret".into(),
            },
        ),
    );
    let tool = trusted_installed_tool(
        "buffer_create_update",
        "Create a Buffer update",
        runtime,
        buffer_create_update_spec(),
    );

    let args = serde_json::json!({"profile_id":"profile-1","text":"Ship it"});
    let result = execute_installed_tool(tool, args).await;
    assert!(result.ok, "trusted runtime buffer tool should succeed");
    let stdout = std::str::from_utf8(&result.stdout).unwrap();
    assert!(stdout.contains("\"update-1\""), "stdout was: {stdout}");
    assert!(stdout.contains("\"success\":true"), "stdout was: {stdout}");
}

#[tokio::test]
async fn installed_tool_executes_via_trusted_runtime_metadata_metricool_provider_config_path() {
    let endpoint = spawn_asserting_request_server(
        "GET",
        &[
            "GET /admin/simpleProfiles?userId=user-123&blogId=blog-456 ",
            "x-mc-auth: metricool-secret",
        ],
        "200 OK",
        r#"[{"id":654321,"userId":123456,"label":"Aura Brand"}]"#,
    );
    let integration = InstalledToolRuntimeIntegration {
        provider_config: std::collections::HashMap::from([
            ("userId".into(), serde_json::json!("user-123")),
            ("blogId".into(), serde_json::json!("blog-456")),
        ]),
        ..runtime_integration(
            "metricool-default",
            InstalledToolRuntimeAuth::Header {
                name: "X-Mc-Auth".into(),
                value: "metricool-secret".into(),
            },
        )
    };
    let tool = trusted_installed_tool(
        "metricool_list_brands",
        "List Metricool brands",
        provider_runtime("metricool", endpoint, integration),
        metricool_list_brands_spec(),
    );

    let result = execute_installed_tool(tool, serde_json::json!({})).await;
    assert!(result.ok, "trusted runtime metricool tool should succeed");
    let stdout = std::str::from_utf8(&result.stdout).unwrap();
    assert!(stdout.contains("\"Aura Brand\""), "stdout was: {stdout}");
}

#[tokio::test]
async fn installed_tool_executes_via_trusted_runtime_metadata_apify_root_json_body() {
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
    let runtime = provider_runtime(
        "apify",
        endpoint,
        runtime_integration(
            "apify-default",
            InstalledToolRuntimeAuth::AuthorizationBearer {
                token: "apify-secret".into(),
            },
        ),
    );
    let spec = serde_json::json!({
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
    });
    let tool = trusted_installed_tool("apify_run_actor", "Run an Apify actor", runtime, spec);

    let args = serde_json::json!({"actor_id":"my-actor","input":{"query":"aura"}});
    let result = execute_installed_tool(tool, args).await;
    assert!(result.ok, "trusted runtime apify tool should succeed");
    let stdout = std::str::from_utf8(&result.stdout).unwrap();
    assert!(stdout.contains("\"run-1\""), "stdout was: {stdout}");
    assert!(stdout.contains("\"READY\""), "stdout was: {stdout}");
}

#[tokio::test]
async fn installed_tool_executes_via_trusted_runtime_metadata_mailchimp_base_url_override() {
    let endpoint = spawn_asserting_request_server(
        "GET",
        &["GET /lists "],
        "200 OK",
        r#"{"lists":[{"id":"list-1","name":"Players","stats":{"member_count":128}}]}"#,
    );
    let integration = InstalledToolRuntimeIntegration {
        base_url: Some(endpoint),
        ..runtime_integration(
            "mailchimp-default",
            InstalledToolRuntimeAuth::Basic {
                username: "anystring".into(),
                password: "mailchimp-secret-us19".into(),
            },
        )
    };
    let spec = serde_json::json!({
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
    });
    let tool = trusted_installed_tool(
        "mailchimp_list_audiences",
        "List Mailchimp audiences",
        provider_runtime("mailchimp", String::new(), integration),
        spec,
    );

    let result = execute_installed_tool(tool, serde_json::json!({})).await;
    assert!(result.ok, "trusted runtime mailchimp tool should succeed");
    let stdout = std::str::from_utf8(&result.stdout).unwrap();
    assert!(stdout.contains("\"list-1\""), "stdout was: {stdout}");
    assert!(stdout.contains("\"Players\""), "stdout was: {stdout}");
}
