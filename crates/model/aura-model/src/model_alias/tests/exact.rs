use super::*;

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
