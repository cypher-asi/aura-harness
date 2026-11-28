//! Sync vs async command tests.

use super::*;

#[test]
fn test_sync_command_immediate_commit() {
    let workspace = create_test_workspace();
    let sandbox = Sandbox::new(workspace.path()).unwrap();

    // Phase 2 hardening: `cmd_run_with_threshold` no longer routes
    // through a shell, so we need a bare executable that's present on
    // both Unix (`echo`) and Windows (`PING.EXE`).
    #[cfg(windows)]
    let (program, args, expected_in_stdout) = (
        "cmd",
        vec![
            "/c".to_string(),
            "echo".to_string(),
            "sync_test".to_string(),
        ],
        "sync_test",
    );
    #[cfg(not(windows))]
    let (program, args, expected_in_stdout) = ("echo", vec!["sync_test".to_string()], "sync_test");

    let (result, command) = cmd_run_with_threshold(
        &sandbox, program, &args, None, 5000, // 5 second threshold
    )
    .unwrap();

    // Fast command should complete within threshold
    match result {
        ThresholdResult::Completed(output) => {
            assert!(output.status.success());
            let stdout = String::from_utf8_lossy(&output.stdout);
            assert!(stdout.contains(expected_in_stdout), "stdout: {stdout}");
        }
        ThresholdResult::Pending(_) => {
            panic!("Expected fast command to complete synchronously");
        }
    }

    assert!(!command.is_empty());
}

#[test]
fn test_async_command_returns_pending() {
    let workspace = create_test_workspace();
    let sandbox = Sandbox::new(workspace.path()).unwrap();

    // Run a slow command with very short threshold
    #[cfg(windows)]
    let (result, command) = cmd_run_with_threshold(
        &sandbox,
        "ping",
        &["-n".to_string(), "10".to_string(), "127.0.0.1".to_string()],
        None,
        50, // 50ms threshold - too short
    )
    .unwrap();

    #[cfg(not(windows))]
    let (result, command) = cmd_run_with_threshold(
        &sandbox,
        "sleep",
        &["5".to_string()],
        None,
        50, // 50ms threshold - too short
    )
    .unwrap();

    // Slow command should return Pending
    match result {
        ThresholdResult::Pending(mut child) => {
            // Process should still be running
            assert!(child.try_wait().unwrap().is_none());
            // Kill it so test cleans up
            let _ = child.kill();
            let _ = child.wait();
        }
        ThresholdResult::Completed(_) => {
            panic!("Expected slow command to return Pending");
        }
    }

    assert!(!command.is_empty());
}

#[tokio::test]
async fn test_async_command_pending_then_complete() {
    let (tx, mut rx) = mpsc::channel(10);
    let manager = Arc::new(ProcessManager::with_defaults(tx));

    let agent_id = AgentId::generate();
    let process_id = ProcessId::generate();
    let action_id = ActionId::generate();
    let reference_hash = Hash::from_content(b"originating transaction");

    // Spawn a fast async process
    #[cfg(windows)]
    let child = std::process::Command::new("cmd.exe")
        .args(["/C", "echo async_complete"])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();

    #[cfg(not(windows))]
    let child = std::process::Command::new("sh")
        .args(["-c", "echo async_complete"])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();

    // Register the process (simulating Pending state)
    manager.register(
        agent_id,
        reference_hash,
        action_id,
        process_id,
        child,
        "echo async_complete".to_string(),
    );

    // Wait for completion transaction
    let completion = tokio::time::timeout(Duration::from_secs(5), rx.recv())
        .await
        .expect("Timeout waiting for completion")
        .expect("Channel closed");

    // Verify completion transaction
    assert_eq!(completion.tx_type, TransactionType::ProcessComplete);
    assert_eq!(completion.reference_tx_hash, Some(reference_hash));
    assert_eq!(completion.agent_id, agent_id);

    // Verify payload
    let payload: ActionResultPayload = serde_json::from_slice(&completion.payload).unwrap();
    assert!(payload.success);
    assert_eq!(payload.action_id, action_id);
    assert_eq!(payload.process_id, process_id);
}
