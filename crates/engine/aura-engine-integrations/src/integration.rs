//! The [`Integration`] trait every external service implements.

use crate::context::{IntegrationContext, SecretSource};
use crate::error::IntegrationError;
use crate::manifest::{AuthKind, Manifest};
use async_trait::async_trait;

/// Whether an integration can be used now, as far as can be told
/// without calling it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Status {
    /// Ready; `via` says how (`stored key`, `gh login as octocat`,
    /// `BOXD_API_KEY`).
    Connected {
        /// How it is signed in
        via: String,
    },
    /// Not signed in; `hint` says how to.
    NotConnected {
        /// How to connect it
        hint: String,
    },
    /// Signed in but can't work as set; `why` says what to fix.
    Misconfigured {
        /// What is wrong
        why: String,
    },
}

impl Status {
    /// Whether it is connected.
    #[must_use]
    pub const fn is_connected(&self) -> bool {
        matches!(self, Self::Connected { .. })
    }
}

/// What a passing health check found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HealthReport {
    /// One line, as the panel shows it (`signed in as octocat`)
    pub summary: String,
}

impl HealthReport {
    /// A report saying `summary`.
    #[must_use]
    pub fn new(summary: impl Into<String>) -> Self {
        Self {
            summary: summary.into(),
        }
    }
}

/// An external service aura talks to.
///
/// Implementations never log or return the credential, keep every call
/// within `ctx.timeout()` (use [`IntegrationContext::send`] or
/// [`IntegrationContext::timed`]), and never send the credential
/// anywhere but their own service.
#[async_trait]
pub trait Integration: Send + Sync {
    /// What it is.
    fn manifest(&self) -> &Manifest;

    /// Whether it is ready, from what is at hand: local and quick, no
    /// network. The default is [`default_status`].
    async fn status(&self, ctx: &IntegrationContext) -> Status {
        default_status(self.manifest(), ctx)
    }

    /// One real call that proves the credential and config work: the
    /// panel's Test, and what [`crate::Registry::connect`] runs before
    /// it stores a credential.
    async fn health(&self, ctx: &IntegrationContext) -> Result<HealthReport, IntegrationError>;

    /// Run tool `tool` (its local name) with `args`. The default knows
    /// no tools.
    async fn call_tool(
        &self,
        ctx: &IntegrationContext,
        tool: &str,
        args: serde_json::Value,
    ) -> Result<serde_json::Value, IntegrationError> {
        let _ = args;
        Err(IntegrationError::UnknownTool {
            id: ctx.id().clone(),
            tool: tool.to_string(),
        })
    }
}

/// The status a manifest's sign-in and `ctx` imply: connected when no
/// sign-in is needed or a credential is there, saying where it came
/// from; otherwise how to connect; misconfigured when a required config
/// field is unset.
#[must_use]
pub fn default_status(manifest: &Manifest, ctx: &IntegrationContext) -> Status {
    if let Some(field) = manifest
        .config
        .iter()
        .find(|field| field.required && ctx.config(&field.key).is_none())
    {
        return Status::Misconfigured {
            why: format!("set {}", field.label),
        };
    }
    let Some(key) = manifest.auth.key() else {
        return match &manifest.auth {
            AuthKind::CliLogin { login, .. } => Status::NotConnected {
                hint: format!("run {login}"),
            },
            AuthKind::OAuth { .. } => Status::NotConnected {
                hint: "OAuth sign-in is not available yet".to_string(),
            },
            AuthKind::None | AuthKind::ApiKey(_) => Status::Connected {
                via: "no sign-in needed".to_string(),
            },
        };
    };
    match ctx.secret_source() {
        Some(SecretSource::Env(name)) => Status::Connected { via: name.clone() },
        Some(SecretSource::Stored | SecretSource::Candidate) => Status::Connected {
            via: format!("stored {}", key.label.to_lowercase()),
        },
        None => Status::NotConnected {
            hint: format!("add a {}: {}", key.label.to_lowercase(), key.hint),
        },
    }
}
