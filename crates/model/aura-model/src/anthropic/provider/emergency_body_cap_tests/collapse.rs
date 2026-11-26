use super::support::body_with_tool_loop_and_image;
use crate::anthropic::provider::body_cap::{
    collapse_messages_to_marker, fit_body_under_cap, BodyFitMode, TRUNCATION_MARKER_PREFIX,
};
use serde_json::json;

#[test]
fn collapse_salvages_instruction_and_tool_result_from_tool_loop() {
    let body = body_with_tool_loop_and_image();

    let (collapsed, _dropped, mode) = fit_body_under_cap(&body, 8 * 1024);
    assert_eq!(mode, BodyFitMode::Collapsed);
    let parsed: serde_json::Value = serde_json::from_slice(&collapsed).unwrap();
    let text = parsed["messages"][0]["content"][0]["text"]
        .as_str()
        .expect("collapsed user text");

    assert!(text.starts_with(TRUNCATION_MARKER_PREFIX));
    assert!(text.contains("Update the dashboard totals"));
    assert!(text.contains("The current total is computed on line 42"));
    assert!(!text.contains("original_len=0,kept=0"));
}

#[test]
fn collapse_never_emits_an_empty_truncation_marker() {
    let body = serde_json::to_vec(&json!({
        "model": "aura-claude-fable-5-1",
        "messages": [
            {
                "role": "user",
                "content": [
                    {
                        "type": "image",
                        "source": {
                            "type": "base64",
                            "media_type": "image/png",
                            "data": "AAAA"
                        }
                    }
                ]
            }
        ],
        "max_tokens": 1024,
        "stream": true,
    }))
    .unwrap();

    let collapsed = collapse_messages_to_marker(&body, 8 * 1024);
    let parsed: serde_json::Value = serde_json::from_slice(&collapsed).unwrap();
    let text = parsed["messages"][0]["content"][0]["text"]
        .as_str()
        .expect("collapsed user text");

    assert!(text.starts_with(TRUNCATION_MARKER_PREFIX));
    assert!(text.contains("no recoverable text"));
    assert!(!text.contains("original_len=0,kept=0"));
}
