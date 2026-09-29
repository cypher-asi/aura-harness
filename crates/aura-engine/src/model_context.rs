//! Model identifier → maximum context-window lookup.
//!
//! Used by [`crate::automaton::AutomatonBridge`] when building the
//! per-run `AgentRunnerConfig` / `AgentIdentity` and by the gateway's
//! chat-session state when caller-selected models land on the wire.
//! Phase B / Commit 3 lifts this out of the gateway-side `session/state.rs`
//! so engine-internal callers don't reach back into `aura-runtime`.

/// Map a model identifier to its maximum context window in tokens.
#[must_use]
pub fn context_window_for_model(model: &str) -> u64 {
    match model {
        // Anthropic
        m if m.contains("fable-5") => 1_000_000,
        m if m.contains("mythos-5") => 1_000_000,
        m if m.contains("opus-5") => 1_000_000,
        m if m.contains("opus-4") => 1_000_000,
        m if m.contains("sonnet-4") => 1_000_000,
        m if m.contains("sonnet-5") => 1_000_000,
        m if m.contains("haiku-4") => 200_000,
        m if m.starts_with("claude") => 200_000,

        // OpenAI
        m if m.contains("gpt-6") => 1_050_000,
        m if m.contains("gpt-5.6") || m.contains("gpt-5-6") => 1_050_000,
        m if m.contains("gpt-5.5") || m.contains("gpt-5-5") => 1_050_000,
        m if m.contains("gpt-5.4-mini")
            || m.contains("gpt-5-4-mini")
            || m.contains("gpt-5.4-nano")
            || m.contains("gpt-5-4-nano") =>
        {
            400_000
        }
        m if m.contains("gpt-5.4") || m.contains("gpt-5-4") => 1_050_000,
        m if m.contains("gpt-4.1") => 1_047_576,
        m if m.contains("gpt-4o") || m.contains("gpt-4-turbo") => 128_000,
        m if m.ends_with("-o1") || m.starts_with("o1") => 200_000,
        m if m.contains("-o3") || m.starts_with("o3") => 200_000,
        m if m.contains("-o4") || m.starts_with("o4") => 200_000,

        // xAI. Aura aliases use hyphenated version separators while the
        // upstream API names use dotted versions, so cover both spellings.
        m if m.contains("grok-4.7") || m.contains("grok-4-7") => 500_000,
        m if m.contains("grok-4.6") || m.contains("grok-4-6") => 500_000,
        m if m.contains("grok-4.5") || m.contains("grok-4-5") => 500_000,
        m if m.contains("grok-4.3") || m.contains("grok-4-3") => 1_000_000,
        m if m.contains("grok-build-0.1")
            || m.contains("grok-build-0-1")
            || m.contains("grok-code-fast") =>
        {
            256_000
        }

        // Managed open-weight aliases route through Fireworks. Keep their
        // binary-sized windows aligned with the router instead of rounding
        // them to the first-party API limits.
        m if m.contains("gpt-oss-120b") || m.contains("oss-120b") => 131_072,
        m if m.contains("qwen2p5-coder-7b") || m.contains("qwen2-5-coder-7b") => 32_768,
        m if m.contains("aura-deepseek-v4")
            || m.contains("accounts/fireworks/models/deepseek-v4") =>
        {
            1_048_576
        }
        m if m.contains("deepseek") => 1_000_000,
        m if m.contains("kimi-k3") => 1_048_576,
        m if m.contains("kimi") => 262_144,
        m if m.contains("minimax-m3") => 512_000,
        m if m.contains("minimax-m2.7")
            || m.contains("minimax-m2-7")
            || m.contains("minimax-m2p7") =>
        {
            196_608
        }
        m if m.contains("glm-5.2") || m.contains("glm-5p2") || m.contains("glm-5-2") => 1_048_576,
        m if m.contains("glm-5.1") || m.contains("glm-5p1") || m.contains("glm-5-1") => 202_752,
        m if m.contains("qwen3p6-plus")
            || m.contains("qwen3-6-plus")
            || m.contains("qwen3p7-plus")
            || m.contains("qwen3-7-plus") =>
        {
            262_144
        }

        // Google
        m if m.contains("gemini") => 1_048_576,
        _ => 200_000,
    }
}

#[cfg(test)]
mod tests {
    use super::context_window_for_model;

