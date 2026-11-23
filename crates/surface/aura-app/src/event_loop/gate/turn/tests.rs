//! The route line of a turn, its prompt before it is classified, and the
//! signal its outcome is recorded with.

use super::attempts::Current;
use super::label::{Label, Routing};
use super::route::route_line;
use super::Ending;
use crate::session_helpers::default_agent_config_with_auth;
use aura_agent::{AgentError, AgentLoopResult};
use aura_core_types::{Hash, OutcomeSignal, PromptCategory, PromptPayload, Route, RouteReason};
use aura_gate::attempts::Terms;
use aura_gate::route::{Candidate, Decision};
use aura_model::model_catalog::{ModelBackend, ModelTier};
use aura_model::{ReasonerError, ThinkingEffort};
use std::time::Instant;

#[test]
fn a_route_line_names_the_model_its_effort_the_work_and_why() {
    let decision = Decision {
        model: Candidate {
            id: "claude/claude-sonnet-4-6".to_string(),
            label: "Sonnet".to_string(),
            backend: ModelBackend::Claude,
            tier: ModelTier::Standard,
            cost_usd: Some(3.0),
        },
        effort: ThinkingEffort::Medium,
        reason: RouteReason::Escalate,
        explored: false,
        propensity: 1.0,
    };
    assert_eq!(
        route_line(&decision, PromptCategory::Debugging),
        "gate → claude-sonnet-4-6 (claude plan) at medium effort · debugging, escalate"
    );
}

#[test]
fn a_label_starts_as_the_prompt_on_the_turns_model_unclassified() {
    let config = default_agent_config_with_auth("mock-model", Some("token".to_string()));
    let routing = Routing {
        classifier: "haiku".to_string(),
        reason: RouteReason::Default,
    };
    let label = Label::new("fix the parser".to_string(), &routing, &config);
    let expected = PromptPayload {
        classifier: "haiku".to_string(),
        route: Route::turn("mock-model", RouteReason::Default),
        ..PromptPayload::new("fix the parser")
    };
    assert_eq!(label.prompt, expected);
    assert_eq!(label.auth_token.as_deref(), Some("token"));
}

/// An ending whose last attempt's messages start at 0 and that Gate's
/// follow-up left at `signal`.
fn ending(signal: Option<OutcomeSignal>) -> Ending {
    Ending {
        last: Current {
            prompt_tx: Hash::from_content(b"fix the parser"),
            turn_start: 0,
            started: Instant::now(),
        },
        category: PromptCategory::Debugging,
        started_ms: 0,
        terms: Terms::default(),
        hub: None,
        cost_usd: 0.0,
        signal,
        changed: None,
    }
}

#[test]
fn a_turn_ends_on_a_stop_then_the_follow_up_then_its_own_signal() {
    let quiet = Ok(AgentLoopResult::default());
    let failed = Err(AgentError::Reason(ReasonerError::Internal(
        "down".to_string(),
    )));
    let checked = Some(OutcomeSignal::ChecksPassed);
    let cases = [
        (checked, &quiet, true, OutcomeSignal::Cancelled),
        (checked, &failed, false, OutcomeSignal::ChecksPassed),
        (None, &failed, false, OutcomeSignal::Error),
        (None, &quiet, false, OutcomeSignal::Unavailable),
    ];
    for (followed, result, stopped, expected) in cases {
        let settled = ending(followed).settle(result, stopped);
        assert_eq!(settled.signal, expected, "{followed:?} {stopped}");
        assert!(settled.left.is_none(), "an unrouted turn logs no attempt");
    }
}
