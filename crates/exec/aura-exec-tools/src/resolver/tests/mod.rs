use super::*;

use aura_core_types::{
    ActionId, AgentId, InstalledToolDefinition, InstalledToolRuntimeAuth,
    InstalledToolRuntimeExecution, InstalledToolRuntimeIntegration,
    InstalledToolRuntimeProviderExecution, ToolAuth,
};

use aura_exec_traits::ExecuteContext;

use serde_json::Value;

use std::io::{Read, Write};

use std::net::TcpListener;

use tempfile::TempDir;

const TRUSTED_INTEGRATION_RUNTIME_METADATA_KEY: &str = "trusted_integration_runtime";

fn make_catalog_and_resolver() -> (Arc<ToolCatalog>, ToolResolver) {
    let cat = Arc::new(ToolCatalog::new());
    let resolver = ToolResolver::new(cat.clone(), ToolConfig::default());
    (cat, resolver)
}

fn test_context() -> (ExecuteContext, TempDir) {
    let dir = TempDir::new().unwrap();
    let ctx = ExecuteContext::new(
        AgentId::generate(),
        ActionId::generate(),
        dir.path().to_path_buf(),
    );
    (ctx, dir)
}

fn spawn_single_response_server(
    expected_auth: Option<&str>,
    response_status: &str,
    response_body: &str,
) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let expected_auth = expected_auth.map(ToOwned::to_owned);
    let response_status = response_status.to_string();
    let response_body = response_body.to_string();
    std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut buf = [0_u8; 8192];
        let n = stream.read(&mut buf).unwrap();
        let request = String::from_utf8_lossy(&buf[..n]);
        assert!(
            request.starts_with("POST ") || request.starts_with("GET "),
            "request was: {request}"
        );
        assert!(
            request.contains("content-type: application/json")
                || request.contains("Content-Type: application/json")
        );
        if let Some(expected) = expected_auth {
            assert!(
                request.contains(&format!("Authorization: {expected}"))
                    || request.contains(&format!("authorization: {expected}")),
                "missing auth header in request: {request}"
            );
        }
        let response = format!(
            "HTTP/1.1 {response_status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{response_body}",
            response_body.len()
        );
        stream.write_all(response.as_bytes()).unwrap();
        stream.flush().unwrap();
    });
    format!("http://{addr}")
}

fn trusted_runtime_metadata(spec: serde_json::Value) -> std::collections::HashMap<String, Value> {
    let mut metadata = std::collections::HashMap::new();
    metadata.insert(TRUSTED_INTEGRATION_RUNTIME_METADATA_KEY.to_string(), spec);
    metadata
}

fn notion_runtime_execution(endpoint: String) -> InstalledToolRuntimeExecution {
    InstalledToolRuntimeExecution::AppProvider(InstalledToolRuntimeProviderExecution {
        provider: "notion".into(),
        base_url: endpoint,
        static_headers: std::collections::HashMap::from([(
            "Notion-Version".to_string(),
            "2026-03-11".to_string(),
        )]),
        integrations: vec![InstalledToolRuntimeIntegration {
            integration_id: "notion-default".into(),
            base_url: None,
            auth: InstalledToolRuntimeAuth::AuthorizationBearer {
                token: "notion-token".into(),
            },
            provider_config: std::collections::HashMap::new(),
        }],
    })
}

fn notion_installed_tool(tool_name: &str, endpoint: String) -> InstalledToolDefinition {
    InstalledToolDefinition {
        name: tool_name.into(),
        description: format!("Notion runtime tool {tool_name}"),
        input_schema: serde_json::json!({"type":"object"}),
        endpoint: "http://unused.local".into(),
        auth: ToolAuth::None,
        timeout_ms: Some(5_000),
        namespace: Some("aura_org_tools".into()),
        required_integration: None,
        runtime_execution: Some(notion_runtime_execution(endpoint)),
        metadata: std::collections::HashMap::new(),
    }
}

