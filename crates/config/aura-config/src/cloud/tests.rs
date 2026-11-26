use super::*;
use std::collections::HashMap;

const KEY: &str = "bxd_AbCdEfGhIjKlMnOpQrStUvWxYz0123456789abcdQ";

fn config(pairs: &[(&str, &str)]) -> Result<CloudConfig, ConfigError> {
    let map: HashMap<String, String> = pairs
        .iter()
        .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
        .collect();
    CloudConfig::from_lookup(|name| map.get(name).cloned())
}

#[test]
fn without_a_credential_cloud_is_off() {
    let cfg = config(&[]).expect("defaults parse");
    assert!(!cfg.is_configured());
    assert_eq!(cfg.base_url(), DEFAULT_BOXD_BASE_URL);
    assert_eq!(cfg.size, MachineSize::S4x16);
    assert_eq!(cfg.idle_hibernate_secs, 300);
    assert!(cfg.isolated);
    assert!(cfg.fork_on_spawn);
}

#[test]
fn an_api_key_configures_cloud() {
    let cfg = config(&[(BOXD_API_KEY, KEY)]).expect("parse");
    assert!(cfg.is_configured());
    assert!(matches!(cfg.credential, Some(BoxdCredential::ApiKey(ref k)) if k.expose() == KEY));
}

#[test]
fn a_token_wins_over_a_key() {
    let cfg = config(&[
        (BOXD_API_KEY, KEY),
        (BOXD_TOKEN, "eyJhbGciOiJIUzI1NiJ9.x.y"),
    ])
    .expect("parse");
    assert!(matches!(cfg.credential, Some(BoxdCredential::Token(_))));
}

#[test]
fn blank_values_are_unset() {
    let cfg = config(&[(BOXD_API_KEY, "   "), (BOXD_TOKEN, "")]).expect("parse");
    assert!(!cfg.is_configured());
}

#[test]
fn secrets_never_print() {
    let cfg = config(&[(BOXD_API_KEY, KEY)]).expect("parse");
    let key = BoxdApiKey::new(KEY);
    let token = BoxdToken::new(KEY);
    for text in [
        format!("{key}"),
        format!("{key:?}"),
        format!("{token}"),
        format!("{token:?}"),
        format!("{cfg:?}"),
        format!("{:?}", cfg.credential),
    ] {
        assert!(!text.contains(KEY), "{text}");
        assert!(!text.contains("AbCdEfGh"), "{text}");
    }
    assert_eq!(key.to_string(), "bxd_…bcdQ");
}

#[test]
fn short_secrets_show_no_tail() {
    assert_eq!(BoxdApiKey::new("bxd_abc").to_string(), "bxd_…");
}

#[test]
fn the_old_url_variable_is_read_when_the_new_one_is_unset() {
    let cfg = config(&[(BOXD_API_URL, "https://example:9443")]).expect("parse");
    assert_eq!(cfg.base_url(), "https://example:9443");
    let cfg = config(&[
        (BOXD_API_URL, "https://old:9443"),
        (BOXD_BASE_URL, "https://new:9443"),
    ])
    .expect("parse");
    assert_eq!(cfg.base_url(), "https://new:9443");
}

#[test]
fn machine_sizes_parse_and_print() {
    for size in MachineSize::ALL {
        assert_eq!(size.as_spec().parse::<MachineSize>(), Ok(size));
        assert_eq!(size.to_string(), size.as_spec());
    }
    assert_eq!("8".parse::<MachineSize>(), Ok(MachineSize::S8x32));
    assert_eq!("4X16G".parse::<MachineSize>(), Ok(MachineSize::S4x16));
    assert_eq!("2x8GiB".parse::<MachineSize>(), Ok(MachineSize::S2x8));
    assert!("4x8".parse::<MachineSize>().is_err());
    assert!("3".parse::<MachineSize>().is_err());
    assert_eq!(MachineSize::S8x32.smaller(), Some(MachineSize::S4x16));
    assert_eq!(MachineSize::S1x4.smaller(), None);
    assert_eq!(MachineSize::S4x16.memory_bytes(), 16 * 1024 * 1024 * 1024);
}

