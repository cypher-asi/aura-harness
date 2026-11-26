use super::*;

#[tokio::test]
async fn test_reason_records_and_returns_response() {
    let (kernel, _db, _ws) = create_new_kernel();
    let request = ModelRequest::builder("test-model", "system prompt")
        .message(aura_model::Message::user("hello"))
        .try_build()
        .unwrap();
    let result = kernel.reason(request).await.unwrap();
    assert_eq!(result.entry.seq, 1);
    assert!(!result.response.message.content.is_empty());
}

/// Invariant §3 strict: when `Kernel::reason` fails because the
/// provider returned `Err`, the kernel MUST still append a
/// `Reasoning` record entry describing the failure. Drop the entry
/// and every downstream audit goes blind to the attempt.
#[tokio::test]
async fn reason_sync_error_records_failed() {
    let db_dir = TempDir::new().unwrap();
    let ws_dir = TempDir::new().unwrap();
    let agent_id = AgentId::generate();
    let store: Arc<dyn Store> = Arc::new(RocksStore::open(db_dir.path(), false).unwrap());
    let provider: Arc<dyn ModelProvider + Send + Sync> =
        Arc::new(MockProvider::new().with_failure());
    let executor = ExecutorRouter::new();
    let config = KernelConfig {
        workspace_base: ws_dir.path().to_path_buf(),
        ..KernelConfig::default()
    };
    let kernel = Kernel::new(store.clone(), provider, executor, config, agent_id).unwrap();

    let request = ModelRequest::builder("test-model", "system")
        .message(aura_model::Message::user("hello"))
        .purpose(aura_model::ModelCallPurpose::Criteria)
        .try_build()
        .unwrap();
    let err = kernel
        .reason(request)
        .await
        .expect_err("failing provider should propagate");
    assert!(matches!(err, crate::KernelError::Reasoner(_)));

    let entries = store.scan_record(agent_id, 1, 10).unwrap();
    assert_eq!(
        entries.len(),
        1,
        "reason() error path must still write exactly one Reasoning entry"
    );
    assert_eq!(entries[0].tx.tx_type, TransactionType::Reasoning);
    let payload: serde_json::Value = serde_json::from_slice(&entries[0].tx.payload).unwrap();
    assert_eq!(
        payload.get("stop_reason").and_then(|v| v.as_str()),
        Some("Error"),
        "payload must carry stop_reason=Error; got {payload}"
    );
    assert!(
        payload.get("error").and_then(|v| v.as_str()).is_some(),
        "payload must carry an error string; got {payload}"
    );
    assert_eq!(payload["model"], "test-model");
    assert_eq!(payload["purpose"], "criteria");
}

/// Invariant §3 strict: the streaming handshake error path must
/// also record a `Reasoning` entry. A stream never materialized so
/// the finalize-on-drop seam in `ReasonStreamHandle` cannot catch
/// this; the kernel itself has to emit the record entry.
#[tokio::test]
async fn reason_streaming_handshake_error_records_failed() {
    let db_dir = TempDir::new().unwrap();
    let ws_dir = TempDir::new().unwrap();
    let agent_id = AgentId::generate();
    let store: Arc<dyn Store> = Arc::new(RocksStore::open(db_dir.path(), false).unwrap());
    let provider: Arc<dyn ModelProvider + Send + Sync> =
        Arc::new(MockProvider::new().with_failure());
    let executor = ExecutorRouter::new();
    let config = KernelConfig {
        workspace_base: ws_dir.path().to_path_buf(),
        ..KernelConfig::default()
    };
    let kernel = Kernel::new(store.clone(), provider, executor, config, agent_id).unwrap();

    let request = ModelRequest::builder("test-model", "system")
        .message(aura_model::Message::user("hello"))
        .try_build()
        .unwrap();
    let err = kernel
        .reason_streaming(request)
        .await
        .err()
        .expect("failing handshake should error");
    assert!(matches!(err, crate::KernelError::Reasoner(_)));

    let entries = store.scan_record(agent_id, 1, 10).unwrap();
    assert_eq!(
        entries.len(),
        1,
        "reason_streaming() handshake error must still write exactly one Reasoning entry"
    );
    assert_eq!(entries[0].tx.tx_type, TransactionType::Reasoning);
    let payload: serde_json::Value = serde_json::from_slice(&entries[0].tx.payload).unwrap();
    assert_eq!(
        payload.get("stop_reason").and_then(|v| v.as_str()),
        Some("Error")
    );
    assert_eq!(
        payload.get("stage").and_then(|v| v.as_str()),
        Some("streaming_handshake")
    );
}

