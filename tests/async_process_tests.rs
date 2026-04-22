//! Async process integration tests.
//!
//! Tests the full async process flow including pending effects and completion transactions.

use aura_agent::ProcessManager;
use aura_core::{
    ActionId, ActionResultPayload, AgentId, Hash, ProcessId, ProcessPending, Transaction,
    TransactionType,
};
use aura_tools::{cmd_run_with_threshold, Sandbox, ThresholdResult};
use std::sync::Arc;
use std::time::Duration;
use tempfile::TempDir;
use tokio::sync::mpsc;

/// Create a test workspace.
fn create_test_workspace() -> TempDir {
    TempDir::new().unwrap()
}

// ============================================================================
// Sync vs Async Command Tests
// ============================================================================

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

// ============================================================================
// Transaction Chain Integrity Tests
// ============================================================================

#[test]
fn test_chain_verification() {
    let agent_id = AgentId::generate();

    // Create a chain of transactions
    let tx1 = Transaction::user_prompt(agent_id, b"first message".to_vec());
    let tx2 = Transaction::user_prompt_chained(agent_id, b"second message".to_vec(), &tx1.hash);
    let _tx3 = Transaction::user_prompt_chained(agent_id, b"third message".to_vec(), &tx2.hash);

    // Note: Transaction::new_chained includes more than just payload in hash
    // but for transactions with same content and prev_hash, should get same hash
    let tx1_clone = Transaction::user_prompt(agent_id, b"first message".to_vec());
    let tx2_clone =
        Transaction::user_prompt_chained(agent_id, b"second message".to_vec(), &tx1_clone.hash);

    assert_eq!(tx1.hash, tx1_clone.hash);
    assert_eq!(tx2.hash, tx2_clone.hash);
}

#[test]
fn test_tampered_chain_detection() {
    let agent_id = AgentId::generate();

    // Create original chain
    let tx1 = Transaction::user_prompt(agent_id, b"first".to_vec());
    let tx2 = Transaction::user_prompt_chained(agent_id, b"second".to_vec(), &tx1.hash);

    // Create a "tampered" chain where middle transaction has different content
    // but someone tries to use the same hash
    let tampered_tx1 = Transaction::user_prompt(agent_id, b"TAMPERED".to_vec());

    // The tampered transaction would have a different hash
    assert_ne!(tx1.hash, tampered_tx1.hash);

    // If someone built on the tampered version, the chain would differ
    let tx2_on_tampered =
        Transaction::user_prompt_chained(agent_id, b"second".to_vec(), &tampered_tx1.hash);
    assert_ne!(tx2.hash, tx2_on_tampered.hash);
}

#[test]
fn test_replay_produces_same_hashes() {
    let agent_id = AgentId::generate();

    // Original sequence
    let messages = vec![b"msg1".to_vec(), b"msg2".to_vec(), b"msg3".to_vec()];

    // First pass
    let mut hashes1 = Vec::new();
    let mut prev_hash = None;
    for msg in &messages {
        let tx = match prev_hash {
            None => Transaction::user_prompt(agent_id, msg.clone()),
            Some(h) => Transaction::user_prompt_chained(agent_id, msg.clone(), &h),
        };
        hashes1.push(tx.hash);
        prev_hash = Some(tx.hash);
    }

    // Replay with same sequence
    let mut hashes2 = Vec::new();
    let mut prev_hash = None;
    for msg in &messages {
        let tx = match prev_hash {
            None => Transaction::user_prompt(agent_id, msg.clone()),
            Some(h) => Transaction::user_prompt_chained(agent_id, msg.clone(), &h),
        };
        hashes2.push(tx.hash);
        prev_hash = Some(tx.hash);
    }

    // Hashes should be identical
    assert_eq!(hashes1, hashes2);
}

#[tokio::test]
async fn test_async_completion_references_original() {
    let (tx, mut rx) = mpsc::channel(10);
    let manager = Arc::new(ProcessManager::with_defaults(tx));

    let agent_id = AgentId::generate();
    let process_id = ProcessId::generate();
    let action_id = ActionId::generate();

    // Create the "originating" transaction
    let orig_tx = Transaction::user_prompt(agent_id, b"run a slow command".to_vec());

    // Spawn a process
    #[cfg(windows)]
    let child = std::process::Command::new("cmd.exe")
        .args(["/C", "echo done"])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();

    #[cfg(not(windows))]
    let child = std::process::Command::new("sh")
        .args(["-c", "echo done"])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();

    // Register with reference to original transaction
    manager.register(
        agent_id,
        orig_tx.hash, // This is the reference
        action_id,
        process_id,
        child,
        "echo done".to_string(),
    );

    // Wait for completion
    let completion = tokio::time::timeout(Duration::from_secs(5), rx.recv())
        .await
        .expect("Timeout")
        .expect("Channel closed");

    // Completion should reference the original
    assert_eq!(completion.reference_tx_hash, Some(orig_tx.hash));
}

