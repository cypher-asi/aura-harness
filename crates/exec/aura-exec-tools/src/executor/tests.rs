
use super::*;
use aura_core_types::{ActionId, AgentId};
use aura_exec_traits::ExecuteContext;
use tempfile::TempDir;

fn create_test_context() -> (ExecuteContext, TempDir) {
    let dir = TempDir::new().unwrap();
    let ctx = ExecuteContext::new(
        AgentId::generate(),
        ActionId::generate(),
        dir.path().to_path_buf(),
    );
    (ctx, dir)
}

#[tokio::test]
async fn test_fs_ls_tool() {
    let (ctx, dir) = create_test_context();
    std::fs::write(dir.path().join("test.txt"), "hello").unwrap();

    let executor = ToolExecutor::with_defaults();
    let tool_call = ToolCall::fs_ls(".");
    let action = Action::delegate_tool(&tool_call).unwrap();

    let effect = executor.execute(&ctx, &action).await.unwrap();
    assert_eq!(effect.status, EffectStatus::Committed);

    let result: ToolResult = serde_json::from_slice(&effect.payload).unwrap();
    assert!(result.ok);
    let output = String::from_utf8_lossy(&result.stdout);
    assert!(output.contains("test.txt"));
}

#[tokio::test]
async fn test_fs_read_tool() {
    let (ctx, dir) = create_test_context();
    std::fs::write(dir.path().join("test.txt"), "Hello, Aura!").unwrap();

    let executor = ToolExecutor::with_defaults();
    let tool_call = ToolCall::fs_read("test.txt", None);
    let action = Action::delegate_tool(&tool_call).unwrap();

    let effect = executor.execute(&ctx, &action).await.unwrap();
    assert_eq!(effect.status, EffectStatus::Committed);

    let result: ToolResult = serde_json::from_slice(&effect.payload).unwrap();
    assert!(result.ok);
    assert_eq!(&result.stdout[..], b"1|Hello, Aura!");
}

#[tokio::test]
async fn test_sandbox_violation() {
    let (ctx, _dir) = create_test_context();

    let executor = ToolExecutor::with_defaults();
    let tool_call = ToolCall::fs_read("../../../etc/passwd", None);
    let action = Action::delegate_tool(&tool_call).unwrap();

    let effect = executor.execute(&ctx, &action).await.unwrap();
    assert_eq!(effect.status, EffectStatus::Failed);

    let result: ToolResult = serde_json::from_slice(&effect.payload).unwrap();
    assert!(!result.ok);
}

#[tokio::test]
async fn test_cmd_disabled() {
    let (ctx, _dir) = create_test_context();

    let config = ToolConfig::default();
    let executor = ToolExecutor::new(config);
    let tool_call = ToolCall::new("run_command", serde_json::json!({"program": "ls"}));
    let action = Action::delegate_tool(&tool_call).unwrap();

    let effect = executor.execute(&ctx, &action).await.unwrap();
    assert_eq!(effect.status, EffectStatus::Failed);
}

#[tokio::test]
async fn test_unknown_tool() {
    let (ctx, _dir) = create_test_context();

    let executor = ToolExecutor::with_defaults();
    let tool_call = ToolCall::new("nonexistent_tool", serde_json::json!({}));
    let action = Action::delegate_tool(&tool_call).unwrap();

    let effect = executor.execute(&ctx, &action).await.unwrap();
    assert_eq!(effect.status, EffectStatus::Failed);

    let result: ToolResult = serde_json::from_slice(&effect.payload).unwrap();
    assert!(!result.ok);
}

/// `(event, path, file existed, succeeded)` as a hook heard it.
type HookCall = (String, std::path::PathBuf, bool, Option<bool>);

/// Records what a [`FileChangeHook`] is told, with whether the file
/// existed at the time.
#[derive(Default)]
struct RecordingHook {
    calls: std::sync::Mutex<Vec<HookCall>>,
}

#[async_trait]
impl FileChangeHook for RecordingHook {
    async fn before_change(&self, path: &std::path::Path, _tool_use_id: Option<&str>) {
        self.calls.lock().unwrap().push((
            "before".to_string(),
            path.to_path_buf(),
            path.exists(),
            None,
        ));
    }

