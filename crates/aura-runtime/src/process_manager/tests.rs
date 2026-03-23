use super::{ProcessManager, ProcessManagerConfig};
use aura_core::{ActionId, ActionResultPayload, AgentId, Hash, ProcessId};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;

#[tokio::test]
async fn test_process_manager_creation() {
    let (tx, _rx) = mpsc::channel(10);
    let manager = ProcessManager::with_defaults(tx);
    assert_eq!(manager.running_count(), 0);
}

#[tokio::test]
async fn test_fast_process_completes() {
    let (tx, mut rx) = mpsc::channel(10);
    let manager = Arc::new(ProcessManager::with_defaults(tx));

    let agent_id = AgentId::generate();
    let process_id = ProcessId::generate();
    let action_id = ActionId::generate();
    let reference_hash = Hash::from_content(b"test tx");

    #[cfg(windows)]
    let child = std::process::Command::new("cmd.exe")
        .args(["/C", "echo hello"])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();

    #[cfg(not(windows))]
    let child = std::process::Command::new("sh")
        .args(["-c", "echo hello"])
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
        "echo hello".to_string(),
    );

    let completion = tokio::time::timeout(Duration::from_secs(5), rx.recv())
        .await
        .expect("Timeout waiting for completion")
        .expect("Channel closed");

    assert_eq!(
        completion.tx_type,
        aura_core::TransactionType::ProcessComplete
    );
    assert_eq!(completion.reference_tx_hash, Some(reference_hash));
}

#[tokio::test]
async fn test_multiple_concurrent_processes() {
    let (tx, mut rx) = mpsc::channel(10);
    let manager = Arc::new(ProcessManager::with_defaults(tx));

    let agent_id = AgentId::generate();
    let reference_hash = Hash::from_content(b"test tx");

    let mut process_ids = Vec::new();
    for i in 0..3 {
        let process_id = ProcessId::generate();
        let action_id = ActionId::generate();
        process_ids.push(process_id);

        #[cfg(windows)]
        let child = std::process::Command::new("cmd.exe")
            .args(["/C", &format!("echo process_{}", i)])
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
            format!("echo process_{}", i),
        );
    }

    let mut completions = Vec::new();
    for _ in 0..3 {
        let completion = tokio::time::timeout(Duration::from_secs(5), rx.recv())
            .await
            .expect("Timeout waiting for completion")
            .expect("Channel closed");
        completions.push(completion);
    }

    for completion in &completions {
        assert_eq!(
            completion.tx_type,
            aura_core::TransactionType::ProcessComplete
        );
        assert_eq!(completion.reference_tx_hash, Some(reference_hash));
    }
}

#[tokio::test]
async fn test_process_timeout() {
    let config = ProcessManagerConfig {
        max_async_timeout_ms: 100,
        poll_interval_ms: 10,
    };
    let (tx, mut rx) = mpsc::channel(10);
    let manager = Arc::new(ProcessManager::new(tx, config));

    let agent_id = AgentId::generate();
    let process_id = ProcessId::generate();
    let action_id = ActionId::generate();
    let reference_hash = Hash::from_content(b"test tx");

    #[cfg(windows)]
    let child = std::process::Command::new("cmd.exe")
        .args(["/C", "ping -n 10 127.0.0.1"])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();

    #[cfg(not(windows))]
    let child = std::process::Command::new("sh")
        .args(["-c", "sleep 10"])
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
        "long running command".to_string(),
    );

    let completion = tokio::time::timeout(Duration::from_secs(2), rx.recv())
        .await
        .expect("Timeout waiting for completion")
        .expect("Channel closed");

    assert_eq!(
        completion.tx_type,
        aura_core::TransactionType::ProcessComplete
    );
    assert_eq!(completion.reference_tx_hash, Some(reference_hash));

    let payload: ActionResultPayload = serde_json::from_slice(&completion.payload).unwrap();
    assert!(!payload.success);
}

#[tokio::test]
async fn test_register_process_tracking() {
    let (tx, _rx) = mpsc::channel(10);
    let manager = Arc::new(ProcessManager::with_defaults(tx));

    let agent_id = AgentId::generate();
    let process_id = ProcessId::generate();
    let action_id = ActionId::generate();
    let reference_hash = Hash::from_content(b"test tx");

    #[cfg(windows)]
    let child = std::process::Command::new("cmd.exe")
        .args(["/C", "ping -n 5 127.0.0.1"])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();

    #[cfg(not(windows))]
    let child = std::process::Command::new("sh")
        .args(["-c", "sleep 5"])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();

    assert_eq!(manager.running_count(), 0);
    assert!(!manager.is_running(&process_id));

    manager.register(
        agent_id,
        reference_hash,
        action_id,
        process_id,
        child,
        "slow command".to_string(),
    );

    tokio::time::sleep(Duration::from_millis(50)).await;
    let _ = manager.running_count();

    let _ = manager.cancel(&process_id);
}

