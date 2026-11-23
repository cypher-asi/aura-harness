//! The notices of the agent's background work: background commands that
//! end, and replies from its spawns. A running turn is steered them as
//! they come; between turns, the run waits for the next, which starts a
//! turn of its own.

use super::drive::Run;
use super::Headless;
use crate::event_loop::notices::{completion_notice, record_completion, Notice};
use crate::event_loop::steering::{SteerSource, Steering};
use crate::event_loop::AgentHub;
use crate::headless::output::{ExitReason, StreamEvent};
use aura_agent_kernel::Kernel;
use aura_core_types::Transaction;
use std::future::Future;
use std::time::Instant;
use tracing::{debug, warn};

impl Headless {
    /// Await `turn`, a turn of `kernel`'s agent, giving it through
    /// `steering` the notices for that agent that arrive meanwhile.
    /// Returns its result, and the notices that came once it no longer
    /// took input, for the next turn.
    pub(super) async fn steer_into<T>(
        &mut self,
        turn: impl Future<Output = T>,
        kernel: &Kernel,
        steering: &Steering,
    ) -> (T, Vec<String>) {
        let mut late = Vec::new();
        tokio::pin!(turn);
        loop {
            tokio::select! {
                result = &mut turn => return (result, late),
                Some(tx) = self.completions.recv() => {
                    if let Some(notice) = background_notice(&self.hub, kernel, tx).await {
                        steer_or_keep(steering, notice, &mut late).await;
                    }
                }
                Some(notice) = self.notices.recv() => {
                    if notice.to == kernel.agent_id {
                        steer_or_keep(steering, notice, &mut late).await;
                    }
                }
            }
        }
    }

    /// Wait for the next notice for `kernel`'s agent: a background
    /// command ended, or a spawn replied. `Ok(None)` when it has no
    /// background work left, or the work outlasts `job_wait` (commands
    /// are then stopped); `Err` when `run`'s deadline passes or it is
    /// interrupted while waiting.
    pub(super) async fn next_notice(
        &mut self,
        kernel: &Kernel,
        run: Run<'_>,
    ) -> Result<Option<String>, ExitReason> {
        if let Some(notice) = self.arrived_notice(kernel).await {
            return Ok(Some(notice));
        }
        if !self.hub.processes.has_background_work(kernel.agent_id) {
            return Ok(None);
        }
        self.await_notice(kernel, run).await
    }

    /// A notice for `kernel`'s agent that has already arrived.
    async fn arrived_notice(&mut self, kernel: &Kernel) -> Option<String> {
        while let Ok(tx) = self.completions.try_recv() {
            if let Some(notice) = background_notice(&self.hub, kernel, tx).await {
                return Some(notice.prompt);
            }
        }
        while let Ok(notice) = self.notices.try_recv() {
            if notice.to == kernel.agent_id {
                return Some(notice.prompt);
            }
        }
        None
    }

    /// Wait for a notice for `kernel`'s agent, as [`Self::next_notice`]
    /// does once none has arrived and the agent has background work.
    async fn await_notice(
        &mut self,
        kernel: &Kernel,
        run: Run<'_>,
    ) -> Result<Option<String>, ExitReason> {
        let wait_until = Instant::now() + self.job_wait;
        let until = run.deadline.map_or(wait_until, |d| d.min(wait_until));
        loop {
            tokio::select! {
                () = run.interrupt.cancelled() => return Err(ExitReason::Interrupted),
                () = tokio::time::sleep_until(until.into()) => {
                    if run.deadline.is_some_and(|d| d <= until) {
                        return Err(ExitReason::MaxTime);
                    }
                    let message = format!(
                        "Stopping background jobs still running after {}s",
                        self.job_wait.as_secs()
                    );
                    warn!("{message}");
                    run.output.event(&StreamEvent::Warning { message: &message });
                    return Ok(None);
                }
                tx = self.completions.recv() => match tx {
                    Some(tx) => {
                        if let Some(notice) = background_notice(&self.hub, kernel, tx).await {
                            return Ok(Some(notice.prompt));
                        }
                    }
                    None => return Ok(None),
                },
                Some(notice) = self.notices.recv() => {
                    if notice.to == kernel.agent_id {
                        return Ok(Some(notice.prompt));
                    }
                }
            }
        }
    }
}

/// Record the background command completion `tx` on `kernel` and return
/// the notice for its agent, when it is `kernel`'s agent and it should be
/// told.
async fn background_notice(hub: &AgentHub, kernel: &Kernel, tx: Transaction) -> Option<Notice> {
    let notice = completion_notice(&hub.processes, &tx);
    if tx.agent_id != kernel.agent_id {
        debug!(agent = %tx.agent_id, "Background command of another agent ended");
        return None;
    }
    record_completion(kernel, &tx).await;
    notice
}

/// Give `notice` to the running turn, or keep it in `late` for the next
/// one when the turn no longer takes input.
async fn steer_or_keep(steering: &Steering, notice: Notice, late: &mut Vec<String>) {
    if !steering
        .send(
            SteerSource::Harness,
            notice.prompt.clone(),
            notice.prompt.clone(),
        )
        .await
    {
        late.push(notice.prompt);
    }
}
