//! Shared E2E test infrastructure.
//!
//! Provides `TestServer`, `WsClient`, and helper utilities used by both
//! `e2e_live` and `e2e_full` integration test suites.
//!
//! Phase A wire-shape migration: every chat session is now bootstrapped
//! via `POST /v1/run` + `WS /stream/:run_id`. The helpers in this module
//! build [`aura_core_protocol::RuntimeRequest`]-shaped JSON payloads, post
//! them to the gateway, and attach a [`WsClient`] to the freshly minted
//! run id — collapsing the previous "open WS + send init frame + wait
//! SessionReady" three-step dance into a single call.
//!
//! Submodules by concern (everything is re-exported here so `common::X`
//! paths stay stable):
//!
//! - `auth`: credentials, test bearer, HTTP client
//! - `server`: in-process `TestServer`
//! - `request`: `RuntimeRequest` builders and run bootstrap
//! - `ws`: `WsClient`
//! - `helpers`: message-stream assertions and filesystem helpers

// Shared test helpers: test code may unwrap/expect.
#![allow(clippy::unwrap_used, clippy::expect_used)]
#![allow(dead_code)]

mod auth;
mod helpers;
mod request;
mod server;
mod ws;

pub use auth::*;
pub use helpers::*;
pub use request::*;
pub use server::*;
pub use ws::*;

#[macro_export]
macro_rules! require_llm {
    () => {
        $crate::common::require_llm_token()
    };
}

#[macro_export]
macro_rules! require_zos {
    () => {
        match $crate::common::optional_zos_credentials() {
            Some(credentials) => credentials,
            None => {
                eprintln!(
                    "skipping credentialed test: set E2E_ZOS_EMAIL and E2E_ZOS_PASSWORD to run it"
                );
                return;
            }
        }
    };
}
