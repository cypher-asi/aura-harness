use super::*;
use aura_core_auth::ProviderKind;
use aura_core_types::{Difficulty, PromptCategory, RouteReason};
use aura_gate_route::{candidates, effort_on, runnable, Work};
use aura_model::model_catalog::{catalog, ModelBackend};

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
            resolve_arm(query, &all, &all, true),
            Ok(ArmSpec::Gate { delegate: false }),
            "{query}"
        );
    }
    assert_eq!(
        resolve_arm("gate", &all, &all, false),
        Ok(ArmSpec::Gate { delegate: false }),
        "plain gate follows the setting"
    );
    assert_eq!(
        resolve_arm(DELEGATE, &all, &all, false),
        Ok(ArmSpec::Gate { delegate: true })
    );
    assert_eq!(
        resolve_arm("opus", &all, &all, true),
        Ok(ArmSpec::Model {
            model: "claude/claude-opus-5-5".to_string()
        })
    );
    assert!(resolve_arm("llama", &all, &all, true).is_err());
}

#[test]
fn gate_against_gate_without_delegation_always_delegates() {
    let all = with_claude_plan();
    let nodelegate = ArmSpec::Gate { delegate: false };
    assert_eq!(
        resolve_arms(("gate", NO_DELEGATE), &all, &all, false),
        Ok((ArmSpec::Gate { delegate: true }, nodelegate.clone()))
    );
    assert_eq!(
        resolve_arms((NO_DELEGATE, "gate"), &all, &all, false),
        Ok((nodelegate.clone(), ArmSpec::Gate { delegate: true }))
    );
    assert_eq!(
        resolve_arms(("gate", "opus"), &all, &all, false),
        Ok((
            nodelegate,
            ArmSpec::Model {
                model: "claude/claude-opus-5-5".to_string()
            }
        ))
    );
    assert!(resolve_arms(("opus", "opus"), &all, &all, true)
        .unwrap_err()
        .contains("Both arms run on claude/claude-opus-5-5"));
    assert!(resolve_arms(("gate", "gate"), &all, &all, true).is_err());
}

const DEEPSEEK: &str = "openrouter/deepseek/deepseek-v3.2";

/// What Gate routes to with Claude and ChatGPT plans and an OpenRouter
/// key, and every model those can run, DeepSeek (which has no curated
/// price) included.
fn plans_and_openrouter() -> (Vec<Candidate>, Vec<Candidate>) {
    let options = catalog("local/qwen3:4b", &[]);
    let can_use = |p| matches!(p, ProviderKind::Claude | ProviderKind::Chatgpt);
    let routed = candidates(&options, can_use, |_| false);
    let mut all = runnable(&options, can_use, |_| false);
    all.push(Candidate::from_id(DEEPSEEK));
    (all, routed)
}

#[test]
fn a_model_gate_does_not_route_to_is_named_by_its_id() {
    let (all, routed) = plans_and_openrouter();
    assert!(resolve_against(DEEPSEEK, &routed).is_err());
    let model = ArmSpec::Model {
        model: DEEPSEEK.to_string(),
    };
    assert_eq!(
        resolve_arm(DEEPSEEK, &all, &routed, true),
        Ok(model.clone())
    );
    assert_eq!(
        resolve_arm("DeepSeek-V3.2", &all, &routed, true),
        Ok(model),
        "by part of its name when Gate's models have none"
    );
    assert_eq!(
        resolve_model("sonnet", &all, &routed).map(|c| c.id.as_str()),
        Ok("claude/claude-sonnet-4-6"),
        "Gate's models come first for part of a name"
    );
    assert!(resolve_model("gate", &all, &routed)
        .unwrap_err()
        .contains("another model"));
}

#[test]
fn an_unknown_model_lists_at_most_twenty_ids() {
    let many: Vec<Candidate> = (0..30)
        .map(|i| Candidate::from_id(&format!("openrouter/acme/model-{i}")))
        .collect();
    let error = resolve_model("llama", &many, &[]).unwrap_err();
    assert!(
        error.starts_with("No available model matches `llama`"),
        "{error}"
    );
    assert!(error.contains("openrouter/acme/model-19"), "{error}");
    assert!(!error.contains("openrouter/acme/model-20"), "{error}");
    assert!(error.ends_with(", …"), "{error}");
    assert!(resolve_model("llama", &[], &[])
        .unwrap_err()
        .contains("sign in"));
}

fn difficulty_arm() -> ArmSpec {
    ArmSpec::ByDifficulty {
        easy: "claude/claude-sonnet-4-6".to_string(),
        medium: DEEPSEEK.to_string(),
        hard: "chatgpt/gpt-5.4".to_string(),
    }
}

#[test]
fn a_difficulty_arm_resolves_each_of_its_models() {
    let (all, routed) = plans_and_openrouter();
    assert_eq!(
        resolve_arm(
            "Difficulty:hard=chatgpt/gpt-5.4, easy=sonnet, medium=deepseek-v3.2",
            &all,
            &routed,
            true
        ),
        Ok(difficulty_arm())
    );
    let missing =
        resolve_arm("difficulty:easy=sonnet,medium=sonnet", &all, &routed, true).unwrap_err();
    assert!(missing.contains("hard"), "{missing}");
    assert!(missing.contains("difficulty:easy=<model>"), "{missing}");
    let unknown = resolve_arm(
        "difficulty:easy=sonnet,medium=sonnet,hard=llama",
        &all,
        &routed,
        true,
    )
    .unwrap_err();
    assert!(
        unknown.starts_with("Hard model: No available model matches `llama`"),
        "{unknown}"
    );
    let label = difficulty_arm().label().into_owned();
    assert!(resolve_arms((&label, &label), &all, &routed, true)
        .unwrap_err()
        .contains("Both arms run on difficulty:easy=claude/claude-sonnet-4-6"));
    assert_eq!(
        resolve_arms((&label, "gate"), &all, &routed, true),
        Ok((difficulty_arm(), ArmSpec::Gate { delegate: true }))
    );
}

#[test]
fn a_difficulty_arm_sends_work_to_the_model_of_its_difficulty() {
    let (all, _) = plans_and_openrouter();
    let routes = DifficultyRoutes::of(&difficulty_arm(), &all).unwrap();
    let work = Work::turn(PromptCategory::Debugging);
    for (difficulty, id) in [
        (Some(Difficulty::Easy), "claude/claude-sonnet-4-6"),
        (Some(Difficulty::Medium), DEEPSEEK),
        (Some(Difficulty::Hard), "chatgpt/gpt-5.4"),
        (None, DEEPSEEK),
    ] {
        let work = work.of(difficulty);
        let decision = routes.decide(&work);
        assert_eq!(decision.model.id, id, "{difficulty:?}");
        assert_eq!(decision.model, *routes.model(difficulty));
        assert_eq!(decision.effort, effort_on(&work, &decision.model));
        assert_eq!(decision.reason, RouteReason::Compare);
        assert!(!decision.explored);
        assert!((decision.propensity - 1.0).abs() < f64::EPSILON);
    }
    assert_eq!(
        routes.easy.backend,
        ModelBackend::Claude,
        "listed models keep their route"
    );
    assert_eq!(
        DifficultyRoutes::of(&ArmSpec::Gate { delegate: true }, &all),
        None
    );
    let unlisted = DifficultyRoutes::of(&difficulty_arm(), &[]).unwrap();
    assert_eq!(unlisted.medium.id, DEEPSEEK);
    assert_eq!(unlisted.medium.backend, ModelBackend::OpenRouter);
}
