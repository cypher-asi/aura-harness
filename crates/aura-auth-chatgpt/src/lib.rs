//! # aura-auth-chatgpt
//!
//! Layer: surface
//!
//! Personal ChatGPT plan access via OpenAI's "Sign in with ChatGPT"
//! token-sharing contract: PKCE authorization against
//! `auth.openai.com` with a loopback callback, dynamic client
//! registration (`dynamic_agent_client` + `ext_agent_host_id`), id_token
//! verification against the issuer's JWKS, and rotating refresh.
//!
//! Mirrors `aura-chatgpt` in the AURA app, minus its database: the
//! caller persists [`aura_core_auth::LinkedAccount`] and tokens.

#![forbid(unsafe_code)]
#![warn(clippy::all)]

mod host;
mod identity;
mod provider;

pub use host::load_or_create_host_id;
pub use provider::ChatgptProvider;

/// Production authority.
pub const AUTH_BASE: &str = "https://auth.openai.com";
/// Resource the tokens are minted for; also the inference API base.
pub const RESOURCE: &str = "https://api.openai.com/v1";
/// Scopes requested at sign-in.
pub const SCOPE: &str =
    "openid profile email offline_access resource.invoke chatgpt.tokens.use.direct";
/// Scope that grants plan inference usage.
pub const PLAN_SCOPE: &str = "chatgpt.tokens.use.direct";
/// Placeholder client id that asks OpenAI to issue a per-install client.
pub const DYNAMIC_CLIENT: &str = "dynamic_agent_client";
/// Where users can check plan usage.
pub const USAGE_URL: &str = "https://chatgpt.com/settings/usage";
