use super::*;

#[test]
fn a_chat_title_is_its_own_message_on_the_turn_s_stream() {
    let json = serde_json::to_string(&OutboundMessage::ChatTitle(ChatTitle {
        title: "Build fix".to_string(),
    }))
    .expect("serialize");
    assert_eq!(json, r#"{"type":"chat_title","title":"Build fix"}"#);
    let back: OutboundMessage = serde_json::from_str(&json).expect("deserialize");
    assert!(matches!(back, OutboundMessage::ChatTitle(t) if t.title == "Build fix"));
}

#[test]
fn tool_result_without_image_omits_image_fields() {
    // The string-only path must serialize byte-identically to the
    // pre-image contract: no `image_base64` / `image_media_type`
    // keys appear when both are `None`.
    let msg = ToolResultMsg {
        name: "read_file".to_string(),
        result: "file contents".to_string(),
        is_error: false,
        tool_use_id: Some("toolu_1".to_string()),
        image_base64: None,
        image_media_type: None,
    };
    let json = serde_json::to_string(&msg).expect("serialize tool result");
    assert!(
        !json.contains("image_base64"),
        "absent image must be skipped: {json}"
    );
    assert!(
        !json.contains("image_media_type"),
        "absent media type must be skipped: {json}"
    );

    let back: ToolResultMsg = serde_json::from_str(&json).expect("deserialize tool result");
    assert_eq!(back.name, "read_file");
    assert_eq!(back.result, "file contents");
    assert!(!back.is_error);
    assert_eq!(back.tool_use_id.as_deref(), Some("toolu_1"));
    assert_eq!(back.image_base64, None);
    assert_eq!(back.image_media_type, None);
}

#[test]
fn tool_result_with_image_round_trips() {
    // A computer-use screenshot result must round-trip both image
    // fields. The base64 here is a tiny placeholder — the test
    // asserts the contract, not the payload.
    let msg = ToolResultMsg {
        name: "computer".to_string(),
        result: "screenshot taken".to_string(),
        is_error: false,
        tool_use_id: Some("toolu_shot".to_string()),
        image_base64: Some("aGVsbG8=".to_string()),
        image_media_type: Some("image/png".to_string()),
    };
    let json = serde_json::to_string(&msg).expect("serialize image tool result");
    assert!(json.contains("\"image_base64\":\"aGVsbG8=\""), "{json}");
    assert!(
        json.contains("\"image_media_type\":\"image/png\""),
        "{json}"
    );

    let back: ToolResultMsg = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(back.image_base64.as_deref(), Some("aGVsbG8="));
    assert_eq!(back.image_media_type.as_deref(), Some("image/png"));
    assert_eq!(back.result, "screenshot taken");
}

#[test]
fn tool_result_legacy_json_deserializes_without_image_fields() {
    // An older harness build emits no image keys at all; the field
    // defaults must keep that payload deserializing cleanly.
    let legacy = serde_json::json!({
        "name": "list_files",
        "result": "a\nb\nc",
        "is_error": false,
    });
    let back: ToolResultMsg = serde_json::from_value(legacy).expect("legacy payload deserializes");
    assert_eq!(back.image_base64, None);
    assert_eq!(back.image_media_type, None);
    assert_eq!(back.tool_use_id, None);
}
