use crate::messages::{
    compact_older_messages, estimate_message_chars, truncate_content, CompactionConfig,
};
use aura_model::{ContentBlock, Message, Role};

fn image(bytes: usize) -> ContentBlock {
    ContentBlock::Image {
        source: aura_model::ImageSource {
            source_type: "base64".to_string(),
            media_type: "image/png".to_string(),
            data: "A".repeat(bytes),
        },
    }
}

fn has_image(msg: &Message) -> bool {
    msg.content
        .iter()
        .any(|b| matches!(b, ContentBlock::Image { .. }))
}

/// Five turns with images attached to messages 0, 2 and 4.
fn image_history() -> Vec<Message> {
    vec![
        Message::new(
            Role::User,
            vec![ContentBlock::text("build the homepage"), image(100_000)],
        ),
        Message::assistant("here is my plan"),
        Message::new(
            Role::User,
            vec![ContentBlock::text("use this reference"), image(50_000)],
        ),
        Message::assistant("working on it"),
        Message::new(
            Role::User,
            vec![ContentBlock::text("latest turn"), image(10_000)],
        ),
    ]
}

/// Root-cause regression for the emergency-body-cap incidents:
/// base64 images outside the preserved recent tail — including the
/// first message, which is exempt from text truncation — must be
/// replaced with text stubs so they stop inflating every request.
#[test]
fn compaction_stubs_images_outside_recent_tail_including_first_message() {
    let mut messages = image_history();
    let before_chars = estimate_message_chars(&messages);

    let config = CompactionConfig {
        tool_result_max_chars: 500,
        text_max_chars: 800,
        preserve_recent: 2,
    };
    compact_older_messages(&mut messages, &config);

    // compact_end = 5 - 2 = 3: messages[0..3] lose images.
    assert!(
        !has_image(&messages[0]),
        "first message image must be stubbed"
    );
    assert!(
        !has_image(&messages[2]),
        "middle message image must be stubbed"
    );
    assert!(
        has_image(&messages[4]),
        "image in the preserved recent tail must survive"
    );
    assert!(
        messages[0].content.iter().any(|b| matches!(
            b,
            ContentBlock::Text { text } if text == "build the homepage"
        )),
        "first message task text must survive verbatim"
    );
    assert!(
        messages[0].content.iter().any(|b| matches!(
            b,
            ContentBlock::Text { text } if text.contains("image removed during context compaction")
        )),
        "stub marker must replace the image"
    );
    assert!(
        estimate_message_chars(&messages) < before_chars / 4,
        "stripping images must collapse the size estimate"
    );
}

#[test]
fn test_truncate_below_threshold() {
    let content = "short";
    assert_eq!(truncate_content(content, 100, None, None), "short");
}

#[test]
fn test_truncate_preserves_head_and_tail() {
    let content = "a".repeat(300);
    let result = truncate_content(&content, 200, None, None);
    assert!(result.contains("content truncated"));
    assert!(result.len() < 300);
}

#[test]
fn test_compact_older_preserves_recent() {
    let mut messages = vec![
        Message::user("first"),
        Message::user("second"),
        Message::user("third"),
        Message::user("fourth"),
    ];
    let config = CompactionConfig {
        tool_result_max_chars: 10,
        text_max_chars: 10,
        preserve_recent: 2,
    };
    compact_older_messages(&mut messages, &config);
    assert_eq!(messages.len(), 4);
}

#[test]
fn test_configurable_head_tail() {
    let content = "a".repeat(10_000);

    let result_default = truncate_content(&content, 3000, None, None);
    assert!(result_default.starts_with(&"a".repeat(1000)));
    assert!(result_default.ends_with(&"a".repeat(1000)));
    assert!(result_default.contains("content truncated"));

    let result_custom = truncate_content(&content, 3000, Some(2000), Some(500));
    let head_part: String = result_custom.chars().take(2000).collect();
    assert_eq!(head_part, "a".repeat(2000));
    assert!(result_custom.ends_with(&"a".repeat(500)));

    let big_content = "b".repeat(20_000);
    let result_micro = truncate_content(&big_content, 10_000, Some(6000), Some(3000));
    assert!(result_micro.starts_with(&"b".repeat(6000)));
    assert!(result_micro.ends_with(&"b".repeat(3000)));
    assert!(result_micro.contains("content truncated"));
}

#[test]
fn test_truncate_scales_oversized_head_tail_requests() {
    let content = "c".repeat(4_000);
    let result = truncate_content(&content, 400, Some(6_000), Some(3_000));

    assert!(result.contains("content truncated"));
    assert!(result.len() < content.len());
}
