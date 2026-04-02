//! Salience scoring for memory retrieval prioritization.
//!
//! Scores combine importance, recency (exponential decay with ~7-day half-life),
//! and access frequency (log-scaled) to rank memory items for prompt injection.

use crate::types::{AgentEvent, Fact, Procedure};
use chrono::{DateTime, Utc};
use std::f32::consts::LN_2;

/// Score a fact using a weighted combination of importance, recency, and access frequency.
#[must_use]
pub fn score_fact(fact: &Fact, now: DateTime<Utc>) -> f32 {
    let recency = recency_decay(fact.last_accessed, now);
    let access = normalized_access(fact.access_count);
    0.2f32.mul_add(access, 0.5f32.mul_add(fact.importance, 0.3 * recency))
}

/// Score an event using importance and recency.
#[must_use]
pub fn score_event(event: &AgentEvent, now: DateTime<Utc>) -> f32 {
    let recency = recency_decay(event.timestamp, now);
    0.4f32.mul_add(event.importance, 0.6 * recency)
}

/// Score a procedure using success rate, recency, and execution frequency.
#[must_use]
pub fn score_procedure(proc: &Procedure, now: DateTime<Utc>) -> f32 {
    let recency = recency_decay(proc.last_used, now);
    let frequency = normalized_access(proc.execution_count);
    0.3f32.mul_add(frequency, 0.3f32.mul_add(recency, 0.4 * proc.success_rate))
}

/// Estimate token count for a string (bytes / 4 approximation).
#[must_use]
pub const fn estimate_tokens(text: &str) -> usize {
    text.len().div_ceil(4)
}

/// Estimate the token cost of a fact's prompt representation.
#[must_use]
pub fn estimate_fact_tokens(fact: &Fact) -> usize {
    let val = match &fact.value {
        serde_json::Value::String(s) => s.clone(),
        other => other.to_string(),
    };
    let line = format!("- {}: {} (confidence: {:.2})", fact.key, val, fact.confidence);
    estimate_tokens(&line)
}

/// Estimate the token cost of an event's prompt representation.
#[must_use]
pub fn estimate_event_tokens(event: &AgentEvent) -> usize {
    let line = format!(
        "- [{}] {}: {}",
        event.timestamp.format("%Y-%m-%d"),
        event.event_type,
        event.summary
    );
    estimate_tokens(&line)
}

/// Estimate the token cost of a procedure's prompt representation.
#[must_use]
pub fn estimate_procedure_tokens(proc: &Procedure) -> usize {
    let steps = proc.steps.join(" -> ");
    let line = format!(
        "- \"{}\": {} (success: {:.0}%)",
        proc.name,
        steps,
        proc.success_rate * 100.0
    );
    estimate_tokens(&line)
}

/// Exponential decay based on time since last access.
/// Returns 1.0 for "just now", decaying toward 0.0 over time.
/// Half-life: ~7 days.
#[allow(clippy::cast_precision_loss)]
fn recency_decay(last_time: DateTime<Utc>, now: DateTime<Utc>) -> f32 {
    let hours = (now - last_time).num_hours().max(0) as f32;
    let half_life_hours: f32 = 7.0 * 24.0;
    (-LN_2 * hours / half_life_hours).exp()
}

/// Normalize access count to 0.0..1.0 range using logarithmic scaling.
#[allow(clippy::cast_precision_loss)]
fn normalized_access(count: u32) -> f32 {
    if count == 0 {
        return 0.0;
    }
    let log_count = (count as f32).ln_1p();
    let log_max = 101.0_f32.ln();
    (log_count / log_max).min(1.0)
}
