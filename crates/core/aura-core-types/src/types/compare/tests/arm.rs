use super::support::{by_difficulty, payload};
use crate::types::compare::{ArmSpec, ComparePayload};
use crate::types::gate::Difficulty;

#[test]
fn arm_labels_read_back() {
    for spec in [
        ArmSpec::Gate { delegate: true },
        ArmSpec::Gate { delegate: false },
        ArmSpec::Model {
            model: "aura-claude-opus-4-7".to_string(),
        },
        by_difficulty(),
    ] {
        assert_eq!(ArmSpec::from_label(&spec.label()), spec);
    }
    assert!(ArmSpec::Gate { delegate: false }.is_gate());
    assert!(!by_difficulty().is_gate());
    assert!(by_difficulty().is_by_difficulty());
    assert_eq!(
        by_difficulty().label(),
        "difficulty:easy=claude/claude-haiku-4-5,medium=openrouter/deepseek/deepseek-v3.2,\
         hard=chatgpt/gpt-5.4"
    );
}

#[test]
fn a_by_difficulty_arm_round_trips_as_by_difficulty() {
    let mut payload = payload();
    payload.arm_b = by_difficulty();
    let bytes = payload.to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(json["arm_b"]["kind"], "by_difficulty");
    assert_eq!(json["arm_b"]["hard"], "chatgpt/gpt-5.4");
    assert_eq!(ComparePayload::from_bytes(&bytes), Some(payload));
}

#[test]
fn by_difficulty_labels_take_their_keys_in_any_order() {
    let parsed = ArmSpec::parse_by_difficulty(" Difficulty: hard = c , easy=a,medium= b ");
    assert_eq!(
        parsed,
        Some(Ok(["a".to_string(), "b".to_string(), "c".to_string()]))
    );
    assert_eq!(ArmSpec::parse_by_difficulty("claude/claude-opus-4-7"), None);
    assert_eq!(ArmSpec::parse_by_difficulty("gate"), None);
}

#[test]
fn malformed_by_difficulty_labels_say_what_is_wrong() {
    let error = |label: &str| match ArmSpec::parse_by_difficulty(label) {
        Some(Err(error)) => error,
        other => panic!("{label:?} gave {other:?}"),
    };
    assert!(error("difficulty:easy=a,medium=b").contains("hard"));
    assert!(error("difficulty:easy=a,easy=b,medium=c,hard=d").contains("easy is given twice"));
    assert!(error("difficulty:easy=a,medium=b,hard=c,extreme=d").contains("`extreme`"));
    assert!(error("difficulty:easy=a,medium,hard=c").contains("`medium`"));
    assert!(error("difficulty:easy=,medium=b,hard=c").contains("easy"));
    assert!(error("difficulty:").contains("easy"));
    let malformed = "difficulty:easy=a,medium=b";
    assert_eq!(
        ArmSpec::from_label(malformed),
        ArmSpec::Model {
            model: malformed.to_string()
        },
        "from_label never fails"
    );
}

#[test]
fn each_arm_names_the_model_for_a_difficulty() {
    let spec = by_difficulty();
    assert_eq!(
        spec.model_for(Some(Difficulty::Easy)),
        Some("claude/claude-haiku-4-5")
    );
    assert_eq!(
        spec.model_for(Some(Difficulty::Medium)),
        Some("openrouter/deepseek/deepseek-v3.2")
    );
    assert_eq!(
        spec.model_for(Some(Difficulty::Hard)),
        Some("chatgpt/gpt-5.4")
    );
    assert_eq!(
        spec.model_for(None),
        Some("openrouter/deepseek/deepseek-v3.2"),
        "unknown difficulty runs on the medium model"
    );
    let model = ArmSpec::Model {
        model: "m".to_string(),
    };
    assert_eq!(model.model_for(Some(Difficulty::Hard)), Some("m"));
    assert_eq!(ArmSpec::Gate { delegate: true }.model_for(None), None);
}
