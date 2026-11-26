use crate::model_catalog::*;

#[test]
fn backend_follows_prefix() {
    assert_eq!(backend_for("local/qwen3:4b"), ModelBackend::Local);
    assert_eq!(backend_for("aura-claude-sonnet-4-6"), ModelBackend::Router);
    assert_eq!(backend_for("qwen3:4b"), ModelBackend::Router);
    assert_eq!(backend_for("claude/claude-opus-4-7"), ModelBackend::Claude);
    assert_eq!(backend_for("chatgpt/gpt-5.4"), ModelBackend::Chatgpt);
    assert_eq!(
        ModelBackend::Router.required_provider(),
        Some(ProviderKind::Zos)
    );
    assert_eq!(ModelBackend::Local.required_provider(), None);
}

#[test]
fn model_key_is_the_same_on_every_route() {
    assert_eq!(model_key("aura-claude-opus-4-7"), "claude-opus-4-7");
    assert_eq!(model_key("claude/claude-opus-4-7"), "claude-opus-4-7");
    assert_eq!(model_key("chatgpt/gpt-5.4"), model_key("aura-gpt-5-4"));
}

#[test]
fn upstream_name_strips_route_prefix() {
    assert_eq!(
        upstream_model_name("claude/claude-opus-4-7"),
        "claude-opus-4-7"
    );
    assert_eq!(upstream_model_name("chatgpt/gpt-5.4"), "gpt-5.4");
    assert_eq!(upstream_model_name("local/qwen3:4b"), "qwen3:4b");
    assert_eq!(upstream_model_name("aura-gpt-5-4"), "aura-gpt-5-4");
    assert_eq!(upstream_model_name("claude/"), "claude/");
}

#[test]
fn local_model_name_strips_prefix() {
    assert_eq!(local_model_name("local/qwen3:4b"), Some("qwen3:4b"));
    assert_eq!(local_model_name("local/"), None);
    assert_eq!(local_model_name("claude-opus-4-7"), None);
}

#[test]
fn openrouter_ids_route_to_openrouter_with_their_vendor_slug() {
    for (model, upstream, key, label) in [
        (
            "openrouter/anthropic/claude-sonnet-4.6",
            "anthropic/claude-sonnet-4.6",
            "claude-sonnet-4-6",
            "anthropic/claude-sonnet-4.6 (openrouter)",
        ),
        (
            "openrouter/openai/gpt-5.4",
            "openai/gpt-5.4",
            "gpt-5-4",
            "openai/gpt-5.4 (openrouter)",
        ),
        (
            "openrouter/anthropic/claude-haiku-4.5:nitro",
            "anthropic/claude-haiku-4.5:nitro",
            "claude-haiku-4-5",
            "anthropic/claude-haiku-4.5:nitro (openrouter)",
        ),
        (
            "openrouter/meta-llama/llama-4:free",
            "meta-llama/llama-4:free",
            "llama-4",
            "meta-llama/llama-4:free (openrouter)",
        ),
    ] {
        assert_eq!(backend_for(model), ModelBackend::OpenRouter, "{model}");
        assert_eq!(upstream_model_name(model), upstream, "{model}");
        assert_eq!(model_key(model), key, "{model}");
        assert_eq!(route_label(model), label, "{model}");
    }
    assert_eq!(upstream_model_name("openrouter/"), "openrouter/");
    assert_eq!(
        ModelBackend::OpenRouter.required_provider(),
        Some(ProviderKind::OpenRouter)
    );
    assert_eq!(
        model_key("openrouter/openai/gpt-5.4"),
        model_key("chatgpt/gpt-5.4")
    );
}

#[test]
fn source_label_names_who_pays() {
    for (backend, source) in [
        (ModelBackend::Local, "local"),
        (ModelBackend::Router, "aura-router"),
        (ModelBackend::Claude, "Claude plan"),
        (ModelBackend::Chatgpt, "ChatGPT plan"),
        (ModelBackend::OpenRouter, "OpenRouter"),
    ] {
        assert_eq!(backend.source_label(), source);
    }
}
