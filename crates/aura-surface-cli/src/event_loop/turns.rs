//! Agent work running on its own task.
//!
//! Every turn (and every command typed in a chat) runs on a spawned task
//! that owns the agent's session until it is done, so agents work in
//! parallel and the event loop keeps handling the UI meanwhile. Each agent
//! still does one thing at a time: its record has a single writer.

use super::agent_events::AgentUi;
use super::agents::AgentSession;
use super::attachments::{expand_mentions_in, mentioned_paths};
use super::handlers::{handle_agent_success, run_agent_turn, system_message};
use super::run_command::run_user_command;
use super::steering::{SteerSource, Steering};
use crate::long_run::LongRunSettings;
use aura_agent::{AgentLoop, CompactHistoryError};
use aura_agent_gate::BlockRunner;
use aura_agent_trace::RunRecorder;
use aura_core_types::{AgentId, RecordEntry, Transaction};
use aura_model_reasoner::{ContentBlock, ImageSource, Message, Role, ToolDefinition};
use aura_terminal::events::ImageAttachment;
use aura_terminal::UiCommand;
use std::sync::Arc;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tracing::{error, info, warn};

/// A message for the model, with what the user attached to it.
pub(super) struct Prompt {
    /// The message as typed; the chat and the record keep this
    pub text: String,
    /// Whether `@path` words in `text` attach files. Only text the user
    /// typed has mentions; a harness notice may quote `@` words verbatim.
    pub mentions: bool,
    /// Images pasted into the message
    pub images: Vec<ImageAttachment>,
}

impl Prompt {
    /// A message typed by the user, with the images pasted into it.
    pub(super) const fn typed(text: String, images: Vec<ImageAttachment>) -> Self {
        Self {
            text,
            mentions: true,
            images,
        }
    }

    /// A message written by the harness, given to the model as is.
    pub(super) const fn plain(text: String) -> Self {
        Self {
            text,
            mentions: false,
            images: Vec::new(),
        }
    }
}

/// Something the user asked an agent to do.
pub(super) enum Work {
    /// A message for the model. `shown` is whether the chat already shows
    /// it.
    Prompt { prompt: Prompt, shown: bool },
    /// A shell command typed in the chat, run in a terminal `cols` wide.
    Command { command: String, cols: u16 },
    /// Summarize the older history now, keeping `focus` in particular.
    Compact { focus: Option<String> },
    /// Implement the plan at `path` (`/build`), or the agent's latest plan.
    Build { path: Option<String> },
}

/// Work ready to run.
pub(super) enum Task {
    /// A model turn answering `prompt`, taking further messages through
    /// `steering` while it runs.
    Turn {
        prompt: Prompt,
        agent_loop: Box<AgentLoop>,
        tools: Vec<ToolDefinition>,
        steering: Steering,
        limits: LongRunSettings,
        recorder: Option<Arc<RunRecorder>>,
        /// Runner that drafts the turn first, on gate turns.
        drafter: Option<Arc<BlockRunner>>,
    },
    /// A typed shell command.
    Command { command: String, cols: u16 },
    /// A summary of the older history, written by `agent_loop`'s model.
    Compact {
        agent_loop: Box<AgentLoop>,
        focus: Option<String>,
    },
}

/// An agent's work that ended, handing its session back.
pub(super) struct WorkDone {
    pub agent_id: AgentId,
    /// `None` when the task panicked; the session is reopened from the
    /// record the next time it is needed.
    pub session: Option<AgentSession>,
    /// Why a turn failed, for the event loop to report.
    pub failure: Option<String>,
    /// Messages sent into the turn that the model never got, to run next.
    pub unseen: Vec<(SteerSource, String)>,
}

/// Run `task` on `session` in the background, reporting to `ui`, and send
/// the session back on `done`.
pub(super) fn spawn(
    session: AgentSession,
    task: Task,
    ui: AgentUi,
    cancel: CancellationToken,
    done: mpsc::Sender<WorkDone>,
) {
    let agent_id = session.agent_id();
    tokio::spawn(async move {
        let work = tokio::spawn(run(session, task, ui, cancel));
        let (session, outcome) = match work.await {
            Ok((session, outcome)) => (Some(session), outcome),
            Err(e) => {
                error!(%agent_id, error = %e, "Agent work panicked");
                (
                    None,
                    Outcome {
                        failure: Some(format!("Agent stopped unexpectedly: {e}")),
                        unseen: Vec::new(),
                    },
                )
            }
        };
        let _ = done
            .send(WorkDone {
                agent_id,
                session,
                failure: outcome.failure,
                unseen: outcome.unseen,
            })
            .await;
    });
}

/// How a piece of work ended.
#[derive(Default)]
struct Outcome {
    failure: Option<String>,
    unseen: Vec<(SteerSource, String)>,
}

