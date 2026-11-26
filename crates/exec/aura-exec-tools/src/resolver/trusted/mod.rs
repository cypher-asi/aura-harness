//! Trusted-integration runtime execution — GitHub, Linear, Slack,
//! Brave Search, Resend, and the generic `TrustedIntegrationRuntimeSpec`
//! interpreter.
//!
//! Phase 2b split this 1.1 KLoC module into a directory:
//!
//! - [`mod@spec`] — the `TrustedIntegrationRuntimeSpec` metadata schema.
//! - [`mod@bindings`] — argument binding resolution, URL templating and
//!   the JSON body / form field builders.
//! - [`mod@http`] — unified `send_provider_request` plus the legacy
//!   `provider_json_request` / `provider_form_request` thin wrappers.
//! - [`mod@transforms`] — `apply_result_transform` (Brave, project-array,
//!   project-object, wrap-pointer) plus the shared `brave_search_results`
//!   helper used by both the inline `brave_search` integration and the
//!   declarative `BraveSearch` transform.
//! - [`mod@guards`] — `apply_success_guard` (Slack OK, GraphQL errors)
//!   plus the shared `graphql_user_errors` helper used by both the
//!   declarative `GraphqlErrors` guard and the bespoke `linear_graphql`
//!   integration.
//! - [`integrations`] — per-integration handlers (`github_*`, `linear_*`,
//!   `brave_search`, `slack_*`, `resend_*`).
//!
//! `mod.rs` keeps the per-spec dispatch entry-point
//! (`execute_trusted_runtime_app_provider`), the per-integration dispatch
//! entry-point (`execute_runtime_app_provider`), and the small free
//! helpers shared across the submodules (runtime-spec parsing,
//! integration selection).

use super::json_paths::optional_string;
use super::{ToolResolver, TRUSTED_INTEGRATION_RUNTIME_METADATA_KEY};
use crate::error::ToolError;
use aura_core_types::{
    InstalledToolDefinition, InstalledToolRuntimeIntegration, InstalledToolRuntimeProviderExecution,
};
use reqwest::Method;
use serde_json::{json, Value};

mod bindings;
mod guards;
mod http;
mod integrations;
mod spec;
mod transforms;

use bindings::{build_form_fields_from_bindings, build_object_from_bindings, build_runtime_url};
pub(super) use spec::{
    TrustedIntegrationArgBinding, TrustedIntegrationArgSource, TrustedIntegrationArgValueType,
    TrustedIntegrationGraphqlSpec, TrustedIntegrationHttpMethod, TrustedIntegrationRestSpec,
    TrustedIntegrationRuntimeSpec, TrustedIntegrationSuccessGuard,
};

// ============================================================================
// Dispatch entry points
// ============================================================================

impl ToolResolver {
    /// Spec-driven dispatch: interpret a [`TrustedIntegrationRuntimeSpec`]
    /// (sourced from `trusted_integration_runtime` metadata) and execute
    /// the corresponding HTTP / GraphQL / built-in integration call.
    pub(super) async fn execute_trusted_runtime_app_provider(
        &self,
        provider: &InstalledToolRuntimeProviderExecution,
        integration: &InstalledToolRuntimeIntegration,
        args: &Value,
        spec: &TrustedIntegrationRuntimeSpec,
    ) -> Result<Value, ToolError> {
        match spec {
            TrustedIntegrationRuntimeSpec::RestJson(rest) => {
                self.execute_rest_json(provider, integration, args, rest)
                    .await
            }
            TrustedIntegrationRuntimeSpec::Graphql(graphql) => {
                self.execute_graphql(provider, integration, args, graphql)
                    .await
            }
            TrustedIntegrationRuntimeSpec::RestForm(rest) => {
                self.execute_rest_form(provider, integration, args, rest)
                    .await
            }
            TrustedIntegrationRuntimeSpec::BraveSearch { vertical } => {
                self.brave_search(provider, integration, args, vertical)
                    .await
            }
            TrustedIntegrationRuntimeSpec::ResendSendEmail => {
                self.resend_send_email(provider, integration, args).await
            }
            TrustedIntegrationRuntimeSpec::GmailSendEmail => {
                self.gmail_send_email(provider, integration, args).await
            }
            TrustedIntegrationRuntimeSpec::GmailCreateDraft => {
                self.gmail_create_draft(provider, integration, args).await
            }
            TrustedIntegrationRuntimeSpec::GoogleCalendarCreateEvent => {
                self.google_calendar_create_event(provider, integration, args)
                    .await
            }
            TrustedIntegrationRuntimeSpec::GoogleCalendarUpdateEvent => {
                self.google_calendar_update_event(provider, integration, args)
                    .await
            }
            TrustedIntegrationRuntimeSpec::GoogleCalendarDeleteEvent => {
                self.google_calendar_delete_event(provider, integration, args)
                    .await
            }
        }
    }

