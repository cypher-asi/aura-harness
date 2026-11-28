use super::*;
use async_trait::async_trait;
use aura_engine_integrations::{
    Access, AuthKind, HealthReport, Integration, IntegrationContext, IntegrationError, KeyAuth,
    Manifest, ToolEffect, ToolSpec,
};
use aura_exec_tools::ToolConfig;

/// A connected-by-key integration with one tool, `echo`.
struct Echo(Manifest);

impl Echo {
    fn new() -> Self {
        let mut manifest = Manifest::new(IntegrationId::new("echo-svc").unwrap(), "Echo");
        manifest.auth = AuthKind::ApiKey(KeyAuth {
            label: "Key".to_string(),
            env: Vec::new(),
            hint: String::new(),
        });
        manifest.tools = vec![ToolSpec {
            name: "say".to_string(),
            description: "Say it".to_string(),
            input_schema: serde_json::json!({"type": "object"}),
            effect: ToolEffect::Read,
            timeout: None,
        }];
        Self(manifest)
    }
}

#[async_trait]
impl Integration for Echo {
    fn manifest(&self) -> &Manifest {
        &self.0
    }

    async fn health(&self, _ctx: &IntegrationContext) -> Result<HealthReport, IntegrationError> {
        Ok(HealthReport::new("ok"))
    }
}

fn service(secrets: Arc<MemorySecrets>) -> Integrations {
    let mut registry = Registry::new(secrets);
    registry.add(Arc::new(Echo::new())).unwrap();
    Integrations::with_registry(registry, BuiltinOptions::default())
}

fn has(service: &Integrations, agent: &str, tool: &str) -> bool {
    let mut executor = ToolExecutor::new(ToolConfig::default());
    service.register_tools(&mut executor, agent, None);
    executor.has_tool(tool)
}

#[tokio::test]
async fn the_built_ins_are_listed_after_the_service_own_integrations() {
    let service = service(Arc::default());
    let ids: Vec<&str> = service
        .registry()
        .list()
        .iter()
        .map(|m| m.id.as_str())
        .collect();
    assert_eq!(ids, ["echo-svc", "github", "boxd", "runpod"]);
}

fn offered(service: &Integrations, agent: &str) -> Vec<String> {
    service
        .tool_definitions(agent, None)
        .into_iter()
        .map(|definition| definition.name)
        .collect()
}

#[tokio::test]
async fn a_connected_integrations_tools_reach_allowed_agents_only() {
    let service = service(Arc::default());
    service.refresh().await;
    assert!(offered(&service, "agent-a").is_empty(), "not connected yet");
    // Registered anyway, so connecting later needs no new kernel; the
    // call itself is refused until then.
    assert!(has(&service, "agent-a", "echo_svc_say"));
    let early = service
        .registry()
        .call("echo-svc", "say", serde_json::json!({}), ("agent-a", None))
        .await;
    assert!(early.is_err());

    let registry = service.registry();
    registry
        .connect("echo-svc", Secret::new("k"))
        .await
        .unwrap();
    service.refresh().await;
    assert_eq!(offered(&service, "agent-a"), ["echo_svc_say"]);

    registry
        .set_access(
            "echo-svc",
            Access::Only {
                agents: vec!["agent-b".to_string()],
                channels: Vec::new(),
            },
        )
        .await
        .unwrap();
    assert!(!has(&service, "agent-a", "echo_svc_say"));
    assert!(has(&service, "agent-b", "echo_svc_say"));
    assert!(offered(&service, "agent-a").is_empty());
    assert_eq!(offered(&service, "agent-b"), ["echo_svc_say"]);
}

#[tokio::test]
async fn a_stored_github_token_is_kept_for_the_repo_tab() {
    let secrets = Arc::new(MemorySecrets::default());
    let github = IntegrationId::new("github").unwrap();
    secrets.put(&github, &Secret::new("ghp_stored")).unwrap();
    let service = service(secrets);
    assert_eq!(service.github_token(), None, "not read before a refresh");
    service.refresh().await;
    if std::env::var_os("GH_TOKEN").is_none() && std::env::var_os("GITHUB_TOKEN").is_none() {
        assert_eq!(service.github_token().as_deref(), Some("ghp_stored"));
    }
}

#[tokio::test]
async fn the_panel_shows_each_actions_result_and_a_failure_as_failed() {
    use aura_ui::events::{IntegrationAction, IntegrationState};
    let service = service(Arc::default());
    let connect = IntegrationAction::Connect {
        secret: "k".to_string(),
    };
    let note = service.run("echo-svc", connect).await.unwrap();
    assert!(note.0, "{note:?}");
    write(&service.notes).insert("echo-svc".to_string(), note);
    let configure = IntegrationAction::Configure {
        key: "console_url".to_string(),
        value: "not a url".to_string(),
    };
    let failed = service.run("boxd", configure).await.unwrap();
    assert!(!failed.0);
    write(&service.notes).insert("boxd".to_string(), failed);

    let panel = service.panel().await;
    let echo = panel.iter().find(|view| view.id == "echo-svc").unwrap();
    assert_eq!(echo.state, IntegrationState::Connected);
    assert_eq!(echo.note.as_deref(), Some("connected: ok"));
    assert_eq!(echo.tools, ["echo_svc_say"]);
    assert!(echo.stored);
    let boxd = panel.iter().find(|view| view.id == "boxd").unwrap();
    assert_eq!(boxd.state, IntegrationState::Failed);
    assert_eq!(boxd.takes_key.as_deref(), Some("API key"));
    let github = panel.iter().find(|view| view.id == "github").unwrap();
    assert_eq!(github.login.as_deref(), Some("gh auth login"));
    assert!(
        github.config.is_empty(),
        "the unset API URL override is hidden"
    );
    assert_eq!(
        boxd.config.len(),
        0,
        "a rejected value is not saved, so still hidden"
    );
    let set = IntegrationAction::Configure {
        key: "api_url".to_string(),
        value: "https://ghe.example.com/api/v3".to_string(),
    };
    assert!(service.run("github", set).await.unwrap().0);
    let panel = service.panel().await;
    let github = panel.iter().find(|view| view.id == "github").unwrap();
    assert_eq!(github.config.len(), 1, "shown once set");
    let runpod = panel.iter().find(|view| view.id == "runpod").unwrap();
    assert_eq!(runpod.takes_key.as_deref(), Some("API key"));
    assert_eq!(runpod.tools.len(), 10);
    assert!(runpod.tools.iter().any(|tool| tool == "runpod_create_pod"));
}

#[test]
fn runpod_reads_are_invocations_and_the_rest_network_calls() {
    let classes = service(Arc::default()).tool_classes();
    assert_eq!(classes["runpod_list_pods"], ToolModeClass::Invoke);
    assert_eq!(classes["runpod_balance"], ToolModeClass::Invoke);
    assert_eq!(classes["echo_svc_say"], ToolModeClass::Invoke);
    for risky in [
        "runpod_create_pod",
        "runpod_pod_action",
        "runpod_run_job",
        "runpod_terminate_pod",
    ] {
        assert_eq!(classes[risky], ToolModeClass::Network, "{risky}");
    }
}
