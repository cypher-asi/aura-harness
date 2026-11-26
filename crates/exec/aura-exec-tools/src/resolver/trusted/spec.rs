//! The `trusted_integration_runtime` metadata schema: the typed form of
//! the JSON blob a trusted installed tool carries to describe its HTTP,
//! GraphQL or built-in integration call.

use serde::Deserialize;
use serde_json::Value;

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(in crate::resolver) enum TrustedIntegrationHttpMethod {
    Get,
    Post,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(in crate::resolver) enum TrustedIntegrationArgValueType {
    String,
    StringList,
    PositiveNumber,
    Boolean,
    Json,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub(in crate::resolver) enum TrustedIntegrationArgSource {
    #[default]
    InputArgs,
    ProviderConfig,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::resolver) struct TrustedIntegrationArgBinding {
    pub(in crate::resolver) arg_names: Vec<String>,
    pub(in crate::resolver) target: String,
    #[serde(default)]
    pub(in crate::resolver) source: TrustedIntegrationArgSource,
    pub(in crate::resolver) value_type: TrustedIntegrationArgValueType,
    #[serde(default)]
    pub(in crate::resolver) required: bool,
    #[serde(default)]
    pub(in crate::resolver) default_value: Option<Value>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(in crate::resolver) enum TrustedIntegrationSuccessGuard {
    #[default]
    None,
    SlackOk,
    GraphqlErrors,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::resolver) struct TrustedIntegrationResultField {
    pub(in crate::resolver) output: String,
    pub(in crate::resolver) pointer: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::resolver) struct TrustedIntegrationResultExtraField {
    pub(in crate::resolver) output: String,
    pub(in crate::resolver) pointer: String,
    #[serde(default)]
    pub(in crate::resolver) default_value: Option<Value>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(in crate::resolver) enum TrustedIntegrationResultTransform {
    WrapPointer {
        key: String,
        pointer: String,
    },
    ProjectArray {
        key: String,
        #[serde(default)]
        pointer: Option<String>,
        fields: Vec<TrustedIntegrationResultField>,
        #[serde(default)]
        extras: Vec<TrustedIntegrationResultExtraField>,
    },
    ProjectObject {
        key: String,
        #[serde(default)]
        pointer: Option<String>,
        fields: Vec<TrustedIntegrationResultField>,
    },
    BraveSearch {
        vertical: String,
    },
}

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(in crate::resolver) enum TrustedIntegrationRuntimeSpec {
    RestJson(TrustedIntegrationRestSpec),
    RestForm(TrustedIntegrationRestSpec),
    Graphql(TrustedIntegrationGraphqlSpec),
    BraveSearch { vertical: String },
    ResendSendEmail,
    GmailSendEmail,
    GmailCreateDraft,
    GoogleCalendarCreateEvent,
    GoogleCalendarUpdateEvent,
    GoogleCalendarDeleteEvent,
}

/// The fields of the `rest_json` and `rest_form` runtime specs.
#[derive(Debug, Clone, Deserialize)]
pub(in crate::resolver) struct TrustedIntegrationRestSpec {
    pub(in crate::resolver) method: TrustedIntegrationHttpMethod,
    pub(in crate::resolver) path: String,
    #[serde(default)]
    pub(in crate::resolver) query: Vec<TrustedIntegrationArgBinding>,
    #[serde(default)]
    pub(in crate::resolver) body: Vec<TrustedIntegrationArgBinding>,
    #[serde(default)]
    pub(in crate::resolver) success_guard: TrustedIntegrationSuccessGuard,
    pub(in crate::resolver) result: TrustedIntegrationResultTransform,
}

/// The fields of the `graphql` runtime spec.
#[derive(Debug, Clone, Deserialize)]
pub(in crate::resolver) struct TrustedIntegrationGraphqlSpec {
    pub(in crate::resolver) query: String,
    #[serde(default)]
    pub(in crate::resolver) variables: Vec<TrustedIntegrationArgBinding>,
    #[serde(default)]
    pub(in crate::resolver) success_guard: TrustedIntegrationSuccessGuard,
    pub(in crate::resolver) result: TrustedIntegrationResultTransform,
}
