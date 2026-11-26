use super::*;
use aura_core_types::{AgentId, PromptPayload};
use serde_json::json;

fn call(agent: AgentId, model: &str, purpose: Option<&str>, output: u64) -> Transaction {
    let mut payload = json!({
        "model": model,
        "input_tokens": 1_000,
        "output_tokens": output,
    });
    if let Some(purpose) = purpose {
        payload["purpose"] = json!(purpose);
    }
    Transaction::new_chained(
        agent,
        TransactionType::Reasoning,
        serde_json::to_vec(&payload).unwrap(),
        None,
    )
}

fn prompt(agent: AgentId, route: Route) -> Transaction {
    Transaction::user_prompt_with(
        agent,
        &PromptPayload {
            route,
            ..PromptPayload::new("fix it")
        },
    )
}

#[test]
fn a_turn_counts_its_classifier_its_calls_and_its_title() {
    let agent = AgentId::generate();
    let sonnet = "claude/claude-sonnet-5-5";
    let haiku = "claude/claude-haiku-5-5";
    let mut calls = TurnCalls::default();
    let seen: Vec<Seen> = [
        call(agent, "claude/claude-opus-5-5", None, 10),
        call(agent, haiku, Some("classify"), 1),
        prompt(agent, Route::turn(sonnet, RouteReason::Default)),
        call(agent, sonnet, None, 100),
        call(agent, sonnet, None, 200),
        Transaction::user_prompt_with(agent, &PromptPayload::new("also add a test")),
        Transaction::new_chained(agent, TransactionType::AgentMsg, b"Done".to_vec(), None),
        call(agent, haiku, Some("title"), 5),
    ]
    .iter()
    .map(|tx| calls.see(tx))
    .collect();
    assert_eq!(
        seen,
        [
            Seen::Call,
            Seen::Other,
            Seen::NewTurn,
            Seen::Call,
            Seen::Call,
            Seen::Other,
            Seen::Other,
            Seen::Call
        ]
    );
    let Footer { models, usage, .. } = calls.footer().unwrap();
    assert_eq!(
        models, "claude-sonnet-5-5 ×2 · gate: claude-haiku-5-5",
        "the earlier turn's Opus call is not this turn's, and the title's model is not shown"
    );
    let usage = usage.unwrap();
    assert_eq!(usage.tokens, 4_000 + 1 + 100 + 200 + 5);
    assert!(usage.cost_usd.is_some_and(|cost| cost > 0.0));

    let unrouted = Transaction::user_prompt_with(agent, &PromptPayload::new("next"));
    assert_eq!(
        calls.see(&unrouted),
        Seen::NewTurn,
        "a prompt without a route after the reply starts a turn"
    );
    assert_eq!(calls.footer(), None);
}

#[test]
fn a_stronger_model_taking_over_carries_the_turn_on() {
    let agent = AgentId::generate();
    let mut calls = TurnCalls::default();
    for tx in [
        prompt(
            agent,
            Route::turn("claude/claude-sonnet-5-5", RouteReason::Default),
        ),
        call(agent, "claude/claude-sonnet-5-5", None, 10),
        call(agent, "claude/claude-opus-5-5", Some("judge"), 10),
        prompt(
            agent,
            Route::turn("claude/claude-opus-5-5", RouteReason::Escalate),
        ),
        call(agent, "claude/claude-opus-5-5", None, 10),
    ] {
        calls.see(&tx);
    }
    assert_eq!(
        calls.footer().unwrap().models,
        "claude-sonnet-5-5 · claude-opus-5-5 · gate: claude-opus-5-5"
    );

    calls.see(&prompt(
        agent,
        Route::turn("local/qwen3", RouteReason::Pinned),
    ));
    assert_eq!(calls.footer(), None, "the next turn starts empty");
    calls.see(&call(agent, "local/qwen3", None, 10));
    let Footer { models, usage, .. } = calls.footer().unwrap();
    assert_eq!(models, "qwen3");
    assert_eq!(usage.unwrap().cost_usd, None, "local calls are not priced");
}

