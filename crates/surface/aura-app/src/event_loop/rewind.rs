//! `/rewind` and `/diff`: undo an agent's turns and review what they
//! changed, from the file checkpoints taken before each turn changed a file
//! (see [`crate::file_checkpoints`]). A rewound turn counts as failed in
//! Gate's record.

use super::agent_events::AgentUi;
use super::agents::todos_from_history;
use super::handlers::system_message;
use super::hud::send_session;
use super::LoopState;
use crate::file_checkpoints::{FileCheckpoints, RestoreReport, Turn, TurnManifest};
use crate::record_loader::chat_from_messages;
use aura_core_types::{AgentId, Hash, SystemKind, Transaction, TransactionType};
use aura_model::{Message, Role};
use aura_ui_terminal::events::RewindScope;
use aura_ui_terminal::UiCommand;
use std::path::PathBuf;
use tracing::{info, warn};

/// Said wherever files are put back: the harness only sees file tools.
const SHELL_NOTE: &str =
    "Changes made by shell commands (run_command, typed commands) are not undone.";

/// `/rewind`: list `agent_id`'s turns, or put its files and/or conversation
/// back to before `turn`.
pub(super) async fn handle_rewind(
    state: &mut LoopState<'_>,
    agent_id: AgentId,
    turn: Option<usize>,
    scope: RewindScope,
) {
    let ui = AgentUi::new(state.commands.clone(), agent_id);
    let Some(files) = state.hub.factory.file_checkpoints.clone() else {
        ui.send(UiCommand::ShowWarning(
            "File checkpoints are off in this terminal".to_string(),
        ))
        .await;
        return;
    };
    let Some(session) = state.idle.get_mut(&agent_id) else {
        ui.send(UiCommand::ShowWarning(
            "Wait for this agent to finish (or press Esc) before rewinding".to_string(),
        ))
        .await;
        return;
    };
    let turns = blocking({
        let files = files.clone();
        let started_ms = session.started_ms;
        move || files.turns(agent_id, started_ms)
    })
    .await
    .unwrap_or_default();
    let Some(number) = turn else {
        ui.send(system_message(turn_list(&turns))).await;
        return;
    };
    let Some(first) = turns.iter().position(|t| t.number == number) else {
        ui.send(UiCommand::ShowWarning(format!(
            "No turn {number}; /rewind lists them"
        )))
        .await;
        return;
    };
    let undone = turns[first..].to_vec();

    // Find the cut before touching any file, so a conversation that can't
    // be rewound leaves the files alone too.
    let cut = if scope.chat() {
        match prompt_position(&session.messages, &undone[0].manifest) {
            Some(cut) => Some(cut),
            None => {
                ui.send(UiCommand::ShowWarning(format!(
                    "Turn {number} is no longer in the conversation the model sees (it was \
                     summarized); use /rewind {number} code to put back only the files"
                )))
                .await;
                return;
            }
        }
    } else {
        None
    };

    let report = if scope.code() {
        blocking({
            let files = files.clone();
            let undone = undone.clone();
            move || files.restore(agent_id, &undone)
        })
        .await
    } else {
        None
    };
    if let Some(cut) = cut {
        session.messages.truncate(cut);
        session.todos = todos_from_history(&session.messages);
        blocking({
            let files = files.clone();
            let undone = undone.clone();
            move || files.mark_rewound(agent_id, &undone)
        })
        .await;
    }

    info!(%agent_id, turn = number, scope = scope.as_str(), "Rewound agent");
    record_rewind(session, number, scope, &undone[0].manifest, report.as_ref()).await;
    let prompts: Vec<Hash> = undone
        .iter()
        .filter_map(|turn| turn.manifest.prompt_tx.as_deref())
        .filter_map(|hex| Hash::from_hex(hex).ok())
        .collect();
    aura_gate::live::note_rewind(&session.kernel, session.agent_id(), &prompts).await;
    if cut.is_some() {
        session.save_checkpoint().await;
        ui.send(UiCommand::LoadConversation(chat_from_messages(
            &session.messages,
        )))
        .await;
        send_session(&ui, session).await;
    }
    ui.send(UiCommand::ShowSuccess(rewind_summary(
        number,
        scope,
        report.as_ref(),
    )))
    .await;
    if let Some(report) = &report {
        let mut notes = vec![SHELL_NOTE.to_string()];
        notes.extend(
            report
                .failed
                .iter()
                .map(|(path, why)| format!("Could not restore {}: {why}", path.display())),
        );
        ui.send(system_message(notes.join("\n"))).await;
    }
}

/// `/diff`: what `agent_id`'s latest turn changed in files, or with
/// `session` everything its session changed.
pub(super) async fn handle_diff(state: &LoopState<'_>, agent_id: AgentId, session: bool) {
    let ui = AgentUi::new(state.commands.clone(), agent_id);
    let Some(files) = state.hub.factory.file_checkpoints.clone() else {
        ui.send(UiCommand::ShowWarning(
            "File checkpoints are off in this terminal".to_string(),
        ))
        .await;
        return;
    };
    let started_ms = state
        .idle
        .get(&agent_id)
        .map(|s| s.started_ms)
        .or_else(|| files.open_session(agent_id));
    let Some(started_ms) = started_ms else {
        ui.send(system_message("No turns in this session yet".to_string()))
            .await;
        return;
    };
    let workspace = state.kernel_for(agent_id).map(|k| k.agent_workspace());
    let shown = blocking(move || session_diff(&files, agent_id, started_ms, session, workspace))
        .await
        .flatten();
    let command = match shown {
        Some((title, diffs)) if diffs.is_empty() => system_message(format!("{title}: no files")),
        Some((title, diffs)) => UiCommand::ShowDiff {
            title,
            files: diffs,
        },
        None => system_message("No turns in this session yet".to_string()),
    };
    ui.send(command).await;
}

