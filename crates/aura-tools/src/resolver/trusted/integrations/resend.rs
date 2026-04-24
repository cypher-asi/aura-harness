//! Resend trusted-integration handlers — `resend_list_domains` and
//! `resend_send_email`. The latter enforces that the agent supplies at
//! least one of `html` / `text` before dispatching to the provider.

use super::super::super::json_paths::{
    optional_string, optional_string_list, required_string, required_string_list,
};
use super::super::ToolResolver;
use crate::error::ToolError;
use aura_core::{InstalledToolRuntimeIntegration, InstalledToolRuntimeProviderExecution};
use reqwest::Method;
use serde_json::{json, Value};

impl ToolResolver {
    pub(in super::super) async fn resend_list_domains(
        &self,
        provider: &InstalledToolRuntimeProviderExecution,
        integration: &InstalledToolRuntimeIntegration,
    ) -> Result<Value, ToolError> {
        let response = self
            .provider_json_request(
                Method::GET,
                &format!("{}/domains", provider.base_url),
                provider,
                integration,
                None,
            )
            .await?;
        let domains = response
            .get("data")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .map(|domain| {
                json!({
                    "id": domain.get("id").and_then(Value::as_str).unwrap_or_default(),
                    "name": domain.get("name").and_then(Value::as_str).unwrap_or_default(),
                    "status": domain.get("status").and_then(Value::as_str).unwrap_or_default(),
                    "created_at": domain.get("created_at").and_then(Value::as_str),
                    "region": domain.get("region").and_then(Value::as_str),
                    "capabilities": domain.get("capabilities").cloned().unwrap_or_else(|| json!({})),
                })
            })
            .collect::<Vec<_>>();
        Ok(json!({
            "domains": domains,
            "has_more": response.get("has_more").and_then(Value::as_bool).unwrap_or(false),
        }))
    }

    pub(in super::super) async fn resend_send_email(
        &self,
        provider: &InstalledToolRuntimeProviderExecution,
        integration: &InstalledToolRuntimeIntegration,
        args: &Value,
    ) -> Result<Value, ToolError> {
        let from = required_string(args, &["from"])?;
        let to = required_string_list(args, &["to"])?;
        let subject = required_string(args, &["subject"])?;
        let html = optional_string(args, &["html"]);
        let text = optional_string(args, &["text"]);
        let cc = optional_string_list(args, &["cc"]);
        let bcc = optional_string_list(args, &["bcc"]);

        if html.is_none() && text.is_none() {
            return Err(ToolError::ExternalToolError(
                "resend_send_email requires at least one of `html` or `text`".into(),
            ));
        }

        let mut payload = json!({
            "from": from,
            "to": to,
            "subject": subject,
        });
        if let Some(html) = html {
            payload["html"] = Value::String(html);
        }
        if let Some(text) = text {
            payload["text"] = Value::String(text);
        }
        if let Some(cc) = cc {
            payload["cc"] = json!(cc);
        }
        if let Some(bcc) = bcc {
            payload["bcc"] = json!(bcc);
        }

        let response = self
            .provider_json_request(
                Method::POST,
                &format!("{}/emails", provider.base_url),
                provider,
                integration,
                Some(payload),
            )
            .await?;
        Ok(json!({
            "email": {
                "id": response.get("id").and_then(Value::as_str).unwrap_or_default(),
            }
        }))
    }
}
