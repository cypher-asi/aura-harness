//! Transaction chain integrity tests.

use super::*;

#[test]
fn test_chain_verification() {
    let agent_id = AgentId::generate();

    // Create a chain of transactions
    let tx1 = Transaction::user_prompt(agent_id, "first message");
    let tx2 = Transaction::user_prompt_chained(agent_id, "second message", &tx1.hash);
    let _tx3 = Transaction::user_prompt_chained(agent_id, "third message", &tx2.hash);

    // Note: Transaction::new_chained includes more than just payload in hash
    // but for transactions with same content and prev_hash, should get same hash
    let tx1_clone = Transaction::user_prompt(agent_id, "first message");
    let tx2_clone = Transaction::user_prompt_chained(agent_id, "second message", &tx1_clone.hash);

    assert_eq!(tx1.hash, tx1_clone.hash);
    assert_eq!(tx2.hash, tx2_clone.hash);
}

#[test]
fn test_tampered_chain_detection() {
    let agent_id = AgentId::generate();

    // Create original chain
    let tx1 = Transaction::user_prompt(agent_id, "first");
    let tx2 = Transaction::user_prompt_chained(agent_id, "second", &tx1.hash);

    // Create a "tampered" chain where middle transaction has different content
    // but someone tries to use the same hash
    let tampered_tx1 = Transaction::user_prompt(agent_id, "TAMPERED");

    // The tampered transaction would have a different hash
    assert_ne!(tx1.hash, tampered_tx1.hash);

    // If someone built on the tampered version, the chain would differ
    let tx2_on_tampered = Transaction::user_prompt_chained(agent_id, "second", &tampered_tx1.hash);
    assert_ne!(tx2.hash, tx2_on_tampered.hash);
}

#[test]
fn test_replay_produces_same_hashes() {
    let agent_id = AgentId::generate();

    // Original sequence
    let messages = vec!["msg1".to_string(), "msg2".to_string(), "msg3".to_string()];

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
    let orig_tx = Transaction::user_prompt(agent_id, "run a slow command");

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
