//! Cron parsing and the validation of process input.

use super::{ProcessError, MAX_PROCESS_NAME_LEN, MAX_PROCESS_PROMPT_BYTES};
use chrono::{DateTime, Utc};
use std::str::FromStr;

/// Normalize a cron expression for the `cron` crate, which expects a
/// seconds field: standard 5-field expressions get `0 ` prepended so
/// `*/5 * * * *` means "every 5 minutes at second 0". 6/7-field
/// expressions pass through unchanged.
fn normalize_cron(expr: &str) -> String {
    let fields = expr.split_whitespace().count();
    if fields == 5 {
        format!("0 {}", expr.trim())
    } else {
        expr.trim().to_string()
    }
}

/// Validate a cron expression (5, 6, or 7 fields; UTC semantics).
///
/// # Errors
/// Returns [`ProcessError::InvalidCron`] when the expression does not
/// parse.
pub fn validate_cron(expr: &str) -> Result<cron::Schedule, ProcessError> {
    if expr.trim().is_empty() {
        return Err(ProcessError::InvalidCron(
            "cron expression must not be empty".into(),
        ));
    }
    cron::Schedule::from_str(&normalize_cron(expr))
        .map_err(|e| ProcessError::InvalidCron(e.to_string()))
}

/// Compute the next fire time (UTC) strictly after `after`.
///
/// # Errors
/// Returns [`ProcessError::InvalidCron`] when the expression does not
/// parse.
pub fn next_run_after(
    expr: &str,
    after: DateTime<Utc>,
) -> Result<Option<DateTime<Utc>>, ProcessError> {
    let schedule = validate_cron(expr)?;
    Ok(schedule.after(&after).next())
}

pub(super) fn validate_name(name: &str) -> Result<(), ProcessError> {
    if name.trim().is_empty() {
        return Err(ProcessError::InvalidName("name must not be empty".into()));
    }
    if name.len() > MAX_PROCESS_NAME_LEN {
        return Err(ProcessError::InvalidName(format!(
            "name exceeds {MAX_PROCESS_NAME_LEN} characters"
        )));
    }
    Ok(())
}

pub(super) fn validate_prompt(prompt: &str) -> Result<(), ProcessError> {
    if prompt.trim().is_empty() {
        return Err(ProcessError::InvalidPrompt(
            "prompt must not be empty".into(),
        ));
    }
    if prompt.len() > MAX_PROCESS_PROMPT_BYTES {
        return Err(ProcessError::InvalidPrompt(format!(
            "prompt exceeds {MAX_PROCESS_PROMPT_BYTES} bytes"
        )));
    }
    Ok(())
}
