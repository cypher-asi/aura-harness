//! Runpod: GPU pods, the GPU catalog, billing and serverless jobs.
//!
//! The tools themselves live in `aura_exec_tools::runpod`; this adapter
//! gives them the integration model's key, config and time limits.
//!
//! - **Key**: the stored one, else `RUNPOD_API_KEY`, read on every call
//!   and only sent as the `Authorization` header.
//! - **Test**: `GET /v2/account`.
//! - **Tools**: six read; create, start/stop/restart and run job spend
//!   money; terminate destroys. Spending limits from the config are
//!   checked before any request that spends is sent, at every approval
//!   level.
//! - The key never goes to a machine ([`crate::CloudDelivery::Never`]).

use crate::context::IntegrationContext;
use crate::error::IntegrationError;
use crate::id::IntegrationId;
use crate::integration::{default_status, HealthReport, Integration, Status};
use crate::manifest::{AuthKind, ConfigField, FieldKind, KeyAuth, Manifest, ToolEffect, ToolSpec};
use async_trait::async_trait;
use aura_exec_tools::runpod::{
    bounded, ApiKey, RunpodClient, RunpodError, RunpodOp, RunpodSettings, RunpodTool, SpendLimits,
    StaticApiKey, ToolKind, DEFAULT_API_BASE, DEFAULT_SERVERLESS_BASE,
};
use aura_exec_tools::Tool;
use std::sync::Arc;
use std::time::Duration;

/// Where to create a key; shown when Runpod isn't connected.
const KEY_HINT: &str = "create an API key at https://console.runpod.io/user/settings \
     (API docs: https://docs.runpod.io/api-reference-v2/overview)";

/// Time limit of every tool but the two below: a spending check may make
/// a few requests before the one that spends.
const TOOL_TIMEOUT: Duration = Duration::from_secs(45);
/// `run_job` with `wait` blocks for the result.
const RUN_JOB_TIMEOUT: Duration = Duration::from_secs(90);
/// `pod_logs` reads the log stream for a while.
const LOGS_TIMEOUT: Duration = Duration::from_secs(30);

/// The Runpod integration.
pub struct Runpod {
    manifest: Manifest,
}

impl Runpod {
    /// Runpod.
    #[must_use]
    pub fn new() -> Self {
        Self {
            manifest: manifest(),
        }
    }
}

impl Default for Runpod {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Integration for Runpod {
    fn manifest(&self) -> &Manifest {
        &self.manifest
    }

    async fn status(&self, ctx: &IntegrationContext) -> Status {
        if let Err(why) = SpendLimits::validate(lookup(ctx)) {
            return Status::Misconfigured { why };
        }
        default_status(&self.manifest, ctx)
    }

    async fn health(&self, ctx: &IntegrationContext) -> Result<HealthReport, IntegrationError> {
        let key = ctx.require_secret(KEY_HINT)?;
        let base = settings(ctx).api_base;
        let request = ctx
            .http()
            .get(format!("{base}/v2/account"))
            .bearer_auth(key.expose());
        let account = ctx.send(request).await?;
        let field = |name: &str| account.get(name).and_then(serde_json::Value::as_str);
        let report = HealthReport::new(format!("account {}", field("id").unwrap_or("?")));
        Ok(match field("email") {
            Some(email) => report.with_detail(mask_email(email)),
            None => report,
        })
    }

    async fn call_tool(
        &self,
        ctx: &IntegrationContext,
        tool: &str,
        args: serde_json::Value,
    ) -> Result<serde_json::Value, IntegrationError> {
        let op = RunpodOp::from_name(&format!("runpod_{tool}")).ok_or_else(|| {
            IntegrationError::UnknownTool {
                id: ctx.id().clone(),
                tool: tool.to_string(),
            }
        })?;
        let key = ctx.require_secret(KEY_HINT)?;
        let source = Arc::new(StaticApiKey(ApiKey::new(key.expose())));
        let client = RunpodClient::with_http(ctx.http().clone(), settings(ctx), source);
        let tool_call = RunpodTool::new(op, client);
        let answer = ctx
            .timed(async {
                tool_call
                    .call(&args)
                    .await
                    .map_err(|error| integration_error(ctx.id(), tool, error))
            })
            .await?;
        Ok(bounded(answer))
    }
}

/// `d***@example.com`: enough to recognize the account by.
fn mask_email(email: &str) -> String {
    match email.split_once('@') {
        Some((user, domain)) => {
            let first: String = user.chars().take(1).collect();
            format!("{first}***@{domain}")
        }
        None => "***".to_string(),
    }
}

/// Config values by key, as [`RunpodSettings::from_lookup`] reads them.
fn lookup(ctx: &IntegrationContext) -> impl Fn(&str) -> Option<String> + '_ {
    |key| ctx.config(key).map(str::to_string)
}

