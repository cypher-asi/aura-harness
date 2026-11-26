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

mod exact;
mod resolution;
mod versions;
