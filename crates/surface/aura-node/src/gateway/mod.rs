//! HTTP/WS gateway for the `aura-node` binary.
//!
//! Phase C / Commit 4 renamed the former `router/` module to
//! `gateway/` and grouped per-endpoint handlers under `handlers/`.
//! Layout:
//!
//! - [`state`] — [`RouterState`] + [`RouterStateConfig`] threaded
//!   through every handler.
//! - [`middleware`] — `create_router`, the middleware-stack assembly
//!   (CORS / governor / body-limit / connect-info / observer /
//!   timeout / trace), the terminal-upgrade delegate, and the
//!   `/health` handler. Split out of the old `router/build.rs`.
//! - [`handlers`] — per-endpoint handler bundles:
//!   [`handlers::run`] (`POST /v1/run`, `/v1/run/list`,
//!   `/v1/run/:id/{status,pause,stop}`), [`handlers::files`],
//!   [`handlers::tx`], [`handlers::memory`], [`handlers::skills`],
//!   [`handlers::tool_permissions`], [`handlers::run_ws`]
//!   (`WS /stream/:run_id`).
//! - [`session`] — per-WebSocket-connection state: chat-session
//!   bootstrap, tool-approval broker, partial-JSON repair.
//! - [`auth_mw`] — bearer-token enforcing axum middleware shared
//!   across protected routes.
//! - [`errors`] — `ApiError`, the canonical JSON failure shape.

pub(crate) mod auth_mw;
pub(crate) mod errors;
pub(crate) mod handlers;
pub(crate) mod middleware;
pub(crate) mod session;
pub(crate) mod state;
mod terminal_ws;

#[cfg(test)]
mod tests;

pub use handlers::cloud::HUB_SKILLS_DIR;
pub use middleware::create_router;
pub use state::{RouterState, RouterStateConfig};