#[test]
fn a_turn_on_a_chosen_model_marks_its_classifier_as_classifying() {
    let agent = AgentId::generate();
    let opus = "claude/claude-opus-5-5";
    let haiku = "claude/claude-haiku-5-5";
    let mut calls = TurnCalls::default();
    for tx in [
        call(agent, haiku, Some("classify"), 1),
        prompt(agent, Route::turn(opus, RouteReason::Pinned)),
        call(agent, opus, None, 100),
        Transaction::user_prompt_with(agent, &PromptPayload::new("also add a test")),
        call(agent, opus, None, 100),
    ] {
        calls.see(&tx);
    }
    assert_eq!(
        calls.footer().unwrap().models,
        "claude-opus-5-5 ×2 · classify: claude-haiku-5-5",
        "Gate didn't pick the model, so its classifier didn't route the turn"
    );

    for tx in [
        Transaction::new_chained(agent, TransactionType::AgentMsg, b"Done".to_vec(), None),
        call(agent, haiku, Some("classify"), 1),
        prompt(agent, Route::turn(opus, RouteReason::Proven)),
        call(agent, opus, None, 100),
    ] {
        calls.see(&tx);
    }
    assert_eq!(
        calls.footer().unwrap().models,
        "claude-opus-5-5 · gate: claude-haiku-5-5",
        "the next turn is routed by Gate"
    );
}

/// A call recording the thinking level `effort` it asked for.
fn thinking_call(agent: AgentId, model: &str, purpose: Option<&str>, effort: &str) -> Transaction {
    let mut payload = json!({
        "model": model,
        "input_tokens": 1_000,
        "output_tokens": 10,
        "thinking_effort": effort,
    });
    if let Some(purpose) = purpose {
        payload["purpose"] = json!(purpose);
    }
    Transaction::new_chained(
        agent,
        TransactionType::Reasoning,
        serde_json::to_vec(&payload).unwrap(),
        None,
    )
}

fn footer_of(txs: &[Transaction]) -> String {
    let mut calls = TurnCalls::default();
    for tx in txs {
        calls.see(tx);
    }
    calls.footer().unwrap().models
}

const SONNET: &str = "claude/claude-sonnet-5-5";
const HAIKU: &str = "claude/claude-haiku-5-5";

type FooterCase = (&'static str, Vec<Transaction>, &'static str);

fn routed_to_sonnet(agent: AgentId) -> Transaction {
    prompt(agent, Route::turn(SONNET, RouteReason::Default))
}

/// Cases where every call of the turn recorded a level.
fn leveled_cases(agent: AgentId) -> [FooterCase; 3] {
    let (sonnet, haiku) = (SONNET, HAIKU);
    let routed = || routed_to_sonnet(agent);
    [
        (
            "one level",
            vec![
                thinking_call(agent, haiku, Some("classify"), "low"),
                routed(),
                thinking_call(agent, sonnet, None, "high"),
                thinking_call(agent, sonnet, None, "high"),
            ],
            "claude-sonnet-5-5 ×2 · high thinking · gate: claude-haiku-5-5",
        ),
        (
            "levels that differ show as a range, lowest first",
            vec![
                routed(),
                thinking_call(agent, sonnet, None, "max"),
                thinking_call(agent, sonnet, None, "minimal"),
                thinking_call(agent, sonnet, None, "medium"),
            ],
            "claude-sonnet-5-5 ×3 · minimal–max thinking",
        ),
        (
            "Gate's and the title's levels are not the turn's",
            vec![
                thinking_call(agent, haiku, Some("classify"), "off"),
                routed(),
                thinking_call(agent, sonnet, None, "medium"),
                thinking_call(agent, "claude/claude-opus-5-5", Some("judge"), "max"),
                thinking_call(agent, haiku, Some("title"), "off"),
            ],
            "claude-sonnet-5-5 · medium thinking · gate: claude-haiku-5-5 \
             · gate: claude-opus-5-5",
        ),
    ]
}

/// Cases where some calls recorded no level, or thinking was off.
fn unleveled_cases(agent: AgentId) -> [FooterCase; 3] {
    let (sonnet, haiku) = (SONNET, HAIKU);
    let routed = || routed_to_sonnet(agent);
    [
        (
            "a call that recorded no level leaves the range alone",
            vec![
                routed(),
                call(agent, sonnet, None, 10),
                thinking_call(agent, sonnet, None, "xhigh"),
            ],
            "claude-sonnet-5-5 ×2 · xhigh thinking",
        ),
        (
            "only Gate recorded a level",
            vec![
                thinking_call(agent, haiku, Some("classify"), "low"),
                routed(),
                call(agent, sonnet, None, 10),
            ],
            "claude-sonnet-5-5 · gate: claude-haiku-5-5",
        ),
        (
            "thinking off throughout",
            vec![
                routed(),
                thinking_call(agent, sonnet, None, "off"),
                thinking_call(agent, sonnet, None, "off"),
            ],
            "claude-sonnet-5-5 ×2 · no thinking",
        ),
    ]
}

