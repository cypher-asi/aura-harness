use super::*;

// ------------------------------------------------------------------
// task_done guard tests
// ------------------------------------------------------------------

#[tokio::test]
async fn task_done_rejects_when_no_file_ops() {
    // After the 2026-05 strip, the rejection no longer distinguishes
    // exploring vs implementing — there is no write gate, so any
    // `task_done` without file ops gets the same "make some changes"
    // message regardless of phase.
    let executor = make_executor();
    let calls = [task_done_call("all done")];
    let results = executor.execute(&calls).await;

    assert_eq!(results.len(), 1);
    assert!(results[0].is_error);
    assert!(!results[0].stop_loop);
    let body = &results[0].content;
    assert!(
        body.contains("have not produced any file changes"),
        "expected file-ops rejection wording: {body}"
    );
    assert!(
        body.contains("no_changes_needed"),
        "rejection must keep the no_changes_needed escape hatch: {body}"
    );
}

#[tokio::test]
async fn task_done_no_writes_rejection_blocks_exploration_detours() {
    let executor = make_executor();
    let rejected = executor
        .execute(&[task_done_call("already implemented")])
        .await;
    assert!(rejected[0].is_error);

    let detour = executor.execute(&[read_file_call("src/lib.rs")]).await;
    assert_eq!(detour.len(), 1);
    assert!(detour[0].is_error);
    assert!(detour[0]
        .content
        .contains("task_done was just rejected because no file changes were produced"));
    assert!(detour[0].content.contains("no_changes_needed: true"));
}

#[tokio::test]
async fn task_done_no_changes_still_passes_after_no_write_rejection() {
    let executor = make_executor();
    let rejected = executor
        .execute(&[task_done_call("already implemented")])
        .await;
    assert!(rejected[0].is_error);

    let done = executor
        .execute(&[task_done_no_changes(
            "outbox CF is already implemented and wired",
        )])
        .await;
    assert_eq!(done.len(), 1);
    assert!(!done[0].is_error);
    assert!(done[0].stop_loop);
}

#[tokio::test]
async fn engine_tools_and_guard_blocks_settle_through_the_inner_executor() {
    let inner = Arc::new(DenyingInner::default());
    let mut executor = make_executor();
    executor.inner = inner.clone();

    let denied = executor.execute(&[task_done_call("all done")]).await;
    assert!(denied[0].is_error);
    assert!(!denied[0].stop_loop);
    assert!(executor.notes.lock().await.is_empty());
    assert!(
        !executor
            .recent_tool_outcomes
            .lock()
            .await
            .task_done_no_writes_rejected
    );

    executor
        .recent_tool_outcomes
        .lock()
        .await
        .task_done_no_writes_rejected = true;
    let blocked = executor.execute(&[read_file_call("src/lib.rs")]).await;
    assert!(blocked[0].is_error);

    assert_eq!(
        *inner.settled.lock().unwrap(),
        vec![
            ("task_done".to_string(), "task_executor"),
            ("read_file".to_string(), "task_guard"),
        ]
    );
}

#[tokio::test]
async fn task_done_succeeds_with_file_ops() {
    let executor = make_executor();
    {
        let mut ops = executor.tracked_file_ops.lock().await;
        ops.push(FileOp::Create {
            path: "src/main.rs".to_string(),
            content: "fn main() {}".to_string(),
        });
    }
    {
        let mut sr = executor.self_review.lock().await;
        sr.record_write("src/main.rs");
        sr.record_read("src/main.rs");
    }

    let calls = [task_done_call("implemented feature")];
    let results = executor.execute(&calls).await;

    assert_eq!(results.len(), 1);
    assert!(!results[0].is_error);
    assert!(results[0].stop_loop);
    assert!(results[0].content.contains("completed"));
}

#[tokio::test]
async fn task_done_allows_no_ops_with_exemption() {
    let executor = make_executor();
    let calls = [task_done_no_changes(
        "analysis task, no code changes required",
    )];
    let results = executor.execute(&calls).await;

    assert_eq!(results.len(), 1);
    assert!(!results[0].is_error);
    assert!(results[0].stop_loop);
    assert!(results[0].content.contains("completed"));
}

#[tokio::test]
async fn task_done_rejects_when_build_command_fails() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut executor = make_executor();
    executor.project_folder = dir.path().to_string_lossy().into_owned();
    #[cfg(unix)]
    {
        executor.build_command = Some("false".to_string());
    }
    #[cfg(windows)]
    {
        executor.build_command = Some("cmd /C exit 1".to_string());
    }
    {
        let mut ops = executor.tracked_file_ops.lock().await;
        ops.push(FileOp::Create {
            path: "src/main.rs".to_string(),
            content: "fn main() {}".to_string(),
        });
    }
    {
        let mut sr = executor.self_review.lock().await;
        sr.record_write("src/main.rs");
        sr.record_read("src/main.rs");
    }

    let results = executor.execute(&[task_done_call("done")]).await;
    assert_eq!(results.len(), 1);
    assert!(
        results[0].is_error,
        "task_done must be rejected when build fails: {}",
        results[0].content
    );
    assert!(
        !results[0].stop_loop,
        "rejected task_done must not stop the loop"
    );
    assert!(
        results[0].content.contains("Build check failed"),
        "rejection must include build failure context: {}",
        results[0].content
    );
}
