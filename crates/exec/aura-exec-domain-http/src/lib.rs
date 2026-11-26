//! # aura-exec-domain-http
//!
//! Layer: exec
//!
//! HTTP-backed [`aura_exec_traits::domain::DomainApi`] implementation
//! plus the JWT-injecting wrapper the automaton bridge uses to stamp
//! a captured token onto every call site that did not supply one.
//!
//! Phase C / Commit 4 moves these two files out of `aura-node` so
//! the gateway crate no longer owns domain HTTP logic or depends on
//! `reqwest` for domain calls. Both crates sit at the surface layer:
//! this crate composes lower-layer types
//! ([`aura_exec_traits::domain::DomainApi`]) into a deployable HTTP
//! edge, and the gateway composes both this crate and the engine into
//! a runnable HTTP/WS server. `aura-node` still has separate direct
//! outbound HTTP surfaces (for example the generation proxy and
//! cross-agent callback path).
//!
//! ## Surface
//!
//! - [`HttpDomainApi`] — the `reqwest`-backed `DomainApi` impl with
//!   Cloudflare-block retry handling, `aura-os-server` base-URL
//!   override routing, and JWT-bearer authentication.
//! - [`JwtDomainApi`] — wraps another `DomainApi` and injects a
//!   captured JWT on every call site that did not supply one.
//!
//! ## Error handling
//!
//! Both impls return the trait's typed
//! [`aura_exec_traits::domain::DomainError`]: a missing JWT, a request
//! that got no response, an unreadable body, a non-success status (with
//! the start of the body), a Cloudflare block that outlasted the
//! retries, or a response that is not the expected JSON. Each message
//! names the request, as the domain tools show the message alone; the
//! `reqwest` / `serde_json` error stays the `source()`.
//! [`HttpDomainApi::new`] fails with [`HttpClientError`].

#![forbid(unsafe_code)]

mod http;
mod jwt;

pub use http::{HttpClientError, HttpDomainApi};
pub use jwt::JwtDomainApi;
