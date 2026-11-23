//! Which catalog models are candidates, and which of them get work.

use aura_core_auth::ProviderKind;
use aura_model::model_catalog::{
    catalog, catalog_with_plans, ModelBackend, ModelOption, ModelTier,
};

use super::{candidates, current, runnable, Candidate};
use crate::fixed_pick;
use crate::fixtures::{ids, signed_in, versions, with_local, LOCAL, NEW_OPUS, NEW_SONNET};

#[test]
fn nothing_signed_in_routes_among_installed_local_models() {
    let none = signed_in(&[]);
    assert!(candidates(&catalog(LOCAL, &[]), &none, |_| false).is_empty());
    let local = candidates(&with_local(), &none, |_| false);
    assert_eq!(ids(&local), [LOCAL]);
    assert_eq!(local[0].cost_usd, Some(0.0));
    assert_eq!(fixed_pick(&local).map(|c| c.id.as_str()), Some(LOCAL));
}

#[test]
fn zos_sign_in_adds_the_router_aliases() {
    let zos = candidates(&with_local(), signed_in(&[ProviderKind::Zos]), |_| false);
    assert_eq!(
        ids(&zos),
        [
            LOCAL,
            "aura-claude-haiku-4-5",
            "aura-gpt-5-4",
            "aura-claude-sonnet-4-6",
            "aura-claude-opus-4-7",
        ]
    );
    assert_eq!(
        fixed_pick(&zos).map(Candidate::route_label).as_deref(),
        Some("aura-claude-sonnet-4-6 (cloud)")
    );
}

#[test]
fn a_claude_plan_adds_only_its_models() {
    let claude = candidates(
        &catalog(LOCAL, &[]),
        signed_in(&[ProviderKind::Claude]),
        |_| false,
    );
    assert_eq!(
        ids(&claude),
        [
            "claude/claude-haiku-4-5",
            "claude/claude-sonnet-4-6",
            "claude/claude-opus-4-7",
        ]
    );
    assert_eq!(
        fixed_pick(&claude).map(Candidate::route_label).as_deref(),
        Some("claude-sonnet-4-6 (claude plan)")
    );
}

#[test]
fn every_sign_in_keeps_one_route_per_model_on_the_plan() {
    let all = candidates(
        &with_local(),
        signed_in(&[
            ProviderKind::Zos,
            ProviderKind::Claude,
            ProviderKind::Chatgpt,
        ]),
        |_| false,
    );
    assert_eq!(
        ids(&all),
        [
            LOCAL,
            "claude/claude-haiku-4-5",
            "chatgpt/gpt-5.4",
            "claude/claude-sonnet-4-6",
            "claude/claude-opus-4-7",
        ]
    );
    assert_eq!(
        fixed_pick(&all).map(|c| c.id.as_str()),
        Some("claude/claude-sonnet-4-6")
    );
}

#[test]
fn plan_models_outside_the_router_list_are_kept_beside_it() {
    let live = [ModelOption {
        id: "claude/claude-sonnet-5-5".to_string(),
        label: "Claude Sonnet 5.5".to_string(),
        backend: ModelBackend::Claude,
        installed: true,
        tier: ModelTier::Standard,
    }];
    let options = catalog_with_plans(LOCAL, &[], &live);
    let both = candidates(
        &options,
        signed_in(&[ProviderKind::Zos, ProviderKind::Claude]),
        |_| false,
    );
    assert!(both.iter().any(|c| c.id == "aura-claude-sonnet-4-6"));
    assert_eq!(
        fixed_pick(&both).map(|c| c.id.as_str()),
        Some("claude/claude-sonnet-5-5"),
        "the cheaper standard model wins"
    );
}

#[test]
fn without_a_standard_model_the_strongest_is_picked() {
    let chatgpt = candidates(&with_local(), signed_in(&[ProviderKind::Chatgpt]), |_| {
        false
    });
    assert_eq!(
        fixed_pick(&chatgpt).map(|c| c.id.as_str()),
        Some("chatgpt/gpt-5.4")
    );
}

#[test]
fn a_dropped_model_is_not_a_candidate() {
    let claude = signed_in(&[ProviderKind::Claude]);
    let dropped = |id: &str| id == "claude/claude-sonnet-4-6";
    let left = candidates(&with_local(), &claude, dropped);
    assert!(!left.iter().any(|c| dropped(&c.id)));
    assert_eq!(
        fixed_pick(&left).map(|c| c.id.as_str()),
        Some("claude/claude-opus-4-7")
    );
}

