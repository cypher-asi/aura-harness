//! Gate's own calls to other models: the classifier that labels a
//! prompt, the writer of a turn's acceptance criteria, and the judge and
//! reviewer of work. Each goes through an agent's [`Kernel::reason`], so
//! the call is recorded like any other, within a time of its own.

mod classify;
mod criteria;
mod judge;

pub use classify::classify_prompt;
pub use criteria::{hold_to_criteria, worker_note_for};
pub use judge::{
    ask_review, cost_text, judge_failed, judge_for, judge_unverified, reviewer_for,
    verdict_judge_for, Judged,
};

use aura_agent_kernel::{Kernel, KernelError};
use aura_model::{ModelRequest, ModelResponse};
use std::fmt;
use std::time::Duration;

/// Why a call through the kernel gave no reply.
enum CallError {
    /// The kernel or the provider failed it
    Failed(KernelError),
    /// It took longer than it was given
    TimedOut(Duration),
}

impl fmt::Display for CallError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Failed(e) => write!(f, "the call failed ({e})"),
            Self::TimedOut(within) => write!(f, "the call took over {}s", within.as_secs()),
        }
    }
}

/// Send `request` through `kernel`, waiting at most `within` for the
/// reply.
async fn reason_within(
    kernel: &Kernel,
    request: ModelRequest,
    within: Duration,
) -> Result<ModelResponse, CallError> {
    match tokio::time::timeout(within, kernel.reason(request)).await {
        Ok(Ok(result)) => Ok(result.response),
        Ok(Err(e)) => Err(CallError::Failed(e)),
        Err(_) => Err(CallError::TimedOut(within)),
    }
}
