//! `ContentBlock`, `StopReason` and `Role` serialization round-trips.

use crate::types::*;

#[test]
fn test_content_block_thinking_serialization_round_trip() {
    let block = ContentBlock::Thinking {
        thinking: "hmm".to_string(),
        signature: Some("sig".to_string()),
    };
    let json = serde_json::to_string(&block).unwrap();
    let parsed: ContentBlock = serde_json::from_str(&json).unwrap();
    match parsed {
        ContentBlock::Thinking {
            thinking,
            signature,
        } => {
            assert_eq!(thinking, "hmm");
            assert_eq!(signature, Some("sig".to_string()));
        }
        _ => panic!("Expected Thinking"),
    }
}

#[test]
fn test_content_block_thinking_without_signature_skips_field() {
    let block = ContentBlock::Thinking {
        thinking: "hmm".to_string(),
        signature: None,
    };
    let json = serde_json::to_string(&block).unwrap();
    assert!(!json.contains("signature"));
}

#[test]
fn test_content_block_tool_use_serialization_round_trip() {
    let block = ContentBlock::tool_use("id1", "run_command", serde_json::json!({"cmd": "ls"}));
    let json = serde_json::to_string(&block).unwrap();
    let parsed: ContentBlock = serde_json::from_str(&json).unwrap();
    match parsed {
        ContentBlock::ToolUse { id, name, input } => {
            assert_eq!(id, "id1");
            assert_eq!(name, "run_command");
            assert_eq!(input["cmd"], "ls");
        }
        _ => panic!("Expected ToolUse"),
    }
}

#[test]
fn test_content_block_tool_result_serialization() {
    let block =
        ContentBlock::tool_result("tu_1", ToolResultContent::text("file contents here"), false);
    let json = serde_json::to_string(&block).unwrap();
    assert!(json.contains("tool_result"));
    assert!(json.contains("tu_1"));
}

#[test]
fn test_stop_reason_serialization() {
    let reasons = [
        (StopReason::EndTurn, "\"end_turn\""),
        (StopReason::ToolUse, "\"tool_use\""),
        (StopReason::MaxTokens, "\"max_tokens\""),
        (StopReason::StopSequence, "\"stop_sequence\""),
    ];
    for (reason, expected) in reasons {
        let json = serde_json::to_string(&reason).unwrap();
        assert_eq!(json, expected);
        let parsed: StopReason = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, reason);
    }
}

#[test]
fn test_role_serialization() {
    assert_eq!(serde_json::to_string(&Role::User).unwrap(), "\"user\"");
    assert_eq!(
        serde_json::to_string(&Role::Assistant).unwrap(),
        "\"assistant\""
    );
}
