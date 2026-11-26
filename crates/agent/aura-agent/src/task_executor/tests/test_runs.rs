use super::*;

#[tokio::test]
async fn task_done_emits_warning_when_tests_fail_but_does_not_retry() {
    // Codex parity replacement for the deleted retry/exhaustion suite:
    // a failing test suite is now a `TestSuiteWarning` event, not a
    // hard rejection. The runner must be invoked exactly once and the
    // `task_done` tool_result must succeed and stop the loop.
    let (tx, mut rx) = tokio::sync::mpsc::channel::<AgentLoopEvent>(8);
    let runner = Arc::new(MockTestRunner::always_fail());
    let mut executor = make_executor_with_runner(runner.clone());
    executor.event_tx = Some(tx);
    seed_with_file_op(&executor).await;

    let results = executor.execute(&[task_done_call("done")]).await;

    assert_eq!(results.len(), 1);
    assert!(
        !results[0].is_error,
        "task_done must succeed even when the test suite fails: {}",
        results[0].content
    );
    assert!(
        results[0].stop_loop,
        "successful task_done must stop the loop"
    );
    let (passed, summary, failed_tests) = recv_test_warning(&mut rx).await;
    assert_eq!(
        *runner.calls.lock().await,
        1,
        "the suite must run exactly once — no retry budget after Codex parity"
    );
    assert!(!passed, "warning must reflect the failing run");
    assert!(
        summary.contains("9 passed"),
        "warning summary must carry the runner outcome: {summary}"
    );
    assert_eq!(failed_tests, vec!["my_crate::tests::it_works".to_string()]);
}

#[tokio::test]
async fn task_done_does_not_hang_when_best_effort_test_run_stalls() {
    let (tx, _rx) = tokio::sync::mpsc::channel::<AgentLoopEvent>(8);
    let mut executor = make_executor_with_runner(Arc::new(HangingTestRunner));
    executor.event_tx = Some(tx);
    seed_with_file_op(&executor).await;

    let results = tokio::time::timeout(
        std::time::Duration::from_secs(1),
        executor.execute(&[task_done_call("done")]),
    )
    .await
    .expect("advisory post-task_done tests must not hang completion");

    assert_eq!(results.len(), 1);
    assert!(!results[0].is_error);
    assert!(results[0].stop_loop);
}

#[test]
fn read_test_command_override_env_reflects_aura_config() {
    // Same defence-in-depth idea as for the disable flag: an operator
    // exporting `AURA_DOD_TEST_COMMAND=` to "clear" the override must
    // not get an empty string handed back as if it were a real
    // command. `aura_config` does that trimming once at startup; this
    // test pins the contract by overriding the installed value.
    fn install_override(value: Option<&str>) -> aura_config::ConfigGuard {
        let mut cfg = aura_config::current();
        cfg.agent.verify.test_command_override = value.map(str::to_string);
        aura_config::install_for_test(cfg)
    }

    {
        let _g = install_override(Some("cargo test --workspace"));
        assert_eq!(
            super::read_test_command_override_env(),
            Some("cargo test --workspace".to_string()),
            "non-empty override must round-trip"
        );
    }
    {
        let _g = install_override(None);
        assert_eq!(
            super::read_test_command_override_env(),
            None,
            "unset override must read as None"
        );
    }
}

#[tokio::test]
async fn resolve_test_command_prefers_env_override_over_project_config() {
    // The override field is captured at construction time, so this
    // test doesn't touch the global env — it exercises the resolution
    // priority directly. The env reader itself is covered above.
    let mut executor = make_executor();
    executor.test_command = Some("cargo test --workspace".to_string());
    executor.test_command_override = Some("pytest -q tests/smoke/".to_string());

    let (cmd, source) = executor
        .resolve_test_command(std::path::Path::new("/tmp/test"))
        .expect("override should resolve");
    assert_eq!(cmd, "pytest -q tests/smoke/");
    assert_eq!(source, "env override");
}

#[tokio::test]
async fn resolve_test_command_falls_back_to_project_config_when_no_override() {
    let mut executor = make_executor();
    executor.test_command = Some("npm test --silent".to_string());
    executor.test_command_override = None;

    let (cmd, source) = executor
        .resolve_test_command(std::path::Path::new("/tmp/test"))
        .expect("project config should resolve");
    assert_eq!(cmd, "npm test --silent");
    assert_eq!(source, "project config");
}

#[tokio::test]
async fn resolve_test_command_falls_back_to_inferred_default() {
    // Build a real on-disk Cargo project so the auto-detect path has
    // something to match. Using a temp dir keeps the test hermetic.
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("Cargo.toml"), "").unwrap();

    let mut executor = make_executor();
    executor.test_command = None;
    executor.test_command_override = None;

    let (cmd, source) = executor
        .resolve_test_command(dir.path())
        .expect("auto-detect should resolve from Cargo.toml");
    assert!(cmd.starts_with("cargo test"), "got {cmd}");
    assert_eq!(source, "manifest auto-detect");
}
