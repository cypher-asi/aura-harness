use super::*;

pub(super) fn example(suite: &str, title: &str) -> BenchExample {
    BenchExample {
        suite: suite.to_string(),
        title: title.to_string(),
    }
}

pub(super) fn session(started_ms: u64) -> BenchSession {
    BenchSession {
        started_ms,
        title: "fix it".to_string(),
        prompts: 1,
        cost_usd: None,
        unpriced_calls: 0,
        models: Vec::new(),
        current: false,
    }
}

/// The benchmark requests sent since the last call, as
/// `<agent> <request>`.
pub(super) fn requests(rx: &mut tokio::sync::mpsc::Receiver<UiEvent>) -> Vec<String> {
    std::iter::from_fn(|| rx.try_recv().ok())
        .filter_map(|event| match event {
            UiEvent::Bench { agent_id, request } => Some(format!("{agent_id} {request:?}")),
            _ => None,
        })
        .collect()
}

/// Top-level agents `a`, in project `web`, and `b`, with `kid` under
/// `a` on screen, a benchmark's run and an archived agent between.
pub(super) fn app_with_trees() -> App {
    let mut app = App::new();
    let agent = |id: &str, depth| AgentSummary {
        id: id.to_string(),
        name: id.to_string(),
        depth,
        ..AgentSummary::default()
    };
    app.agents = vec![
        AgentSummary {
            project: Some("web".to_string()),
            ..agent("a", 0)
        },
        agent("kid", 1),
        AgentSummary {
            bench_run: true,
            ..agent("run", 0)
        },
        AgentSummary {
            archived_at_ms: Some(1),
            ..agent("old", 0)
        },
        agent("b", 0),
    ];
    app.active_agent_id = "kid".to_string();
    app
}

pub(super) const OPUS_5_5: &str = "claude/claude-opus-5-5";

/// An app offering `ids`, each available, from the provider its prefix
/// names.
pub(super) fn app_with_models(ids: &[(&str, &str)]) -> App {
    let mut app = App::new();
    app.models = ids
        .iter()
        .map(|(id, label)| crate::events::ModelOptionView {
            id: (*id).to_string(),
            label: (*label).to_string(),
            local: false,
            installed: true,
            group: id.split('/').next().unwrap_or_default().to_string(),
            provider: None,
            available: true,
            context_window: None,
        })
        .collect();
    app
}

pub(super) const DEEPSEEK: &str = "openrouter/deepseek/deepseek-v3.2";

pub(super) fn three_models() -> App {
    app_with_models(&[
        ("gate", "Gate"),
        ("claude/claude-sonnet-4-6", "Claude Sonnet 4.6"),
        (DEEPSEEK, "DeepSeek V3.2"),
        ("chatgpt/gpt-5.4", "GPT-5.4"),
    ])
}