async fn run(
    mut session: AgentSession,
    task: Task,
    ui: AgentUi,
    cancel: CancellationToken,
) -> (AgentSession, Outcome) {
    let outcome = match task {
        Task::Turn {
            prompt,
            agent_loop,
            tools,
            steering,
            limits,
            recorder,
            drafter,
        } => {
            let turn = TurnParts {
                agent_loop: &agent_loop,
                tools,
                steering: &steering,
                limits,
                recorder,
                drafter,
            };
            run_prompt(&mut session, prompt, turn, &ui, &cancel).await
        }
        Task::Command { command, cols } => {
            run_user_command(&mut session, &command, cols, &ui).await;
            Outcome::default()
        }
        Task::Compact { agent_loop, focus } => {
            run_compact(&mut session, &agent_loop, focus.as_deref(), &ui, &cancel).await;
            Outcome::default()
        }
    };
    (session, outcome)
}

/// What a turn runs on, besides the session.
struct TurnParts<'a> {
    agent_loop: &'a AgentLoop,
    tools: Vec<ToolDefinition>,
    steering: &'a Steering,
    limits: LongRunSettings,
    recorder: Option<Arc<RunRecorder>>,
    drafter: Option<Arc<BlockRunner>>,
}

/// Record `text` on `session` as a prompt from the user. The kernels'
/// record observer shows the entry in the Record panel.
pub(super) async fn record_user_prompt(session: &AgentSession, text: String) -> Result<(), String> {
    record_prompt(session, text).await.map(|_| ())
}

/// Record `text` on `session` as a prompt from the user, returning the
/// transaction and the record entry it became.
pub(crate) async fn record_prompt(
    session: &AgentSession,
    text: String,
) -> Result<(Transaction, RecordEntry), String> {
    let prompt_tx = Transaction::user_prompt(session.agent_id(), text);
    let (tx, token) = enqueue_and_dequeue(session, prompt_tx)?;
    let result = session
        .kernel
        .process_dequeued(tx.clone(), token)
        .await
        .map_err(|e| {
            error!(error = %e, "Failed to persist prompt record via kernel");
            format!("Kernel error: {e}")
        })?;
    Ok((tx, result.entry))
}

/// Record `prompt` as typed and run a turn answering it. The model gets
/// the files it mentions as well.
async fn run_prompt(
    session: &mut AgentSession,
    prompt: Prompt,
    turn: TurnParts<'_>,
    ui: &AgentUi,
    cancel: &CancellationToken,
) -> Outcome {
    drain_stale_inbox(session).await;

    if let Err(problem) = record_user_prompt(session, prompt.text.clone()).await {
        ui.send(UiCommand::ShowError(problem)).await;
        ui.send(UiCommand::Complete).await;
        return Outcome::default();
    }
    if let Some(files) = &session.file_checkpoints {
        files.begin_turn(
            session.agent_id(),
            session.started_ms,
            session.messages.len(),
            &prompt.text,
        );
    }
    let request = prompt.text.clone();
    session.messages.push(model_message(session, prompt).await);
    if let Some(runner) = &turn.drafter {
        ui.send(system_message(
            "gate: a fast worker drafts this request; the coordinator reviews it next".into(),
        ))
        .await;
        if let Some(draft) = crate::gate::draft(runner, &session.kernel, &request).await {
            session.messages.extend(draft);
        }
    }

    let (process_result, forwarded) = run_agent_turn(
        session,
        turn.agent_loop,
        turn.tools,
        ui,
        cancel,
        turn.steering,
        turn.limits,
        turn.recorder.clone(),
    )
    .await;
    if let Some(files) = &session.file_checkpoints {
        files.end_turn(session.agent_id());
    }
    session.usage.add(&forwarded.usage);
    if let Some(path) = &forwarded.plan_path {
        session.active_plan = Some(path.clone());
        session.building_plan = false;
    }
    let (seen, unseen) = turn.steering.finish(
        process_result
            .as_ref()
            .map_or(&[][..], |result| result.messages.as_slice()),
    );
    let cancelled = cancel.is_cancelled();
    if let Some(rec) = &turn.recorder {
        super::trace::end_turn(rec, &process_result, cancelled);
    }
    if cancelled {
        ui.send(UiCommand::ShowWarning("Turn cancelled".to_string()))
            .await;
    }

    let failure = match process_result {
        Ok(result) => {
            handle_agent_success(session, result, &forwarded, ui, &seen).await;
            None
        }
        Err(e) if cancelled => {
            info!(error = %e, "Cancelled turn ended with an error");
            ui.send(UiCommand::Complete).await;
            None
        }
        Err(e) => {
            error!(error = %e, "Agent loop failed");
            Some(e.to_string())
        }
    };
    Outcome { failure, unseen }
}