/// Who paid for a call is read off the requested model id's route
/// prefix, so every `Reasoning` entry records it as `route`, whatever
/// model name the upstream echoed back.
#[tokio::test]
async fn reasoning_entries_record_the_requested_route() {
    let route = "openrouter/anthropic/claude-sonnet-4.6";
    let request = || {
        ModelRequest::builder(route, "system")
            .message(aura_model::Message::user("hello"))
            .try_build()
            .unwrap()
    };
    let (kernel, _db, _ws) = create_new_kernel();
    let done = kernel.reason(request()).await.unwrap();
    assert_eq!(reasoning_payload(&done.entry)["route"], route);

    let (handle, _stream) = kernel.reason_streaming(request()).await.unwrap();
    let entry = handle
        .record_completed(
            "anthropic/claude-4.6-sonnet-20260101",
            aura_model::StopReason::EndTurn,
            &aura_model::Usage::default(),
            &[],
            "",
            None,
        )
        .unwrap();
    let payload = reasoning_payload(&entry);
    assert_eq!(payload["route"], route);
    assert_eq!(payload["model"], "anthropic/claude-4.6-sonnet-20260101");

    let (handle, _stream) = kernel.reason_streaming(request()).await.unwrap();
    let failed = handle.record_failed("stream cut").unwrap();
    assert_eq!(reasoning_payload(&failed)["route"], route);
}

/// The reply's footer shows the thinking level a turn ran at, so every
/// completed model call records the level its request asked for.
#[tokio::test]
async fn reason_records_the_requested_thinking_effort() {
    let (kernel, _db, _ws) = create_new_kernel();
    let high = kernel
        .reason(request_with_effort(Some(aura_model::ThinkingEffort::High)))
        .await
        .unwrap();
    assert_eq!(reasoning_payload(&high.entry)["thinking_effort"], "high");

    let off = kernel
        .reason(request_with_effort(Some(aura_model::ThinkingEffort::Off)))
        .await
        .unwrap();
    assert_eq!(reasoning_payload(&off.entry)["thinking_effort"], "off");

    let unset = kernel.reason(request_with_effort(None)).await.unwrap();
    assert!(
        reasoning_payload(&unset.entry)
            .get("thinking_effort")
            .is_none(),
        "a request that left the level to the provider records none"
    );
}

#[tokio::test]
async fn reason_streaming_records_the_requested_thinking_effort() {
    let (kernel, _db, _ws) = create_new_kernel();
    let (handle, _stream) = kernel
        .reason_streaming(request_with_effort(Some(aura_model::ThinkingEffort::XHigh)))
        .await
        .unwrap();
    let entry = handle
        .record_completed(
            "test-model",
            aura_model::StopReason::EndTurn,
            &aura_model::Usage::default(),
            &[],
            "",
            None,
        )
        .unwrap();
    assert_eq!(reasoning_payload(&entry)["thinking_effort"], "xhigh");

    let (handle, _stream) = kernel
        .reason_streaming(request_with_effort(None))
        .await
        .unwrap();
    let entry = handle
        .record_completed(
            "test-model",
            aura_model::StopReason::EndTurn,
            &aura_model::Usage::default(),
            &[],
            "",
            None,
        )
        .unwrap();
    assert!(reasoning_payload(&entry).get("thinking_effort").is_none());
}

#[tokio::test]
async fn test_sequence_across_process_and_reason() {
    let (kernel, _db, _ws) = create_new_kernel();

    let tx = Transaction::user_prompt(kernel.agent_id, "prompt");
    let r1 = kernel.process_direct(tx).await.unwrap();
    assert_eq!(r1.entry.seq, 1);

    let request = ModelRequest::builder("test-model", "system")
        .message(aura_model::Message::user("test"))
        .try_build()
        .unwrap();
    let r2 = kernel.reason(request).await.unwrap();
    assert_eq!(r2.entry.seq, 2);

    let tx2 = Transaction::new_chained(
        kernel.agent_id,
        TransactionType::AgentMsg,
        "response".as_bytes().to_vec(),
        None,
    );
    let r3 = kernel.process_direct(tx2).await.unwrap();
    assert_eq!(r3.entry.seq, 3);
}
