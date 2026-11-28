use super::dispatch::*;
use super::*;
use aura_core_protocol::ModelSelection;
use aura_core_types::{SubagentExit, SubagentResult};

#[test]
fn council_member_preserves_explicit_effort_and_limits() {
    let model = ModelSelection {
        reasoning_effort: Some(aura_core_protocol::ReasoningEffort::XHigh),
        max_turns: Some(7),
        ..Default::default()
    };
    let (effort, budget) = member_execution_profile(&model);
    assert_eq!(effort.as_deref(), Some("xhigh"));
    let budget = budget.unwrap();
    assert_eq!(budget.max_iterations, 7);
    assert_eq!(
        member_execution_profile(&ModelSelection::default()),
        (None, None)
    );
}

fn test_members(model_ids: &[&str]) -> Vec<CouncilMember> {
    model_ids
        .iter()
        .enumerate()
        .map(|(i, id)| CouncilMember {
            id: i.to_string(),
            model: ModelSelection {
                id: Some((*id).to_string()),
                ..ModelSelection::default()
            },
            role: None,
        })
        .collect()
}

fn second_opinion_members(model_ids: &[&str]) -> Vec<CouncilMember> {
    model_ids
        .iter()
        .enumerate()
        .map(|(i, id)| CouncilMember {
            id: i.to_string(),
            model: ModelSelection {
                id: Some((*id).to_string()),
                ..ModelSelection::default()
            },
            role: Some(if i == 0 {
                CouncilMemberRole::Aggregator
            } else {
                CouncilMemberRole::Reference
            }),
        })
        .collect()
}

#[test]
fn truncate_members_caps_and_keeps_synthesizer() {
    let members = test_members(&["a", "b", "c", "d", "e", "f"]);
    let capped = truncate_members(members, 4);
    let ids: Vec<String> = capped
        .iter()
        .map(|m| m.model.id.clone().unwrap_or_default())
        .collect();
    assert_eq!(ids, vec!["a", "b", "c", "d"]);
}

#[test]
fn truncate_members_keeps_all_under_cap() {
    let members = test_members(&["a", "b"]);
    assert_eq!(truncate_members(members, 4).len(), 2);
}

#[test]
fn latest_user_query_returns_most_recent_user_message() {
    let messages = vec![
        ConversationMessage {
            role: "user".to_string(),
            content: "first".to_string(),
        },
        ConversationMessage {
            role: "assistant".to_string(),
            content: "reply".to_string(),
        },
        ConversationMessage {
            role: "user".to_string(),
            content: "latest question".to_string(),
        },
    ];
    assert_eq!(latest_user_query(&messages), "latest question");
    assert_eq!(latest_user_query(&[]), "");
}

#[test]
fn classify_members_detects_second_opinion_only_with_complete_roles() {
    let council = test_members(&["final", "ref"]);
    assert_eq!(
        classify_council_members(&council).expect("plain council"),
        CouncilRunKind::Council
    );

    let second_opinion = second_opinion_members(&["final", "ref-a", "ref-b"]);
    assert_eq!(
        classify_council_members(&second_opinion).expect("second opinion"),
        CouncilRunKind::SecondOpinion
    );

    let mut missing_reference_role = second_opinion_members(&["final", "ref"]);
    missing_reference_role[1].role = None;
    let err = classify_council_members(&missing_reference_role)
        .expect_err("partial roles must fail closed");
    assert_eq!(err.code, "invalid_second_opinion_request");

    let aggregator_only = vec![CouncilMember {
        id: "0".into(),
        model: ModelSelection {
            id: Some("final".into()),
            ..ModelSelection::default()
        },
        role: Some(CouncilMemberRole::Aggregator),
    }];
    let err = classify_council_members(&aggregator_only).expect_err("references are required");
    assert_eq!(err.code, "second_opinion_no_references");
}

#[test]
fn second_opinion_dispatches_references_only_with_no_tools() {
    let members = second_opinion_members(&["final", "kimi", "deepseek"]);
    let dispatched: Vec<usize> = members
        .iter()
        .enumerate()
        .filter(|(index, member)| {
            should_dispatch_member(CouncilRunKind::SecondOpinion, *index, member)
        })
        .map(|(index, _)| index)
        .collect();

    assert_eq!(
        dispatched,
        vec![1, 2],
        "aggregator slot 0 must not run as a reference child"
    );
    assert_eq!(
        member_tool_subset(CouncilRunKind::SecondOpinion),
        Some(vec![])
    );
    let addendum =
        member_system_prompt_addendum(CouncilRunKind::SecondOpinion).expect("advisor addendum");
    assert!(addendum.contains("NOT the acting/final agent"));
    assert!(addendum.contains("no tools are available"));
    assert!(addendum.contains("Separate known facts from assumptions"));
}

