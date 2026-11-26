//! Gate routing of turns.

use super::*;

#[tokio::test]
async fn gate_routes_each_category_and_records_why() {
    let (fixture, mut commands) = fixture();
    let mut b = borrowed(&fixture);
    let links = b.links.clone();
    let first = fixture.first;
    let mut done = std::mem::replace(&mut b.done, mpsc::channel(1).1);
    let mut state = loop_state(&fixture, &mut b);
    state.models = gate_on_zos(&fixture);
    learn_questions(&fixture.hub.gate, "aura-claude-haiku-4-5", 10);

    turn(
        &mut state,
        &mut done,
        &links,
        first,
        "what does gate do? (easy)",
    )
    .await;
    turn(&mut state, &mut done, &links, first, "fix the parser").await;

    let prompts = prompts_of(&state, first);
    assert_eq!(
        prompts[0].2.route,
        Route::turn("aura-claude-haiku-4-5", RouteReason::Proven),
        "ten passing questions prove haiku on easy questions, which it answers \
         with thinking off"
    );
    assert_eq!(prompts[1].2.category, Some(PromptCategory::Debugging));
    assert_eq!(
        prompts[1].2.route,
        Route::turn("aura-claude-sonnet-4-6", RouteReason::Default).at_effort("medium"),
        "debugging starts on the standard tier: gpt-5.4 lists cheaper than \
         sonnet, but the frontier is reached only by failing"
    );
    let models: Vec<String> = outcomes_of(&state, first)
        .into_iter()
        .map(|(_, o)| o.model)
        .collect();
    assert_eq!(models, ["aura-claude-haiku-4-5", "aura-claude-sonnet-4-6"]);
    let shown = route_lines(&mut commands);
    assert_eq!(
        shown,
        [
            "gate → aura-claude-haiku-4-5 (cloud) with thinking off · question, proven",
            "gate → aura-claude-sonnet-4-6 (cloud) at medium effort · debugging, default",
        ]
    );
    let recent = fixture.hub.gate.recent();
    assert_eq!(recent.len(), 2);
    assert_eq!(recent[1].reason, RouteReason::Default);
}

#[tokio::test]
async fn the_classifiers_difficulty_moves_where_a_turn_starts_and_is_recorded() {
    let (fixture, _commands) = fixture();
    let mut b = borrowed(&fixture);
    let links = b.links.clone();
    let first = fixture.first;
    let mut done = std::mem::replace(&mut b.done, mpsc::channel(1).1);
    let mut state = loop_state(&fixture, &mut b);
    state.models = gate_on_zos(&fixture);

    for text in [
        "what does gate do? (easy)",
        "what does gate do?",
        "fix the parser (hard)",
    ] {
        turn(&mut state, &mut done, &links, first, text).await;
    }

    let prompts: Vec<(Option<PromptCategory>, Option<Difficulty>, String)> =
        prompts_of(&state, first)
            .into_iter()
            .map(|(_, _, p)| (p.category, p.difficulty, p.route.model))
            .collect();
    assert_eq!(
        prompts,
        [
            (
                Some(PromptCategory::Question),
                Some(Difficulty::Easy),
                "aura-claude-haiku-4-5".to_string()
            ),
            (
                Some(PromptCategory::Question),
                Some(Difficulty::Medium),
                "aura-claude-sonnet-4-6".to_string()
            ),
            (
                Some(PromptCategory::Debugging),
                Some(Difficulty::Hard),
                "aura-claude-sonnet-4-6".to_string()
            ),
        ],
        "an easy question starts on the fast tier; medium and hard work on \
         the standard one, hard debugging too"
    );
}
