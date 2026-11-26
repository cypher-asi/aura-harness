//! Regression tests for `agent_loop_stream_timeout`.

use super::super::state::agent_loop_stream_timeout;

/// Serializes the env-mutating tests below. Rust runs unit tests in
/// parallel by default and `std::env::{set_var, remove_var}` are
/// process-global, so concurrent mutation would race.
static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// RAII guard that snapshots the current value of `AURA_MODEL_TIMEOUT_MS`
/// on construction and restores it on drop. Keeps the global env clean
/// across test runs even if an assertion panics.
struct EnvVarGuard {
    key: &'static str,
    prev: Option<String>,
}

impl EnvVarGuard {
    fn capture(key: &'static str) -> Self {
        Self {
            key,
            prev: std::env::var(key).ok(),
        }
    }
}

impl Drop for EnvVarGuard {
    fn drop(&mut self) {
        match &self.prev {
            Some(value) => std::env::set_var(self.key, value),
            None => std::env::remove_var(self.key),
        }
    }
}

#[test]
fn agent_loop_stream_timeout_defaults_when_env_unset() {
    let _lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _guard = EnvVarGuard::capture("AURA_MODEL_TIMEOUT_MS");
    std::env::remove_var("AURA_MODEL_TIMEOUT_MS");

    // Default reasoner timeout (300s) + 30s margin = 330s.
    assert_eq!(
        agent_loop_stream_timeout(),
        std::time::Duration::from_secs(330),
    );
}

#[test]
fn agent_loop_stream_timeout_reads_env_in_milliseconds_with_margin() {
    let _lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _guard = EnvVarGuard::capture("AURA_MODEL_TIMEOUT_MS");

    std::env::set_var("AURA_MODEL_TIMEOUT_MS", "600000");
    assert_eq!(
        agent_loop_stream_timeout(),
        std::time::Duration::from_secs(630),
    );

    std::env::set_var("AURA_MODEL_TIMEOUT_MS", "  120000  ");
    assert_eq!(
        agent_loop_stream_timeout(),
        std::time::Duration::from_secs(150),
    );
}

#[test]
fn agent_loop_stream_timeout_falls_back_for_invalid_or_zero_values() {
    let _lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _guard = EnvVarGuard::capture("AURA_MODEL_TIMEOUT_MS");

    std::env::set_var("AURA_MODEL_TIMEOUT_MS", "");
    assert_eq!(
        agent_loop_stream_timeout(),
        std::time::Duration::from_secs(330),
    );

    std::env::set_var("AURA_MODEL_TIMEOUT_MS", "not-a-number");
    assert_eq!(
        agent_loop_stream_timeout(),
        std::time::Duration::from_secs(330),
    );

    std::env::set_var("AURA_MODEL_TIMEOUT_MS", "0");
    assert_eq!(
        agent_loop_stream_timeout(),
        std::time::Duration::from_secs(330),
    );
}
