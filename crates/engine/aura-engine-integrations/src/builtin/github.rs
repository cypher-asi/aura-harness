//! GitHub: the `gh` CLI login or a token, as the REPO tab already uses
//! them, and whether the hub has a GitHub App for cloud pushes.
//!
//! - **Status** (local): a stored token or `GH_TOKEN`/`GITHUB_TOKEN`,
//!   else `gh auth token` succeeding (it reads gh's own config, no
//!   network; its output is dropped unread).
//! - **Test**: `GET <api>/user` with the token, else `gh api user`.
//! - **Connect** stores a token, sealed. **Disconnect** removes only that
//!   token; a `gh` login or an env token stays.
//!
//! The GitHub App's installation tokens for cloud machines are made and
//! delivered by `aura-engine-cloud` as before (boxd host-bound secrets);
//! nothing here sends a GitHub credential to a machine.

use crate::context::IntegrationContext;
use crate::error::IntegrationError;
use crate::id::IntegrationId;
use crate::integration::{default_status, HealthReport, Integration, Status};
use crate::manifest::{AuthKind, ConfigField, FieldKind, KeyAuth, Manifest};
use async_trait::async_trait;
use std::process::Stdio;
use tokio::process::Command;

/// Where GitHub's REST API is, unless configured otherwise.
const DEFAULT_API: &str = "https://api.github.com";

/// The GitHub integration.
pub struct GitHub {
    manifest: Manifest,
    gh: String,
    app: bool,
}

impl GitHub {
    /// GitHub, noting whether the hub has a GitHub App (`app`).
    #[must_use]
    pub fn new(app: bool) -> Self {
        Self::with_gh("gh", app)
    }

    /// GitHub calling the `gh` at `gh` (a test's stand-in).
    #[must_use]
    pub fn with_gh(gh: &str, app: bool) -> Self {
        Self {
            manifest: manifest(),
            gh: gh.to_string(),
            app,
        }
    }

    fn app_note(&self) -> &'static str {
        if self.app {
            "; GitHub App set for cloud pushes"
        } else {
            ""
        }
    }

    /// Run `gh <args>` within the context's time limit; its stdout when
    /// it succeeds, `None` when it fails or isn't installed.
    async fn gh(&self, ctx: &IntegrationContext, args: &[&str]) -> Option<String> {
        let mut command = Command::new(&self.gh);
        command
            .args(args)
            .env("GH_PROMPT_DISABLED", "1")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true);
        let run = async {
            command.output().await.map_err(|e| IntegrationError::Other {
                id: ctx.id().clone(),
                message: format!("{}: {e}", self.gh),
            })
        };
        let output = ctx.timed(run).await.ok()?;
        output
            .status
            .success()
            .then(|| String::from_utf8_lossy(&output.stdout).trim().to_string())
    }
}

#[async_trait]
impl Integration for GitHub {
    fn manifest(&self) -> &Manifest {
        &self.manifest
    }

    async fn status(&self, ctx: &IntegrationContext) -> Status {
        if ctx.secret().is_some() {
            return match default_status(&self.manifest, ctx) {
                Status::Connected { via } => Status::Connected {
                    via: format!("{via}{}", self.app_note()),
                },
                other => other,
            };
        }
        if self.gh(ctx, &["auth", "token"]).await.is_some() {
            return Status::Connected {
                via: format!("gh login{}", self.app_note()),
            };
        }
        Status::NotConnected {
            hint: "run gh auth login, or add a token".to_string(),
        }
    }

    async fn health(&self, ctx: &IntegrationContext) -> Result<HealthReport, IntegrationError> {
        if let Some(token) = ctx.secret() {
            let api = ctx.config("api_url").unwrap_or(DEFAULT_API);
            let request = ctx
                .http()
                .get(format!("{}/user", api.trim_end_matches('/')))
                .bearer_auth(token.expose())
                .header("Accept", "application/vnd.github+json")
                .header("User-Agent", "aura");
            let user = ctx.send(request).await?;
            let login = user["login"].as_str().unwrap_or("an unnamed account");
            return Ok(HealthReport::new(format!("signed in as {login}")));
        }
        match self.gh(ctx, &["api", "user", "--jq", ".login"]).await {
            Some(login) if !login.is_empty() => {
                Ok(HealthReport::new(format!("gh signed in as {login}")))
            }
            _ => Err(IntegrationError::NotConnected {
                id: ctx.id().clone(),
                hint: "run gh auth login, or add a token".to_string(),
            }),
        }
    }
}

fn manifest() -> Manifest {
    let mut manifest = Manifest::new(IntegrationId::builtin("github"), "GitHub");
    manifest.icon = "⑂".to_string();
    manifest.description = "Repositories, pull requests and pushes".to_string();
    manifest.provides = vec![
        "pull requests on the REPO tab".to_string(),
        "pushes from cloud machines (GitHub App)".to_string(),
    ];
    manifest.auth = AuthKind::CliLogin {
        program: "gh".to_string(),
        login: "gh auth login".to_string(),
        token: Some(KeyAuth {
            label: "Token".to_string(),
            env: vec!["GH_TOKEN".to_string(), "GITHUB_TOKEN".to_string()],
            hint: "a personal access token from github.com/settings/tokens".to_string(),
        }),
    };
    manifest.config = vec![ConfigField {
        key: "api_url".to_string(),
        label: "API URL".to_string(),
        kind: FieldKind::Url,
        required: false,
        default: Some(DEFAULT_API.to_string()),
        advanced: true,
    }];
    manifest
}
