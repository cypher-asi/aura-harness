use super::*;

const SONNET: &str = "aura-claude-sonnet-4-6";
const OPUS: &str = "aura-claude-opus-4-7";

#[test]
fn the_estimate_prices_every_run_and_judge_call() {
    let one_prompt = estimate(SONNET, OPUS, OPUS, 3, 1);
    let one_sonnet = estimate_cost_usd(SONNET, &REFERENCE_RUN).unwrap();
    let one_opus = estimate_cost_usd(OPUS, &REFERENCE_RUN).unwrap();
    let one_judge = estimate_cost_usd(OPUS, &REFERENCE_JUDGE).unwrap();
    assert!((one_prompt.a_usd.unwrap() - 3.0 * one_sonnet).abs() < 1e-9);
    assert!((one_prompt.b_usd.unwrap() - 3.0 * one_opus).abs() < 1e-9);
    assert!((one_prompt.judge_usd.unwrap() - 3.0 * one_judge).abs() < 1e-9);
    assert!(one_prompt.fully_priced());
    assert!(one_prompt.b_usd > one_prompt.a_usd);

    let replay = estimate(SONNET, OPUS, OPUS, 3, 4);
    assert!(
        (replay.a_usd.unwrap() - 12.0 * one_sonnet).abs() < 1e-9,
        "each prompt is a turn"
    );
    assert!(
        (replay.judge_usd.unwrap() - 3.0 * one_judge).abs() < 1e-9,
        "but each pair is judged once"
    );
}

#[test]
fn local_models_are_free_and_unknown_ones_unpriced() {
    let local = estimate("local/qwen3:4b", "mystery-model", "local/qwen3:4b", 2, 1);
    assert_eq!(local.a_usd, Some(0.0));
    assert_eq!(local.b_usd, None);
    assert!(!local.fully_priced());
    assert!(local.known_usd().abs() < f64::EPSILON);
}
