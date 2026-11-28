//! boxd: the cloud machines cloud agents run on.
//!
//! - **Status** (local): a stored key, else `BOXD_TOKEN` or
//!   `BOXD_API_KEY`, which `.env` files set at startup (as `aura cloud
//!   setup` wrote them). How cloud mode itself reads its key is
//!   unchanged; see `docs/integrations.md` for moving it onto the sealed
//!   store.
//! - **Test**: an API key (`bxd_…`) is exchanged at the console's
//!   `POST /api/v1/auth/token`, as cloud mode does; a `BOXD_TOKEN` is
//!   used as is and can't be tested without a machine call.
//! - The key never goes to a machine ([`crate::CloudDelivery::Never`]).

use crate::context::IntegrationContext;
use crate::error::IntegrationError;
use crate::id::IntegrationId;
use crate::integration::{HealthReport, Integration};
use crate::manifest::{AuthKind, ConfigField, FieldKind, KeyAuth, Manifest};
use async_trait::async_trait;

/// The console that exchanges keys for tokens, unless configured.
const DEFAULT_CONSOLE: &str = "https://app.boxd.sh";
/// What every boxd API key starts with (`aura-config`'s
/// `BOXD_API_KEY_PREFIX`).
const KEY_PREFIX: &str = "bxd_";

/// The boxd integration.
pub struct Boxd {
    manifest: Manifest,
}

impl Boxd {
    /// boxd.
    #[must_use]
    pub fn new() -> Self {
        Self {
            manifest: manifest(),
        }
    }
}

impl Default for Boxd {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Integration for Boxd {
    fn manifest(&self) -> &Manifest {
        &self.manifest
    }

    async fn health(&self, ctx: &IntegrationContext) -> Result<HealthReport, IntegrationError> {
        let key = ctx.require_secret("run aura cloud setup, or add an API key")?;
        if !key.expose().starts_with(KEY_PREFIX) {
            return Ok(HealthReport::new("a boxd token is set; it is used as is"));
        }
        let console = ctx.config("console_url").unwrap_or(DEFAULT_CONSOLE);
        let url = format!("{}/api/v1/auth/token", console.trim_end_matches('/'));
        let request = ctx
            .http()
            .post(url)
            .json(&serde_json::json!({ "api_key": key.expose() }));
        let answer = ctx.send(request).await?;
        if answer
            .get("token")
            .and_then(serde_json::Value::as_str)
            .is_none()
        {
            return Err(IntegrationError::Other {
                id: ctx.id().clone(),
                message: "boxd accepted the key but sent no token".to_string(),
            });
        }
        Ok(HealthReport::new("API key accepted"))
    }
}

fn manifest() -> Manifest {
    let mut manifest = Manifest::new(IntegrationId::builtin("boxd"), "boxd");
    manifest.icon = "☁".to_string();
    manifest.description = "Cloud machines for cloud agents".to_string();
    manifest.provides = vec!["a machine for each cloud agent".to_string()];
    manifest.auth = AuthKind::ApiKey(KeyAuth {
        label: "API key".to_string(),
        env: vec!["BOXD_TOKEN".to_string(), "BOXD_API_KEY".to_string()],
        hint: "run aura cloud setup, or paste a key from app.boxd.sh".to_string(),
    });
    manifest.config = vec![ConfigField {
        key: "console_url".to_string(),
        label: "Console URL".to_string(),
        kind: FieldKind::Url,
        required: false,
        default: Some(DEFAULT_CONSOLE.to_string()),
    }];
    manifest
}
