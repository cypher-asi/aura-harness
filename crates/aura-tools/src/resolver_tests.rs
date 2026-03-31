use super::*;
use aura_core::{ActionId, AgentId};
use aura_kernel::ExecuteContext;
use tempfile::TempDir;

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

#[test]
fn resolver_has_builtin_tools() {
    let (_cat, resolver) = make_catalog_and_resolver();
    let tools = resolver.visible_tools(ToolProfile::Core);
    let names: std::collections::HashSet<_> = tools.iter().map(|t| t.name.as_str()).collect();
    assert!(names.contains("read_file"));
}

#[test]
fn visible_tools_returns_core() {
    let (_cat, resolver) = make_catalog_and_resolver();
    let tools = resolver.visible_tools(ToolProfile::Core);
    let names: std::collections::HashSet<_> = tools.iter().map(|t| t.name.as_str()).collect();
    assert!(names.contains("read_file"));
}

#[tokio::test]
async fn execute_builtin_tool() {
    let (_cat, resolver) = make_catalog_and_resolver();
    let (ctx, dir) = test_context();
    std::fs::write(dir.path().join("hello.txt"), "world").unwrap();

    let tc = ToolCall::fs_ls(".");
    let action = Action::delegate_tool(&tc).unwrap();
    let effect = resolver.execute(&ctx, &action).await.unwrap();
    assert_eq!(effect.status, EffectStatus::Committed);
}

#[tokio::test]
async fn unknown_tool_returns_failed_effect() {
    let (_cat, resolver) = make_catalog_and_resolver();
    let (ctx, _dir) = test_context();

    let tc = ToolCall::new("no_such_tool", serde_json::json!({}));
    let action = Action::delegate_tool(&tc).unwrap();
    let effect = resolver.execute(&ctx, &action).await.unwrap();
    assert_eq!(effect.status, EffectStatus::Failed);
    let result: ToolResult = serde_json::from_slice(&effect.payload).unwrap();
    let err_msg = std::str::from_utf8(&result.stderr).unwrap();
    assert!(
        err_msg.contains("unknown tool"),
        "truly unknown tool should say 'unknown tool', got: {err_msg}",
    );
}

#[tokio::test]
async fn domain_tool_without_executor_falls_through_to_unknown() {
    let (_cat, resolver) = make_catalog_and_resolver();
    let (ctx, _dir) = test_context();

    let tc = ToolCall::new("create_spec", serde_json::json!({"project_id": "p1"}));
    let action = Action::delegate_tool(&tc).unwrap();
    let effect = resolver.execute(&ctx, &action).await.unwrap();
    assert_eq!(effect.status, EffectStatus::Failed);
    let result: ToolResult = serde_json::from_slice(&effect.payload).unwrap();
    let err_msg = std::str::from_utf8(&result.stderr).unwrap();
    assert!(
        err_msg.contains("unknown tool"),
        "domain tool without executor should now be 'unknown tool', got: {err_msg}",
    );
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
            })
        }
        async fn update_spec(
            &self,
            _: &str,
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
        async fn orbit_api_call(
            &self,
            _: &str,
            _: &str,
            _: Option<&serde_json::Value>,
            _: Option<&str>,
        ) -> anyhow::Result<String> {
            Ok("{}".into())
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

#[tokio::test]
async fn domain_tool_succeeds_with_inaccessible_workspace() {
    use crate::domain_tools::DomainToolExecutor;

    let cat = Arc::new(ToolCatalog::new());
    let resolver = ToolResolver::new(cat, ToolConfig::default()).with_domain_executor(Arc::new(
        DomainToolExecutor::new(Arc::new(stub_domain::StubDomainApi)),
    ));

    let ctx = ExecuteContext::new(
        AgentId::generate(),
        ActionId::generate(),
        std::path::PathBuf::from("/nonexistent/impossible/workspace"),
    );

    let tc = ToolCall::new(
        "create_spec",
        serde_json::json!({
            "project_id": "p1",
            "title": "Hello World",
            "content": "# Hello"
        }),
    );
    let result = resolver.execute_tool(&ctx, &tc).await;
    assert!(
        result.is_ok(),
        "domain tool should succeed even with inaccessible workspace"
    );
    let tr = result.unwrap();
    let stdout = std::str::from_utf8(&tr.stdout).unwrap();
    assert!(
        stdout.contains("\"ok\":true"),
        "create_spec should return ok:true, got: {stdout}"
    );
}

#[tokio::test]
async fn get_project_succeeds_with_inaccessible_workspace() {
    use crate::domain_tools::DomainToolExecutor;

    let cat = Arc::new(ToolCatalog::new());
    let resolver = ToolResolver::new(cat, ToolConfig::default()).with_domain_executor(Arc::new(
        DomainToolExecutor::new(Arc::new(stub_domain::StubDomainApi)),
    ));

    let ctx = ExecuteContext::new(
        AgentId::generate(),
        ActionId::generate(),
        std::path::PathBuf::from("/nonexistent/impossible/workspace"),
    );

    let tc = ToolCall::new("get_project", serde_json::json!({"project_id": "p1"}));
    let result = resolver.execute_tool(&ctx, &tc).await;
    assert!(
        result.is_ok(),
        "get_project should succeed even with inaccessible workspace"
    );
    let tr = result.unwrap();
    let stdout = std::str::from_utf8(&tr.stdout).unwrap();
    assert!(
        stdout.contains("\"ok\":true"),
        "get_project should return ok:true, got: {stdout}"
    );
}

#[tokio::test]
async fn fs_disabled_returns_failed() {
    let cat = Arc::new(ToolCatalog::new());
    let mut config = ToolConfig::default();
    config.enable_fs = false;
    let resolver = ToolResolver::new(cat, config);
    let (ctx, _dir) = test_context();

    let tc = ToolCall::fs_read("test.txt", None);
    let action = Action::delegate_tool(&tc).unwrap();
    let effect = resolver.execute(&ctx, &action).await.unwrap();
    assert_eq!(effect.status, EffectStatus::Failed);
}

#[test]
fn every_exposed_core_tool_has_handler() {
    let (_cat, resolver) = make_catalog_and_resolver();
    let core = _cat.tools_for_profile(ToolProfile::Core);
    for t in &core {
        let has_handler = resolver.inner.has_tool(&t.name);
        assert!(
            has_handler,
            "core tool '{}' has no built-in handler",
            t.name,
        );
    }
}
