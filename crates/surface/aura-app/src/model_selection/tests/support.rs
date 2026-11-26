use super::*;

pub(super) const LOCAL: &str = "local/qwen3:4b";

pub(super) fn signed_in(user: Option<&str>, plans: &[ProviderKind]) -> AuthSummary {
    AuthSummary {
        zos_user: user.map(str::to_string),
        subscriptions: plans.to_vec(),
        ..AuthSummary::default()
    }
}

pub(super) fn selection_on(auth: AuthSummary, model: &str) -> (ModelSelection, AgentLoopConfig) {
    let mut config = AgentLoopConfig::for_agent("seed");
    let mut selection = ModelSelection {
        current: String::new(),
        default_local: LOCAL.to_string(),
        local_url: "http://127.0.0.1:9".to_string(),
        local_status: LocalServerStatus::Ready {
            version: "0.14.0".to_string(),
            models: vec!["qwen3:4b".to_string()],
        },
        auth,
        plan_models: Vec::new(),
        dropped: BTreeSet::new(),
        router_limits: RouterLimits {
            stream_timeout: config.stream_timeout,
            stream_event_timeout: config.stream_event_timeout,
        },
        thinking: None,
        prefs_path: None,
    };
    selection.apply(&mut config, model);
    (selection, config)
}

pub(super) fn selection(user: Option<&str>) -> (ModelSelection, AgentLoopConfig) {
    selection_on(signed_in(user, &[]), LOCAL)
}
