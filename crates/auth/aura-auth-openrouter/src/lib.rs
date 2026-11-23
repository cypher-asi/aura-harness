//! # aura-auth-openrouter
//!
//! Layer: auth
//!
//! OpenRouter account access via its public OAuth PKCE flow
//! (<https://openrouter.ai/docs/guides/overview/auth/oauth>): the browser
//! sign-in returns a code that is exchanged for a user-owned API key,
//! billed to the user's OpenRouter credits. Users may also paste an
//! existing `sk-or-` key, which is checked against `GET /api/v1/key`.
//!
//! The key is stored like plan tokens: as the access token, with no
//! refresh token and an expiry far in the future. Every endpoint and
//! header lives in this crate so it can be updated in one place.

#![forbid(unsafe_code)]
#![warn(clippy::all)]

mod provider;

pub use provider::OpenRouterProvider;

/// Constant client id recorded on links; OpenRouter has no client registration.
pub const CLIENT_ID: &str = "openrouter";
/// Environment variable that overrides [`API_BASE`] (shared with inference).
pub const API_URL_ENV: &str = "AURA_OPENROUTER_URL";
/// API base; endpoints below are `{API_BASE}/v1/...`.
pub const API_BASE: &str = "https://openrouter.ai/api";
/// Browser sign-in page.
pub const AUTHORIZE_URL: &str = "https://openrouter.ai/auth";
/// Exchanges an authorization code for an API key.
pub const KEYS_URL: &str = "https://openrouter.ai/api/v1/auth/keys";
/// Reports label, limits and tier of the key in the `Authorization` header.
pub const KEY_INFO_URL: &str = "https://openrouter.ai/api/v1/key";
/// Sentinel redirect URI for headless sign-in. OpenRouter has no hosted
/// code page URI: when an attempt uses this value, the authorization URL
/// omits `callback_url` and `state`, and the page shows a code to paste.
pub const MANUAL_REDIRECT: &str = "urn:aura:openrouter:paste";
/// Name given to keys created by sign-in, shown in OpenRouter's key list.
pub const KEY_LABEL: &str = "Aura";
/// Prefix of every OpenRouter API key.
pub const KEY_PREFIX: &str = "sk-or-";
/// App attribution sent as `HTTP-Referer` on inference requests.
pub const APP_REFERER: &str = "https://github.com/cypher-asi/aura-harness";
/// App name sent as `X-Title` on inference requests.
pub const APP_TITLE: &str = "Aura";