async fn assert_notion_runtime_tool(
    tool_name: &str,
    args: Value,
    expected_method: &str,
    expected_request_parts: &[&str],
    response_body: &str,
    expected_stdout: &str,
) {
    let (_cat, resolver) = make_catalog_and_resolver();
    let mut expected_parts = vec![
        "authorization: Bearer notion-token",
        "notion-version: 2026-03-11",
    ];
    expected_parts.extend_from_slice(expected_request_parts);
    let endpoint =
        spawn_asserting_request_server(expected_method, &expected_parts, "200 OK", response_body);
    let resolver = resolver.with_installed_tools(vec![notion_installed_tool(tool_name, endpoint)]);
    let (ctx, _dir) = test_context();

    let tc = ToolCall::new(tool_name, args);
    let action = Action::delegate_tool(&tc).unwrap();
    let effect = resolver.execute(&ctx, &action).await.unwrap();
    assert_eq!(effect.status, EffectStatus::Committed);
    let result: ToolResult = serde_json::from_slice(&effect.payload).unwrap();
    assert!(result.ok, "{tool_name} should succeed");
    let stdout = std::str::from_utf8(&result.stdout).unwrap();
    assert!(
        stdout.contains(expected_stdout),
        "expected `{expected_stdout}` in stdout for {tool_name}, got: {stdout}"
    );
}

fn spawn_asserting_response_server(
    expected_method: &str,
    expected_headers: &[(&str, &str)],
    response_status: &str,
    response_body: &str,
) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let expected_method = expected_method.to_string();
    let expected_headers = expected_headers
        .iter()
        .map(|(name, value)| ((*name).to_string(), (*value).to_string()))
        .collect::<Vec<_>>();
    let response_status = response_status.to_string();
    let response_body = response_body.to_string();
    std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut buf = [0_u8; 8192];
        let n = stream.read(&mut buf).unwrap();
        let request = String::from_utf8_lossy(&buf[..n]);
        assert!(
            request.starts_with(&format!("{expected_method} ")),
            "request was: {request}"
        );
        for (name, value) in expected_headers {
            assert!(
                request.contains(&format!("{name}: {value}"))
                    || request.contains(&format!("{}: {}", name.to_ascii_lowercase(), value)),
                "missing header `{name}: {value}` in request: {request}"
            );
        }
        let response = format!(
            "HTTP/1.1 {response_status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{response_body}",
            response_body.len()
        );
        stream.write_all(response.as_bytes()).unwrap();
        stream.flush().unwrap();
    });
    format!("http://{addr}")
}

fn spawn_asserting_request_server(
    expected_method: &str,
    expected_request_parts: &[&str],
    response_status: &str,
    response_body: &str,
) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let expected_method = expected_method.to_string();
    let expected_request_parts = expected_request_parts
        .iter()
        .map(|part| (*part).to_string())
        .collect::<Vec<_>>();
    let response_status = response_status.to_string();
    let response_body = response_body.to_string();
    std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut buf = [0_u8; 8192];
        let n = stream.read(&mut buf).unwrap();
        let request = String::from_utf8_lossy(&buf[..n]);
        assert!(
            request.starts_with(&format!("{expected_method} ")),
            "request was: {request}"
        );
        for expected in expected_request_parts {
            assert!(
                request.contains(&expected),
                "missing request part `{expected}` in request: {request}"
            );
        }
        let response = format!(
            "HTTP/1.1 {response_status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{response_body}",
            response_body.len()
        );
        stream.write_all(response.as_bytes()).unwrap();
        stream.flush().unwrap();
    });
    format!("http://{addr}")
}

mod stub_domain {
    use async_trait::async_trait;
    use aura_tools_domain::*;

    pub struct StubDomainApi;

