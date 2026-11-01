//! # aura-auth-claude
//!
//! Layer: surface
//!
//! Claude Pro/Max plan access via the OAuth client used by Claude Code:
//! PKCE authorization on `claude.ai`, a `localhost` loopback callback
//! with a paste-code fallback, JSON token exchange and refresh, and
//! `Bearer` + `anthropic-beta: oauth-2025-04-20` on inference requests.
//!
//! Anthropic does not publish this flow for third-party clients; its
//! terms may restrict subscription use outside Claude Code. Every
//! endpoint and header lives in this file so it can be updated in one
//! place.

#![forbid(unsafe_code)]
#![warn(clippy::all)]

mod provider;

pub use provider::ClaudeProvider;

/// Public OAuth client id used by Claude Code.
pub const CLIENT_ID: &str = "9d1c250a-e61b-44d9-88ed-5944d1962f5e";
/// Authorization endpoint.
pub const AUTHORIZE_URL: &str = "https://claude.ai/oauth/authorize";
/// Token endpoint (code exchange and refresh).
pub const TOKEN_URL: &str = "https://console.anthropic.com/v1/oauth/token";
/// Hosted page that shows `code#state` for manual paste.
pub const MANUAL_REDIRECT: &str = "https://console.anthropic.com/oauth/code/callback";
/// Scopes requested at sign-in.
pub const SCOPE: &str = "user:profile user:inference";
/// Scope that grants plan inference usage.
pub const PLAN_SCOPE: &str = "user:inference";
/// Beta flag that enables OAuth bearer auth on the Messages API.
pub const OAUTH_BETA: &str = "oauth-2025-04-20";
/// Messages API base for subscription inference.
pub const API_BASE: &str = "https://api.anthropic.com";
/// System prompt opening required for subscription tokens.
pub const SYSTEM_PREAMBLE: &str = "You are Claude Code, Anthropic's official CLI for Claude.";
/// Shown before sign-in so users accept the terms risk knowingly.
pub const TERMS_NOTICE: &str = "Claude plan access uses the sign-in flow built for Claude Code. \
Anthropic's terms may not allow plan use in other apps, and the flow can change without notice.";