    #[test]
    fn anthropic_aura_aliases() {
        assert_eq!(context_window_for_model("aura-claude-fable-5-1"), 1_000_000);
        assert_eq!(
            context_window_for_model("aura-claude-mythos-5-1"),
            1_000_000
        );
        assert_eq!(context_window_for_model("aura-claude-opus-5"), 1_000_000);
        assert_eq!(context_window_for_model("aura-claude-opus-4-7"), 1_000_000);
        assert_eq!(context_window_for_model("aura-claude-opus-4-6"), 1_000_000);
        assert_eq!(
            context_window_for_model("aura-claude-sonnet-4-6"),
            1_000_000
        );
        assert_eq!(context_window_for_model("aura-claude-sonnet-5"), 1_000_000);
        assert_eq!(context_window_for_model("aura-claude-haiku-4-5"), 200_000);
    }

    #[test]
    fn anthropic_bare_names() {
        assert_eq!(context_window_for_model("claude-fable-5-1"), 1_000_000);
        assert_eq!(context_window_for_model("claude-mythos-5-1"), 1_000_000);
        assert_eq!(context_window_for_model("claude-opus-5"), 1_000_000);
        assert_eq!(context_window_for_model("claude-opus-4-6"), 1_000_000);
        assert_eq!(context_window_for_model("claude-sonnet-4-6"), 1_000_000);
        assert_eq!(context_window_for_model("claude-sonnet-5"), 1_000_000);
        assert_eq!(context_window_for_model("claude-haiku-4-5"), 200_000);
        assert_eq!(context_window_for_model("claude-3-5-sonnet"), 200_000);
    }

    #[test]
    fn openai_gpt5_aura_aliases() {
        assert_eq!(context_window_for_model("aura-gpt-6-astra"), 1_050_000);
        assert_eq!(context_window_for_model("aura-gpt-6-sol"), 1_050_000);
        assert_eq!(context_window_for_model("aura-gpt-6-luna"), 1_050_000);
        assert_eq!(context_window_for_model("aura-gpt-5-6-sol"), 1_050_000);
        assert_eq!(context_window_for_model("aura-gpt-5-6-terra"), 1_050_000);
        assert_eq!(context_window_for_model("aura-gpt-5-6-luna"), 1_050_000);
        assert_eq!(context_window_for_model("aura-gpt-5-5"), 1_050_000);
        assert_eq!(context_window_for_model("aura-gpt-5-4"), 1_050_000);
        assert_eq!(context_window_for_model("aura-gpt-5-4-mini"), 400_000);
        assert_eq!(context_window_for_model("aura-gpt-5-4-nano"), 400_000);
    }

    #[test]
    fn openai_gpt5_direct_names() {
        assert_eq!(context_window_for_model("gpt-5.6"), 1_050_000);
        assert_eq!(context_window_for_model("gpt-5.6-sol"), 1_050_000);
        assert_eq!(context_window_for_model("gpt-5.6-terra"), 1_050_000);
        assert_eq!(context_window_for_model("gpt-5.6-luna"), 1_050_000);
        assert_eq!(context_window_for_model("gpt-5.5"), 1_050_000);
        assert_eq!(context_window_for_model("gpt-5.4"), 1_050_000);
        assert_eq!(context_window_for_model("gpt-5.4-mini"), 400_000);
        assert_eq!(context_window_for_model("gpt-5.4-nano"), 400_000);
    }

    #[test]
    fn openai_gpt4_and_reasoning() {
        assert_eq!(context_window_for_model("aura-gpt-4.1"), 1_047_576);
        assert_eq!(context_window_for_model("gpt-4.1"), 1_047_576);
        assert_eq!(context_window_for_model("gpt-4o"), 128_000);
        assert_eq!(context_window_for_model("gpt-4-turbo"), 128_000);
        assert_eq!(context_window_for_model("o3"), 200_000);
        assert_eq!(context_window_for_model("aura-o3"), 200_000);
        assert_eq!(context_window_for_model("o4-mini"), 200_000);
        assert_eq!(context_window_for_model("aura-o4-mini"), 200_000);
        assert_eq!(context_window_for_model("o1"), 200_000);
    }

