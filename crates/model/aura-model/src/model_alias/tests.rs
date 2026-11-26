use super::*;

fn candidate(id: &'static str, label: &'static str, tier: ModelTier) -> AliasCandidate<'static> {
    AliasCandidate {
        id,
        label,
        tier,
        available: true,
    }
}

fn unavailable(mut candidate: AliasCandidate<'static>) -> AliasCandidate<'static> {
    candidate.available = false;
    candidate
}

fn catalog() -> Vec<AliasCandidate<'static>> {
    vec![
        candidate(
            "aura-claude-opus-4-7",
            "Claude Opus 4.7",
            ModelTier::Frontier,
        ),
        candidate(
            "aura-claude-sonnet-4-6",
            "Claude Sonnet 4.6",
            ModelTier::Standard,
        ),
        candidate("aura-claude-haiku-4-5", "Claude Haiku 4.5", ModelTier::Fast),
        candidate(
            "claude/claude-opus-4-7",
            "Claude Opus 4.7",
            ModelTier::Frontier,
        ),
        candidate(
            "claude/claude-sonnet-4-6",
            "Claude Sonnet 4.6",
            ModelTier::Standard,
        ),
        candidate(
            "openrouter/anthropic/claude-opus-4.7",
            "Claude Opus 4.7",
            ModelTier::Frontier,
        ),
        candidate(
            "openrouter/anthropic/claude-opus-4.1",
            "Claude Opus 4.1",
            ModelTier::Frontier,
        ),
        candidate(
            "openrouter/anthropic/claude-haiku-4.5",
            "Claude Haiku 4.5",
            ModelTier::Fast,
        ),
    ]
}

fn only_openrouter_available(
    candidates: Vec<AliasCandidate<'static>>,
) -> Vec<AliasCandidate<'static>> {
    candidates
        .into_iter()
        .map(|mut c| {
            c.available = c.id.starts_with("openrouter/");
            c
        })
        .collect()
}

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

#[test]
fn deepseek_prefers_latest_release_over_variants_and_older_versions() {
    let candidates = [
        candidate(
            "openrouter/deepseek/deepseek-chat-v3.1",
            "DeepSeek V3.1",
            ModelTier::Standard,
        ),
        candidate(
            "openrouter/deepseek/deepseek-v3.2",
            "DeepSeek V3.2",
            ModelTier::Standard,
        ),
        candidate(
            "openrouter/deepseek/deepseek-v3.2-exp",
            "DeepSeek V3.2 Exp",
            ModelTier::Standard,
        ),
        candidate(
            "openrouter/deepseek/deepseek-r1",
            "DeepSeek R1",
            ModelTier::Standard,
        ),
        candidate(
            "openrouter/deepseek/deepseek-r1-distill-qwen-32b",
            "DeepSeek R1 Distill Qwen 32B",
            ModelTier::Standard,
        ),
    ];
    for query in ["deepseek", "deep", "openrouter deepseek", "DeepSeek"] {
        assert_eq!(
            resolve_alias(query, &candidates, ""),
            Some("openrouter/deepseek/deepseek-v3.2"),
            "{query}"
        );
    }
}

fn gpt_catalog() -> Vec<AliasCandidate<'static>> {
    vec![
        candidate(
            "openrouter/openai/gpt-5.4-mini",
            "GPT-5.4 Mini",
            ModelTier::Fast,
        ),
        candidate("chatgpt/gpt-5.4", "GPT-5.4", ModelTier::Frontier),
        candidate("openrouter/openai/gpt-4.5", "GPT-4.5", ModelTier::Frontier),
    ]
}

#[test]
fn gpt_prefers_frontier_over_fast_mini() {
    assert_eq!(
        resolve_alias("gpt", &gpt_catalog(), ""),
        Some("chatgpt/gpt-5.4")
    );
}

#[test]
fn gpt_numbers_must_be_adjacent_and_in_order() {
    assert_eq!(
        resolve_alias("gpt 5.4", &gpt_catalog(), ""),
        Some("chatgpt/gpt-5.4")
    );
    assert_eq!(
        resolve_alias("gpt 4.5", &gpt_catalog(), ""),
        Some("openrouter/openai/gpt-4.5")
    );
    let older_only = [candidate(
        "openrouter/openai/gpt-4.5",
        "GPT-4.5",
        ModelTier::Frontier,
    )];
    assert_eq!(resolve_alias("gpt 5.4", &older_only, ""), None);
}

