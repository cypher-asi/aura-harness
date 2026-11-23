//! How an attempt that ran to its end went, as Gate reads it: the signal
//! it ended with, and how it failed, in words for the judge.

use aura_agent::{AgentError, AgentLoopResult};
use aura_core_types::OutcomeSignal;
use aura_gate::evidence::checks::{last_check, last_check_seen};
use aura_gate::evidence::outcome::turn_signal;

/// The signal of a turn that ran to its end with `result`, whose messages
/// from `turn_start` on are the turn's own; `None` when nothing says how
/// it went, and the judge has to. A turn the model's provider cut short
/// is [`OutcomeSignal::Unavailable`] whatever it did before, since its
/// work stopped where the model did not choose to stop it.
pub(crate) fn ended_signal(
    result: &Result<AgentLoopResult, AgentError>,
    turn_start: usize,
) -> Option<OutcomeSignal> {
    match result {
        Err(e) if e.is_unavailable() => Some(OutcomeSignal::Unavailable),
        Err(_) => Some(OutcomeSignal::Error),
        Ok(result) if result.llm_error.is_some() && result.llm_unavailable => {
            Some(OutcomeSignal::Unavailable)
        }
        Ok(result) => {
            let errored = result.llm_error.is_some() && result.total_text.trim().is_empty();
            let messages = result.messages.get(turn_start..).unwrap_or_default();
            turn_signal(errored, last_check(messages))
        }
    }
}

/// How a turn that ran to its end with `result` failed, for the judge:
/// its error, or its last check and what the check printed. `None` when
/// it did neither.
pub(crate) fn failure_text(
    result: &Result<AgentLoopResult, AgentError>,
    turn_start: usize,
) -> Option<String> {
    let result = match result {
        Err(e) => return Some(format!("The run ended in an error: {e}")),
        Ok(result) => result,
    };
    if let Some(error) = result
        .llm_error
        .as_ref()
        .filter(|_| result.total_text.trim().is_empty())
    {
        return Some(format!("The model call failed: {error}"));
    }
    let messages = result.messages.get(turn_start..).unwrap_or_default();
    last_check_seen(messages)
        .filter(|check| !check.passed)
        .map(|check| {
            format!(
                "The last test or build command it ran, `{}`, failed. What the command printed:\n{}",
                check.command, check.output
            )
        })
}
