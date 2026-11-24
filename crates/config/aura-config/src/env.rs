//! Centralized env-var names and parsing primitives.
//!
//! This module owns every env-var **name constant** that
//! `aura-config` parses. The boundary test `tests/config_boundary.rs`
//! at the workspace root reads [`ENV_VAR_NAMES`] and asserts no crate
//! other than `aura-config` calls `std::env::var(...)` on any of these
//! names. Tests that need to mutate one of these vars must do so via
//! [`crate::install_for_test`] instead.

use std::env::VarError;
use std::str::FromStr;

use thiserror::Error;

// ---------------------------------------------------------------------------
// Owned env-var names.
//
// These constants are the SINGLE source of truth for the textual env-var
// names. Add a new owned var by appending it here AND including it in
// `ENV_VAR_NAMES`.
// ---------------------------------------------------------------------------

pub const AURA_AGENT_DISABLE_COMPACTION: &str = "AURA_AGENT_DISABLE_COMPACTION";
pub const AURA_AGENT_IMPLEMENT_NOW: &str = "AURA_AGENT_IMPLEMENT_NOW";
pub const AURA_AGENT_IMPLEMENT_NOW_THRESHOLD: &str = "AURA_AGENT_IMPLEMENT_NOW_THRESHOLD";
pub const AURA_AGENT_IMPLEMENT_NOW_BLOCK: &str = "AURA_AGENT_IMPLEMENT_NOW_BLOCK";
pub const AURA_AGENT_BOOTSTRAP_SPEC_BYTES: &str = "AURA_AGENT_BOOTSTRAP_SPEC_BYTES";
pub const AURA_AGENT_BOOTSTRAP_STRIP_CODE_FENCES: &str = "AURA_AGENT_BOOTSTRAP_STRIP_CODE_FENCES";
pub const AURA_AGENT_BOOTSTRAP_CONTEXT_CHARS: &str = "AURA_AGENT_BOOTSTRAP_CONTEXT_CHARS";
pub const AURA_TURN_TOOL_HEARTBEAT_INTERVAL_SECS: &str = "AURA_TURN_TOOL_HEARTBEAT_INTERVAL_SECS";
pub const AURA_DOD_TEST_COMMAND: &str = "AURA_DOD_TEST_COMMAND";
pub const AURA_SIMPLE_MODEL: &str = "AURA_SIMPLE_MODEL";
pub const AURA_LLM_MAX_RETRIES: &str = "AURA_LLM_MAX_RETRIES";
pub const AURA_LLM_BACKOFF_INITIAL_MS: &str = "AURA_LLM_BACKOFF_INITIAL_MS";
pub const AURA_LLM_BACKOFF_CAP_MS: &str = "AURA_LLM_BACKOFF_CAP_MS";
pub const AURA_DEV_LOOP_ENABLED_THINKING: &str = "AURA_DEV_LOOP_ENABLED_THINKING";

/// Mode for sessions that don't request one. Wire format is the
/// snake-case [`aura_core_modes::AgentMode`] serde representation —
/// `agent|plan|ask|debug`. Consumed by [`crate::AgentConfig::default_mode`].
pub const AURA_DEFAULT_MODE: &str = "AURA_DEFAULT_MODE";
/// Older name for [`AURA_DEFAULT_MODE`], read when the new one is unset.
pub const AURA_FLEET_DEFAULT_MODE: &str = "AURA_FLEET_DEFAULT_MODE";
pub const AURA_SUBAGENT_MAX_DEPTH: &str = "AURA_SUBAGENT_MAX_DEPTH";
pub const AURA_SUBAGENT_MAX_RUNNING: &str = "AURA_SUBAGENT_MAX_RUNNING";
pub const AURA_SUBAGENT_DEFAULT_MAX_ITERATIONS: &str = "AURA_SUBAGENT_DEFAULT_MAX_ITERATIONS";
pub const AURA_SUBAGENT_DEFAULT_TIMEOUT_MS: &str = "AURA_SUBAGENT_DEFAULT_TIMEOUT_MS";
pub const AURA_CONFLICT_ENABLED: &str = "AURA_CONFLICT_ENABLED";
pub const AURA_CONFLICT_DEFAULT_WAIT_MS: &str = "AURA_CONFLICT_DEFAULT_WAIT_MS";

// Phase 4a — AURA_HOME resolution. `CODEX_HOME` is a read-only compat
// alias used only by `crate::aura_home::AuraHome::resolve`; it is
// listed here so the boundary test enforces that no other crate reads
// it directly either.
pub const AURA_HOME: &str = "AURA_HOME";
pub const CODEX_HOME: &str = "CODEX_HOME";