#[test]
fn number_after_a_word_names_the_version() {
    assert_eq!(
        resolve_alias("gpt 4", &gpt_catalog(), ""),
        Some("openrouter/openai/gpt-4.5")
    );
    let newer_only = [candidate("chatgpt/gpt-5.4", "GPT-5.4", ModelTier::Frontier)];
    assert_eq!(resolve_alias("gpt 4", &newer_only, ""), None);

    let sonnet = [candidate(
        "claude/claude-sonnet-4-5",
        "Claude Sonnet 4.5",
        ModelTier::Standard,
    )];
    assert_eq!(
        resolve_alias("claude 4.5", &sonnet, ""),
        Some("claude/claude-sonnet-4-5")
    );
    let opus = [candidate(
        "claude/claude-opus-4-7",
        "Claude Opus 4.7",
        ModelTier::Frontier,
    )];
    assert_eq!(
        resolve_alias("opus 4", &opus, ""),
        Some("claude/claude-opus-4-7")
    );
}

#[test]
fn a_later_placement_of_a_word_still_binds_its_number() {
    let candidates = [candidate(
        "aura-opus-3-opus-4",
        "Mixed",
        ModelTier::Frontier,
    )];
    assert_eq!(
        resolve_alias("opus 4", &candidates, ""),
        Some("aura-opus-3-opus-4")
    );
    assert_eq!(resolve_alias("opus 5", &candidates, ""), None);
}

#[test]
fn prefix_matching_needs_three_characters() {
    assert_eq!(resolve_alias("op", &catalog(), ""), None);
    assert_eq!(
        resolve_alias("ope", &catalog(), ""),
        Some("openrouter/anthropic/claude-opus-4.7")
    );
    let candidates = [
        candidate("chatgpt/o4-mini", "OpenAI o4 Mini", ModelTier::Fast),
        candidate("openrouter/openai/o3", "OpenAI o3", ModelTier::Frontier),
    ];
    assert_eq!(
        resolve_alias("o3", &candidates, ""),
        Some("openrouter/openai/o3")
    );
    let longer_only = [candidate(
        "openrouter/openai/o3x",
        "OpenAI o3x",
        ModelTier::Frontier,
    )];
    assert_eq!(resolve_alias("o3", &longer_only, ""), None);
}

#[test]
fn compact_query_falls_back_to_separator_free_key() {
    assert_eq!(
        resolve_alias("gpt5", &gpt_catalog(), ""),
        Some("chatgpt/gpt-5.4")
    );
    assert_eq!(
        resolve_alias("gpt54", &gpt_catalog(), ""),
        Some("chatgpt/gpt-5.4")
    );
    assert_eq!(
        resolve_alias("opus47", &catalog(), "claude/claude-sonnet-4-6"),
        Some("claude/claude-opus-4-7")
    );
}

#[test]
fn sonnet_with_version_picks_that_version() {
    let candidates = [
        candidate(
            "claude/claude-sonnet-4-6",
            "Claude Sonnet 4.6",
            ModelTier::Standard,
        ),
        candidate(
            "claude/claude-sonnet-4-5-20250929",
            "Claude Sonnet 4.5",
            ModelTier::Standard,
        ),
    ];
    assert_eq!(
        resolve_alias("sonnet 4.5", &candidates, ""),
        Some("claude/claude-sonnet-4-5-20250929")
    );
    assert_eq!(
        resolve_alias("son", &candidates, ""),
        Some("claude/claude-sonnet-4-6")
    );
}

#[test]
fn opus_with_version_matches_only_that_version() {
    let candidates = [
        candidate(
            "claude/claude-opus-4-7",
            "Claude Opus 4.7",
            ModelTier::Frontier,
        ),
        candidate(
            "claude/claude-opus-4-6",
            "Claude Opus 4.6",
            ModelTier::Frontier,
        ),
    ];
    assert_eq!(
        resolve_alias("opus 4.6", &candidates, ""),
        Some("claude/claude-opus-4-6")
    );
}

#[test]
fn openrouter_opus_picks_the_openrouter_route() {
    assert_eq!(
        resolve_alias("openrouter opus", &catalog(), "claude/claude-sonnet-4-6"),
        Some("openrouter/anthropic/claude-opus-4.7")
    );
}

