//! What an arm's agent and trace are called.

use super::super::arm::{arm_name, short_label};
use aura_core_types::{ArmSpec, CompareArm};

#[test]
fn an_arms_agent_and_trace_name_it_shortly() {
    let gate = ArmSpec::Gate { delegate: false };
    let model = ArmSpec::Model {
        model: "openrouter/anthropic/claude-opus-4-7".to_string(),
    };
    let by_difficulty = ArmSpec::ByDifficulty {
        easy: "a".to_string(),
        medium: "b".to_string(),
        hard: "c".to_string(),
    };
    assert_eq!(arm_name(CompareArm::A, &gate), "A · gate:nodelegate");
    assert_eq!(arm_name(CompareArm::B, &model), "B · claude-opus-4-7");
    assert_eq!(arm_name(CompareArm::B, &by_difficulty), "B · by difficulty");
    assert_eq!(short_label(&model), "openrouter/anthropic/claude-opus-4-7");
    assert_eq!(short_label(&by_difficulty), "by difficulty");
}
