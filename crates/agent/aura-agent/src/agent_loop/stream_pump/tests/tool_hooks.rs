use super::*;

/// `pump_cache_hit_short_circuits_tool_spawn` (E.4 mandatory):
/// when `state.tool_cache.exact` already has a hit for a cacheable
/// tool's input, the pump must serve the cached result *without*
/// running the tool, handing it to the executor as a preset so the
/// call is still recorded and gated. The other (uncached) call in the
/// same model response is still spawned.
#[tokio::test(start_paused = true)]
async fn pump_cache_hit_short_circuits_tool_spawn() {
    let executor = CountingExecutor::default();
    let config = AgentLoopConfig::for_agent("claude-test-model");
    let cached_input = serde_json::json!({});
    let cache_key = aura_config::tool_result_cache_key("read_file", &cached_input);
    let mut state = crate::agent_loop::LoopState::new_for_tests(&config, Vec::new());
    state
        .tool_cache
        .exact
        .insert(cache_key, "cached-payload".to_string());

    let stream = mk_stream(vec![
        mk_call("toolu_cached", "read_file"),
        mk_call("toolu_fresh", "run_command"),
        completed(false),
    ]);

    let outcome = drive_stream(
        test_ctx(&config, &executor),
        stream,
        &mut state,
        "test-model",
    )
    .await;
    assert_cached_then_fresh(outcome);
    let invocations = executor.invocations.lock().await;
    let invoked_ids: Vec<_> = invocations.iter().map(|c| c.id.clone()).collect();
    assert_eq!(
        invoked_ids,
        vec!["toolu_fresh".to_string()],
        "executor must be invoked ONLY for the uncached tool; cache hits short-circuit spawn"
    );
    assert_eq!(
        executor.presets(),
        vec![("toolu_cached".to_string(), "cache")],
        "a cache hit must still reach the executor as a preset"
    );
}

/// The cached call's memoised result, then the fresh call's, in the
/// order they were submitted.
fn assert_cached_then_fresh(outcome: StreamPumpOutcome) {
    match outcome {
        StreamPumpOutcome::Completed { tool_results, .. } => {
            let ids: Vec<_> = tool_results.iter().map(|(c, _)| c.id.clone()).collect();
            assert_eq!(
                ids,
                vec!["toolu_cached", "toolu_fresh"],
                "cached + spawned results must preserve FIFO submission order"
            );
            let cached_result = &tool_results[0].1;
            assert!(
                !cached_result.is_error,
                "cached hit must surface as a non-error"
            );
            assert_eq!(
                cached_result.content, "cached-payload",
                "cached hit must return the memoised payload verbatim"
            );
        }
        other => panic!("expected Completed, got {other:?}"),
    }
}

#[derive(Default)]
struct BuildSpyExecutor {
    build_calls: tokio::sync::Mutex<u32>,
}

#[async_trait]
impl AgentToolExecutor for BuildSpyExecutor {
    async fn execute(&self, tool_calls: &[ToolCallInfo]) -> Vec<ToolCallResult> {
        tool_calls
            .iter()
            .map(|tc| successful_write_result(&tc.id))
            .collect()
    }
    async fn auto_build_check(&self) -> Option<crate::types::AutoBuildResult> {
        *self.build_calls.lock().await += 1;
        Some(crate::types::AutoBuildResult {
            success: false,
            output: "compile error: missing semicolon".into(),
            error_count: 1,
        })
    }
}

/// A successful `write_file` result that modified `src/foo.rs`.
fn successful_write_result(tool_use_id: &str) -> ToolCallResult {
    ToolCallResult {
        tool_use_id: tool_use_id.to_string(),
        content: "wrote".to_string(),
        is_error: false,
        kind: aura_core_types::ToolResultKind::Ok,
        stop_loop: false,
        file_changes: vec![crate::types::FileChange {
            path: "src/foo.rs".into(),
            kind: crate::types::FileChangeKind::Modify,
            lines_added: 3,
            lines_removed: 0,
        }],
        image: None,
    }
}

/// An assistant response asking for one `write_file` call, plus that call.
fn write_file_response(tool_use_id: &str) -> (ModelResponse, ToolCallInfo) {
    let input = serde_json::json!({"path": "src/foo.rs", "content": "fn a(){}"});
    let response = ModelResponse::new(
        StopReason::ToolUse,
        Message::new(
            Role::Assistant,
            vec![ContentBlock::tool_use(
                tool_use_id,
                "write_file",
                input.clone(),
            )],
        ),
        Usage::new(1, 1),
        ProviderTrace::new("test", 0),
    );
    let tool_call = ToolCallInfo {
        id: tool_use_id.to_string(),
        name: "write_file".to_string(),
        input,
    };
    (response, tool_call)
}

fn has_build_warning(messages: &[Message]) -> bool {
    messages.iter().any(|m| {
        m.content.iter().any(|b| match b {
            ContentBlock::Text { text } => text.contains("Build check failed"),
            ContentBlock::ToolResult {
                content: aura_model::ToolResultContent::Text(t),
                ..
            } => t.contains("Build check failed"),
            _ => false,
        })
    })
}

/// `pump_triggers_auto_build_on_write` (Phase 4 unified): a
/// successful `write_file` flowing through the pump fires the
/// `run_auto_build` step inside
/// [`crate::agent_loop::tool_pipeline::process_tool_results`], mirroring
/// the buffered path. The failing-build text is appended to the
/// trailing tool_result-bearing user message via
/// `push_tool_result_message`, so the existence of that side
/// message in `state.messages` is the observable proof.
#[tokio::test(start_paused = true)]
async fn pump_triggers_auto_build_on_write() {
    let executor = BuildSpyExecutor::default();
    let config = AgentLoopConfig {
        auto_build_cooldown: 0,
        ..AgentLoopConfig::for_agent("claude-test-model")
    };
    let (response, tool_call) = write_file_response("toolu_w");
    let tool_result = successful_write_result("toolu_w");
    let mut state = crate::agent_loop::LoopState::new_for_tests(&config, Vec::new());
    let agent = crate::agent_loop::AgentLoop::new(config.clone());

    // Drive only the post-stream dispatch path — the pre-stream
    // pump already has its own coverage, and the auto-build
    // wiring lives inside the unified `tool_pipeline::dispatch`.
    let ctx = crate::agent_loop::tool_pipeline::ToolEffectCtx {
        executor: &executor,
        event_tx: None,
    };
    let _should_break = crate::agent_loop::tool_pipeline::dispatch(
        &agent,
        &mut state,
        &response,
        crate::agent_loop::tool_pipeline::ToolBatch::PreExecuted(vec![(tool_call, tool_result)]),
        ctx,
    )
    .await;

    let calls = *executor.build_calls.lock().await;
    assert_eq!(
        calls, 1,
        "successful write must trigger auto_build_check exactly once on the pump path"
    );
    assert!(
        has_build_warning(&state.messages),
        "failing auto-build output must surface in the tool_result-bearing message"
    );
}
