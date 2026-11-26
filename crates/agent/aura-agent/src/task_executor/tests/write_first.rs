use super::*;

// ------------------------------------------------------------------
// Phase 1 contract — TaskToolExecutor accepts writes / task_done
// without ever requiring a `submit_plan` call (harness-v2).
// ------------------------------------------------------------------

/// `make_executor` already starts in `TaskPhase::Implementing`, but the
/// FIRST tool the executor sees in this test is a `write_file`. Pre-
/// Phase-1, that call was rejected from `Exploring` until `submit_plan`
/// flipped the phase. Phase 1 changed the production executor's initial
/// phase to `Implementing` (see `agent_runner::execute_task_tracked`),
/// so the very first interaction may be a write — pin that here at the
/// executor level so a future revert to a write-gated `Exploring`
/// default fails loudly.
#[tokio::test]
async fn task_tool_executor_accepts_write_file_as_first_interaction() {
    let executor = make_executor();
    let call = ToolCallInfo {
        id: "wf_first".into(),
        name: "write_file".into(),
        input: serde_json::json!({
            "path": "src/lib.rs",
            "content": "pub fn first() {}",
        }),
    };

    let results = executor.execute(&[call]).await;

    assert_eq!(results.len(), 1);
    assert!(
        !results[0].is_error,
        "first-iteration write_file must reach the inner executor (Phase 1 contract): {}",
        results[0].content,
    );
    let ops = executor.tracked_file_ops.lock().await;
    assert_eq!(
        ops.len(),
        1,
        "first-iteration write_file must record a tracked file op",
    );
    let phase = executor.task_phase.lock().await;
    assert!(
        matches!(*phase, TaskPhase::Implementing { .. }),
        "executor must remain in Implementing after a successful write \
         (Phase 1 contract: writes do not transition phase)",
    );
}

/// End-to-end Phase 1 pin: a fresh executor accepts `write_file`,
/// then a self-review `read_file`, then `task_done` — no
/// `submit_plan` anywhere in the sequence. Pre-Phase-1, the
/// `task_done` call would have been rejected by the write-gate
/// because the executor started in `Exploring`; the only way out
/// was a valid `submit_plan`. Phase 1 dropped that gate, and this
/// test pins the new contract end-to-end through the executor's
/// public surface.
#[tokio::test]
async fn write_file_then_task_done_succeeds_without_submit_plan() {
    let executor = make_executor();

    let write = ToolCallInfo {
        id: "wf_1".into(),
        name: "write_file".into(),
        input: serde_json::json!({
            "path": "src/lib.rs",
            "content": "pub fn answer() -> u32 { 42 }",
        }),
    };
    let write_results = executor.execute(&[write]).await;
    assert_eq!(write_results.len(), 1);
    assert!(
        !write_results[0].is_error,
        "write_file must succeed without submit_plan: {}",
        write_results[0].content,
    );

    // The completion guard requires a self-review read of every
    // modified file before `task_done` — re-reading is part of the
    // existing DoD-precheck contract that Phase 1 explicitly does
    // NOT change. Drive it through the same `execute` surface so
    // we don't bypass the self-review tracker.
    let read = ToolCallInfo {
        id: "rf_1".into(),
        name: "read_file".into(),
        input: serde_json::json!({"path": "src/lib.rs"}),
    };
    let read_results = executor.execute(&[read]).await;
    assert!(!read_results[0].is_error, "self-review read must succeed");

    let done = task_done_call("implemented answer()");
    let done_results = executor.execute(&[done]).await;
    assert_eq!(done_results.len(), 1);
    assert!(
        !done_results[0].is_error,
        "task_done must succeed after write_file + self-review read \
         without submit_plan: {}",
        done_results[0].content,
    );
    assert!(
        done_results[0].stop_loop,
        "successful task_done must request loop stop",
    );

    // The executor must have NEVER seen a submit_plan tool call. The
    // only way `task_done` can have succeeded is if the write-gate is
    // gone, which is the headline Phase 1 contract. The tracked
    // file-op count is checked against the executor's internal
    // mutex directly because Phase 7 dropped the cross-layer mirror
    // on `TaskExecutionResult`.
    let mut result = TaskExecutionResult::default();
    executor.merge_into_result(&mut result).await;
    assert_eq!(
        executor.tracked_file_ops.lock().await.len(),
        1,
        "executor must have tracked the write-gate-less write",
    );
    assert!(
        !*executor.no_changes_needed.lock().await,
        "no_changes_needed must stay false on the write path",
    );
}

/// `handle_submit_plan` must flip the shared exploration-reset signal
/// on the successful `Ok(())` branch so the wrapping agent loop knows
/// to zero its exploration/read-guard counters at the next iteration.
#[tokio::test]
async fn submit_plan_flips_reset_signal() {
    let signal = Arc::new(AtomicBool::new(false));
    let executor = make_executor_with_signal(Arc::clone(&signal));

    let plan_call = ToolCallInfo {
        id: "sp_1".into(),
        name: "submit_plan".into(),
        input: serde_json::json!({
            "approach": "fix the bug by adding a null check that prevents the crash",
            "files_to_modify": ["src/main.rs"],
            "key_decisions": ["use an early return"],
        }),
    };
    let results = executor.execute(&[plan_call]).await;
    assert!(!results[0].is_error, "plan should be accepted");

    assert!(
        signal.load(Ordering::Acquire),
        "reset signal must be flipped to true after successful plan acceptance"
    );
}

/// Companion: a rejected `submit_plan` must NOT flip the reset signal,
/// otherwise the agent loop would reset counters every time the model
/// submitted a malformed plan.
#[tokio::test]
async fn invalid_plan_does_not_flip_reset_signal() {
    let signal = Arc::new(AtomicBool::new(false));
    let executor = make_executor_with_signal(Arc::clone(&signal));

    let plan_call = ToolCallInfo {
        id: "sp_1".into(),
        name: "submit_plan".into(),
        input: serde_json::json!({
            "approach": "short",
            "files_to_modify": [],
        }),
    };
    let results = executor.execute(&[plan_call]).await;
    assert!(results[0].is_error, "plan should be rejected");

    assert!(
        !signal.load(Ordering::Acquire),
        "reset signal must NOT be flipped on rejected plan"
    );
}