#[test]
fn qwen_picks_highest_local_version() {
    let candidates = [
        candidate("local/qwen2.5:7b", "qwen2.5:7b", ModelTier::Local),
        candidate("local/qwen3:4b", "qwen3:4b", ModelTier::Local),
        candidate("local/llama3.2:3b", "llama3.2:3b", ModelTier::Local),
    ];
    assert_eq!(
        resolve_alias("qwen", &candidates, ""),
        Some("local/qwen3:4b")
    );
}

#[test]
fn exact_id_matches_case_insensitively() {
    assert_eq!(
        resolve_alias("  OpenRouter/Anthropic/Claude-Opus-4.1 ", &catalog(), ""),
        Some("openrouter/anthropic/claude-opus-4.1")
    );
}

#[test]
fn exact_label_picks_the_best_ranked_candidate_with_that_label() {
    assert_eq!(
        resolve_alias("claude opus 4.1", &catalog(), ""),
        Some("openrouter/anthropic/claude-opus-4.1")
    );
    assert_eq!(
        resolve_alias("CLAUDE OPUS 4.7", &catalog(), "claude/claude-sonnet-4-6"),
        Some("claude/claude-opus-4-7")
    );
    assert_eq!(
        resolve_alias("Claude Opus 4.7", &catalog(), "aura-claude-sonnet-4-6"),
        Some("claude/claude-opus-4-7")
    );
    let candidates = only_openrouter_available(catalog());
    assert_eq!(
        resolve_alias("Claude Opus 4.7", &candidates, "aura-claude-sonnet-4-6"),
        Some("openrouter/anthropic/claude-opus-4.7")
    );
}

#[test]
fn resolve_exact_takes_only_exact_ids_and_labels() {
    assert_eq!(resolve_exact("opus", &catalog(), ""), None);
    assert_eq!(resolve_exact("claude/opus", &catalog(), ""), None);
    assert_eq!(
        resolve_exact(
            " Aura-Claude-Opus-4-7 ",
            &catalog(),
            "claude/claude-sonnet-4-6"
        ),
        Some("aura-claude-opus-4-7")
    );
    assert_eq!(
        resolve_exact("claude haiku 4.5", &catalog(), "aura-claude-sonnet-4-6"),
        Some("aura-claude-haiku-4-5")
    );
}

#[test]
fn gate_resolves_only_by_exact_match() {
    let mut candidates = catalog();
    candidates.insert(0, candidate(GATE_MODEL_ID, "Gate", ModelTier::Frontier));
    assert_eq!(resolve_alias("Gate", &candidates, ""), Some(GATE_MODEL_ID));
    assert_eq!(resolve_alias("gat", &candidates, ""), None);
    let only_gate = [candidate(GATE_MODEL_ID, "Gate", ModelTier::Frontier)];
    assert_eq!(resolve_alias("ga", &only_gate, ""), None);
    assert_eq!(resolve_alias("gate model", &only_gate, ""), None);
}

#[test]
fn query_with_slash_resolves_only_by_exact_id() {
    assert_eq!(resolve_alias("anthropic/opus", &catalog(), ""), None);
    assert_eq!(resolve_alias("claude/opus", &catalog(), ""), None);
}

#[test]
fn unmatched_queries_resolve_to_none() {
    for query in ["", "   ", "zzzz", "-.-", "llama"] {
        assert_eq!(resolve_alias(query, &catalog(), ""), None, "{query:?}");
    }
}

#[test]
fn number_does_not_match_a_longer_number() {
    let candidates = [candidate("aura-gpt-50", "GPT 50", ModelTier::Frontier)];
    assert_eq!(resolve_alias("5", &candidates, ""), None);

    let mut with_five = candidates.to_vec();
    with_five.push(candidate("chatgpt/gpt-5.4", "GPT-5.4", ModelTier::Frontier));
    assert_eq!(resolve_alias("5", &with_five, ""), Some("chatgpt/gpt-5.4"));
}

#[test]
fn version_skips_size_tags_and_stops_at_dates() {
    let cases: &[(&str, &[u32])] = &[
        ("claude-opus-4-7", &[4, 7]),
        ("claude-sonnet-4-5-20250929", &[4, 5]),
        ("deepseek-chat-v3-1", &[3, 1]),
        ("deepseek-r1-distill-qwen-32b", &[1]),
        ("qwen3:4b", &[3]),
        ("qwen2-5:7b", &[2, 5]),
        ("gpt-4o", &[]),
        ("claude-opus", &[]),
    ];
    for (key, expected) in cases {
        assert_eq!(version(key), *expected, "{key}");
    }
}
