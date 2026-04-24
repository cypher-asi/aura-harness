//! Slack trusted-integration handlers — `slack_list_channels` and
//! `slack_post_message`. Both gate the response through the shared
//! `ensure_slack_ok` helper to translate Slack's `ok: false` envelope
//! into a tool error.

use super::super::super::json_paths::{ensure_slack_ok, required_string};
use super::super::ToolResolver;
use crate::error::ToolError;
use aura_core::{InstalledToolRuntimeIntegration, InstalledToolRuntimeProviderExecution};
use reqwest::Method;
use serde_json::{json, Value};

impl ToolResolver {
    pub(in super::super) async fn slack_list_channels(
        &self,
        provider: &InstalledToolRuntimeProviderExecution,
        integration: &InstalledToolRuntimeIntegration,
    ) -> Result<Value, ToolError> {
        let url = format!(
            "{}/conversations.list?types=public_channel,private_channel&exclude_archived=true&limit=100",
            provider.base_url
        );
        let response = self
            .provider_json_request(Method::GET, &url, provider, integration, None)
            .await?;
        ensure_slack_ok(&response)?;
        let channels = response
            .get("channels")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .map(|channel| {
                json!({
                    "id": channel.get("id").and_then(Value::as_str).unwrap_or_default(),
                    "name": channel.get("name").and_then(Value::as_str).unwrap_or_default(),
                    "is_private": channel.get("is_private").and_then(Value::as_bool).unwrap_or(false),
                })
            })
            .collect::<Vec<_>>();
        Ok(json!({ "channels": channels }))
    }

    pub(in super::super) async fn slack_post_message(
        &self,
        provider: &InstalledToolRuntimeProviderExecution,
        integration: &InstalledToolRuntimeIntegration,
        args: &Value,
    ) -> Result<Value, ToolError> {
        let channel_id = required_string(args, &["channel_id", "channelId"])?;
        let text = required_string(args, &["text", "message"])?;
        let response = self
            .provider_json_request(
                Method::POST,
                &format!("{}/chat.postMessage", provider.base_url),
                provider,
                integration,
                Some(json!({
                    "channel": channel_id,
                    "text": text,
                })),
            )
            .await?;
        ensure_slack_ok(&response)?;
        Ok(json!({
            "message": {
                "channel": response.get("channel").and_then(Value::as_str).unwrap_or_default(),
                "ts": response.get("ts").and_then(Value::as_str).unwrap_or_default(),
            }
        }))
    }
}
