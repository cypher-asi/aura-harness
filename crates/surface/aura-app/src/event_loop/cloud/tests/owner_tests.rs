use super::*;
use aura_core_auth::{Owner, ProviderKind};

#[test]
fn only_the_placing_user_manages_a_cloud_agent() {
    let alice = Owner::Zos {
        user_id: "alice".into(),
    };
    let bob = Owner::Zos {
        user_id: "bob".into(),
    };
    assert_eq!(refusal_for(&alice, Some("Alice".into()), &alice), None);
    assert_eq!(
        refusal_for(&alice, Some("Alice".into()), &bob).as_deref(),
        Some("Placed by Alice; sign in as Alice to manage it.")
    );
    assert!(
        refusal_for(&Owner::Local, None, &bob).is_some_and(|note| note.contains("local profile"))
    );
}

#[test]
fn plan_models_are_recognized() {
    assert_eq!(
        plan_of_model("claude/claude-sonnet-4"),
        Some(ProviderKind::Claude)
    );
    assert_eq!(plan_of_model("chatgpt/gpt-5"), Some(ProviderKind::Chatgpt));
    assert_eq!(
        plan_of_model("openrouter/x/y"),
        Some(ProviderKind::OpenRouter)
    );
    assert_eq!(plan_of_model("aura-claude-sonnet"), None);
    assert_eq!(plan_of_model("local/qwen3:4b"), None);
    assert_eq!(plan_of_model("zos/x"), None);
}