    #[async_trait]
    impl DomainApi for StubDomainApi {
        async fn list_specs(
            &self,
            _: &str,
            _: Option<&str>,
        ) -> anyhow::Result<Vec<SpecDescriptor>> {
            Ok(vec![])
        }
        async fn get_spec(&self, _: &str, _: Option<&str>) -> anyhow::Result<SpecDescriptor> {
            anyhow::bail!("stub")
        }
        async fn create_spec(
            &self,
            _: &str,
            title: &str,
            _: &str,
            _: u32,
            _: Option<&str>,
        ) -> anyhow::Result<SpecDescriptor> {
            Ok(SpecDescriptor {
                id: "s1".into(),
                project_id: "p1".into(),
                title: title.into(),
                content: String::new(),
                order: 0,
                parent_id: None,
                content_hash: None,
            })
        }
        async fn update_spec(
            &self,
            _: &str,
            _: Option<&str>,
            _: Option<&str>,
            _: Option<&str>,
            _: Option<&str>,
        ) -> anyhow::Result<SpecDescriptor> {
            anyhow::bail!("stub")
        }
        async fn delete_spec(&self, _: &str, _: Option<&str>) -> anyhow::Result<()> {
            Ok(())
        }
        async fn list_tasks(
            &self,
            _: &str,
            _: Option<&str>,
            _: Option<&str>,
        ) -> anyhow::Result<Vec<TaskDescriptor>> {
            Ok(vec![])
        }
        async fn create_task(
            &self,
            _: &str,
            _: &str,
            _: &str,
            _: &str,
            _: &[String],
            _: u32,
            _: Option<&str>,
        ) -> anyhow::Result<TaskDescriptor> {
            anyhow::bail!("stub")
        }
        async fn update_task(
            &self,
            _: &str,
            _: TaskUpdate,
            _: Option<&str>,
        ) -> anyhow::Result<TaskDescriptor> {
            anyhow::bail!("stub")
        }
        async fn delete_task(&self, _: &str, _: Option<&str>) -> anyhow::Result<()> {
            Ok(())
        }
        async fn transition_task(
            &self,
            _: &str,
            _: &str,
            _: Option<&str>,
        ) -> anyhow::Result<TaskDescriptor> {
            anyhow::bail!("stub")
        }
        async fn claim_next_task(
            &self,
            _: &str,
            _: &str,
            _: Option<&str>,
        ) -> anyhow::Result<Option<TaskDescriptor>> {
            Ok(None)
        }
        async fn get_task(&self, _: &str, _: Option<&str>) -> anyhow::Result<TaskDescriptor> {
            anyhow::bail!("stub")
        }
        async fn get_project(
            &self,
            project_id: &str,
            _: Option<&str>,
        ) -> anyhow::Result<ProjectDescriptor> {
            Ok(ProjectDescriptor {
                id: project_id.into(),
                name: "test".into(),
                path: String::new(),
                description: None,
                tech_stack: None,
                build_command: None,
                test_command: None,
            })
        }
        async fn update_project(
            &self,
            _: &str,
            _: ProjectUpdate,
            _: Option<&str>,
        ) -> anyhow::Result<ProjectDescriptor> {
            anyhow::bail!("stub")
        }
        async fn create_log(
            &self,
            _: &str,
            _: &str,
            _: &str,
            _: Option<&str>,
            _: Option<&serde_json::Value>,
            _: Option<&str>,
        ) -> anyhow::Result<serde_json::Value> {
            Ok(serde_json::json!({}))
        }
        async fn list_logs(
            &self,
            _: &str,
            _: Option<&str>,
            _: Option<u64>,
            _: Option<&str>,
        ) -> anyhow::Result<serde_json::Value> {
            Ok(serde_json::json!([]))
        }
        async fn get_project_stats(
            &self,
            _: &str,
            _: Option<&str>,
        ) -> anyhow::Result<serde_json::Value> {
            Ok(serde_json::json!({}))
        }
        async fn list_messages(&self, _: &str, _: &str) -> anyhow::Result<Vec<MessageDescriptor>> {
            Ok(vec![])
        }
        async fn save_message(&self, _: SaveMessageParams) -> anyhow::Result<()> {
            Ok(())
        }
        async fn create_session(
            &self,
            _: CreateSessionParams,
        ) -> anyhow::Result<SessionDescriptor> {
            anyhow::bail!("stub")
        }
        async fn get_active_session(&self, _: &str) -> anyhow::Result<Option<SessionDescriptor>> {
            Ok(None)
        }
        async fn network_api_call(
            &self,
            _: &str,
            _: &str,
            _: Option<&serde_json::Value>,
            _: Option<&str>,
        ) -> anyhow::Result<String> {
            Ok("{}".into())
        }
    }

    use crate::domain_tools as aura_tools_domain;
}

mod builtin;
mod installed_http;
mod notion;
mod runtime_providers;
mod trusted_metadata;
mod trusted_providers;
mod workspace;
