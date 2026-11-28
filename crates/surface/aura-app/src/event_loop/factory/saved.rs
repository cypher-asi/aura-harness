//! The software factory's stages kept across restarts.
//!
//! They are saved in the inbox's file ([`crate::event_loop::inbox`]'s
//! `saved`), in the same write as the reports members owe, so the two
//! never disagree: a member saved mid-build still owes the report its
//! build defers. The file is replaced atomically on a blocking thread,
//! bounded, and read as empty with a warning when it is corrupt.
//!
//! On restore a member continues where it was:
//!
//! | Saved stage | Work put back in its queue |
//! |---|---|
//! | assigned | none: the task is the inbox's queued or undelivered message |
//! | planning, turn under way | the prompt that turn started with, in plan mode |
//! | planning, waiting for a reply | none: the reply comes back to a planning turn |
//! | building | the build of its plan |
//! | shipping | the ship prompt |
//!
//! A cloud member whose turn was under way gets nothing queued: its turn
//! goes on on its machine and is reattached. Members that settled or
//! failed (`Flows::leave`) are not saved, and archived or removed ones
//! are dropped on restore.

use super::flow::{FlowRecord, Stage};
use super::hooks;
use crate::event_loop::subagents::lock;
use crate::event_loop::turns::{Prompt, Request, Work};
use crate::event_loop::{LoopState, Permissions};
use aura_core_types::AgentId;
use aura_ui::PermissionMode;
use serde::{Deserialize, Serialize};
use tracing::info;

/// Most members' stages kept.
pub(crate) const MAX_FLOWS: usize = 1_000;
/// Most bytes of prompts kept, all members together, so the file stays
/// well under the inbox file's read limit. A prompt past it is not kept:
/// that member's planning turn isn't queued again, and the stall sweep
/// asks it to carry on instead.
const MAX_PROMPT_BYTES: usize = 2 * 1024 * 1024;

/// A stage, as the file says it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum SavedStage {
    Assigned,
    Planning,
    Building,
    Shipping,
}

/// One member's place in the flow, as the file keeps it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct SavedFlow {
    /// Hex id of the member
    pub agent: String,
    pub stage: SavedStage,
    /// The plan it builds
    #[serde(default)]
    pub plan: Option<String>,
    /// The text its latest turn started with
    #[serde(default)]
    pub prompt: Option<String>,
    /// Its stage's work is in flight or queued (see `FlowRecord`)
    #[serde(default)]
    pub pending: bool,
    /// It had a turn under way when saved
    #[serde(default)]
    pub running: bool,
}

/// What the file keeps of `state`'s flows. A prompt is kept only where a
/// restore needs it, for a planning turn under way, and within
/// [`MAX_PROMPT_BYTES`].
pub(in crate::event_loop) fn saved_flows(state: &LoopState<'_>) -> Vec<SavedFlow> {
    let running = |agent| state.hub.is_working(agent) || state.cloud.is_running(agent);
    let mut budget = MAX_PROMPT_BYTES;
    let records = state.hub.factory_flows.records(running);
    records
        .into_iter()
        .map(|record| {
            let replans = record.stage == Stage::Planning && record.pending;
            let prompt = record.prompt.filter(|text| replans && text.len() <= budget);
            budget -= prompt.as_ref().map_or(0, String::len);
            SavedFlow {
                agent: record.agent.to_hex(),
                stage: saved_stage(record.stage),
                plan: record.plan,
                prompt,
                pending: record.pending,
                running: running(record.agent),
            }
        })
        .collect()
}

/// Put `saved` back on `state`, those of agents `live` accepts, and queue
/// the work each continues with. Returns the agents that got work.
pub(in crate::event_loop) fn put_back(
    state: &mut LoopState<'_>,
    saved: Vec<SavedFlow>,
    live: impl Fn(&str) -> Option<AgentId>,
) -> Vec<AgentId> {
    let flows: Vec<(FlowRecord, bool)> = saved
        .into_iter()
        .filter_map(|flow| {
            let agent = live(&flow.agent)?;
            let record = FlowRecord {
                agent,
                stage: stage_of(flow.stage),
                plan: flow.plan,
                prompt: flow.prompt,
                pending: flow.pending,
            };
            Some((record, flow.running))
        })
        .collect();
    let mut started = Vec::new();
    for (record, running) in &flows {
        if record.stage == Stage::Planning {
            plan_mode(state, record.agent);
        }
        let work = continuing_work(state, record, *running);
        let resumes = work.is_some();
        let record = FlowRecord {
            pending: resumes,
            ..record.clone()
        };
        state
            .hub
            .factory_flows
            .restore(std::slice::from_ref(&record));
        if let Some(work) = work {
            state.queued.push_front(record.agent, work);
            started.push(record.agent);
        }
    }
    if !flows.is_empty() {
        info!(
            members = flows.len(),
            resumed = started.len(),
            "Restored the software factory's stages"
        );
    }
    started
}

/// Put planning member `agent` back in plan mode, which its next turn's
/// kernel takes on as it starts.
fn plan_mode(state: &LoopState<'_>, agent: AgentId) {
    let hub = &state.hub;
    if hub.mode_of(agent) != PermissionMode::Plan {
        let approval = hub.permissions_of(agent).approval;
        hub.set_permissions(agent, Permissions::new(PermissionMode::Plan, approval));
    }
}

/// The work `record`'s member continues with, if the factory must start
/// it again. `running`: its turn was under way when saved.
fn continuing_work(state: &LoopState<'_>, record: &FlowRecord, running: bool) -> Option<Work> {
    let cloud = lock(&state.hub.cloud_members).contains(&record.agent);
    if !record.pending || (cloud && running) {
        return None;
    }
    match record.stage {
        Stage::Assigned => None,
        Stage::Planning => replan(state, record),
        Stage::Building => Some(hooks::build_work(cloud, record.plan.as_deref()?)),
        Stage::Shipping => {
            let offers = !cloud || crate::event_loop::cloud::node_offers_ship(state, record.agent);
            Some(hooks::ship_work(offers))
        }
    }
}

/// The planning turn of `record`'s member again, from the prompt it
/// started with, unless the inbox put that prompt back in its queue.
fn replan(state: &LoopState<'_>, record: &FlowRecord) -> Option<Work> {
    let text = record.prompt.clone()?;
    if state.queued.has_prompt(record.agent, &text) {
        return None;
    }
    Some(Work::Prompt {
        prompt: Prompt::inbox(text, Request::Resume),
        shown: false,
    })
}

const fn saved_stage(stage: Stage) -> SavedStage {
    match stage {
        Stage::Assigned => SavedStage::Assigned,
        Stage::Planning => SavedStage::Planning,
        Stage::Building => SavedStage::Building,
        Stage::Shipping => SavedStage::Shipping,
    }
}

const fn stage_of(stage: SavedStage) -> Stage {
    match stage {
        SavedStage::Assigned => Stage::Assigned,
        SavedStage::Planning => Stage::Planning,
        SavedStage::Building => Stage::Building,
        SavedStage::Shipping => Stage::Shipping,
    }
}