    /// The `rest_json` spec: a JSON request built from the bindings.
    async fn execute_rest_json(
        &self,
        provider: &InstalledToolRuntimeProviderExecution,
        integration: &InstalledToolRuntimeIntegration,
        args: &Value,
        rest: &TrustedIntegrationRestSpec,
    ) -> Result<Value, ToolError> {
        let url = build_runtime_url(provider, integration, &rest.path, &rest.query, args)?;
        let body = build_object_from_bindings(&rest.body, args, &integration.provider_config)?;
        let response = self
            .provider_json_request(
                trusted_http_method(&rest.method),
                &url,
                provider,
                integration,
                body,
            )
            .await?;
        guards::apply_success_guard(&response, &rest.success_guard)?;
        transforms::apply_result_transform(&response, &rest.result, args)
    }

    /// The `graphql` spec: a POST of the query and bound variables to the
    /// provider's base URL.
    async fn execute_graphql(
        &self,
        provider: &InstalledToolRuntimeProviderExecution,
        integration: &InstalledToolRuntimeIntegration,
        args: &Value,
        graphql: &TrustedIntegrationGraphqlSpec,
    ) -> Result<Value, ToolError> {
        let variables =
            build_object_from_bindings(&graphql.variables, args, &integration.provider_config)?
                .unwrap_or_else(|| json!({}));
        let response = self
            .provider_json_request(
                Method::POST,
                &provider.base_url,
                provider,
                integration,
                Some(json!({
                    "query": graphql.query,
                    "variables": variables,
                })),
            )
            .await?;
        guards::apply_success_guard(&response, &graphql.success_guard)?;
        transforms::apply_result_transform(&response, &graphql.result, args)
    }

    /// The `rest_form` spec: a form-encoded request built from the
    /// bindings.
    async fn execute_rest_form(
        &self,
        provider: &InstalledToolRuntimeProviderExecution,
        integration: &InstalledToolRuntimeIntegration,
        args: &Value,
        rest: &TrustedIntegrationRestSpec,
    ) -> Result<Value, ToolError> {
        let url = build_runtime_url(provider, integration, &rest.path, &rest.query, args)?;
        let body = build_form_fields_from_bindings(&rest.body, args, &integration.provider_config)?;
        let response = self
            .provider_form_request(
                trusted_http_method(&rest.method),
                &url,
                provider,
                integration,
                body,
            )
            .await?;
        guards::apply_success_guard(&response, &rest.success_guard)?;
        transforms::apply_result_transform(&response, &rest.result, args)
    }

