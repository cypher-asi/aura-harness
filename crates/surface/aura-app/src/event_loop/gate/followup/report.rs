//! What a step of an attempt's follow-up found, shown where the turn
//! runs as soon as it is found: in the terminal, or in the stream
//! `aura -p` writes.

use super::review::Reviewed;
use crate::event_loop::agent_events::AgentUi;
use crate::event_loop::handlers::system_message;
use crate::event_loop::task_check::CheckRun;
use aura_core_types::OutcomeSignal;
use aura_gate::live::Judged;
use std::future::Future;

/// What a step of an attempt's follow-up found.
pub(crate) enum Found<'a> {
    /// The harness is about to run `command`, its check of the change
    Checking(&'a str),
    /// The harness ran `command` on `model`'s change, which says `signal`
    /// of it
    Checked {
        model: &'a str,
        command: &'a str,
        check: &'a CheckRun,
        signal: Option<OutcomeSignal>,
    },
    /// The judge's verdict on work no check vouched for
    Judged(&'a Judged),
    /// What the reviewer made of work about to stand
    Reviewed(&'a Reviewed),
}

impl Found<'_> {
    /// The line the terminal shows of it.
    pub(crate) fn line(&self) -> String {
        match self {
            Self::Checking(command) => format!("gate: running `{command}` to check the change"),
            Self::Checked { check, .. } => check.describe(),
            Self::Judged(judged) => judged.describe(),
            Self::Reviewed(reviewed) => reviewed.describe(),
        }
    }
}

/// Where a follow-up shows what its steps find.
pub(crate) trait Report: Sync {
    /// Show `found`.
    fn report(&self, found: Found<'_>) -> impl Future<Output = ()> + Send;
}

impl Report for AgentUi {
    async fn report(&self, found: Found<'_>) {
        self.send(system_message(found.line())).await;
    }
}
