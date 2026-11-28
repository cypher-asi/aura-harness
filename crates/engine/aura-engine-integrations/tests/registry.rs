//! The integration model end to end, with a fake service: the registry,
//! credentials that are never stored in plain text, health checks and
//! their time limits, and which agents get which tools.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use async_trait::async_trait;
use aura_engine_integrations::{
    tool_name, Access, AuthKind, ConfigField, FieldKind, HealthReport, Integration,
    IntegrationContext, IntegrationError, IntegrationId, KeyAuth, Manifest, MemorySecrets,
    Registry, SealedSecrets, Secret, SecretStore, Status, ToolEffect, ToolSpec,
};
use aura_exec_tools::{Sandbox, Tool, ToolConfig, ToolContext};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

const GOOD_KEY: &str = "sk-live-0123456789";

/// A service that accepts [`GOOD_KEY`] only, can be made slow, and has
/// one tool, `echo`.
struct Fake {
    manifest: Manifest,
    slow: bool,
    checks: AtomicUsize,
}

fn fake(id: &str, slow: bool) -> Arc<Fake> {
    let mut manifest = Manifest::new(IntegrationId::new(id).unwrap(), format!("Fake {id}"));
    manifest.auth = AuthKind::ApiKey(KeyAuth {
        label: "API key".to_string(),
        env: vec![format!(
            "AURA_TEST_{}_KEY",
            id.to_uppercase().replace('-', "_")
        )],
        hint: "from the fake's settings".to_string(),
    });
    manifest.config = vec![ConfigField {
        key: "region".to_string(),
        label: "Region".to_string(),
        kind: FieldKind::Choice(vec!["eu".to_string(), "us".to_string()]),
        required: false,
        default: Some("eu".to_string()),
    }];
    manifest.tools = vec![ToolSpec {
        name: "echo".to_string(),
        description: "Echo the arguments".to_string(),
        input_schema: serde_json::json!({"type": "object"}),
        effect: ToolEffect::Read,
        timeout: None,
    }];
    Arc::new(Fake {
        manifest,
        slow,
        checks: AtomicUsize::new(0),
    })
}

#[async_trait]
impl Integration for Fake {
    fn manifest(&self) -> &Manifest {
        &self.manifest
    }

    async fn health(&self, ctx: &IntegrationContext) -> Result<HealthReport, IntegrationError> {
        self.checks.fetch_add(1, Ordering::SeqCst);
        if self.slow {
            tokio::time::sleep(Duration::from_secs(30)).await;
        }
        let key = ctx.require_secret("add a key")?;
        if key.expose() == GOOD_KEY {
            Ok(HealthReport::new(format!(
                "region {}",
                ctx.config("region").unwrap_or("?")
            )))
        } else {
            Err(IntegrationError::Http {
                id: ctx.id().clone(),
                status: 401,
                message: "bad key".to_string(),
            })
        }
    }

    async fn call_tool(
        &self,
        ctx: &IntegrationContext,
        tool: &str,
        args: serde_json::Value,
    ) -> Result<serde_json::Value, IntegrationError> {
        assert_eq!(tool, "echo");
        assert!(ctx.secret().is_some());
        Ok(serde_json::json!({ "echo": args }))
    }
}

fn registry_with(integrations: &[Arc<Fake>]) -> Registry {
    let mut registry =
        Registry::new(Arc::new(MemorySecrets::default())).with_timeout(Duration::from_millis(200));
    for integration in integrations {
        registry
            .add(Arc::clone(integration) as Arc<dyn Integration>)
            .unwrap();
    }
    registry
}

#[test]
fn ids_are_unique_and_the_list_keeps_its_order() {
    let mut registry = registry_with(&[fake("beta", false), fake("alpha", false)]);
    let names: Vec<&str> = registry.list().iter().map(|m| m.id.as_str()).collect();
    assert_eq!(names, ["beta", "alpha"]);
    let again = registry.add(fake("alpha", false));
    assert!(matches!(again, Err(IntegrationError::Duplicate(id)) if id.as_str() == "alpha"));
    assert!(registry.get("gamma").is_none());
}

