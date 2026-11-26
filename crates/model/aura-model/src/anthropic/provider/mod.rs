mod body_cap;
mod client;
mod diagnostics;
mod errors;
mod model_provider;
mod request;
mod retry;
mod throttle;
mod waf;

pub use retry::exp_backoff_with_jitter;

use crate::ModelRequest;

/// Saturating cast from a `Duration::as_millis()` result (`u128`) into
/// the `u64` shape every observability frame in this crate emits.
/// Wall-clock elapsed values are bounded by request timeouts, so the
/// cast is bounds-safe; the saturate keeps the function infallible
/// without forcing every call site to repeat the rationale.
#[allow(clippy::cast_possible_truncation)] // saturate-on-overflow above; rationale on the function.
const fn millis_as_u64(elapsed: u128) -> u64 {
    if elapsed > u64::MAX as u128 {
        u64::MAX
    } else {
        elapsed as u64
    }
}

#[derive(Debug, Clone, Copy)]
struct RequestRoutingContext {
    has_aura_project_id: bool,
    has_aura_agent_id: bool,
    has_aura_org_id: bool,
    has_aura_session_id: bool,
}

impl RequestRoutingContext {
    fn from_request(request: &ModelRequest) -> Self {
        Self {
            has_aura_project_id: request
                .aura_project_id
                .as_deref()
                .map(str::trim)
                .is_some_and(|value| !value.is_empty()),
            has_aura_agent_id: request
                .aura_agent_id
                .as_deref()
                .map(str::trim)
                .is_some_and(|value| !value.is_empty()),
            has_aura_org_id: request
                .aura_org_id
                .as_deref()
                .map(str::trim)
                .is_some_and(|value| !value.is_empty()),
            has_aura_session_id: request
                .aura_session_id
                .as_deref()
                .map(str::trim)
                .is_some_and(|value| !value.is_empty()),
        }
    }

    fn project_label(self) -> &'static str {
        if self.has_aura_project_id {
            "present"
        } else {
            "missing"
        }
    }

    fn agent_label(self) -> &'static str {
        if self.has_aura_agent_id {
            "present"
        } else {
            "missing"
        }
    }

    fn org_label(self) -> &'static str {
        if self.has_aura_org_id {
            "present"
        } else {
            "missing"
        }
    }

    fn session_label(self) -> &'static str {
        if self.has_aura_session_id {
            "present"
        } else {
            "missing"
        }
    }
}

#[cfg(test)]
mod emergency_body_cap_tests;
#[cfg(test)]
mod openrouter_slug_tests;
#[cfg(test)]
mod request_diagnostics_tests;
#[cfg(test)]
mod retry_tests;
