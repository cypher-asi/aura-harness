//! End-to-end integration tests against a live aura-node server.
//!
//! These tests boot a real node, connect via HTTP and WebSocket, and exercise
//! health, REST, WebSocket session lifecycle, and LLM/tool flows against live
//! services.
//!
//! Run all e2e tests:
//! ```text
//! cargo test --test it e2e_live::
//! ```
//!
//! Tests that require credentials gracefully skip (pass with a SKIP note)
//! when the relevant env vars are missing.
//!
//! Required environment (via `.env` or exported):
//! - `AURA_ROUTER_URL` (defaulted) and a JWT via `AURA_ROUTER_JWT` or `aura login`.
//!
//! Phase A note: every chat session is bootstrapped via the canonical
//! `POST /v1/run` + `WS /stream/:run_id` two-step exchange. The old
//! `WS /stream` + `InboundMessage::SessionInit` first-frame contract
//! is gone; tests that exercised it (`test_ws_session_init_twice_errors`,
//! `test_ws_user_message_before_init`) have been deleted.

// Local wrapper around `common::require_llm_token` that first gates on
// `AURA_RUN_LIVE_E2E`. The inner call used to be `$crate::require_llm!()`,
// which resolved back to *this* macro (the local definition shadows the
// `#[macro_export]` sibling in `tests/it/common/mod.rs`) — triggering
// unbounded recursion on the first real run. Calling the function
// directly sidesteps the macro-resolution ambiguity.
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

mod rest;
mod session_config;
mod tool_errors;
mod tools;
mod ws_turns;