/// Run-trace capture level: `off`, `decisions` (default) or `full`.
/// Consumed by [`crate::TraceConfig::level`].
pub const AURA_TRACE: &str = "AURA_TRACE";

/// Success rate (0..=1) a model needs on a category to become Gate's
/// pick. Consumed by [`crate::GateConfig::target`].
pub const AURA_GATE_TARGET: &str = "AURA_GATE_TARGET";
/// Counted outcomes before Gate trusts a model's success rate. Consumed
/// by [`crate::GateConfig::min_tries`].
pub const AURA_GATE_MIN_TRIES: &str = "AURA_GATE_MIN_TRIES";
/// Share (0..=1) of Gate's routes sent to the next cheaper model.
/// Consumed by [`crate::GateConfig::explore_rate`].
pub const AURA_GATE_EXPLORE: &str = "AURA_GATE_EXPLORE";
/// `off` lets the lead agent under Gate edit files and run any command
/// itself; delegating is then only recommended. Consumed by
/// [`crate::GateConfig::delegate`].
pub const AURA_GATE_DELEGATE: &str = "AURA_GATE_DELEGATE";
/// `off` stops Gate's judge model from reviewing routed turns that
/// failed; a failed test, build or run then always counts against the
/// model. Consumed by [`crate::GateConfig::judge`].
pub const AURA_GATE_JUDGE: &str = "AURA_GATE_JUDGE";
/// `off` stops Gate asking a routed turn that changed files without
/// running a test or build to verify its change. Consumed by
/// [`crate::GateConfig::nudge`].
pub const AURA_GATE_NUDGE: &str = "AURA_GATE_NUDGE";
/// `off` stops the harness running the project's tests on a routed
/// turn that changed files and ran none. Consumed by
/// [`crate::GateConfig::harness_check`].
pub const AURA_GATE_CHECK: &str = "AURA_GATE_CHECK";
/// Whether Gate's judge reviews work a routed turn is about to let stand:
/// `on`, `log` (ask and log the verdict without acting on it) or `off`.
/// Consumed by [`crate::GateConfig::review`].
pub const AURA_GATE_REVIEW: &str = "AURA_GATE_REVIEW";
/// Whether Gate writes down a routed code turn's acceptance criteria
/// before the work starts and reviews the work against them: `on`, `log`
/// (write them, show them to the worker, let exacting ones rate the work
/// hard, and log the review's verdict without acting on it) or `off`.
/// Consumed by
/// [`crate::GateConfig::criteria`].
pub const AURA_GATE_CRITERIA: &str = "AURA_GATE_CRITERIA";
/// Share (0..=1) of routed turns whose work gets a review that is only
/// logged (`AURA_GATE_REVIEW` or `AURA_GATE_CRITERIA` at `log`). Consumed
/// by [`crate::GateConfig::review_sample`].
pub const AURA_GATE_REVIEW_SAMPLE: &str = "AURA_GATE_REVIEW_SAMPLE";

// Cloud agents on boxd machines. `BOXD_*` are the names boxd's own SDKs
// read, so one `.env` serves both. All are consumed by
// [`crate::CloudConfig::from_env`] and nothing else.

