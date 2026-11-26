use crate::anthropic::AnthropicProvider;

#[test]
fn claude_on_openrouter_counts_as_anthropic() {
    assert!(AnthropicProvider::model_looks_like_anthropic(
        "anthropic/claude-sonnet-4.6"
    ));
    assert!(AnthropicProvider::model_looks_like_anthropic(
        "claude-opus-4-7"
    ));
    assert!(!AnthropicProvider::model_looks_like_anthropic(
        "openai/gpt-5.4"
    ));
}