    async fn after_change(
        &self,
        path: &std::path::Path,
        _tool_use_id: Option<&str>,
        succeeded: bool,
    ) {
        self.calls.lock().unwrap().push((
            "after".to_string(),
            path.to_path_buf(),
            path.exists(),
            Some(succeeded),
        ));
    }
}

#[tokio::test]
async fn file_change_hook_brackets_file_writes_only() {
    let (ctx, dir) = create_test_context();
    let hook = Arc::new(RecordingHook::default());
    let executor = ToolExecutor::with_defaults().with_file_change_hook(hook.clone());

    let write = ToolCall::new(
        "write_file",
        serde_json::json!({"path": "new.txt", "content": "hi"}),
    );
    let action = Action::delegate_tool(&write).unwrap();
    executor.execute(&ctx, &action).await.unwrap();
    let read = ToolCall::fs_read("new.txt", None);
    let action = Action::delegate_tool(&read).unwrap();
    executor.execute(&ctx, &action).await.unwrap();

    let calls = hook.calls.lock().unwrap().clone();
    let target = dir.path().canonicalize().unwrap().join("new.txt");
    assert_eq!(
        calls,
        [
            ("before".to_string(), target.clone(), false, None),
            ("after".to_string(), target, true, Some(true)),
        ]
    );
}

#[tokio::test]
async fn file_change_hook_hears_about_failed_edits() {
    let (ctx, dir) = create_test_context();
    std::fs::write(dir.path().join("a.txt"), "one").unwrap();
    let hook = Arc::new(RecordingHook::default());
    let executor = ToolExecutor::with_defaults().with_file_change_hook(hook.clone());

    let edit = ToolCall::new(
        "edit_file",
        serde_json::json!({"path": "a.txt", "old_text": "missing", "new_text": "x"}),
    );
    let action = Action::delegate_tool(&edit).unwrap();
    executor.execute(&ctx, &action).await.unwrap();

    let calls = hook.calls.lock().unwrap().clone();
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[1].3, Some(false));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_edits_to_one_file_all_land() {
    let (ctx, dir) = create_test_context();
    let count = 20;
    let original: String = (0..count).map(|i| format!("item-{i:02}\n")).collect();
    std::fs::write(dir.path().join("many.txt"), &original).unwrap();
    let executor = ToolExecutor::with_defaults();

    let actions: Vec<Action> = (0..count)
        .map(|i| {
            let edit = ToolCall::new(
                "edit_file",
                serde_json::json!({
                    "path": "many.txt",
                    "old_text": format!("item-{i:02}"),
                    "new_text": format!("done-{i:02}"),
                }),
            );
            Action::delegate_tool(&edit).unwrap()
        })
        .collect();
    let effects =
        futures_util::future::join_all(actions.iter().map(|a| executor.execute(&ctx, a))).await;
    for effect in effects {
        assert_eq!(effect.unwrap().status, EffectStatus::Committed);
    }

    let expected: String = (0..count).map(|i| format!("done-{i:02}\n")).collect();
    assert_eq!(
        std::fs::read_to_string(dir.path().join("many.txt")).unwrap(),
        expected,
        "every edit that reported success must be on disk"
    );
}

#[tokio::test]
async fn test_register_custom_tool() {
    let mut executor = ToolExecutor::with_defaults();
    assert!(executor.tools.contains_key("list_files"));
    assert!(!executor.tools.contains_key("custom_tool"));

    // Custom tools can be registered at runtime
    struct DummyTool;

    #[async_trait]
    impl Tool for DummyTool {
        fn name(&self) -> &str {
            "custom_tool"
        }
        fn definition(&self) -> aura_core_types::ToolDefinition {
            aura_core_types::ToolDefinition::new(
                "custom_tool",
                "A test tool",
                serde_json::json!({"type": "object"}),
            )
        }
        async fn execute(
            &self,
            _ctx: &ToolContext,
            _args: serde_json::Value,
        ) -> Result<ToolResult, ToolError> {
            Ok(ToolResult::success("custom_tool", "ok"))
        }
    }

    executor.register(Box::new(DummyTool));
    assert!(executor.tools.contains_key("custom_tool"));
}