    #[test]
    fn deepseek_and_fireworks() {
        assert_eq!(context_window_for_model("aura-deepseek-v4-pro"), 1_048_576);
        assert_eq!(
            context_window_for_model("aura-deepseek-v4-flash"),
            1_048_576
        );
        assert_eq!(context_window_for_model("deepseek-v4-pro"), 1_000_000);
        assert_eq!(context_window_for_model("aura-kimi-k3"), 1_048_576);
        assert_eq!(context_window_for_model("aura-kimi-k2-5"), 262_144);
        assert_eq!(context_window_for_model("aura-kimi-k2-6"), 262_144);
        assert_eq!(context_window_for_model("aura-kimi-k2-7-code"), 262_144);
        assert_eq!(context_window_for_model("aura-oss-120b"), 131_072);
        assert_eq!(context_window_for_model("aura-minimax-m3"), 512_000);
        assert_eq!(context_window_for_model("aura-minimax-m2-7"), 196_608);
        assert_eq!(context_window_for_model("aura-glm-5-2"), 1_048_576);
        assert_eq!(context_window_for_model("aura-glm-5-1"), 202_752);
        assert_eq!(context_window_for_model("aura-qwen3-6-plus"), 262_144);
        assert_eq!(context_window_for_model("aura-qwen3-7-plus"), 262_144);
    }

    #[test]
    fn xai_aura_aliases_and_direct_names() {
        for (alias, direct, expected) in [
            ("aura-grok-4-7", "grok-4.7", 500_000),
            ("aura-grok-4-6", "grok-4.6", 500_000),
            ("aura-grok-4-5", "grok-4.5", 500_000),
            ("aura-grok-4-3", "grok-4.3", 1_000_000),
            ("aura-grok-build-0-1", "grok-build-0.1", 256_000),
        ] {
            assert_eq!(context_window_for_model(alias), expected, "{alias}");
            assert_eq!(context_window_for_model(direct), expected, "{direct}");
        }
    }

    #[test]
    fn google_aura_aliases() {
        for model in [
            "aura-gemini-3-1-pro",
            "aura-gemini-3-5-flash",
            "aura-gemini-3-flash",
            "aura-gemini-3-1-flash-lite",
            "aura-gemini-2-5-pro",
            "aura-gemini-2-5-flash",
            "aura-gemini-2-5-flash-lite",
        ] {
            assert_eq!(context_window_for_model(model), 1_048_576, "{model}");
        }
    }

    #[test]
    fn current_aura_catalog_matches_router_windows() {
        for (model, expected) in [
            ("aura-claude-fable-5-1", 1_000_000),
            ("aura-claude-opus-5-5", 1_000_000),
            ("aura-claude-fable-5", 1_000_000),
            ("aura-claude-opus-5", 1_000_000),
            ("aura-claude-opus-4-8", 1_000_000),
            ("aura-claude-opus-4-7", 1_000_000),
            ("aura-claude-opus-4-6", 1_000_000),
            ("aura-claude-sonnet-5", 1_000_000),
            ("aura-claude-sonnet-4-6", 1_000_000),
            ("aura-claude-haiku-4-5", 200_000),
            ("aura-gpt-6-astra", 1_050_000),
            ("aura-gpt-6-sol", 1_050_000),
            ("aura-gpt-6-luna", 1_050_000),
            ("aura-gpt-5-6-sol", 1_050_000),
            ("aura-gpt-5-6-terra", 1_050_000),
            ("aura-gpt-5-6-luna", 1_050_000),
            ("aura-gpt-5-5", 1_050_000),
            ("aura-gpt-5-4", 1_050_000),
            ("aura-gpt-5-4-mini", 400_000),
            ("aura-gpt-5-4-nano", 400_000),
            ("aura-oss-120b", 131_072),
            ("aura-grok-4-7", 500_000),
            ("aura-grok-4-6", 500_000),
            ("aura-grok-4-5", 500_000),
            ("aura-grok-4-3", 1_000_000),
            ("aura-grok-build-0-1", 256_000),
            ("aura-deepseek-v4-pro", 1_048_576),
            ("aura-deepseek-v4-flash", 1_048_576),
            ("aura-kimi-k3", 1_048_576),
            ("aura-kimi-k2-7-code", 262_144),
            ("aura-kimi-k2-6", 262_144),
            ("aura-minimax-m3", 512_000),
            ("aura-glm-5-2", 1_048_576),
            ("aura-gemini-3-1-pro", 1_048_576),
            ("aura-gemini-3-5-flash", 1_048_576),
            ("aura-gemini-3-flash", 1_048_576),
            ("aura-gemini-3-1-flash-lite", 1_048_576),
            ("aura-gemini-2-5-pro", 1_048_576),
            ("aura-gemini-2-5-flash", 1_048_576),
            ("aura-gemini-2-5-flash-lite", 1_048_576),
        ] {
            assert_eq!(context_window_for_model(model), expected, "{model}");
        }
    }

    #[test]
    fn unknown_model_gets_safe_default() {
        assert_eq!(context_window_for_model("unknown-model-xyz"), 200_000);
    }
}
