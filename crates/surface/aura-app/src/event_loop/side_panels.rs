//! What the panels beside the chat show of the agent coming on screen:
//! its plans and plan strip, its Record panel, its TASKS tab and its
//! latest stored run.
//!
//! A switch sends the chat first and leaves these to a job of its own, so
//! the event loop goes on to the next event, another switch included,
//! while they are read: a long record or a workspace with hundreds of
//! tasks takes a while. The next switch stops the job; what it would
//! still send is for an agent no longer on screen, and is sent again when
//! that agent comes back.

use super::agent_events::AgentUi;
use super::plan_mode::{plan_view_among, plan_views};
use super::subagents::AgentHub;
use super::LoopState;
use crate::record_loader::{recent_record_summaries, ByHead};
use aura_core_types::AgentId;
use aura_ui::UiCommand;
use std::future::Future;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::mpsc;
use tracing::{debug, warn};

impl LoopState<'_> {
    /// Fill the side panels of `agent_id`, now on screen, in a job of its
    /// own.
    pub(super) fn fill_side_panels(&mut self, agent_id: AgentId) {
        self.stop_side_panels();
        let panels = SidePanels {
            hub: Arc::clone(&self.hub),
            commands: self.commands.clone(),
            agent_id,
            workspace: self
                .kernel_for(agent_id)
                .map(|kernel| kernel.agent_workspace()),
            // A busy agent's turn shows its plan in the strip.
            strip: self.idle.get(&agent_id).map(|session| {
                (
                    session.kernel.agent_workspace(),
                    session.active_plan.clone(),
                )
            }),
        };
        self.side_panels = Some(tokio::spawn(panels.fill()));
    }

    /// Stop filling the side panels of the agent that was on screen.
    pub(super) fn stop_side_panels(&mut self) {
        if let Some(job) = self.side_panels.take() {
            job.abort();
        }
    }
}

/// The side panels of the agent on screen, to fill in.
struct SidePanels {
    hub: Arc<AgentHub>,
    commands: mpsc::Sender<UiCommand>,
    agent_id: AgentId,
    /// Where its plans and tasks are kept, when its kernel is here
    workspace: Option<PathBuf>,
    /// Its idle session's workspace and plan, for the plan strip; `None`
    /// leaves the strip as it is
    strip: Option<(PathBuf, Option<String>)>,
}

impl SidePanels {
    /// Fill every panel at once: a long record takes the Record panel a
    /// second or more to read in a development build, which the others
    /// don't wait for.
    async fn fill(self) {
        let started = Instant::now();
        let (plans_ms, records_ms, tasks_ms, trace_ms) = tokio::join!(
            done_ms(started, self.send_plans()),
            done_ms(started, self.send_records()),
            done_ms(started, self.send_tasks()),
            done_ms(started, self.send_trace()),
        );
        debug!(
            agent_id = %self.agent_id,
            plans_ms,
            records_ms,
            tasks_ms,
            trace_ms,
            "Filled the side panels"
        );
    }

    /// The Plans tab, and the plan strip of an idle agent.
    async fn send_plans(&self) {
        let (workspace, strip) = (self.workspace.clone(), self.strip.clone());
        if workspace.is_none() && strip.is_none() {
            return;
        }
        let read = blocking(move || {
            let plans = workspace.as_deref().map(plan_views);
            let strip = strip.map(|(workspace, plan)| {
                plan.and_then(|path| {
                    plan_view_among(plans.as_deref().unwrap_or_default(), &workspace, &path)
                })
            });
            (plans, strip)
        })
        .await;
        let Some((plans, strip)) = read else {
            return;
        };
        let ui = AgentUi::new(self.commands.clone(), self.agent_id);
        if let Some(plans) = plans {
            ui.send(UiCommand::SetPlans(plans)).await;
        }
        if let Some(strip) = strip {
            ui.send(UiCommand::SetPlan(strip)).await;
        }
    }

    /// The Record panel, in one replace-all command: it clears the
    /// previous agent's records even when this agent's can't be read.
    async fn send_records(&self) {
        let read = ByHead::get_or_read_off_loop(
            &self.hub.shown_records,
            &self.hub.factory.store,
            self.agent_id,
            recent_record_summaries,
        )
        .await;
        let (records, problem) = match read {
            Ok(summaries) => (summaries, None),
            Err(e) => (Vec::new(), Some(e)),
        };
        let _ = self.commands.send(UiCommand::SetRecords(records)).await;
        if let Some(e) = problem {
            let _ = self
                .commands
                .send(UiCommand::ShowWarning(format!(
                    "Could not load record history: {e}"
                )))
                .await;
        }
    }

    async fn send_tasks(&self) {
        let Some(workspace) = self.workspace.clone() else {
            return;
        };
        let (hub, agent_id) = (Arc::clone(&self.hub), self.agent_id);
        let views = blocking(move || hub.factory.tasks.all_in(agent_id, &workspace)).await;
        if let Some(views) = views {
            self.hub
                .factory
                .tasks
                .send_tasks(self.agent_id, views)
                .await;
        }
    }

    /// The TRACE tab's latest stored run. It is sent from the blocking
    /// thread that reads it: the run is only ever sent once, so a stopped
    /// job must not drop it.
    async fn send_trace(&self) {
        // An agent with nothing recorded has never run, so it has no stored
        // run to look for, and looking finds none only after reading every
        // run's metadata.
        let has_records = self
            .hub
            .factory
            .store
            .get_head_seq(self.agent_id)
            .map_or(true, |head| head > 0);
        if !has_records && !self.hub.is_running(self.agent_id) {
            return;
        }
        let (hub, commands, agent_id) =
            (Arc::clone(&self.hub), self.commands.clone(), self.agent_id);
        blocking(move || {
            if let Some(history) = hub.traces.history_of(agent_id) {
                let _ = commands.blocking_send(history);
            }
        })
        .await;
    }
}

/// When `step` is done, in milliseconds since `started`.
async fn done_ms(started: Instant, step: impl Future<Output = ()>) -> u64 {
    step.await;
    ms_since(started)
}

/// `read` on a blocking thread; `None` if it panicked.
async fn blocking<T: Send + 'static>(read: impl FnOnce() -> T + Send + 'static) -> Option<T> {
    tokio::task::spawn_blocking(read)
        .await
        .inspect_err(|e| warn!(error = %e, "Could not read a side panel"))
        .ok()
}

/// Milliseconds since `at`, for timing logs.
pub(super) fn ms_since(at: Instant) -> u64 {
    u64::try_from(at.elapsed().as_millis()).unwrap_or(u64::MAX)
}
