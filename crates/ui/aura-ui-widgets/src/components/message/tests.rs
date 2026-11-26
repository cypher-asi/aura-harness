use super::*;

#[test]
fn test_message_creation() {
    let msg = Message::new(MessageRole::User, "Hello, world!");
    assert_eq!(msg.role(), MessageRole::User);
    assert_eq!(msg.content(), "Hello, world!");
}

#[test]
fn test_message_streaming() {
    let mut msg = Message::new(MessageRole::Assistant, "");
    assert!(!msg.is_streaming);
    msg.set_streaming(true);
    assert!(msg.is_streaming);
}

#[test]
fn test_version_changes_with_content_and_differs_per_clone() {
    let mut msg = Message::new(MessageRole::Assistant, "Hello");
    let before = msg.version();
    msg.append("!");
    assert_ne!(msg.version(), before);
    assert_ne!(msg.clone().version().0, msg.version().0);
}

#[test]
fn test_message_append() {
    let mut msg = Message::new(MessageRole::Assistant, "Hello");
    msg.append(", world!");
    assert_eq!(msg.content(), "Hello, world!");
}
