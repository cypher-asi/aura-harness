//! # aura-auth-kit
//!
//! Layer: surface
//!
//! Shared plumbing for the per-provider auth crates: PKCE attempt
//! generation, the ephemeral loopback callback listener, pasted-code
//! parsing, a hardened HTTP client, and error redaction. Contains no
//! provider-specific endpoints.

#![forbid(unsafe_code)]
#![warn(clippy::all)]

mod browser;
pub mod http;
mod loopback;
mod pkce;
mod redact;

pub use browser::open_browser;
pub use loopback::{LoopbackListener, CALLBACK_TIMEOUT};
pub use pkce::{new_attempt, parse_pasted_code, random_token};
pub use redact::redact_error;
