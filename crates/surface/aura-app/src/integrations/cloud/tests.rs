use super::*;
use aura_engine_integrations::{IntegrationId, MemorySecrets, Registry, SecretStore};
use std::sync::Arc;

const PANEL_KEY: &str = "bxd_panel_0123456789abcdefghij";
const ENV_KEY: &str = "bxd_env_0123456789abcdefghijkl";

fn env(vars: &[(&str, &str)]) -> CloudConfig {
    let vars: Vec<(String, String)> = vars
        .iter()
        .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
        .collect();
    CloudConfig::from_lookup(|name| {
        vars.iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.clone())
    })
    .unwrap()
}

/// The app's integrations with boxd's key `stored` in their store.
fn service(stored: Option<&str>) -> Integrations {
    let secrets = Arc::new(MemorySecrets::default());
    if let Some(key) = stored {
        let boxd = IntegrationId::new("boxd").unwrap();
        secrets.put(&boxd, &Secret::new(key)).unwrap();
    }
    Integrations::with_registry(Registry::new(secrets), Default::default())
}

fn api_key(config: &CloudConfig) -> Option<String> {
    match config.credential.as_ref()? {
        BoxdCredential::ApiKey(key) => Some(key.expose().to_string()),
        BoxdCredential::Token(token) => Some(format!("token {}", token.expose())),
    }
}

#[test]
fn the_panels_stored_key_is_what_cloud_mode_uses() {
    let (config, source) = service(Some(PANEL_KEY)).cloud_config(env(&[]));
    assert!(
        config.is_configured(),
        "cloud mode starts with only a panel key"
    );
    assert_eq!(api_key(&config).as_deref(), Some(PANEL_KEY));
    assert_eq!(source, BoxdKeySource::Integrations);
}

#[test]
fn a_key_from_the_environment_or_env_file_still_works() {
    let (config, source) = service(None).cloud_config(env(&[("BOXD_API_KEY", ENV_KEY)]));
    assert_eq!(api_key(&config).as_deref(), Some(ENV_KEY));
    assert_eq!(source, BoxdKeySource::EnvApiKey);
    let (config, source) = service(None).cloud_config(env(&[("BOXD_TOKEN", "jwt")]));
    assert_eq!(api_key(&config).as_deref(), Some("token jwt"));
    assert_eq!(source, BoxdKeySource::EnvToken);
    let (config, source) = service(None).cloud_config(env(&[]));
    assert!(!config.is_configured());
    assert_eq!(source, BoxdKeySource::None);
}

#[test]
fn the_stored_key_wins_over_the_environment() {
    let both = env(&[("BOXD_API_KEY", ENV_KEY), ("BOXD_TOKEN", "jwt")]);
    let (config, source) = service(Some(PANEL_KEY)).cloud_config(both);
    assert_eq!(api_key(&config).as_deref(), Some(PANEL_KEY));
    assert_eq!(source, BoxdKeySource::Integrations);
}

#[test]
fn a_stored_value_without_the_key_prefix_is_used_as_a_token() {
    let (config, _) = with_stored_key(env(&[]), Some(&Secret::new("a-jwt")));
    assert_eq!(api_key(&config).as_deref(), Some("token a-jwt"));
    let (config, source) =
        with_stored_key(env(&[("BOXD_API_KEY", ENV_KEY)]), Some(&Secret::new(" ")));
    assert_eq!(
        source,
        BoxdKeySource::EnvApiKey,
        "an empty stored key is no key"
    );
    assert_eq!(api_key(&config).as_deref(), Some(ENV_KEY));
}

#[test]
fn the_source_is_named_without_the_key() {
    let (config, source) = service(Some(PANEL_KEY)).cloud_config(env(&[]));
    let said = format!("{source} {:?}", config.credential);
    assert!(!said.contains(PANEL_KEY), "{said}");
    assert!(source.to_string().contains("Integrations panel"));
}

#[tokio::test]
async fn a_refresh_notes_a_stored_key_for_the_cloud_views() {
    let service = service(Some(PANEL_KEY));
    assert!(!service.has_stored_boxd_key(), "unknown before a refresh");
    service.refresh().await;
    assert!(service.has_stored_boxd_key());
}