#[test]
fn second_opinion_reference_prompt_uses_factual_rubric() {
    let prompt = member_prompt(CouncilRunKind::SecondOpinion, "review this migration");

    assert!(prompt.contains("Second Opinion reference task"));
    assert!(prompt.contains("review this migration"));
    assert!(prompt.contains("Ground claims in the request above"));
    assert!(prompt.contains("Mark assumptions explicitly"));
    assert!(prompt.contains("Do not claim to have used tools"));
}

fn completed_answer(index: usize, model: &str, body: &str) -> MemberAnswer {
    MemberAnswer {
        index,
        model_label: model.to_string(),
        outcome: Ok(SubagentResult {
            child_agent_id: None,
            final_message: body.to_string(),
            total_input_tokens: 0,
            total_output_tokens: 0,
            files_changed: Vec::new(),
            exit: SubagentExit::Completed,
        }),
    }
}

/// The synthesis prompt must embed the question, each member's model
/// label + answer, and steer the synthesizer to integrate (not list)
/// them.
#[test]
fn synthesis_prompt_embeds_question_and_member_answers() {
    let answers = vec![
        completed_answer(0, "model-a", "rust is a systems language"),
        completed_answer(1, "model-b", "rust has a borrow checker"),
    ];
    let prompt = build_synthesis_prompt("what is rust?", &answers, CouncilMechanism::Synthesize);

    assert!(prompt.contains("what is rust?"), "embeds the question");
    assert!(
        prompt.contains("model-a") && prompt.contains("model-b"),
        "labels members"
    );
    assert!(
        prompt.contains("rust is a systems language")
            && prompt.contains("rust has a borrow checker"),
        "embeds each member's answer"
    );
    assert!(prompt.contains("Member 0") && prompt.contains("Member 1"));
    assert!(
        prompt.to_lowercase().contains("synthesize") || prompt.to_lowercase().contains("combined"),
        "asks for synthesis"
    );
}

/// A failed / rejected member is surfaced (with its reason) rather
/// than silently dropped, so the synthesizer can account for it.
#[test]
fn synthesis_prompt_surfaces_member_failures() {
    let answers = vec![
        completed_answer(0, "model-a", "ok answer"),
        MemberAnswer {
            index: 1,
            model_label: "model-b".to_string(),
            outcome: Ok(SubagentResult::rejected("depth exceeded")),
        },
    ];
    let prompt = build_synthesis_prompt("q", &answers, CouncilMechanism::Synthesize);
    assert!(prompt.contains("ok answer"));
    assert!(
        prompt.contains("depth exceeded"),
        "rejected member reason is surfaced: {prompt}"
    );
}

#[test]
fn second_opinion_final_prompt_treats_references_as_private_guidance() {
    let answers = vec![
        completed_answer(1, "kimi", "Likely issue is stale cache."),
        MemberAnswer {
            index: 2,
            model_label: "deepseek".into(),
            outcome: Err("network timeout".into()),
        },
    ];
    let prompt = build_second_opinion_prompt("Why is CI failing?", &answers);

    assert!(prompt.contains("final model in Aura Second Opinion"));
    assert!(prompt.contains("private guidance"));
    assert!(prompt.contains("advisory, not as ground truth"));
    assert!(prompt.contains("facts present in the conversation"));
    assert!(prompt.contains("You may call tools if verification"));
    assert!(prompt.contains("Likely issue is stale cache."));
    assert!(prompt.contains("reference dispatch error: network timeout"));
    assert!(
        !prompt.contains("Do NOT call any tools"),
        "Second Opinion final model should retain acting-agent ability to verify"
    );
}

/// Each mechanism steers the combine turn with its own instruction
/// block while still embedding every member's answer.
#[test]
fn synthesis_prompt_varies_by_mechanism() {
    let answers = vec![
        completed_answer(0, "model-a", "answer one"),
        completed_answer(1, "model-b", "answer two"),
    ];

    let synthesize = build_synthesis_prompt("q", &answers, CouncilMechanism::Synthesize);
    assert!(synthesize.contains("## Synthesize"));
    assert!(synthesize.contains("ONE combined answer"));

    let contrast = build_synthesis_prompt("q", &answers, CouncilMechanism::Contrast);
    assert!(contrast.contains("## Contrast"));
    assert!(
        contrast.contains("Do NOT collapse everything into a single recommendation"),
        "contrast must not force a single recommendation: {contrast}"
    );

    let side_by_side = build_synthesis_prompt("q", &answers, CouncilMechanism::SideBySide);
    assert!(side_by_side.contains("## Side by side"));
    assert!(
        side_by_side.contains("Do NOT merge"),
        "side-by-side must not merge answers: {side_by_side}"
    );

    // Every mechanism still embeds each member's answer.
    for prompt in [&synthesize, &contrast, &side_by_side] {
        assert!(prompt.contains("answer one") && prompt.contains("answer two"));
    }
}
