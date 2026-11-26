use super::*;

#[test]
fn startup_defaults_to_gate() {
    assert_eq!(resolve_startup_model(None, None, |_| false), GATE_MODEL_ID);
    assert_eq!(
        resolve_startup_model(Some("  "), None, |_| true),
        GATE_MODEL_ID
    );
}

#[test]
fn startup_env_model_wins() {
    assert_eq!(
        resolve_startup_model(Some("aura-claude-opus-4-7"), Some(LOCAL), |_| false),
        "aura-claude-opus-4-7"
    );
}

#[test]
fn startup_persisted_model_requires_its_sign_in() {
    let guest = AuthSummary::default();
    let zos = signed_in(Some("alice"), &[]);
    let claude = signed_in(None, &[ProviderKind::Claude]);
    let start = |persisted: &str, auth: &AuthSummary| {
        resolve_startup_model(None, Some(persisted), |id| usable(auth, id))
    };
    assert_eq!(start("aura-claude-sonnet-4-6", &guest), GATE_MODEL_ID);
    assert_eq!(
        start("aura-claude-sonnet-4-6", &zos),
        "aura-claude-sonnet-4-6"
    );
    assert_eq!(start("local/llama3.2:3b", &guest), "local/llama3.2:3b");
    assert_eq!(start("claude/claude-sonnet-4-6", &zos), GATE_MODEL_ID);
    assert_eq!(
        start("claude/claude-sonnet-4-6", &claude),
        "claude/claude-sonnet-4-6"
    );
    assert_eq!(start(GATE_MODEL_ID, &guest), GATE_MODEL_ID);
}