#[test]
fn machine_size_serde_uses_the_spec_string() {
    let json = serde_json::to_string(&MachineSize::S8x32).expect("serialize");
    assert_eq!(json, "\"8x32\"");
    let back: MachineSize = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(back, MachineSize::S8x32);
}

#[test]
fn a_bad_size_names_its_variable() {
    let err = config(&[(AURA_CLOUD_SIZE, "huge")]).expect_err("must fail");
    assert!(err.to_string().contains(AURA_CLOUD_SIZE), "{err}");
}

#[test]
fn a_bad_number_names_its_variable() {
    let err = config(&[(AURA_CLOUD_IDLE_HIBERNATE_SECS, "soon")]).expect_err("must fail");
    assert!(
        err.to_string().contains(AURA_CLOUD_IDLE_HIBERNATE_SECS),
        "{err}"
    );
    let err = config(&[(AURA_CLOUD_PRICE_VCPU_HOUR, "-1")]).expect_err("must fail");
    assert!(
        err.to_string().contains(AURA_CLOUD_PRICE_VCPU_HOUR),
        "{err}"
    );
    let err = config(&[(AURA_CLOUD_ISOLATED, "maybe")]).expect_err("must fail");
    assert!(err.to_string().contains(AURA_CLOUD_ISOLATED), "{err}");
}

#[test]
fn overrides_apply() {
    let cfg = config(&[
        (AURA_CLOUD_SIZE, "8x32"),
        (AURA_CLOUD_IDLE_HIBERNATE_SECS, "60"),
        (AURA_CLOUD_IDLE_STANDBY_SECS, "20"),
        (AURA_CLOUD_ISOLATED, "0"),
        (AURA_CLOUD_FORK_ON_SPAWN, "off"),
        (AURA_CLOUD_PRICE_VCPU_HOUR, "0.05"),
        (BOXD_REGION, "nyc"),
        (BOXD_ORG, "acme"),
        (AURA_CLOUD_BINARY, " /opt/aura-linux "),
    ])
    .expect("parse");
    assert_eq!(cfg.binary, Some(PathBuf::from("/opt/aura-linux")));
    assert_eq!(cfg.size, MachineSize::S8x32);
    assert_eq!(cfg.idle_hibernate_secs, 60);
    assert_eq!(cfg.idle_standby_secs, 20);
    assert!(!cfg.isolated);
    assert!(!cfg.fork_on_spawn);
    assert_eq!(cfg.price_vcpu_hour, Some(0.05));
    assert_eq!(cfg.region.as_deref(), Some("nyc"));
    assert_eq!(cfg.org.as_deref(), Some("acme"));
}

#[test]
fn the_github_app_needs_its_id_and_key() {
    assert_eq!(config(&[]).expect("parse").github_app, None);
    let cfg = config(&[
        (AURA_GITHUB_APP_ID, " Iv23liAbc "),
        (AURA_GITHUB_APP_PRIVATE_KEY_PATH, "/keys/aura.pem"),
    ])
    .expect("parse");
    assert_eq!(
        cfg.github_app,
        Some(GithubAppConfig {
            app_id: "Iv23liAbc".into(),
            private_key_path: PathBuf::from("/keys/aura.pem"),
            workflows: false,
        })
    );
    let cfg = config(&[
        (AURA_GITHUB_APP_ID, "12345"),
        (AURA_GITHUB_APP_PRIVATE_KEY_PATH, "/keys/aura.pem"),
        (AURA_GITHUB_APP_WORKFLOWS, "1"),
    ])
    .expect("parse");
    assert!(cfg.github_app.is_some_and(|app| app.workflows));
    let half = config(&[(AURA_GITHUB_APP_ID, "12345")]).expect("parse");
    assert_eq!(half.github_app, None, "an id without a key is off");
    assert_eq!(
        expand_home("~/.aura/app.pem", Some(PathBuf::from("/home/u"))),
        PathBuf::from("/home/u/.aura/app.pem")
    );
    assert_eq!(
        expand_home("/keys/app.pem", Some(PathBuf::from("/home/u"))),
        PathBuf::from("/keys/app.pem")
    );
    let err = config(&[(AURA_GITHUB_APP_WORKFLOWS, "sometimes")]).expect_err("must fail");
    assert!(err.to_string().contains(AURA_GITHUB_APP_WORKFLOWS), "{err}");
}
