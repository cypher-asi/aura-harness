//! ProcessPending and ActionResultPayload tests.

use super::*;

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