/// The user message the model gets for `prompt`: its images, then its
/// text with the mentioned files from the agent's workspace appended.
async fn model_message(session: &AgentSession, prompt: Prompt) -> Message {
    let Prompt {
        text,
        mentions,
        images,
    } = prompt;
    let text = if mentions && !mentioned_paths(&text).is_empty() {
        let expanded = expand_mentions_in(session.kernel.agent_workspace(), text).await;
        if !expanded.attached.is_empty() {
            info!(files = ?expanded.attached, "Attached mentioned files");
        }
        expanded.text
    } else {
        text
    };
    if images.is_empty() {
        return Message::user(text);
    }
    info!(count = images.len(), "Attached pasted images");
    user_message_with_images(text, images)
}

/// A user message holding `images` and then `text`. Anthropic advises
/// putting images before the text that refers to them.
fn user_message_with_images(text: String, images: Vec<ImageAttachment>) -> Message {
    let mut content: Vec<ContentBlock> = images
        .into_iter()
        .map(|image| ContentBlock::Image {
            source: ImageSource {
                source_type: "base64".to_string(),
                media_type: image.media_type,
                data: image.data,
            },
        })
        .collect();
    content.push(ContentBlock::Text { text });
    Message::new(Role::User, content)
}

/// Replace the older history of `session` with a summary. Only the
/// conversation the model sees changes; the record keeps every message.
async fn run_compact(
    session: &mut AgentSession,
    agent_loop: &AgentLoop,
    focus: Option<&str>,
    ui: &AgentUi,
    cancel: &CancellationToken,
) {
    ui.send(UiCommand::SetStatus("Summarizing history...".to_string()))
        .await;
    let compacted = agent_loop
        .compact_history(
            &session.model_gateway,
            &session.messages,
            &session.todos,
            focus,
            Some(cancel),
        )
        .await;
    match compacted {
        Ok(compacted) => {
            info!(
                before = compacted.before_tokens,
                after = compacted.after_tokens,
                "Compacted history on request"
            );
            session.messages = compacted.messages;
            session.save_checkpoint().await;
            ui.send(UiCommand::Compacted(format!(
                "Summarized older history: ~{} → ~{} tokens",
                compacted.before_tokens, compacted.after_tokens
            )))
            .await;
        }
        Err(CompactHistoryError::TooShort) => {
            ui.send(system_message(
                "Nothing to summarize yet: the history is still short".to_string(),
            ))
            .await;
        }
        Err(CompactHistoryError::Cancelled) => {
            ui.send(UiCommand::ShowWarning("Summary cancelled".to_string()))
                .await;
        }
        Err(e) => {
            warn!(error = %e, "Compacting history failed");
            ui.send(UiCommand::ShowWarning(format!(
                "Could not summarize the history: {e}"
            )))
            .await;
        }
    }
    ui.send(UiCommand::Complete).await;
}

/// Clear transactions left in the agent's inbox by an earlier run.
pub(crate) async fn drain_stale_inbox(session: &AgentSession) {
    let store = session.kernel.store();
    let mut stale_count = 0;
    while let Ok(Some((token, tx))) = store.dequeue_tx(session.agent_id()) {
        warn!(
            stale_inbox_seq = token.inbox_seq(),
            stale_tx_type = ?tx.tx_type,
            "Discarding stale inbox transaction"
        );
        match session.kernel.process_dequeued(tx, token).await {
            Ok(_result) => {
                stale_count += 1;
            }
            Err(e) => {
                error!(error = %e, "Failed to clear stale transaction via kernel");
                break;
            }
        }
        if stale_count > 10 {
            error!("Too many stale transactions, aborting drain");
            break;
        }
    }
}

/// Put `tx` through the agent's inbox, returning it with its dequeue token.
fn enqueue_and_dequeue(
    session: &AgentSession,
    tx: Transaction,
) -> Result<(Transaction, aura_store_db::DequeueToken), String> {
    let store = session.kernel.store();
    if let Err(e) = store.enqueue_tx(&tx) {
        error!(error = %e, "Failed to enqueue transaction");
        return Err(format!("Storage error: {e}"));
    }

    let (token, dequeued_tx) = match store.dequeue_tx(session.agent_id()) {
        Ok(Some(item)) => item,
        Ok(None) => {
            error!("Transaction was enqueued but not found in inbox");
            return Err("Storage error: the message was not queued".to_string());
        }
        Err(e) => {
            error!(error = %e, "Failed to dequeue transaction");
            return Err(format!("Storage error: {e}"));
        }
    };

    if dequeued_tx.hash != tx.hash {
        error!("Transaction mismatch after draining stale entries");
    }

    Ok((tx, token))
}
