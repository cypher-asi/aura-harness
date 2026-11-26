//! # aura-auth-kit
//!
//! Layer: auth
//!
//! Shared plumbing for the per-provider auth crates: PKCE attempt
//! generation, the ephemeral loopback callback listener, pasted-code
//! parsing, a hardened HTTP client, error redaction, and the
//! constant-time bearer-token check shared by the HTTP servers.
//! Contains no provider-specific endpoints.

#![forbid(unsafe_code)]

pub mod bearer;
mod browser;
pub mod http;
mod loopback;
mod pkce;
mod plan;
mod redact;

pub use browser::open_browser;
pub use loopback::{LoopbackListener, CALLBACK_TIMEOUT};
pub use pkce::{new_attempt, parse_pasted_code, random_token};
pub use plan::plan_label;
pub use redact::redact_error;