#[test]
fn the_footer_names_the_level_the_turn_thought_at() {
    let agent = AgentId::generate();
    let cases = leveled_cases(agent)
        .into_iter()
        .chain(unleveled_cases(agent));
    for (case, txs, expected) in cases {
        assert_eq!(footer_of(&txs), expected, "{case}");
    }
}

fn routed_call(agent: AgentId, model: &str, route: &str, output: u64) -> Transaction {
    let payload = json!({
        "model": model,
        "route": route,
        "input_tokens": 1_000,
        "output_tokens": output,
    });
    Transaction::new_chained(
        agent,
        TransactionType::Reasoning,
        serde_json::to_vec(&payload).unwrap(),
        None,
    )
}

#[test]
fn a_turn_splits_its_tokens_by_the_sign_in_that_paid() {
    let agent = AgentId::generate();
    let mut calls = TurnCalls::default();
    for tx in [
        prompt(
            agent,
            Route::turn("claude/claude-sonnet-4-6", RouteReason::Pinned),
        ),
        routed_call(agent, "claude-sonnet-4-6", "claude/claude-sonnet-4-6", 100),
        routed_call(
            agent,
            "anthropic/claude-4.5-haiku-20251001",
            "openrouter/anthropic/claude-haiku-4.5",
            10,
        ),
        routed_call(agent, "claude-sonnet-4-6", "claude/claude-sonnet-4-6", 50),
    ] {
        calls.see(&tx);
    }
    let footer = calls.footer().unwrap();
    let labels: Vec<&str> = footer.sources.iter().map(|s| s.label.as_str()).collect();
    assert_eq!(labels, ["Claude plan", "OpenRouter"]);
    assert_eq!(footer.sources[0].tokens, 2_000 + 150);
    assert_eq!(footer.sources[1].tokens, 1_010);
    assert!(
        footer.sources[1].cost_usd.is_some_and(|cost| cost > 0.0),
        "the OpenRouter route is priced by the model it names"
    );
    assert_eq!(
        footer.usage.unwrap().tokens,
        footer.sources.iter().map(|s| s.tokens).sum::<u64>()
    );
}

#[test]
fn a_turn_with_an_unpriced_call_is_partly_priced() {
    let agent = AgentId::generate();
    let mut calls = TurnCalls::default();
    for tx in [
        prompt(
            agent,
            Route::turn("claude/claude-sonnet-4-6", RouteReason::Pinned),
        ),
        routed_call(agent, "claude-sonnet-4-6", "claude/claude-sonnet-4-6", 100),
        routed_call(agent, "mystery/model-x", "openrouter/mystery/model-x", 10),
        call(agent, "local/qwen3:4b", None, 10),
    ] {
        calls.see(&tx);
    }
    let footer = calls.footer().unwrap();
    let usage = footer.usage.unwrap();
    assert!(usage.cost_usd.is_some_and(|cost| cost > 0.0));
    assert_eq!(
        usage.unpriced_calls, 1,
        "the unknown model is left out of the cost; the local one is free"
    );
    let unpriced: Vec<(&str, u32)> = (footer.sources.iter())
        .map(|s| (s.label.as_str(), s.unpriced_calls))
        .collect();
    assert_eq!(
        unpriced,
        [("Claude plan", 0), ("OpenRouter", 1), ("local", 0)]
    );
    assert_eq!(footer.sources[1].cost_usd, None, "nothing it ran is priced");
}

#[test]
fn records_without_a_route_are_put_down_to_the_model_they_name() {
    let agent = AgentId::generate();
    let mut calls = TurnCalls::default();
    for tx in [
        prompt(
            agent,
            Route::turn("aura-claude-sonnet-4-6", RouteReason::Pinned),
        ),
        call(agent, "aura-claude-sonnet-4-6", None, 10),
        call(agent, "local/qwen3:4b", None, 10),
    ] {
        calls.see(&tx);
    }
    let sources = calls.footer().unwrap().sources;
    let labels: Vec<&str> = sources.iter().map(|s| s.label.as_str()).collect();
    assert_eq!(labels, ["aura-router", "local"]);
    assert_eq!(sources[1].cost_usd, None, "local calls are not priced");
}