#[test]
fn an_older_version_of_a_model_is_not_current() {
    let all = versions();
    assert_eq!(all.len(), 11, "each is still a candidate, to name outright");
    let mut kept: Vec<&str> = current(&all).into_iter().map(|c| c.id.as_str()).collect();
    kept.sort_unstable();
    assert_eq!(
        kept,
        [
            "claude/claude-fable-5-1",
            "claude/claude-fable-5-2",
            "claude/claude-haiku-5-5",
            NEW_OPUS,
            NEW_SONNET,
        ],
        "fable 5.2 has no known price, so nothing is known against 5.1"
    );
    assert_eq!(fixed_pick(&all).map(|c| c.id.as_str()), Some(NEW_SONNET));
}

fn option(id: &str, backend: ModelBackend, tier: ModelTier) -> ModelOption {
    ModelOption {
        id: id.to_string(),
        label: id.to_string(),
        backend,
        installed: true,
        tier,
    }
}

const OPENROUTER_SONNET: &str = "openrouter/anthropic/claude-sonnet-4.6";

fn sonnet_on_every_route() -> Vec<ModelOption> {
    vec![
        option(
            OPENROUTER_SONNET,
            ModelBackend::OpenRouter,
            ModelTier::Standard,
        ),
        option(
            "aura-claude-sonnet-4-6",
            ModelBackend::Router,
            ModelTier::Standard,
        ),
        option(
            "claude/claude-sonnet-4-6",
            ModelBackend::Claude,
            ModelTier::Standard,
        ),
    ]
}

#[test]
fn openrouter_models_are_candidates_only_with_a_curated_price() {
    let options = [
        option(
            OPENROUTER_SONNET,
            ModelBackend::OpenRouter,
            ModelTier::Standard,
        ),
        option(
            "openrouter/acme/unknown-model",
            ModelBackend::OpenRouter,
            ModelTier::Standard,
        ),
    ];
    let openrouter = candidates(&options, signed_in(&[ProviderKind::OpenRouter]), |_| false);
    assert_eq!(ids(&openrouter), [OPENROUTER_SONNET]);
    assert!(openrouter[0].cost_usd.is_some());
}

#[test]
fn runnable_keeps_every_route_and_unpriced_openrouter_models() {
    const DEEPSEEK: &str = "openrouter/deepseek/deepseek-v3.2";
    let mut options = sonnet_on_every_route();
    options.push(option(
        DEEPSEEK,
        ModelBackend::OpenRouter,
        ModelTier::Standard,
    ));
    let can_use = signed_in(&[ProviderKind::Claude, ProviderKind::OpenRouter]);
    let all = runnable(&options, &can_use, |_| false);
    let all_ids = ids(&all);
    for id in ["claude/claude-sonnet-4-6", OPENROUTER_SONNET, DEEPSEEK] {
        assert!(all_ids.contains(&id), "{id} in {all_ids:?}");
    }
    assert!(
        !all_ids.contains(&"aura-claude-sonnet-4-6"),
        "not signed in to the router"
    );
    assert_eq!(all_ids.last(), Some(&DEEPSEEK), "unpriced last");
    assert_eq!(all.last().map(|c| c.tier), Some(ModelTier::Standard));

    let routed = candidates(&options, &can_use, |_| false);
    assert_eq!(ids(&routed), ["claude/claude-sonnet-4-6"]);
    let kept = runnable(&options, &can_use, |id| id == DEEPSEEK);
    assert!(!ids(&kept).contains(&DEEPSEEK), "dropped stays dropped");
}

#[test]
fn the_router_route_is_kept_over_openrouter() {
    let both = candidates(
        &sonnet_on_every_route(),
        signed_in(&[ProviderKind::Zos, ProviderKind::OpenRouter]),
        |_| false,
    );
    assert_eq!(ids(&both), ["aura-claude-sonnet-4-6"]);
}

#[test]
fn the_plan_route_is_kept_over_router_and_openrouter() {
    let all = candidates(
        &sonnet_on_every_route(),
        signed_in(&[
            ProviderKind::Zos,
            ProviderKind::OpenRouter,
            ProviderKind::Claude,
        ]),
        |_| false,
    );
    assert_eq!(ids(&all), ["claude/claude-sonnet-4-6"]);
}
