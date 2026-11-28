//! # aura-engine-integrations
//!
//! Layer: engine
//!
//! One standard way to plug an external service (GitHub, boxd, RunPod,
//! Vercel, ...) into aura. An integration is a [`Manifest`] (id, name,
//! icon, what it provides, how it signs in, its config fields and the
//! tools it gives agents) and an [`Integration`] (a quick local
//! [`Integration::status`], a [`Integration::health`] check that makes
//! one real call, and [`Integration::call_tool`]). The [`Registry`]
//! holds every integration and does the rest the same way for all of
//! them: connect, configure, test, disconnect, who may use it, the
//! panel's [`IntegrationView`]s, and the agent tools
//! ([`IntegrationTool`]).
//!
//! See `docs/integrations.md` for the design and "How to add an
//! integration".
//!
//! ## Invariants
//!
//! - A credential is only ever stored sealed ([`SealedSecrets`]: one
//!   AES-256-GCM file under the local store's key) or in memory
//!   ([`MemorySecrets`], for tests). [`Secret`]'s `Debug` and `Display`
//!   never show it, and no error carries it.
//! - [`Registry::connect`] stores a credential only after
//!   [`Integration::health`] passed with it.
//! - Every status, health check and tool call runs within a timeout
//!   ([`IntegrationContext::timeout`]); running past it is
//!   [`IntegrationError::Timeout`].
//! - An agent gets an integration's tools only while it is connected and
//!   its [`Access`] allows that agent or its channel; each call checks
//!   both again.
//! - A credential never goes to a cloud machine unless the manifest
//!   says so ([`CloudDelivery::HostBound`]), and then only as boxd's
//!   placeholder.

mod context;
mod error;
mod id;
mod integration;
mod manifest;
mod registry;
mod secret;
mod settings;
mod tools;

pub use context::{IntegrationContext, SecretSource};
pub use error::IntegrationError;
pub use id::IntegrationId;
pub use integration::{default_status, HealthReport, Integration, Status};
pub use manifest::{
    AuthKind, CloudDelivery, ConfigField, FieldKind, KeyAuth, Manifest, ToolEffect, ToolSpec,
};
pub use registry::{IntegrationView, Registry, DEFAULT_TIMEOUT};
pub use secret::{MemorySecrets, SealedSecrets, Secret, SecretStore, SECRETS_FILE};
pub use settings::{Access, SETTINGS_FILE};
pub use tools::{tool_name, IntegrationTool};
