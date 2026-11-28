//! Handing work and messages to agents: now, into a running turn, or
//! queued until the agent is free.

use super::start_work::start_work;
use super::steering::SteerSource;
use super::turns::{Prompt, Work};
use super::{attachments, cloud, hud, LoopState};
use aura_core_types::AgentId;
use aura_ui::events::ImageAttachment;
use tracing::info;

/// Give `text` from `source` to `agent_id`. A harness notice goes into its
/// running turn if it has one that still takes input; a user's message
/// waits in the queue while the agent is busy. Otherwise it runs as a
/// prompt. `shown` is whether the chat already shows it.
pub(super) async fn deliver(
    state: &mut LoopState<'_>,
    agent_id: AgentId,
    source: SteerSource,
    text: String,
    shown: bool,
) {
    deliver_with_images(state, agent_id, source, text, Vec::new(), shown).await;
}

/// [`deliver`] for a user message with pasted `images`. A running turn
/// only takes text, so a message with images waits for the next turn.
pub(crate) async fn deliver_with_images(
    state: &mut LoopState<'_>,
    agent_id: AgentId,
    source: SteerSource,
    text: String,
    images: Vec<ImageAttachment>,
    shown: bool,
) {
    if !images.is_empty() {
        let work = Work::Prompt {
            prompt: Prompt::typed(text, images),
            shown,
        };
        submit(state, agent_id, work).await;
        return;
    }
    // A user's message waits its turn in the queue, and the user can send
    // it now (`UiEvent::SendQueuedNow`). Harness notices still go straight
    // into the running turn.
    if source == SteerSource::Harness {
        if let Some(steering) = state.hub.steering(agent_id) {
            let body = steered_body(state, agent_id, source, &text).await;
            if steering.send(source, text.clone(), body).await {
                info!(%agent_id, ?source, "Sent notice into the running turn");
                return;
            }
        }
    }
    submit(state, agent_id, prompt_work(source, text, shown)).await;
}

/// What the model gets for `text` sent into a running turn: a user's
/// message has the files it mentions appended, read from the agent's
/// workspace.
pub(super) async fn steered_body(
    state: &LoopState<'_>,
    agent_id: AgentId,
    source: SteerSource,
    text: &str,
) -> String {
    if source != SteerSource::User || attachments::mentioned_paths(text).is_empty() {
        return text.to_string();
    }
    let Some(kernel) = state.kernel_for(agent_id) else {
        return text.to_string();
    };
    attachments::expand_mentions_in(kernel.agent_workspace(), text.to_string())
        .await
        .text
}

/// A prompt from `source`. Harness notices keep their label so the model
/// can tell them from the user, and carry on with the agent's latest task
/// instead of starting one.
pub(super) fn prompt_work(source: SteerSource, text: String, shown: bool) -> Work {
    match source {
        SteerSource::User => Work::Prompt {
            prompt: Prompt::typed(text, Vec::new()),
            shown,
        },
        SteerSource::Harness => Work::Prompt {
            prompt: Prompt::notice(format!("{}{text}", source.prefix())),
            shown: false,
        },
    }
}

/// Give `work` to `agent_id`: now if it is idle, else once its current
/// work ends.
pub(super) async fn submit(state: &mut LoopState<'_>, agent_id: AgentId, work: Work) {
    let Some(work) = cloud::submit(state, agent_id, work).await else {
        return;
    };
    if state.hub.is_running(agent_id) {
        info!(%agent_id, "Agent is busy; queueing work");
        state.queued.push_back(agent_id, work);
        hud::send_queued(state, agent_id).await;
        return;
    }
    start_work(state, agent_id, work).await;
}

/// Force the oldest queued work of `agent_id` to run now: the running turn
/// is cancelled, and `handle_work_done` then starts the front of the queue.
/// The queue itself is kept, so nothing the user queued is dropped, and
/// what the stopped turn was asked through its inbox waits for the next
/// turn that ends well.
pub(super) async fn send_queued_now(state: &mut LoopState<'_>, agent_id: AgentId) {
    if state.queued.len(agent_id) == 0 {
        return;
    }
    if state.cloud.cancel(agent_id) {
        info!(%agent_id, "Sending queued work now; stopping the cloud turn");
        state.hub.inbox.interrupt(agent_id);
        return;
    }
    if !state.hub.is_running(agent_id) {
        start_next(state, agent_id).await;
        return;
    }
    info!(%agent_id, "Sending queued work now; cancelling the running turn");
    // A call waiting for an answer holds the turn; deny it so the turn
    // sees the cancellation.
    state.approvals.cancel_agent(agent_id).await;
    if state.hub.cancel(agent_id) {
        state.hub.inbox.interrupt(agent_id);
    }
}

/// Start the oldest work waiting for `agent_id`, if any, where it runs
/// now. A local agent's work goes through the cloud's submit too: one
/// just back from the cloud must first drop its session, which is older
/// than its record.
pub(super) async fn start_next(state: &mut LoopState<'_>, agent_id: AgentId) {
    cloud::start_queued(state, agent_id).await;
}
