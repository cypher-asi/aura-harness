//! Comprehensive E2E integration tests for Aura Swarm.
//!
//! This file exercises every conceivable end-user use case and tool from
//! the outside, connecting via HTTP and WebSocket exactly as a real client
//! would.  Tests are organised into suites:
//!
//!   1. REST API data-flow
//!   2. POST `/v1/run` validation (Phase A wire shape)
//!   3. WebSocket session configuration (mock provider)
//!   4. WebSocket protocol edge-cases
//!   5. ZOS login + JWT authentication  (requires `E2E_ZOS_EMAIL` / `E2E_ZOS_PASSWORD`)
//!   6. LLM tool coverage               (requires LLM credentials)
//!   7. Streaming protocol fidelity      (requires LLM credentials)
//!   8. Concurrency & stress
//!
//! Phase A note: every chat run is bootstrapped via the canonical
//! `POST /v1/run` + `WS /stream/:run_id` two-step exchange. The
//! legacy `InboundMessage::SessionInit` first-frame contract no
//! longer exists; tests that exercised it (double-init, malformed
//! session_init, JWT-via-session-init-token) have been deleted.
//!
//! Run everything:
//! ```text
//! cargo test --test it e2e_full::
//! ```
//!
//! Run only suites that need no credentials:
//! ```text
//! cargo test --test it e2e_full::rest::
//! cargo test --test it e2e_full::protocol::run_validation_
//! cargo test --test it e2e_full::ws_cfg::
//! cargo test --test it e2e_full::protocol::ws_proto_
//! cargo test --test it e2e_full::stress::
//! ```

// Local wrapper around `common::require_llm_token` that first gates on
// `AURA_RUN_LIVE_E2E`. Mirrors the shadowing macro in `tests/it/e2e_live/mod.rs`
// so the live-LLM suites share one opt-in switch: without
// `AURA_RUN_LIVE_E2E=1` every test that needs a reachable model + valid
// JWT skips (returns early) instead of failing against an unreachable or
// unauthenticated router. Calling the function directly (rather than the
// `#[macro_export]` sibling) sidesteps macro-resolution recursion.
macro_rules! require_llm {
    () => {{
        let enabled = std::env::var("AURA_RUN_LIVE_E2E")
            .map(|value| matches!(value.as_str(), "1" | "true" | "TRUE" | "yes" | "YES"))
            .unwrap_or(false);
        if !enabled {
            eprintln!("skipping live LLM test: set AURA_RUN_LIVE_E2E=1 to enable it");
            return;
        }
        $crate::common::require_llm_token()
    }};
}

#[cfg(test)]
mod jwt;
#[cfg(test)]
mod protocol;
#[cfg(test)]
mod rest;
#[cfg(test)]
mod stream;
#[cfg(test)]
mod stress;
#[cfg(test)]
mod support;
#[cfg(test)]
mod tools;
#[cfg(test)]
mod tools_more;
#[cfg(test)]
mod ws_cfg;