/// boxd API key (`bxd_…`), exchanged for a short-lived token. Consumed
/// by [`crate::CloudConfig::credential`].
pub const BOXD_API_KEY: &str = "BOXD_API_KEY";
/// A boxd token the user already holds; wins over [`BOXD_API_KEY`].
/// Consumed by [`crate::CloudConfig::credential`].
pub const BOXD_TOKEN: &str = "BOXD_TOKEN";
/// boxd API endpoint. Consumed by [`crate::CloudConfig::base_url`].
pub const BOXD_BASE_URL: &str = "BOXD_BASE_URL";
/// Deprecated older name for [`BOXD_BASE_URL`], read when the new one is
/// unset.
pub const BOXD_API_URL: &str = "BOXD_API_URL";
/// Region for new machines (unset = Frankfurt). Consumed by
/// [`crate::CloudConfig::region`].
pub const BOXD_REGION: &str = "BOXD_REGION";
/// boxd organization to act in. Consumed by [`crate::CloudConfig::org`].
pub const BOXD_ORG: &str = "BOXD_ORG";
/// Machine size for new cloud agents (`1x4`, `2x8`, `4x16`, `8x32`).
/// Consumed by [`crate::CloudConfig::size`].
pub const AURA_CLOUD_SIZE: &str = "AURA_CLOUD_SIZE";
/// Idle seconds before a cloud machine hibernates. Consumed by
/// [`crate::CloudConfig::idle_hibernate_secs`].
pub const AURA_CLOUD_IDLE_HIBERNATE_SECS: &str = "AURA_CLOUD_IDLE_HIBERNATE_SECS";
/// Idle seconds before a cloud machine goes to standby (`0` = off).
/// Consumed by [`crate::CloudConfig::idle_standby_secs`].
pub const AURA_CLOUD_IDLE_STANDBY_SECS: &str = "AURA_CLOUD_IDLE_STANDBY_SECS";
/// Whether cloud machines are created isolated. Consumed by
/// [`crate::CloudConfig::isolated`].
pub const AURA_CLOUD_ISOLATED: &str = "AURA_CLOUD_ISOLATED";
/// Whether a spawned child of a cloud agent gets a fork of its parent's
/// machine. Consumed by [`crate::CloudConfig::fork_on_spawn`].
pub const AURA_CLOUD_FORK_ON_SPAWN: &str = "AURA_CLOUD_FORK_ON_SPAWN";
/// Price of one vCPU-hour, for the live cost estimate. Consumed by
/// [`crate::CloudConfig::price_vcpu_hour`].
pub const AURA_CLOUD_PRICE_VCPU_HOUR: &str = "AURA_CLOUD_PRICE_VCPU_HOUR";
/// Price of one GiB-hour of memory. Consumed by
/// [`crate::CloudConfig::price_mem_gib_hour`].
pub const AURA_CLOUD_PRICE_MEM_GIB_HOUR: &str = "AURA_CLOUD_PRICE_MEM_GIB_HOUR";
/// Price of one GiB-hour of disk. Consumed by
/// [`crate::CloudConfig::price_disk_gib_hour`].
pub const AURA_CLOUD_PRICE_DISK_GIB_HOUR: &str = "AURA_CLOUD_PRICE_DISK_GIB_HOUR";

/// The full list of env-var names this crate owns. Boundary tests use
/// this to enforce that no other crate calls `std::env::var(...)` on
/// any of these names.
pub const ENV_VAR_NAMES: &[&str] = &[
    AURA_AGENT_DISABLE_COMPACTION,
    AURA_AGENT_IMPLEMENT_NOW,
    AURA_AGENT_IMPLEMENT_NOW_THRESHOLD,
    AURA_AGENT_IMPLEMENT_NOW_BLOCK,
    AURA_AGENT_BOOTSTRAP_SPEC_BYTES,
    AURA_AGENT_BOOTSTRAP_STRIP_CODE_FENCES,
    AURA_AGENT_BOOTSTRAP_CONTEXT_CHARS,
    AURA_TURN_TOOL_HEARTBEAT_INTERVAL_SECS,
    AURA_DOD_TEST_COMMAND,
    AURA_SIMPLE_MODEL,
    AURA_LLM_MAX_RETRIES,
    AURA_LLM_BACKOFF_INITIAL_MS,
    AURA_LLM_BACKOFF_CAP_MS,
    AURA_DEV_LOOP_ENABLED_THINKING,
    AURA_DEFAULT_MODE,
    AURA_FLEET_DEFAULT_MODE,
    AURA_SUBAGENT_MAX_DEPTH,
    AURA_SUBAGENT_MAX_RUNNING,
    AURA_SUBAGENT_DEFAULT_MAX_ITERATIONS,
    AURA_SUBAGENT_DEFAULT_TIMEOUT_MS,
    AURA_CONFLICT_ENABLED,
    AURA_CONFLICT_DEFAULT_WAIT_MS,
    AURA_HOME,
    CODEX_HOME,
    AURA_TRACE,
    AURA_GATE_TARGET,
    AURA_GATE_MIN_TRIES,
    AURA_GATE_EXPLORE,
    AURA_GATE_DELEGATE,
    AURA_GATE_JUDGE,
    AURA_GATE_NUDGE,
    AURA_GATE_CHECK,
    AURA_GATE_REVIEW,
    AURA_GATE_CRITERIA,
    AURA_GATE_REVIEW_SAMPLE,
    BOXD_API_KEY,
    BOXD_TOKEN,
    BOXD_BASE_URL,
    BOXD_API_URL,
    BOXD_REGION,
    BOXD_ORG,
    AURA_CLOUD_SIZE,
    AURA_CLOUD_IDLE_HIBERNATE_SECS,
    AURA_CLOUD_IDLE_STANDBY_SECS,
    AURA_CLOUD_ISOLATED,
    AURA_CLOUD_FORK_ON_SPAWN,
    AURA_CLOUD_PRICE_VCPU_HOUR,
    AURA_CLOUD_PRICE_MEM_GIB_HOUR,
    AURA_CLOUD_PRICE_DISK_GIB_HOUR,
];