    /// Per-integration dispatch: pick a hard-coded handler by tool name
    /// for the providers Aura ships with built-in support.
    pub(super) async fn execute_runtime_app_provider(
        &self,
        tool: &InstalledToolDefinition,
        args: &Value,
        provider: &InstalledToolRuntimeProviderExecution,
    ) -> Result<Value, ToolError> {
        let integration = select_runtime_integration(provider, args)?;
        match tool.name.as_str() {
            "github_list_repos" => self.github_list_repos(provider, integration).await,
            "github_create_issue" => self.github_create_issue(provider, integration, args).await,
            "linear_list_teams" => self.linear_list_teams(provider, integration).await,
            "linear_create_issue" => self.linear_create_issue(provider, integration, args).await,
            "slack_list_channels" => self.slack_list_channels(provider, integration).await,
            "slack_post_message" => self.slack_post_message(provider, integration, args).await,
            "notion_search_pages" => self.notion_search_pages(provider, integration, args).await,
            "notion_fetch_page" => self.notion_fetch_page(provider, integration, args).await,
            "notion_get_block_children" => {
                self.notion_get_block_children(provider, integration, args)
                    .await
            }
            "notion_append_block_children" => {
                self.notion_append_block_children(provider, integration, args)
                    .await
            }
            "notion_create_page" => self.notion_create_page(provider, integration, args).await,
            "notion_update_page" => self.notion_update_page(provider, integration, args).await,
            "notion_update_page_markdown" => {
                self.notion_update_page_markdown(provider, integration, args)
                    .await
            }
            "notion_query_data_source" => {
                self.notion_query_data_source(provider, integration, args)
                    .await
            }
            "notion_create_database" => {
                self.notion_create_database(provider, integration, args)
                    .await
            }
            "notion_create_data_source" => {
                self.notion_create_data_source(provider, integration, args)
                    .await
            }
            "brave_search_web" => self.brave_search(provider, integration, args, "web").await,
            "brave_search_news" => self.brave_search(provider, integration, args, "news").await,
            "resend_list_domains" => self.resend_list_domains(provider, integration).await,
            "resend_send_email" => self.resend_send_email(provider, integration, args).await,
            other => Err(ToolError::ExternalToolError(format!(
                "runtime execution is not implemented for installed tool `{other}`"
            ))),
        }
    }
}

// ============================================================================
// Shared helpers — runtime spec parsing, integration selection
// ============================================================================

/// Parse the `trusted_integration_runtime` metadata blob (if any) on
/// `tool` into a typed [`TrustedIntegrationRuntimeSpec`]. Returns
/// `Ok(None)` when the tool has no such metadata.
pub(super) fn trusted_runtime_spec(
    tool: &InstalledToolDefinition,
) -> Result<Option<TrustedIntegrationRuntimeSpec>, ToolError> {
    let Some(raw) = tool.metadata.get(TRUSTED_INTEGRATION_RUNTIME_METADATA_KEY) else {
        return Ok(None);
    };
    serde_json::from_value(raw.clone()).map(Some).map_err(|e| {
        ToolError::ExternalToolError(format!(
            "invalid trusted integration runtime metadata for `{}`: {e}",
            tool.name
        ))
    })
}

/// Resolve an `integration_id` from `args` (or default to the first) to
/// pick which credential set the dispatch should use.
pub(super) fn select_runtime_integration<'a>(
    provider: &'a InstalledToolRuntimeProviderExecution,
    args: &Value,
) -> Result<&'a InstalledToolRuntimeIntegration, ToolError> {
    let requested = optional_string(args, &["integration_id", "integrationId"]);
    if let Some(requested) = requested {
        return provider
            .integrations
            .iter()
            .find(|integration| integration.integration_id == requested)
            .ok_or_else(|| {
                ToolError::ExternalToolError(format!(
                    "requested integration `{requested}` is not installed for runtime execution"
                ))
            });
    }
    provider.integrations.first().ok_or_else(|| {
        ToolError::ExternalToolError("no runtime integration credentials are available".into())
    })
}

pub(super) fn trusted_http_method(method: &TrustedIntegrationHttpMethod) -> Method {
    match method {
        TrustedIntegrationHttpMethod::Get => Method::GET,
        TrustedIntegrationHttpMethod::Post => Method::POST,
    }
}
