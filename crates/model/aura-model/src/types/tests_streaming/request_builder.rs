//! `ModelRequest` builder: additional edge cases.

use crate::types::*;

#[test]
fn test_model_request_builder_with_thinking() {
    use crate::types::request::ThinkingConfig;

    let request = ModelRequest::builder("model", "system")
        .thinking(ThinkingConfig {
            budget_tokens: 4096,
        })
        .try_build()
        .unwrap();

    assert!(request.thinking.is_some());
    assert_eq!(request.thinking.unwrap().budget_tokens, 4096);
}

#[test]
fn test_model_request_builder_with_auth_token() {
    let request = ModelRequest::builder("model", "system")
        .auth_token(Some("tok_abc".to_string()))
        .try_build()
        .unwrap();

    assert_eq!(request.auth_token, Some("tok_abc".to_string()));
}

#[test]
fn test_model_request_builder_with_upstream_provider_family() {
    let request = ModelRequest::builder("model", "system")
        .upstream_provider_family(Some("deepseek".to_string()))
        .try_build()
        .unwrap();

    assert_eq!(
        request.upstream_provider_family,
        Some("deepseek".to_string())
    );
}

#[test]
fn test_model_request_builder_multiple_messages() {
    let request = ModelRequest::builder("model", "system")
        .message(Message::user("first"))
        .message(Message::assistant("response"))
        .message(Message::user("second"))
        .try_build()
        .unwrap();

    assert_eq!(request.messages.len(), 3);
    assert_eq!(request.messages[0].role, Role::User);
    assert_eq!(request.messages[1].role, Role::Assistant);
    assert_eq!(request.messages[2].role, Role::User);
}
