use super::*;

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
