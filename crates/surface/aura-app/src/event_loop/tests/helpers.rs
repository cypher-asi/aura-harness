//! Helpers shared by several event loop test topics.

use super::*;

pub(super) fn open_again(fixture: &Fixture, agent_id: AgentId) -> AgentSession {
    let agent = lock_registry_agent(fixture, agent_id);
    fixture.hub.factory.open(&agent, Vec::new(), None).unwrap()
}

pub(super) fn todo(content: &str) -> aura_exec_tools::todo::TodoItem {
    aura_exec_tools::todo::TodoItem {
        id: "1".to_string(),
        content: content.to_string(),
        status: aura_exec_tools::todo::TodoStatus::InProgress,
        assignee: None,
    }
}

/// Results of the tool calls in `agent_id`'s idle session, as `is_error`.
pub(super) fn tool_results(state: &LoopState<'_>, agent_id: AgentId) -> Vec<bool> {
    state.idle[&agent_id]
        .messages
        .iter()
        .flat_map(|m| m.content.iter())
        .filter_map(|b| match b {
            aura_model::ContentBlock::ToolResult { is_error, .. } => Some(*is_error),
            _ => None,
        })
        .collect()
}

/// Run a turn of `agent_id` the way `run_prompt` checkpoints it: the
/// prompt and reply go into the session, and each `(file, content)` is
/// written through the file hook (`None` deletes the file).
pub(super) async fn checkpointed_turn(
    fixture: &Fixture,
    state: &mut LoopState<'_>,
    agent_id: AgentId,
    prompt: &str,
    edits: &[(&str, Option<&str>)],
) {
    use aura_model::Message;

    let session = state.idle.get_mut(&agent_id).unwrap();
    fixture
        .files
        .begin_turn(agent_id, session.started_ms, session.messages.len(), prompt);
    session.messages.push(Message::user(prompt));
    let hook = fixture.files.hook(vec![agent_id], None);
    for (name, content) in edits {
        let path = fixture.workspace.join(name);
        hook.before_change(&path, Some("t")).await;
        match content {
            Some(text) => std::fs::write(&path, text).unwrap(),
            None => std::fs::remove_file(&path).unwrap(),
        }
        hook.after_change(&path, Some("t"), true).await;
    }
    session
        .messages
        .push(Message::assistant(format!("did {prompt}")));
    fixture.files.end_turn(agent_id);
}

pub(super) fn system_texts(commands: &[(Option<String>, UiCommand)]) -> String {
    commands
        .iter()
        .filter_map(|(_, c)| match c {
            UiCommand::ShowMessage(m) => Some(m.content.clone()),
            UiCommand::ShowSuccess(s) | UiCommand::ShowWarning(s) => Some(s.clone()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Every entry in `agent_id`'s record, oldest first.
pub(super) fn record_of(
    state: &LoopState<'_>,
    agent_id: AgentId,
) -> Vec<aura_core_types::RecordEntry> {
    state
        .kernel_for(agent_id)
        .unwrap()
        .store()
        .scan_record(agent_id, 1, 10_000)
        .unwrap()
}

/// The prompts in `agent_id`'s record, oldest first.
pub(super) fn prompts_of(
    state: &LoopState<'_>,
    agent_id: AgentId,
) -> Vec<(u64, Hash, PromptPayload)> {
    record_of(state, agent_id)
        .into_iter()
        .filter_map(|e| e.tx.prompt_payload().map(|p| (e.seq, e.tx.hash, p)))
        .collect()
}

/// The outcomes in `agent_id`'s record, oldest first.
pub(super) fn outcomes_of(state: &LoopState<'_>, agent_id: AgentId) -> Vec<(u64, OutcomePayload)> {
    record_of(state, agent_id)
        .into_iter()
        .filter_map(|e| e.tx.outcome_payload().map(|o| (e.seq, o)))
        .collect()
}

/// Run a turn answering `text` on `agent_id` to its end.
pub(super) async fn turn(
    state: &mut LoopState<'_>,
    done: &mut mpsc::Receiver<WorkDone>,
    links: &mpsc::Sender<auth::LinkOutcome>,
    agent_id: AgentId,
    text: &str,
) -> WorkDone {
    handle_ui_event(state, message(agent_id, text, true), links).await;
    let finished = next_done(done).await;
    let reported = WorkDone {
        agent_id: finished.agent_id,
        session: None,
        failure: finished.failure.clone(),
        unseen: Vec::new(),
        refused_models: finished.refused_models.clone(),
    };
    handle_work_done(state, finished).await;
    reported
}

/// `agent` mode at `yolo`: nothing asks.
pub(super) fn yolo() -> Permissions {
    Permissions::new(PermissionMode::Agent, ApprovalLevel::Yolo)
}

/// Let `agent_id` run commands without asking.
pub(super) async fn unrestricted(
    state: &mut LoopState<'_>,
    links: &mpsc::Sender<auth::LinkOutcome>,
    agent_id: AgentId,
) {
    let event = UiEvent::SetApproval {
        agent_id: agent_id.to_hex(),
        level: ApprovalLevel::Yolo,
    };
    handle_ui_event(state, event, links).await;
}

/// Gate selected with a zOS sign-in: the router's models, no local ones.
pub(super) fn gate_on_zos(fixture: &Fixture) -> ModelSelection {
    ModelSelection::signed_in(
        &fixture.config,
        aura_model::model_catalog::GATE_MODEL_ID,
        aura_auth::AuthSummary {
            zos_user: Some("alice".to_string()),
            ..aura_auth::AuthSummary::default()
        },
    )
}

/// Feed `live` `n` question turns on `model` that passed
/// their checks, recorded by some other agent.
pub(super) fn learn_questions(live: &GateLive, model: &str, n: usize) {
    use aura_agent_kernel::RecordObserver;
    let agent = AgentId::generate();
    for i in 0..n {
        let mut prompt = PromptPayload::new(format!("earlier question {i}"));
        prompt.category = Some(PromptCategory::Question);
        prompt.route = Route::turn(model, RouteReason::Default);
        let prompt_tx = Transaction::user_prompt_with(agent, &prompt);
        let outcome = OutcomePayload {
            prompt_tx: prompt_tx.hash,
            category: PromptCategory::Question,
            model: model.to_string(),
            success: Some(true),
            signal: OutcomeSignal::ChecksPassed,
            cost_usd: 0.001,
            subtask_cost_usd: 0.0,
            latency_ms: 10,
        };
        let seq = 2 * u64::try_from(i).unwrap();
        live.on_entry(&aura_core_types::RecordEntry::builder(seq + 1, prompt_tx).build());
        live.on_entry(
            &aura_core_types::RecordEntry::builder(seq + 2, Transaction::outcome(agent, &outcome))
                .build(),
        );
    }
}

/// The chat lines saying where Gate sent turns.
pub(super) fn route_lines(commands: &mut mpsc::Receiver<UiCommand>) -> Vec<String> {
    drain(commands)
        .into_iter()
        .filter_map(|(_, c)| match c {
            UiCommand::ShowMessage(m) if m.content.contains("gate → ") => Some(m.content),
            _ => None,
        })
        .collect()
}

/// Text of the user messages in `agent_id`'s idle session.
pub(super) fn user_texts(state: &LoopState<'_>, agent_id: AgentId) -> Vec<String> {
    state.idle[&agent_id]
        .messages
        .iter()
        .filter(|m| m.role == aura_model::Role::User)
        .map(aura_model::Message::text_content)
        .collect()
}