#[tokio::test]
async fn connect_stores_a_key_only_after_its_health_check_passes() {
    let service = fake("svc", false);
    let registry = registry_with(&[Arc::clone(&service)]);
    assert!(matches!(
        registry.status("svc").await.unwrap(),
        Status::NotConnected { .. }
    ));

    let refused = registry.connect("svc", Secret::new("wrong")).await;
    assert!(matches!(
        refused,
        Err(IntegrationError::Http { status: 401, .. })
    ));
    assert!(
        !registry.status("svc").await.unwrap().is_connected(),
        "nothing stored"
    );

    let report = registry
        .connect("svc", Secret::new(GOOD_KEY))
        .await
        .unwrap();
    assert_eq!(
        report.summary, "region eu",
        "config defaults reach the check"
    );
    assert_eq!(
        registry.status("svc").await.unwrap(),
        Status::Connected {
            via: "stored api key".to_string()
        }
    );
    assert_eq!(registry.test("svc").await.unwrap().summary, "region eu");

    assert!(registry.disconnect("svc").await.unwrap());
    assert!(!registry.status("svc").await.unwrap().is_connected());
    assert!(!registry.disconnect("svc").await.unwrap(), "nothing left");
}

#[tokio::test]
async fn a_health_check_past_its_time_limit_times_out() {
    let registry = registry_with(&[fake("slow", true)]);
    let started = std::time::Instant::now();
    let err = registry
        .connect("slow", Secret::new(GOOD_KEY))
        .await
        .unwrap_err();
    assert!(matches!(err, IntegrationError::Timeout { .. }), "{err}");
    assert!(err.to_string().contains("did not answer"), "{err}");
    assert!(started.elapsed() < Duration::from_secs(5));
    assert!(!registry.status("slow").await.unwrap().is_connected());
}

#[tokio::test]
async fn config_values_must_fit_their_field() {
    let registry = registry_with(&[fake("svc", false)]);
    registry.configure("svc", "region", "us").await.unwrap();
    let ctx = registry.context("svc").await.unwrap();
    assert_eq!(ctx.config("region"), Some("us"));
    assert!(registry.configure("svc", "region", "mars").await.is_err());
    assert!(registry.configure("svc", "nope", "x").await.is_err());
    registry.configure("svc", "region", "").await.unwrap();
    let ctx = registry.context("svc").await.unwrap();
    assert_eq!(
        ctx.config("region"),
        Some("eu"),
        "cleared back to the default"
    );
}

#[tokio::test]
async fn an_env_variable_connects_and_a_stored_key_wins_over_it() {
    let registry = registry_with(&[fake("env-svc", false)]);
    std::env::set_var("AURA_TEST_ENV_SVC_KEY", "from-env");
    let status = registry.status("env-svc").await.unwrap();
    assert_eq!(
        status,
        Status::Connected {
            via: "AURA_TEST_ENV_SVC_KEY".to_string()
        }
    );
    registry
        .connect("env-svc", Secret::new(GOOD_KEY))
        .await
        .unwrap();
    let ctx = registry.context("env-svc").await.unwrap();
    assert_eq!(ctx.secret().unwrap().expose(), GOOD_KEY);
    std::env::remove_var("AURA_TEST_ENV_SVC_KEY");
}

#[tokio::test]
async fn tools_go_only_to_allowed_agents_of_connected_integrations() {
    let registry = Arc::new(registry_with(&[fake("svc", false)]));
    assert!(
        registry.tools_for("agent-a", None).await.is_empty(),
        "not connected"
    );

    registry
        .connect("svc", Secret::new(GOOD_KEY))
        .await
        .unwrap();
    let tools = registry.tools_for("agent-a", None).await;
    assert_eq!(tools.len(), 1);
    assert_eq!(tools[0].name(), "svc_echo");
    assert!(tools[0]
        .definition()
        .description
        .starts_with("Fake svc integration:"));

    let only = Access::Only {
        agents: vec!["agent-b".to_string()],
        channels: vec!["chan".to_string()],
    };
    registry.set_access("svc", only).await.unwrap();
    assert!(registry.tools_for("agent-a", None).await.is_empty());
    assert_eq!(registry.tools_for("agent-a", Some("chan")).await.len(), 1);
    assert_eq!(registry.tools_for("agent-b", None).await.len(), 1);
}