// ============================================================================
// Concurrent Process Tests
// ============================================================================

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

        #[cfg(windows)]
        let child = std::process::Command::new("cmd.exe")
            .args(["/C", &format!("echo process_{i}")])
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .unwrap();

        #[cfg(not(windows))]
        let child = std::process::Command::new("sh")
            .args(["-c", &format!("echo process_{}", i)])
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .unwrap();

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
    #[cfg(windows)]
    let (sync_program, sync_args_1) = (
        "cmd",
        vec!["/c".to_string(), "echo".to_string(), "sync1".to_string()],
    );
    #[cfg(not(windows))]
    let (sync_program, sync_args_1) = ("echo", vec!["sync1".to_string()]);

    let (result1, _) =
        cmd_run_with_threshold(&sandbox, sync_program, &sync_args_1, None, 5000).unwrap();

    let sync1_output = match result1 {
        ThresholdResult::Completed(output) => {
            assert!(output.status.success());
            String::from_utf8_lossy(&output.stdout).to_string()
        }
        ThresholdResult::Pending(_) => panic!("Expected sync completion"),
    };
    assert!(sync1_output.contains("sync1"));

    // Second: async slow command
    let process_id = ProcessId::generate();
    let action_id = ActionId::generate();
    let reference_hash = Hash::from_content(b"async trigger");

    #[cfg(windows)]
    let child = std::process::Command::new("cmd.exe")
        .args(["/C", "echo async_middle"])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();

    #[cfg(not(windows))]
    let child = std::process::Command::new("sh")
        .args(["-c", "echo async_middle"])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();

    manager.register(
        agent_id,
        reference_hash,
        action_id,
        process_id,
        child,
        "echo async_middle".to_string(),
    );

    // Third: another synchronous fast command (while async is pending)
    #[cfg(windows)]
    let sync_args_2 = vec!["/c".to_string(), "echo".to_string(), "sync2".to_string()];
    #[cfg(not(windows))]
    let sync_args_2 = vec!["sync2".to_string()];

    let (result2, _) =
        cmd_run_with_threshold(&sandbox, sync_program, &sync_args_2, None, 5000).unwrap();

    let sync2_output = match result2 {
        ThresholdResult::Completed(output) => {
            assert!(output.status.success());
            String::from_utf8_lossy(&output.stdout).to_string()
        }
        ThresholdResult::Pending(_) => panic!("Expected sync completion"),
    };
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

// ============================================================================
// ProcessPending Payload Tests
// ============================================================================

#[test]
fn test_process_pending_payload_creation() {
    let process_id = ProcessId::generate();
    let command = "npm install --save-dev typescript";

    let pending = ProcessPending::new(process_id, command);

    assert_eq!(pending.process_id, process_id);
    assert_eq!(pending.command, command);
}

#[test]
fn test_process_pending_serialization() {
    let process_id = ProcessId::generate();
    let command = "cargo build --release";

    let pending = ProcessPending::new(process_id, command);

    // Serialize to JSON
    let json = serde_json::to_string(&pending).unwrap();
    assert!(json.contains("process_id"));
    assert!(json.contains("command"));

    // Deserialize back
    let parsed: ProcessPending = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed.process_id, process_id);
    assert_eq!(parsed.command, command);
}

// ============================================================================
// ActionResultPayload Tests
// ============================================================================

#[test]
fn test_action_result_success_payload() {
    let action_id = ActionId::generate();
    let process_id = ProcessId::generate();

    let payload = ActionResultPayload::success(
        action_id,
        process_id,
        Some(0),
        b"Build successful!".to_vec(),
        5000,
    );

    assert!(payload.success);
    assert_eq!(payload.action_id, action_id);
    assert_eq!(payload.process_id, process_id);
    assert_eq!(payload.exit_code, Some(0));
    assert_eq!(payload.duration_ms, 5000);
    assert_eq!(&payload.stdout[..], b"Build successful!");
}

#[test]
fn test_action_result_failure_payload() {
    let action_id = ActionId::generate();
    let process_id = ProcessId::generate();

    let payload = ActionResultPayload::failure(
        action_id,
        process_id,
        Some(1),
        b"Error: compilation failed".to_vec(),
        3000,
    );

    assert!(!payload.success);
    assert_eq!(payload.exit_code, Some(1));
    assert_eq!(&payload.stderr[..], b"Error: compilation failed");
}

#[test]
fn test_action_result_roundtrip() {
    let action_id = ActionId::generate();
    let process_id = ProcessId::generate();

    let payload = ActionResultPayload::success(
        action_id,
        process_id,
        Some(0),
        b"output data".to_vec(),
        1234,
    );

    // Serialize and deserialize
    let json = serde_json::to_string(&payload).unwrap();
    let parsed: ActionResultPayload = serde_json::from_str(&json).unwrap();

    assert_eq!(parsed.action_id, action_id);
    assert_eq!(parsed.process_id, process_id);
    assert_eq!(parsed.success, payload.success);
    assert_eq!(parsed.exit_code, payload.exit_code);
    assert_eq!(parsed.duration_ms, payload.duration_ms);
}
