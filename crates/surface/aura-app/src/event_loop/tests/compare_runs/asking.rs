//! What `/compare` needs before it starts, and what it refuses.

use super::*;

#[tokio::test]
async fn a_comparison_needs_a_prompt_and_waits_to_be_confirmed() {
    let (fixture, mut commands) = fixture();
    let mut b = borrowed(&fixture);
    let links = b.links.clone();
    let mut state = loop_state(&fixture, &mut b);
    state.models = gate_on_zos(&fixture);

    compare(&mut state, &links, "").await;
    assert!(system_texts(&drain(&mut commands)).contains("No comparisons yet"));
    compare(&mut state, &links, "--repeats 2").await;
    assert!(system_texts(&drain(&mut commands)).contains("No prompt to compare yet"));
    compare(&mut state, &links, "go").await;
    assert!(system_texts(&drain(&mut commands)).contains("Nothing to start"));

    compare(&mut state, &links, "write the answer --against sonnet").await;
    let estimate = system_texts(&drain(&mut commands));
    assert!(
        estimate.contains(
            "\n  A             gate (likely aura-claude-sonnet-4-6 (cloud))\n  B             aura-claude-sonnet-4-6 (cloud)\n"
        ),
        "{estimate}"
    );
    compare(&mut state, &links, "cancel").await;
    assert!(system_texts(&drain(&mut commands)).contains("Dropped the comparison"));
    assert!(subagents::lock(&state.hub.registry).compares().is_empty());
}

#[tokio::test]
async fn both_arms_must_differ_and_an_unknown_session_is_refused() {
    let (fixture, mut commands) = fixture();
    let mut b = borrowed(&fixture);
    let links = b.links.clone();
    let mut state = loop_state(&fixture, &mut b);
    state.models = gate_on_zos(&fixture);

    compare(&mut state, &links, "write the answer --a opus --b opus").await;
    assert!(
        system_texts(&drain(&mut commands)).contains("Both arms run on"),
        "same arm twice"
    );
    compare(&mut state, &links, "--session 42").await;
    assert!(system_texts(&drain(&mut commands)).contains("No session started at 42"));
    compare(&mut state, &links, "--session current").await;
    assert!(system_texts(&drain(&mut commands)).contains("no session to replay yet"));
}