/// The tools' settings from `ctx`'s config: base URLs and limits.
fn settings(ctx: &IntegrationContext) -> RunpodSettings {
    RunpodSettings::from_lookup(lookup(ctx))
}

/// The integration error for a Runpod failure. Messages are the tools'
/// own, which never carry the key.
fn integration_error(id: &IntegrationId, tool: &str, error: RunpodError) -> IntegrationError {
    match error {
        RunpodError::MissingKey { .. } => IntegrationError::NotConnected {
            id: id.clone(),
            hint: KEY_HINT.to_string(),
        },
        RunpodError::Unauthorized { status, .. } => IntegrationError::Http {
            id: id.clone(),
            status,
            message: error.to_string(),
        },
        RunpodError::InvalidArgs { .. } => IntegrationError::BadArgs {
            id: id.clone(),
            tool: tool.to_string(),
            message: error.to_string(),
        },
        RunpodError::Unreachable { .. } | RunpodError::Timeout { .. } => {
            IntegrationError::Network {
                id: id.clone(),
                message: error.to_string(),
            }
        }
        RunpodError::Limit { .. } => IntegrationError::Refused {
            id: id.clone(),
            reason: error.to_string(),
        },
        other => IntegrationError::Other {
            id: id.clone(),
            message: other.to_string(),
        },
    }
}

fn manifest() -> Manifest {
    let mut manifest = Manifest::new(IntegrationId::builtin("runpod"), "Runpod");
    manifest.icon = "▣".to_string();
    manifest.description = "Rent GPUs: pods, serverless jobs and the GPU catalog".to_string();
    manifest.provides = vec![
        "GPU pods".to_string(),
        "serverless jobs".to_string(),
        "GPU prices".to_string(),
    ];
    manifest.auth = AuthKind::ApiKey(KeyAuth {
        label: "API key".to_string(),
        env: vec![aura_exec_tools::runpod::RUNPOD_API_KEY_ENV.to_string()],
        hint: KEY_HINT.to_string(),
    });
    manifest.config = config_fields();
    manifest.tools = tool_specs();
    manifest
}

fn config_fields() -> Vec<ConfigField> {
    let limits = SpendLimits::default();
    let number = |integer: bool| FieldKind::Number {
        min: Some(0.0),
        max: None,
        integer,
    };
    let field = |key: &str, label: &str, kind: FieldKind, default: String| {
        let mut field = ConfigField::new(key, label, kind);
        field.default = Some(default);
        field
    };
    let url = |key: &str, label: &str, default: &str| {
        let mut field = field(key, label, FieldKind::Url, default.to_string());
        // Overrides for tests and proxies, hidden until set.
        field.advanced = true;
        field
    };
    vec![
        field(
            "max_hourly_usd",
            "Most a pod may cost per hour (USD)",
            number(false),
            format!("{:.2}", limits.max_hourly_usd),
        ),
        field(
            "max_gpu_count",
            "Most GPUs in one pod",
            number(true),
            limits.max_gpu_count.to_string(),
        ),
        field(
            "max_running_pods",
            "Most pods running at once",
            number(true),
            limits.max_running_pods.to_string(),
        ),
        field(
            "max_job_seconds",
            "Longest a serverless job may run (seconds)",
            number(true),
            limits.max_job_seconds.to_string(),
        ),
        url("api_base", "API URL", DEFAULT_API_BASE),
        url(
            "serverless_base",
            "Serverless API URL",
            DEFAULT_SERVERLESS_BASE,
        ),
    ]
}

/// One spec per Runpod operation, described as the tools describe
/// themselves under the default limits.
fn tool_specs() -> Vec<ToolSpec> {
    let source = Arc::new(StaticApiKey(None));
    let client = RunpodClient::with_http(reqwest::Client::new(), RunpodSettings::default(), source);
    RunpodOp::ALL
        .into_iter()
        .map(|op| {
            let definition = RunpodTool::new(op, client.clone()).definition();
            ToolSpec {
                name: op
                    .name()
                    .strip_prefix("runpod_")
                    .unwrap_or_else(|| op.name())
                    .to_string(),
                description: definition.description,
                input_schema: definition.input_schema,
                effect: effect(op.kind()),
                timeout: Some(timeout(op)),
            }
        })
        .collect()
}

const fn effect(kind: ToolKind) -> ToolEffect {
    match kind {
        ToolKind::Read => ToolEffect::Read,
        ToolKind::Spends => ToolEffect::Spends,
        ToolKind::Destroys => ToolEffect::Destroys,
    }
}

const fn timeout(op: RunpodOp) -> Duration {
    match op {
        RunpodOp::RunJob => RUN_JOB_TIMEOUT,
        RunpodOp::PodLogs => LOGS_TIMEOUT,
        _ => TOOL_TIMEOUT,
    }
}

#[cfg(test)]
mod tests;