#[tokio::test]
async fn test_cancel_process() {
    let (tx, _rx) = mpsc::channel(10);
    let manager = Arc::new(ProcessManager::with_defaults(tx));

    let agent_id = AgentId::generate();
    let process_id = ProcessId::generate();
    let action_id = ActionId::generate();
    let reference_hash = Hash::from_content(b"test tx");

    #[cfg(windows)]
    let child = std::process::Command::new("cmd.exe")
        .args(["/C", "ping -n 30 127.0.0.1"])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();

    #[cfg(not(windows))]
    let child = std::process::Command::new("sh")
        .args(["-c", "sleep 30"])
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
        "slow command".to_string(),
    );

    tokio::time::sleep(Duration::from_millis(50)).await;

    let cancelled = manager.cancel(&process_id);
    assert!(cancelled);

    assert!(!manager.is_running(&process_id));

    let cancelled_again = manager.cancel(&process_id);
    assert!(!cancelled_again);
}

#[test]
fn test_create_pending_payload() {
    let process_id = ProcessId::generate();
    let command = "echo hello";

    let payload = ProcessManager::create_pending_payload(process_id, command);

    assert_eq!(payload.process_id, process_id);
    assert_eq!(payload.command, command);
}

#[test]
fn test_process_manager_config_defaults() {
    let config = ProcessManagerConfig::default();
    assert_eq!(config.max_async_timeout_ms, 600_000);
    assert_eq!(config.poll_interval_ms, 100);
}

#[test]
fn test_process_manager_config_custom() {
    let config = ProcessManagerConfig {
        max_async_timeout_ms: 1000,
        poll_interval_ms: 10,
    };
    assert_eq!(config.max_async_timeout_ms, 1000);
    assert_eq!(config.poll_interval_ms, 10);
}

#[tokio::test]
async fn test_is_running_false_for_unknown() {
    let (tx, _rx) = mpsc::channel(10);
    let manager = ProcessManager::with_defaults(tx);
    let unknown_id = ProcessId::generate();
    assert!(!manager.is_running(&unknown_id));
}

#[tokio::test]
async fn test_cancel_unknown_returns_false() {
    let (tx, _rx) = mpsc::channel(10);
    let manager = ProcessManager::with_defaults(tx);
    let unknown_id = ProcessId::generate();
    assert!(!manager.cancel(&unknown_id));
}

#[tokio::test]
async fn test_multiple_processes_complete_concurrently() {
    let (tx, mut rx) = mpsc::channel(100);
    let manager = Arc::new(ProcessManager::new(
        tx,
        ProcessManagerConfig {
            max_async_timeout_ms: 30_000,
            poll_interval_ms: 10,
        },
    ));

    let agent_id = AgentId::generate();
    let reference_hash = Hash::from_content(b"concurrent test");

    let count = 5;
    for i in 0..count {
        let process_id = ProcessId::generate();
        let action_id = ActionId::generate();

        #[cfg(windows)]
        let child = std::process::Command::new("cmd.exe")
            .args(["/C", &format!("echo concurrent_{i}")])
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .unwrap();

        #[cfg(not(windows))]
        let child = std::process::Command::new("sh")
            .args(["-c", &format!("echo concurrent_{i}")])
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
            format!("echo concurrent_{i}"),
        );
    }

    let mut completions = Vec::new();
    for _ in 0..count {
        let completion = tokio::time::timeout(Duration::from_secs(10), rx.recv())
            .await
            .expect("Timeout waiting for completion")
            .expect("Channel closed");
        completions.push(completion);
    }

    assert_eq!(completions.len(), count);
    for c in &completions {
        assert_eq!(c.tx_type, aura_core::TransactionType::ProcessComplete);
    }
}

#[tokio::test]
async fn test_failed_process_sends_failure() {
    let (tx, mut rx) = mpsc::channel(10);
    let manager = Arc::new(ProcessManager::with_defaults(tx));

    let agent_id = AgentId::generate();
    let process_id = ProcessId::generate();
    let action_id = ActionId::generate();
    let reference_hash = Hash::from_content(b"fail test");

    #[cfg(windows)]
    let child = std::process::Command::new("cmd.exe")
        .args(["/C", "exit 1"])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();

    #[cfg(not(windows))]
    let child = std::process::Command::new("sh")
        .args(["-c", "exit 1"])
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
        "exit 1".to_string(),
    );

    let completion = tokio::time::timeout(Duration::from_secs(5), rx.recv())
        .await
        .expect("Timeout")
        .expect("Channel closed");

    assert_eq!(
        completion.tx_type,
        aura_core::TransactionType::ProcessComplete
    );

    let payload: ActionResultPayload = serde_json::from_slice(&completion.payload).unwrap();
    assert!(!payload.success);
}