#[tokio::test]
async fn a_tool_runs_through_the_registry_and_checks_again_on_each_call() {
    let registry = Arc::new(registry_with(&[fake("svc", false)]));
    registry
        .connect("svc", Secret::new(GOOD_KEY))
        .await
        .unwrap();
    let tool = registry.tools_for("agent-a", None).await.remove(0);
    let dir = tempfile::tempdir().unwrap();
    let ctx = ToolContext::new(Sandbox::new(dir.path()).unwrap(), ToolConfig::default());
    let result = tool
        .execute(&ctx, serde_json::json!({"x": 1}))
        .await
        .unwrap();
    assert!(result.ok);
    assert_eq!(
        String::from_utf8_lossy(&result.stdout),
        r#"{"echo":{"x":1}}"#
    );

    registry.disconnect("svc").await.unwrap();
    let err = tool.execute(&ctx, serde_json::json!({})).await.unwrap_err();
    assert!(err.to_string().contains("not connected"), "{err}");
    let denied = registry
        .call("svc", "nope", serde_json::json!({}), ("agent-a", None))
        .await;
    assert!(matches!(denied, Err(IntegrationError::UnknownTool { .. })));
}

#[test]
fn tool_names_carry_the_integration_prefix() {
    let id = IntegrationId::new("run-pod").unwrap();
    assert_eq!(tool_name(&id, "list_pods"), "run_pod_list_pods");
}

#[test]
fn a_secret_never_shows_in_debug_or_display() {
    let secret = Secret::new(format!("  {GOOD_KEY}\n"));
    assert_eq!(secret.expose(), GOOD_KEY, "trimmed");
    assert!(!format!("{secret:?} {secret}").contains("sk-live"));
}

#[test]
fn sealed_credentials_are_never_written_in_plain_text() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("integrations").join("secrets.sealed");
    let key = [7u8; 32];
    let store = SealedSecrets::new(path.clone(), &key);
    let id = IntegrationId::new("svc").unwrap();
    store.put(&id, &Secret::new(GOOD_KEY)).unwrap();

    let bytes = std::fs::read(&path).unwrap();
    assert!(aura_store_db::SealCipher::is_sealed(&bytes));
    let text = String::from_utf8_lossy(&bytes);
    assert!(!text.contains(GOOD_KEY) && !text.contains("svc"), "{text}");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
    }

    let reopened = SealedSecrets::new(path.clone(), &key);
    assert_eq!(reopened.get(&id).unwrap().unwrap().expose(), GOOD_KEY);
    let wrong = SealedSecrets::new(path.clone(), &[8u8; 32]);
    let err = wrong.get(&id).unwrap_err();
    assert!(err.to_string().contains("does not open"), "{err}");
    assert!(
        wrong.put(&id, &Secret::new("x")).is_err(),
        "never overwritten"
    );
    assert!(reopened.remove(&id).unwrap());
    assert!(reopened.get(&id).unwrap().is_none());

    std::fs::write(&path, b"{\"svc\":\"plain\"}").unwrap();
    assert!(
        reopened.get(&id).is_err(),
        "a plain file is refused, not read"
    );
}

#[tokio::test]
async fn settings_persist_and_hold_no_credential() {
    let dir = tempfile::tempdir().unwrap();
    let settings = dir.path().join("config.json");
    let make = || {
        let mut registry = Registry::new(Arc::new(MemorySecrets::default()))
            .with_settings_file(settings.clone())
            .unwrap();
        registry.add(fake("svc", false)).unwrap();
        registry
    };
    let registry = make();
    registry.configure("svc", "region", "us").await.unwrap();
    registry
        .connect("svc", Secret::new(GOOD_KEY))
        .await
        .unwrap();
    registry
        .set_access(
            "svc",
            Access::Only {
                agents: vec!["a".to_string()],
                channels: Vec::new(),
            },
        )
        .await
        .unwrap();

    let text = std::fs::read_to_string(&settings).unwrap();
    assert!(!text.contains(GOOD_KEY), "{text}");
    let again = make();
    assert_eq!(
        again.context("svc").await.unwrap().config("region"),
        Some("us")
    );
    assert!(!again
        .access(&IntegrationId::new("svc").unwrap())
        .allows("b", None));
}

#[tokio::test]
async fn views_list_every_integration_with_its_status() {
    let registry = registry_with(&[fake("one", false), fake("two", false)]);
    registry
        .connect("two", Secret::new(GOOD_KEY))
        .await
        .unwrap();
    let views = registry.views().await;
    assert_eq!(views.len(), 2);
    assert!(!views[0].status.is_connected() && !views[0].stored);
    assert!(views[1].status.is_connected() && views[1].stored);
    assert_eq!(views[1].config[0].1.as_deref(), Some("eu"));
    assert_eq!(views[1].access, Access::AllAgents);
}
