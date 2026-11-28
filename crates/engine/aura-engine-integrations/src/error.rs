//! What can go wrong with an integration. No message carries a
//! credential, and a service's response body only through its own short
//! error message ([`crate::IntegrationContext::send`]).

use crate::id::IntegrationId;
use std::time::Duration;
use thiserror::Error;

/// Why an integration operation failed.
#[derive(Debug, Error)]
pub enum IntegrationError {
    /// Not a valid integration id.
    #[error(
        "{0:?} is not a valid integration id (a-z, 0-9 and -, up to 32, starting with a letter)"
    )]
    InvalidId(String),
    /// Another integration has this id.
    #[error("an integration with id {0} is already registered")]
    Duplicate(IntegrationId),
    /// No integration has this id.
    #[error("no integration {0}")]
    Unknown(String),
    /// It has no credential, or isn't signed in.
    #[error("{id} is not connected: {hint}")]
    NotConnected {
        /// The integration
        id: IntegrationId,
        /// How to connect it
        hint: String,
    },
    /// A call ran past its time.
    #[error("{id} did not answer within {}s", after.as_secs())]
    Timeout {
        /// The integration
        id: IntegrationId,
        /// The limit it ran past
        after: Duration,
    },
    /// The service answered with an error status.
    #[error("{id} answered {status}: {message}")]
    Http {
        /// The integration
        id: IntegrationId,
        /// HTTP status
        status: u16,
        /// The service's own error message, cut short
        message: String,
    },
    /// The service couldn't be reached.
    #[error("{id} could not be reached: {message}")]
    Network {
        /// The integration
        id: IntegrationId,
        /// What failed, without the URL
        message: String,
    },
    /// The integration has no such tool.
    #[error("{id} has no tool {tool}")]
    UnknownTool {
        /// The integration
        id: IntegrationId,
        /// The tool asked for
        tool: String,
    },
    /// A tool's arguments are wrong.
    #[error("{id} {tool}: {message}")]
    BadArgs {
        /// The integration
        id: IntegrationId,
        /// The tool
        tool: String,
        /// What is wrong
        message: String,
    },
    /// A config value is wrong, or the integration can't do what was
    /// asked (connect with a key when it signs in another way).
    #[error("{id} {key}: {message}")]
    BadConfig {
        /// The integration
        id: IntegrationId,
        /// The config key or action
        key: String,
        /// What is wrong
        message: String,
    },
    /// This agent or channel may not use the integration.
    #[error("{id} is not available to this agent")]
    Denied {
        /// The integration
        id: IntegrationId,
    },
    /// The credential or settings store failed.
    #[error("integration store: {0}")]
    Store(String),
    /// Anything else the integration reports.
    #[error("{id}: {message}")]
    Other {
        /// The integration
        id: IntegrationId,
        /// What happened
        message: String,
    },
}
