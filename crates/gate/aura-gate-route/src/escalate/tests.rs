//! Where failed work goes next, and at what effort.

use aura_core_auth::ProviderKind;
use aura_core_types::{OutcomeSignal, RouteLevel, RouteReason};
use aura_model::model_catalog::catalog;
use aura_model::ThinkingEffort;

use super::{escalate, has_stronger_model};
use crate::fixtures::{
    all, claude, new_sonnet_failing, signed_in, versions, HAIKU, LOCAL, NEW_OPUS, NEW_SONNET,
    OLD_SONNET, OPUS, SONNET,
};
use crate::stats::GateStats;
use crate::{candidates, Candidate};

/// The model and effort a turn that ran on `failed` at `effort` and ended
/// with `signal` goes on with among the Claude plan's models.
fn after(
    signal: OutcomeSignal,
    failed: &str,
    effort: ThinkingEffort,
) -> Option<(String, ThinkingEffort)> {
    escalate(
        &GateStats::default(),
        RouteLevel::Turn,
        (failed, Some(effort)),
        signal,
        &claude(),
    )
    .map(|d| (d.model.id, d.effort))
}

/// [`after`] a judge found the work unfinished.
fn up(failed: &str, effort: ThinkingEffort) -> Option<(String, ThinkingEffort)> {
    after(OutcomeSignal::JudgedFail, failed, effort)
}

#[test]
fn a_failed_attempt_escalates_to_the_cheapest_model_one_tier_up() {
    let empty = GateStats::default();
    let up = |failed: &str, candidates: &[Candidate]| {
        let signal = OutcomeSignal::ChecksFailed;
        escalate(
            &empty,
            RouteLevel::Subtask,
            (failed, None),
            signal,
            candidates,
        )
        .map(|d| {
            assert_eq!(d.reason, RouteReason::Escalate);
            assert!(!d.explored);
            d.model.id
        })
    };
    assert_eq!(up(HAIKU, &claude()).as_deref(), Some(SONNET));
    assert_eq!(up(SONNET, &claude()).as_deref(), Some(OPUS));
    assert_eq!(
        up(SONNET, &all()).as_deref(),
        Some("chatgpt/gpt-5.4"),
        "the cheaper of the frontier models"
    );
    assert_eq!(up(LOCAL, &claude()).as_deref(), Some(HAIKU));
    assert_eq!(up(OPUS, &claude()), None, "nothing is stronger");
    assert_eq!(up(HAIKU, &[]), None);
}

#[test]
fn a_failed_check_or_an_error_moves_the_work_up_at_its_effort() {
    for signal in [OutcomeSignal::ChecksFailed, OutcomeSignal::Error] {
        assert_eq!(
            after(signal, SONNET, ThinkingEffort::High),
            Some((OPUS.to_string(), ThinkingEffort::High)),
            "{signal} moves work to a stronger model"
        );
        assert_eq!(
            after(signal, OPUS, ThinkingEffort::High),
            None,
            "{signal} does not send the strongest model over its own work again"
        );
    }
}

#[test]
fn a_judged_failure_climbs_to_max_on_the_strongest_model_only() {
    assert_eq!(
        up(SONNET, ThinkingEffort::High),
        Some((OPUS.to_string(), ThinkingEffort::High)),
        "a stronger model first, at the same effort"
    );
    assert_eq!(
        up(HAIKU, ThinkingEffort::Off),
        Some((SONNET.to_string(), ThinkingEffort::Medium)),
        "work that ran with thinking off moves up at medium"
    );
    assert_eq!(
        up(OPUS, ThinkingEffort::Medium),
        Some((OPUS.to_string(), ThinkingEffort::High))
    );
    assert_eq!(
        up(OPUS, ThinkingEffort::High),
        Some((OPUS.to_string(), ThinkingEffort::Max)),
        "max only once a judge found high unfinished on the strongest model"
    );
    assert_eq!(up(OPUS, ThinkingEffort::Max), None);
    let route = escalate(
        &GateStats::default(),
        RouteLevel::Turn,
        (OPUS, Some(ThinkingEffort::High)),
        OutcomeSignal::JudgedFail,
        &claude(),
    )
    .unwrap()
    .route(RouteLevel::Turn);
    assert_eq!(route.effort.as_deref(), Some("max"));
}

#[test]
fn no_max_step_on_a_model_that_folds_max_into_high() {
    let gpt = candidates(
        &catalog(LOCAL, &[]),
        signed_in(&[ProviderKind::Chatgpt]),
        |_| false,
    );
    let top = gpt.iter().max_by_key(|c| c.tier).unwrap().id.clone();
    assert_eq!(
        escalate(
            &GateStats::default(),
            RouteLevel::Turn,
            (&top, Some(ThinkingEffort::High)),
            OutcomeSignal::JudgedFail,
            &gpt
        ),
        None,
        "no max step on a model that folds max into high"
    );
}

#[test]
fn only_another_model_is_a_stronger_one() {
    let empty = GateStats::default();
    let stronger = |ran: &str, effort| {
        has_stronger_model(&empty, RouteLevel::Turn, (ran, Some(effort)), &claude())
    };
    assert!(stronger(SONNET, ThinkingEffort::High));
    assert!(
        !stronger(OPUS, ThinkingEffort::High),
        "max on the same model is not a stronger model"
    );
    assert!(!stronger(OPUS, ThinkingEffort::Max));
}

#[test]
fn a_failed_attempt_goes_up_a_tier_past_older_versions() {
    let stats = new_sonnet_failing();
    for (failed, next) in [
        ("claude/claude-haiku-5-5", NEW_SONNET),
        (NEW_SONNET, NEW_OPUS),
        (OLD_SONNET, NEW_OPUS),
    ] {
        assert_eq!(
            escalate(
                &stats,
                RouteLevel::Turn,
                (failed, None),
                OutcomeSignal::ChecksFailed,
                &versions(),
            )
            .unwrap()
            .model
            .id,
            next,
            "after {failed}"
        );
    }
}