/// A title and the file diffs for `/diff`; `None` when the session has no
/// turns.
fn session_diff(
    files: &FileCheckpoints,
    agent_id: AgentId,
    started_ms: u64,
    whole_session: bool,
    workspace: Option<PathBuf>,
) -> Option<(String, Vec<aura_ui_terminal::events::FileDiffData>)> {
    let workspace = workspace.as_deref();
    if whole_session {
        let manifests = files.session_manifests(agent_id, started_ms);
        if manifests.is_empty() {
            return None;
        }
        let diffs = files.diff(agent_id, &manifests, workspace);
        return Some(("Files changed since the session started".to_string(), diffs));
    }
    let latest = files.turns(agent_id, started_ms).pop()?;
    let title = format!(
        "Files changed by turn {} · {}",
        latest.number, latest.manifest.prompt
    );
    let diffs = files.diff(agent_id, [&latest.manifest], workspace);
    Some((title, diffs))
}

/// Where turn `manifest`'s prompt is in `messages`: where it was put, or
/// the last copy before that if the history has shifted since.
fn prompt_position(messages: &[Message], manifest: &TurnManifest) -> Option<usize> {
    let is_prompt = |m: &Message| m.role == Role::User && manifest.is_prompt(&m.text_content());
    if messages.get(manifest.message_index).is_some_and(is_prompt) {
        return Some(manifest.message_index);
    }
    let end = manifest.message_index.min(messages.len());
    messages[..end].iter().rposition(is_prompt)
}

/// Note the rewind in the agent's record.
async fn record_rewind(
    session: &super::AgentSession,
    number: usize,
    scope: RewindScope,
    manifest: &TurnManifest,
    report: Option<&RestoreReport>,
) {
    let shown = |paths: Vec<&PathBuf>| -> Vec<String> {
        paths.iter().map(|p| p.display().to_string()).collect()
    };
    let payload = serde_json::json!({
        "system_kind": SystemKind::Rewind,
        "action": "rewind",
        "scope": scope.as_str(),
        "turn": number,
        "turn_started_ms": manifest.started_ms,
        "prompt": manifest.prompt,
        "restored": report.map(|r| shown(r.restored.iter().collect())),
        "failed": report.map(|r| shown(r.failed.iter().map(|(p, _)| p).collect())),
    });
    let Ok(bytes) = serde_json::to_vec(&payload) else {
        warn!("Failed to serialize rewind payload");
        return;
    };
    let tx = Transaction::new_chained(session.agent_id(), TransactionType::System, bytes, None);
    if let Err(e) = session.kernel.process_direct(tx).await {
        warn!(error = %e, "Failed to record rewind via kernel");
    }
}

/// `Rewound to before turn 3: restored 2 files and the conversation`.
fn rewind_summary(number: usize, scope: RewindScope, report: Option<&RestoreReport>) -> String {
    let mut parts = Vec::new();
    if let Some(report) = report {
        parts.push(match report.restored.len() {
            0 => "no files to restore".to_string(),
            1 => "restored 1 file".to_string(),
            n => format!("restored {n} files"),
        });
    }
    if scope.chat() {
        parts.push("the conversation".to_string());
    }
    format!("Rewound to before turn {number}: {}", parts.join(" and "))
}

/// The `/rewind` list, newest first.
fn turn_list(turns: &[Turn]) -> String {
    if turns.is_empty() {
        return "No turns in this session yet".to_string();
    }
    let mut out = String::from(
        "Turns of this session, newest first. /rewind <n> [code|chat|both] undoes turn n \
         and everything after it (default both):",
    );
    for turn in turns.iter().rev() {
        let files = match turn.manifest.files.len() {
            0 => "no files".to_string(),
            1 => "1 file".to_string(),
            n => format!("{n} files"),
        };
        out.push_str(&format!(
            "\n  {}. {} · {} · {files}",
            turn.number,
            format_time(turn.manifest.started_ms),
            turn.manifest.prompt
        ));
    }
    out.push('\n');
    out.push_str(SHELL_NOTE);
    out
}

/// `14:03` in local time.
fn format_time(ts_ms: u64) -> String {
    i64::try_from(ts_ms)
        .ok()
        .and_then(chrono::DateTime::from_timestamp_millis)
        .map_or_else(
            || "?".to_string(),
            |dt| dt.with_timezone(&chrono::Local).format("%H:%M").to_string(),
        )
}

/// Run file work off the event loop; `None` (logged) if it panicked.
async fn blocking<T: Send + 'static>(work: impl FnOnce() -> T + Send + 'static) -> Option<T> {
    match tokio::task::spawn_blocking(work).await {
        Ok(value) => Some(value),
        Err(e) => {
            warn!(error = %e, "File checkpoint task failed");
            None
        }
    }
}
