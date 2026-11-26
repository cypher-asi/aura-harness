use super::*;

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
