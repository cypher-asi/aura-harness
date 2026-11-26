use super::*;

#[test]
fn opus_resolves_to_an_opus_4_7() {
    let resolved = resolve_alias("opus", &catalog(), "");
    assert!(
        resolved.is_some_and(|id| model_key(id) == "claude-opus-4-7"),
        "{resolved:?}"
    );
}

#[test]
fn opus_stays_on_the_claude_plan_route_of_the_current_model() {
    assert_eq!(
        resolve_alias("opus", &catalog(), "claude/claude-sonnet-4-6"),
        Some("claude/claude-opus-4-7")
    );
}

#[test]
fn opus_prefers_the_linked_claude_plan_over_the_current_router_route() {
    assert_eq!(
        resolve_alias("opus", &catalog(), "aura-claude-sonnet-4-6"),
        Some("claude/claude-opus-4-7")
    );
    assert_eq!(
        resolve_alias("opus", &catalog(), "openrouter/anthropic/claude-haiku-4.5"),
        Some("claude/claude-opus-4-7")
    );
}

#[test]
fn opus_stays_on_the_router_route_of_the_current_model_without_a_plan() {
    let candidates: Vec<_> = catalog()
        .into_iter()
        .map(|c| {
            if c.id.starts_with("claude/") {
                unavailable(c)
            } else {
                c
            }
        })
        .collect();
    assert_eq!(
        resolve_alias("opus", &candidates, "aura-claude-sonnet-4-6"),
        Some("aura-claude-opus-4-7")
    );
}

#[test]
fn linked_plan_beats_a_newer_version_on_openrouter() {
    let candidates = [
        candidate(
            "openrouter/anthropic/claude-opus-4.8",
            "Claude Opus 4.8",
            ModelTier::Frontier,
        ),
        candidate(
            "aura-claude-opus-4-8",
            "Claude Opus 4.8",
            ModelTier::Frontier,
        ),
        candidate(
            "claude/claude-opus-4-7",
            "Claude Opus 4.7",
            ModelTier::Frontier,
        ),
    ];
    assert_eq!(
        resolve_alias("opus", &candidates, GATE_MODEL_ID),
        Some("claude/claude-opus-4-7")
    );
    // Naming the route still reaches it.
    assert_eq!(
        resolve_alias("openrouter opus", &candidates, GATE_MODEL_ID),
        Some("openrouter/anthropic/claude-opus-4.8")
    );
}

#[test]
fn opus_with_gate_current_and_only_openrouter_available_picks_openrouter_4_7() {
    let candidates = only_openrouter_available(catalog());
    assert_eq!(
        resolve_alias("opus", &candidates, GATE_MODEL_ID),
        Some("openrouter/anthropic/claude-opus-4.7")
    );
}

#[test]
fn available_older_model_beats_unavailable_newer_one() {
    let candidates = [
        unavailable(candidate(
            "claude/claude-opus-4-7",
            "Claude Opus 4.7",
            ModelTier::Frontier,
        )),
        candidate(
            "aura-claude-opus-4-5",
            "Claude Opus 4.5",
            ModelTier::Frontier,
        ),
    ];
    assert_eq!(
        resolve_alias("opus", &candidates, "claude/claude-sonnet-4-6"),
        Some("aura-claude-opus-4-5")
    );
}

#[test]
fn best_unavailable_match_prefers_the_router_route() {
    let candidates: Vec<_> = catalog().into_iter().map(unavailable).collect();
    assert_eq!(
        resolve_alias("opus", &candidates, ""),
        Some("aura-claude-opus-4-7")
    );
    assert_eq!(
        resolve_alias("haiku", &candidates, GATE_MODEL_ID),
        Some("aura-claude-haiku-4-5")
    );
}

#[test]
fn dated_plan_id_stays_on_the_current_route() {
    let candidates = [
        candidate("aura-claude-haiku-4-5", "Claude Haiku 4.5", ModelTier::Fast),
        candidate(
            "claude/claude-haiku-4-5-20251001",
            "Claude Haiku 4.5",
            ModelTier::Fast,
        ),
    ];
    assert_eq!(
        resolve_alias("haiku", &candidates, "claude/claude-sonnet-4-6"),
        Some("claude/claude-haiku-4-5-20251001")
    );
    // Off any route, the date does not make the key longer either,
    // so the route order picks the plan.
    assert_eq!(
        resolve_alias("haiku", &candidates, ""),
        Some("claude/claude-haiku-4-5-20251001")
    );
    // The linked plan wins over the current route.
    assert_eq!(
        resolve_alias("haiku", &candidates, "aura-claude-sonnet-4-6"),
        Some("claude/claude-haiku-4-5-20251001")
    );
}

#[test]
fn without_date_drops_only_a_trailing_snapshot() {
    let cases = [
        ("claude-haiku-4-5-20251001", "claude-haiku-4-5"),
        ("gpt-5-4-2026-03-05", "gpt-5-4"),
        ("claude-opus-4-1234", "claude-opus-4"),
        ("claude-opus-4-7", "claude-opus-4-7"),
        ("gpt-5-4-mini", "gpt-5-4-mini"),
        ("model-123-04-05", "model-123-04-05"),
        ("qwen3:4b", "qwen3:4b"),
    ];
    for (key, expected) in cases {
        assert_eq!(without_date(key), expected, "{key}");
    }
}
