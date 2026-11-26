use super::*;

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
