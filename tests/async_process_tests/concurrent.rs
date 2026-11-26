//! Concurrent process tests.

use super::*;

#[tokio::test]
async fn test_multiple_async_processes() {
    let (tx, mut rx) = mpsc::channel(20);
    let manager = Arc::new(ProcessManager::with_defaults(tx));

    let agent_id = AgentId::generate();
    let reference_hash = Hash::from_content(b"batch job");

    // Start multiple concurrent processes
    let num_processes = 5;
    let mut process_ids = Vec::new();

    for i in 0..num_processes {
        let process_id = ProcessId::generate();
        let action_id = ActionId::generate();
        process_ids.push(process_id);

        let child = spawn_shell(&format!("echo process_{i}"));

        manager.register(
            agent_id,
            reference_hash,
            action_id,
            process_id,
            child,
            format!("echo process_{i}"),
        );
    }

    // Collect all completions
    let mut completions = Vec::new();
    for _ in 0..num_processes {
        let completion = tokio::time::timeout(Duration::from_secs(10), rx.recv())
            .await
            .expect("Timeout waiting for completion")
            .expect("Channel closed");
        completions.push(completion);
    }

    // All should be ProcessComplete
    for completion in &completions {
        assert_eq!(completion.tx_type, TransactionType::ProcessComplete);
    }

    // All should reference the same batch transaction
    for completion in &completions {
        assert_eq!(completion.reference_tx_hash, Some(reference_hash));
    }

    // All payloads should indicate success
    for completion in &completions {
        let payload: ActionResultPayload = serde_json::from_slice(&completion.payload).unwrap();
        assert!(payload.success);
    }
}

#[tokio::test]
async fn test_interleaved_sync_async() {
    let workspace = create_test_workspace();
    let sandbox = Sandbox::new(workspace.path()).unwrap();
    let (tx, mut rx) = mpsc::channel(10);
    let manager = Arc::new(ProcessManager::with_defaults(tx));

    let agent_id = AgentId::generate();

    // First: synchronous fast command (Phase 2: no-shell path)
    let sync1_output = run_sync_echo(&sandbox, "sync1");
    assert!(sync1_output.contains("sync1"));

    // Second: async slow command
    let process_id = ProcessId::generate();
    let action_id = ActionId::generate();
    let reference_hash = Hash::from_content(b"async trigger");

    let child = spawn_shell("echo async_middle");

    manager.register(
        agent_id,
        reference_hash,
        action_id,
        process_id,
        child,
        "echo async_middle".to_string(),
    );

    // Third: another synchronous fast command (while async is pending)
    let sync2_output = run_sync_echo(&sandbox, "sync2");
    assert!(sync2_output.contains("sync2"));

    // Now wait for async completion
    let completion = tokio::time::timeout(Duration::from_secs(5), rx.recv())
        .await
        .expect("Timeout")
        .expect("Channel closed");

    assert_eq!(completion.tx_type, TransactionType::ProcessComplete);
    let payload: ActionResultPayload = serde_json::from_slice(&completion.payload).unwrap();
    assert!(payload.success);
}

/// Spawn `cmd` through the platform shell with piped stdout/stderr.
fn spawn_shell(cmd: &str) -> std::process::Child {
    let (shell, flag) = if cfg!(windows) {
        ("cmd.exe", "/C")
    } else {
        ("sh", "-c")
    };
    std::process::Command::new(shell)
        .args([flag, cmd])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap()
}

/// Run a fast `echo <word>` synchronously (Phase 2: no-shell path) and
/// return its stdout, asserting it completed successfully.
fn run_sync_echo(sandbox: &Sandbox, word: &str) -> String {
    #[cfg(windows)]
    let (program, args) = (
        "cmd",
        vec!["/c".to_string(), "echo".to_string(), word.to_string()],
    );
    #[cfg(not(windows))]
    let (program, args) = ("echo", vec![word.to_string()]);

    let (result, _) = cmd_run_with_threshold(sandbox, program, &args, None, 5000).unwrap();

    match result {
        ThresholdResult::Completed(output) => {
            assert!(output.status.success());
            String::from_utf8_lossy(&output.stdout).to_string()
        }
        ThresholdResult::Pending(_) => panic!("Expected sync completion"),
    }
}
