use super::*;
use aura_core_auth::ProviderKind;
use aura_gate_route::candidates;
use aura_model::model_catalog::catalog;

fn zos_candidates() -> Vec<Candidate> {
    candidates(
        &catalog("local/qwen3:4b", &["qwen3:4b".to_string()]),
        |p| p == ProviderKind::Zos,
        |_| false,
    )
}

fn with_claude_plan() -> Vec<Candidate> {
    let mut all = zos_candidates();
    let mut opus55 = all
        .iter()
        .find(|c| c.id == "aura-claude-opus-4-7")
        .unwrap()
        .clone();
    opus55.id = "claude/claude-opus-5-5".to_string();
    all.push(opus55);
    all
}

#[test]
fn the_strongest_opus_is_the_default_opponent() {
    let zos = zos_candidates();
    assert_eq!(default_against(&zos).unwrap().id, "aura-claude-opus-4-7");
    assert_eq!(
        default_against(&with_claude_plan()).unwrap().id,
        "claude/claude-opus-5-5",
        "the later version wins"
    );
    let no_opus: Vec<Candidate> = zos.iter().filter(|c| !is_opus(c)).cloned().collect();
    assert_eq!(
        default_against(&no_opus).unwrap().id,
        "aura-gpt-5-4",
        "else the strongest frontier model"
    );
    let local: Vec<Candidate> = zos
        .iter()
        .filter(|c| c.tier == ModelTier::Local)
        .cloned()
        .collect();
    assert_eq!(default_against(&local).unwrap().id, "local/qwen3:4b");
    assert_eq!(default_against(&[]), None);
}

#[test]
fn against_names_a_model_exactly_or_by_part_of_its_name() {
    let all = with_claude_plan();
    assert_eq!(
        resolve_against("opus", &all).unwrap().id,
        "claude/claude-opus-5-5"
    );
    assert_eq!(
        resolve_against("AURA-CLAUDE-OPUS-4-7", &all).unwrap().id,
        "aura-claude-opus-4-7"
    );
    assert_eq!(
        resolve_against("sonnet", &all).unwrap().id,
        "aura-claude-sonnet-4-6"
    );
    let missing = resolve_against("llama", &all).unwrap_err();
    assert!(missing.contains("aura-claude-haiku-4-5"), "{missing}");
    assert!(resolve_against("opus", &[])
        .unwrap_err()
        .contains("sign in"));
}

#[test]
fn arms_name_gate_with_or_without_delegation_or_a_model() {
    let all = with_claude_plan();
    for query in [NO_DELEGATE, " Gate:NoDelegate "] {
        assert_eq!(
            resolve_arm(query, &all, true),
            Ok(ArmSpec::Gate { delegate: false }),
            "{query}"
        );
    }
    assert_eq!(
        resolve_arm("gate", &all, false),
        Ok(ArmSpec::Gate { delegate: false }),
        "plain gate follows the setting"
    );
    assert_eq!(
        resolve_arm(DELEGATE, &all, false),
        Ok(ArmSpec::Gate { delegate: true })
    );
    assert_eq!(
        resolve_arm("opus", &all, true),
        Ok(ArmSpec::Model {
            model: "claude/claude-opus-5-5".to_string()
        })
    );
    assert!(resolve_arm("llama", &all, true).is_err());
}

#[test]
fn gate_against_gate_without_delegation_always_delegates() {
    let all = with_claude_plan();
    let nodelegate = ArmSpec::Gate { delegate: false };
    assert_eq!(
        resolve_arms(("gate", NO_DELEGATE), &all, false),
        Ok((ArmSpec::Gate { delegate: true }, nodelegate.clone()))
    );
    assert_eq!(
        resolve_arms((NO_DELEGATE, "gate"), &all, false),
        Ok((nodelegate.clone(), ArmSpec::Gate { delegate: true }))
    );
    assert_eq!(
        resolve_arms(("gate", "opus"), &all, false),
        Ok((
            nodelegate,
            ArmSpec::Model {
                model: "claude/claude-opus-5-5".to_string()
            }
        ))
    );
    assert!(resolve_arms(("opus", "opus"), &all, true)
        .unwrap_err()
        .contains("Both arms run on claude/claude-opus-5-5"));
    assert!(resolve_arms(("gate", "gate"), &all, true).is_err());
}