// ---------------------------------------------------------------------------
// Error type
// ---------------------------------------------------------------------------

/// Failure to parse a centralized env override.
#[derive(Debug, Error)]
pub enum ConfigError {
    /// The env var was set to a non-empty value that did not parse as
    /// the expected numeric / boolean shape.
    #[error("env var `{name}` has invalid value `{raw}`: {message}")]
    InvalidValue {
        /// Name of the env var (one of the constants in this module).
        name: &'static str,
        /// Raw value as read from the process environment.
        raw: String,
        /// Underlying parse error message.
        message: String,
    },
}

// ---------------------------------------------------------------------------
// Parse helpers (each `lookup_*` returns `None` for unset / blank vars
// so the caller can fall through to its compile-time default).
// ---------------------------------------------------------------------------

fn raw_var(name: &'static str) -> Option<String> {
    match std::env::var(name) {
        Ok(v) => {
            let trimmed = v.trim();
            if trimmed.is_empty() {
                None
            } else {
                Some(trimmed.to_string())
            }
        }
        Err(VarError::NotPresent | VarError::NotUnicode(_)) => None,
    }
}

/// Parse a numeric env var. Returns `None` for unset / blank vars.
pub(crate) fn lookup_numeric<T>(name: &'static str) -> Result<Option<T>, ConfigError>
where
    T: FromStr,
    T::Err: std::fmt::Display,
{
    let Some(raw) = raw_var(name) else {
        return Ok(None);
    };
    raw.parse::<T>()
        .map(Some)
        .map_err(|e| ConfigError::InvalidValue {
            name,
            raw,
            message: e.to_string(),
        })
}

/// Parse a non-zero numeric env var. Returns `None` for unset / blank
/// vars or when the value is zero (so consumers default to compile-time
/// fallbacks instead of disabling the knob accidentally).
pub(crate) fn lookup_nonzero_usize(name: &'static str) -> Result<Option<usize>, ConfigError> {
    Ok(lookup_numeric::<usize>(name)?.filter(|v| *v > 0))
}

/// Lookup a string env var. Returns `None` for unset / blank vars.
pub(crate) fn lookup_string(name: &'static str) -> Option<String> {
    raw_var(name)
}

/// Parse a truthy env var with explicit truthy / falsy literals.
///
/// `truthy_default` is the value returned when the env var is unset.
/// `truthy_literals` and `falsy_literals` are matched case-insensitively
/// against the trimmed raw value; any other non-empty value falls back
/// to `truthy_default`.
pub(crate) fn lookup_bool(
    name: &'static str,
    truthy_default: bool,
    truthy_literals: &[&str],
    falsy_literals: &[&str],
) -> bool {
    let Some(raw) = raw_var(name) else {
        return truthy_default;
    };
    let lower = raw.to_ascii_lowercase();
    if truthy_literals
        .iter()
        .any(|lit| lit.eq_ignore_ascii_case(&lower))
    {
        true
    } else if falsy_literals
        .iter()
        .any(|lit| lit.eq_ignore_ascii_case(&lower))
    {
        false
    } else {
        truthy_default
    }
}

/// Truthy literals used by "on by default unless explicitly disabled"
/// knobs (e.g. `AURA_AGENT_IMPLEMENT_NOW_BLOCK`). Matches the previous
/// inline `matches!(... Ok("0") | Ok("false") | Ok("no") | Ok("off"))`
/// negation logic.
pub(crate) const TRUTHY_LITERALS: &[&str] = &["1", "true", "yes", "on"];
pub(crate) const FALSY_LITERALS: &[&str] = &["0", "false", "no", "off"];

// ---------------------------------------------------------------------------
// Cross-module test serialisation
// ---------------------------------------------------------------------------

/// Crate-wide test lock for env-mutating tests.
///
/// `aura-config`'s `from_env`-style tests all share the same
/// process-wide env. Without coordination, a parallel test that
/// sets `AURA_SUBAGENT_MAX_DEPTH = "not-a-number"` will trip the
/// root `AuraConfig::from_env` happy-path test running in another
/// thread. Every env-touching test in this crate (including the
/// `lib.rs` smoke tests) acquires this mutex before mutating any
/// owned env var.
#[cfg(test)]
pub(crate) static ENV_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
