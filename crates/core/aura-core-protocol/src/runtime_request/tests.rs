use super::*;

#[test]
fn reasoning_effort_round_trips_snake_case() {
    for (tier, wire) in [
        (ReasoningEffort::Minimal, "\"minimal\""),
        (ReasoningEffort::Low, "\"low\""),
        (ReasoningEffort::Medium, "\"medium\""),
        (ReasoningEffort::High, "\"high\""),
        (ReasoningEffort::XHigh, "\"xhigh\""),
        (ReasoningEffort::Max, "\"max\""),
    ] {
        let json = serde_json::to_string(&tier).expect("serialize tier");
        assert_eq!(json, wire);
        let back: ReasoningEffort = serde_json::from_str(&json).expect("deserialize tier");
        assert_eq!(back, tier);
        assert_eq!(tier.as_wire(), &wire[1..wire.len() - 1]);
    }
}

#[test]
fn reasoning_effort_from_wire_preserves_xhigh() {
    assert_eq!(
        ReasoningEffort::from_wire("xhigh"),
        Some(ReasoningEffort::XHigh)
    );
    assert_eq!(ReasoningEffort::from_wire("MIN"), None);
    assert_eq!(
        ReasoningEffort::from_wire("minimal"),
        Some(ReasoningEffort::Minimal)
    );
}

#[test]
fn council_mechanism_round_trips_snake_case() {
    for (mechanism, wire) in [
        (CouncilMechanism::Synthesize, "\"synthesize\""),
        (CouncilMechanism::Contrast, "\"contrast\""),
        (CouncilMechanism::SideBySide, "\"side_by_side\""),
    ] {
        let json = serde_json::to_string(&mechanism).expect("serialize mechanism");
        assert_eq!(json, wire);
        let back: CouncilMechanism = serde_json::from_str(&json).expect("deserialize mechanism");
        assert_eq!(back, mechanism);
        assert_eq!(mechanism.as_wire(), &wire[1..wire.len() - 1]);
    }
}

#[test]
fn council_mechanism_defaults_to_synthesize() {
    assert_eq!(CouncilMechanism::default(), CouncilMechanism::Synthesize);
    // A council request that omits `mechanism` (older client) folds
    // into the default rather than failing to deserialize.
    let json = r#"{"kind":"council","params":{"members":[],"conversation_messages":[]}}"#;
    let parsed: RuntimeRequestType =
        serde_json::from_str(json).expect("deserialize legacy council request");
    match parsed {
        RuntimeRequestType::Council { mechanism, .. } => {
            assert_eq!(mechanism, CouncilMechanism::Synthesize);
        }
        other => panic!("expected council, got {other:?}"),
    }
}

#[test]
fn council_member_role_is_optional_and_round_trips() {
    let legacy: CouncilMember = serde_json::from_str(r#"{"id":"0","model":{"id":"final"}}"#)
        .expect("deserialize legacy council member");
    assert_eq!(legacy.role, None);

    let member = CouncilMember {
        id: "1".to_string(),
        model: ModelSelection {
            id: Some("reference".to_string()),
            ..ModelSelection::default()
        },
        role: Some(CouncilMemberRole::Reference),
    };
    let json = serde_json::to_string(&member).expect("serialize role");
    assert!(
        json.contains(r#""role":"reference""#),
        "role must be present for second-opinion members: {json}"
    );
    let back: CouncilMember = serde_json::from_str(&json).expect("deserialize role");
    assert_eq!(back.role, Some(CouncilMemberRole::Reference));
}

#[test]
fn a_title_model_is_sent_only_when_the_chat_needs_a_title() {
    let untitled = ModelSelection {
        id: Some("claude/claude-sonnet".to_string()),
        ..ModelSelection::default()
    };
    let json = serde_json::to_string(&untitled).expect("serialize");
    assert!(!json.contains("title_model"), "{json}");
    let older: ModelSelection =
        serde_json::from_str(r#"{"id":"claude/claude-sonnet"}"#).expect("deserialize");
    assert_eq!(older.title_model, None);

    let titled = ModelSelection {
        title_model: Some("claude/claude-haiku".to_string()),
        ..untitled
    };
    let json = serde_json::to_string(&titled).expect("serialize");
    assert!(
        json.contains(r#""title_model":"claude/claude-haiku""#),
        "{json}"
    );
    let back: ModelSelection = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(back.title_model.as_deref(), Some("claude/claude-haiku"));
}

#[test]
fn council_mechanism_from_wire_is_lenient() {
    assert_eq!(
        CouncilMechanism::from_wire("SIDE_BY_SIDE"),
        Some(CouncilMechanism::SideBySide)
    );
    assert_eq!(
        CouncilMechanism::from_wire("side-by-side"),
        Some(CouncilMechanism::SideBySide)
    );
    assert_eq!(
        CouncilMechanism::from_wire(" contrast "),
        Some(CouncilMechanism::Contrast)
    );
    assert_eq!(CouncilMechanism::from_wire("bogus"), None);
    assert_eq!(CouncilMechanism::from_wire(""), None);
}
