//! Loading and displaying historical records from the store.

use crate::agent_registry::AgentRegistry;
use crate::event_loop::run_command::is_user_tool_use;
use crate::turn_calls::{Seen, TurnCalls};
use aura_agent_kernel::decode_tool_effect;
use aura_core_types::{
    ActionResultPayload, AgentId, EffectStatus, PlanUsage, PolicyApprover, PolicyCheckOutcome,
    PolicyDecision, RecordEntry, ToolExecution, ToolGateVerdict, ToolProposal, Transaction,
    TransactionType,
};
use aura_exec_tools::plan_tools::{plan_path_from_output, CREATE_PLAN_TOOL_NAME};
use aura_exec_tools::todo::TODO_WRITE_TOOL_NAME;
use aura_gate::compare::ModelCall;
use aura_model::{Message, ModelCallPurpose, Usage};
use aura_store_record::{ReadStore, StoreError};
use aura_ui_terminal::{
    events::{
        ActivityData, AgentSummary, MessageData, MessageRole, PolicyDetail, ProjectSummary,
        RecordStatus, RecordSummary, MAX_RECORDS,
    },
    UiCommand,
};
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use tokio::sync::mpsc;
use tracing::warn;

/// Number of entries loaded into the Record panel: as many as it holds.
const RECORD_PANEL_LIMIT: usize = MAX_RECORDS;

/// Number of trailing entries scanned when rebuilding an agent's chat.
const HISTORY_SCAN_LIMIT: usize = 1000;

/// Number of chat messages kept after rebuilding, activity blocks included
/// (matches the UI cap).
const HISTORY_MESSAGE_LIMIT: usize = 300;

/// Upper bound on the summary string; the panel truncates to its own
/// width, this only keeps huge prompts from being copied around.
const MAX_INFO_CHARS: usize = 240;

/// Upper bound on a plain-text payload shown in the record detail view.
const MAX_MESSAGE_CHARS: usize = 200;

/// Upper bound on a pretty-printed JSON payload; the detail view scrolls,
/// so this only guards against huge tool payloads.
const MAX_JSON_MESSAGE_CHARS: usize = 20_000;

/// Load the most recent record entries for an agent as UI summaries,
/// oldest first.
///
/// # Errors
/// Returns the store error if the head sequence or scan fails.
pub fn recent_record_summaries(
    store: &dyn ReadStore,
    agent_id: AgentId,
) -> Result<Vec<RecordSummary>, StoreError> {
    Ok(scan_tail(store, agent_id, RECORD_PANEL_LIMIT)?
        .iter()
        .map(|entry| {
            build_record_summary(
                entry.seq,
                &entry.tx,
                entry,
                format_local_time(entry.tx.ts_ms),
            )
        })
        .collect())
}

/// Load existing records from the store and send to UI.
///
/// Uses `try_send`, so it is only suitable before the UI starts draining
/// the channel (startup); async callers should send summaries themselves.
/// The history goes as one [`UiCommand::SetRecords`], so its length does
/// not count against the channel's capacity.
pub fn load_existing_records(
    store: &dyn ReadStore,
    agent_id: AgentId,
    commands: &mpsc::Sender<UiCommand>,
) {
    match recent_record_summaries(store, agent_id) {
        Ok(summaries) => {
            if let Err(e) = commands.try_send(UiCommand::SetRecords(summaries)) {
                warn!(error = %e, "Failed to queue record history for the UI");
            }
        }
        Err(e) => {
            warn!(error = %e, "Failed to load records");
            let _ = commands.try_send(UiCommand::ShowWarning(format!(
                "Could not load record history: {e}"
            )));
        }
    }
}

/// Rebuild the conversation the model sees from an agent's record: its
/// prompts and replies since the last session start, oldest first. Tool
/// calls are left out; their `tool_use`/`tool_result` pairs can't be
/// replayed safely, and the replies already describe what they did.
///
/// # Errors
/// Returns the store error if the head sequence or scan fails.
pub fn model_history(store: &dyn ReadStore, agent_id: AgentId) -> Result<Vec<Message>, StoreError> {
    let mut messages = Vec::new();
    for entry in scan_tail(store, agent_id, HISTORY_SCAN_LIMIT)? {
        push_model_message(&mut messages, &entry.tx);
    }
    Ok(messages)
}

/// The prompts and replies recorded after `after_seq`, for replaying on
/// top of a checkpoint taken at `after_seq`. `None` when the checkpoint
/// no longer applies: a session started since, or too much happened to
/// scan.
///
/// # Errors
/// Returns the store error if the head sequence or scan fails.
pub fn model_history_since(
    store: &dyn ReadStore,
    agent_id: AgentId,
    after_seq: u64,
) -> Result<Option<Vec<Message>>, StoreError> {
    let head_seq = store.get_head_seq(agent_id)?;
    if head_seq < after_seq {
        return Ok(None);
    }
    let newer = usize::try_from(head_seq - after_seq).unwrap_or(usize::MAX);
    if newer > HISTORY_SCAN_LIMIT {
        return Ok(None);
    }
    let mut messages = Vec::new();
    if newer == 0 {
        return Ok(Some(messages));
    }
    for entry in store.scan_record(agent_id, after_seq + 1, newer)? {
        if entry.tx.tx_type == TransactionType::SessionStart {
            return Ok(None);
        }
        push_model_message(&mut messages, &entry.tx);
    }
    Ok(Some(messages))
}

/// Add what `tx` contributes to the model's view of the conversation.
fn push_model_message(messages: &mut Vec<Message>, tx: &Transaction) {
    let text = || String::from_utf8_lossy(&tx.payload).trim().to_string();
    match tx.tx_type {
        TransactionType::SessionStart => messages.clear(),
        TransactionType::UserPrompt => {
            let text = prompt_text(tx).trim().to_string();
            if !text.is_empty() {
                messages.push(Message::user(text));
            }
        }
        TransactionType::AgentMsg => {
            let text = text();
            if !text.is_empty() {
                messages.push(Message::assistant(text));
            }
        }
        _ => {}
    }
}

/// A chat transcript of `messages`: their text, and each tool call with
/// its result.
#[must_use]
pub fn chat_from_messages(messages: &[Message]) -> Vec<MessageData> {
    use aura_model::{ContentBlock, Role, ToolResultContent};

    let mut chat: Vec<MessageData> = Vec::new();
    let mut tool_calls: HashMap<String, usize> = HashMap::new();
    for message in messages {
        let role = match message.role {
            Role::User => MessageRole::User,
            Role::Assistant => MessageRole::Assistant,
        };
        for block in &message.content {
            match block {
                ContentBlock::Text { text } if !text.trim().is_empty() => {
                    chat.push(MessageData::new(role, text));
                }
                ContentBlock::ToolUse { id, name, input } => {
                    tool_calls.insert(id.clone(), chat.len());
                    chat.push(MessageData {
                        activity: Some(ActivityData::Tool {
                            id: id.clone(),
                            name: name.clone(),
                            args: input.to_string(),
                            result: None,
                            is_error: false,
                            duration_ms: None,
                            policy: None,
                        }),
                        ..MessageData::new(MessageRole::Assistant, "")
                    });
                }
                ContentBlock::ToolResult {
                    tool_use_id,
                    content,
                    is_error,
                } => {
                    let Some(&index) = tool_calls.get(tool_use_id) else {
                        continue;
                    };
                    if let Some(ActivityData::Tool {
                        result,
                        is_error: failed,
                        ..
                    }) = &mut chat[index].activity
                    {
                        *result = Some(match content {
                            ToolResultContent::Text(text) => text.clone(),
                            ToolResultContent::Json(value) => value.to_string(),
                        });
                        *failed = *is_error;
                    }
                }
                _ => {}
            }
        }
    }
    let skip = chat.len().saturating_sub(HISTORY_MESSAGE_LIMIT);
    chat.into_iter().skip(skip).collect()
}

/// Rebuild an agent's chat transcript from its record, oldest first.
///
/// # Errors
/// Returns the store error if the head sequence or scan fails.
pub fn conversation_history(
    store: &dyn ReadStore,
    agent_id: AgentId,
) -> Result<Vec<MessageData>, StoreError> {
    let mut history: Vec<MessageData> = Vec::new();
    // Tool use id -> (index in `history`, proposal time)
    let mut tool_calls: HashMap<String, (usize, u64)> = HashMap::new();
    // The models of the current turn, and where its replies are in `history`
    let mut calls = TurnCalls::default();
    let mut replies: Vec<usize> = Vec::new();
    for entry in scan_tail(store, agent_id, HISTORY_SCAN_LIMIT)? {
        let tx = &entry.tx;
        match calls.see(tx) {
            Seen::NewTurn => replies.clear(),
            Seen::Call => {
                let footer = calls.footer();
                for &at in &replies {
                    set_footer(&mut history[at], footer.clone());
                }
            }
            Seen::Other => {}
        }
        match tx.tx_type {
            TransactionType::UserPrompt => {
                history.extend(history_message(tx));
            }
            TransactionType::Reasoning => {
                history.extend(thinking_message(tx));
            }
            TransactionType::AgentMsg => {
                if let Some(mut message) = history_message(tx) {
                    set_footer(&mut message, calls.footer());
                    replies.push(history.len());
                    history.push(message);
                }
            }
            TransactionType::ToolProposal if !is_user_proposal(tx) => {
                match serde_json::from_slice::<ToolProposal>(&tx.payload) {
                    Ok(proposal) => {
                        tool_calls.insert(proposal.tool_use_id.clone(), (history.len(), tx.ts_ms));
                        history.push(tool_call_message(&proposal, tx.ts_ms));
                    }
                    Err(_) => history.extend(history_message(tx)),
                }
            }
            TransactionType::PolicyDecision => {
                let decision = serde_json::from_slice::<PolicyDecision>(&tx.payload).ok();
                let call = decision
                    .as_ref()
                    .and_then(|decision| tool_calls.get(&decision.tool_use_id));
                match (call, decision.as_ref().and_then(policy_outcome)) {
                    (Some(&(index, _)), Some(outcome)) => {
                        set_tool_policy(&mut history[index], outcome);
                    }
                    _ => history.extend(history_message(tx)),
                }
            }
            TransactionType::ToolExecution => {
                let Some(execution) = tool_execution(&entry) else {
                    continue;
                };
                if let Some(&(index, proposed_at)) = tool_calls.get(&execution.tool_use_id) {
                    finish_tool_call(
                        &mut history[index],
                        &execution,
                        tx.ts_ms.saturating_sub(proposed_at),
                    );
                }
            }
            _ => history.extend(history_message(tx)),
        }
    }
    let skip = history.len().saturating_sub(HISTORY_MESSAGE_LIMIT);
    Ok(history.into_iter().skip(skip).collect())
}

/// Put the footer of a reply's turn, its models and their usage, on `reply`.
fn set_footer(reply: &mut MessageData, footer: Option<crate::turn_calls::Footer>) {
    match footer {
        Some(footer) => {
            reply.model = Some(footer.models);
            reply.usage = footer.usage;
            reply.sources = footer.sources;
        }
        None => {
            reply.model = None;
            reply.usage = None;
            reply.sources = Vec::new();
        }
    }
}

/// The model that answered the call a `Reasoning` entry records.
fn reasoning_model(payload: &Value) -> Option<String> {
    recorded_model(payload, "model")
}

/// The model id named under `key` in a `Reasoning` payload, unless it is
/// empty or `unknown`.
fn recorded_model(payload: &Value, key: &str) -> Option<String> {
    payload
        .get(key)
        .and_then(Value::as_str)
        .filter(|model| !model.is_empty() && *model != "unknown")
        .map(str::to_string)
}

fn call_tokens(payload: &Value) -> Usage {
    let tokens = |key: &str| payload.get(key).and_then(Value::as_u64);
    Usage {
        input_tokens: tokens("input_tokens").unwrap_or(0),
        output_tokens: tokens("output_tokens").unwrap_or(0),
        cache_creation_input_tokens: tokens("cache_creation_input_tokens"),
        cache_read_input_tokens: tokens("cache_read_input_tokens"),
    }
}

/// The call `tx` records, when it is a model call that got an answer:
/// the model that answered, the id it was asked for (when the record
/// kept it), what it was made for and its tokens.
pub(crate) fn model_call(tx: &Transaction) -> Option<ModelCall> {
    if tx.tx_type != TransactionType::Reasoning {
        return None;
    }
    let payload = json_payload(tx)?;
    Some(ModelCall {
        model: reasoning_model(&payload)?,
        route: recorded_model(&payload, "route"),
        purpose: recorded_purpose(&payload),
        usage: call_tokens(&payload),
    })
}

/// What the model call `tx` records was made for, when it was made
/// beside the agent's own work.
pub(crate) fn call_purpose(tx: &Transaction) -> Option<ModelCallPurpose> {
    if tx.tx_type != TransactionType::Reasoning {
        return None;
    }
    recorded_purpose(&json_payload(tx)?)
}

/// The purpose a `Reasoning` payload names, unless it names none this
/// build knows.
fn recorded_purpose(payload: &Value) -> Option<ModelCallPurpose> {
    payload
        .get("purpose")
        .cloned()
        .and_then(|purpose| serde_json::from_value(purpose).ok())
}

/// The plan usage the call `tx` records, when it is a model call whose
/// provider reported any; a payload that does not read is none.
pub(crate) fn plan_usage(tx: &Transaction) -> Vec<PlanUsage> {
    if tx.tx_type != TransactionType::Reasoning {
        return Vec::new();
    }
    json_payload(tx)
        .and_then(|payload| payload.get("plan_usage").cloned())
        .and_then(|value| serde_json::from_value(value).ok())
        .unwrap_or_default()
}

/// The model's thinking from a `Reasoning` entry, when it thought.
fn thinking_message(tx: &Transaction) -> Option<MessageData> {
    let payload = json_payload(tx)?;
    let text = payload.get("thinking").and_then(Value::as_str)?;
    if text.trim().is_empty() {
        return None;
    }
    Some(MessageData {
        timestamp_ms: Some(tx.ts_ms),
        activity: Some(ActivityData::Thinking {
            text: text.to_string(),
            duration_ms: payload.get("thinking_ms").and_then(Value::as_u64),
        }),
        ..MessageData::new(MessageRole::Assistant, "")
    })
}

/// A tool call the model proposed, without its outcome yet.
fn tool_call_message(proposal: &ToolProposal, ts_ms: u64) -> MessageData {
    MessageData {
        timestamp_ms: Some(ts_ms),
        activity: Some(ActivityData::Tool {
            id: proposal.tool_use_id.clone(),
            name: proposal.tool.clone(),
            args: proposal.args.to_string(),
            result: None,
            is_error: false,
            duration_ms: None,
            policy: None,
        }),
        ..MessageData::new(MessageRole::Assistant, "")
    }
}

/// Record the policy decision on the tool call `message` shows.
fn set_tool_policy(message: &mut MessageData, decision: String) {
    if let Some(ActivityData::Tool { policy, .. }) = &mut message.activity {
        *policy = Some(decision);
    }
}

/// Fill in a tool call's outcome from its execution entry.
fn finish_tool_call(message: &mut MessageData, execution: &ToolExecution, took_ms: u64) {
    let Some(ActivityData::Tool {
        result,
        is_error,
        duration_ms,
        ..
    }) = &mut message.activity
    else {
        return;
    };
    *is_error = execution.is_error || execution.decision != ToolGateVerdict::Approved;
    *result = match execution.decision {
        ToolGateVerdict::Approved => execution.result.clone(),
        ToolGateVerdict::Denied => Some(execution.reason.as_deref().map_or_else(
            || "denied".to_string(),
            |reason| format!("denied: {reason}"),
        )),
        ToolGateVerdict::PendingApproval => Some("awaiting approval".to_string()),
    };
    *duration_ms = Some(execution.duration_ms.unwrap_or(took_ms));
}

/// The tool execution in `entry`, with its output read from the entry's
/// effects (the kernel no longer copies it into the payload).
pub(crate) fn tool_execution(entry: &RecordEntry) -> Option<ToolExecution> {
    let mut execution = serde_json::from_slice::<ToolExecution>(&entry.tx.payload).ok()?;
    if execution.result.is_none() {
        if let Some(effect) = entry.effects.first() {
            let decoded = decode_tool_effect(effect);
            execution.is_error |= decoded.is_error;
            execution.result = Some(decoded.content);
        }
    }
    Some(execution)
}

/// Path of the plan `agent_id` last wrote with `create_plan` or
/// `todo_write`, from its recent record.
#[must_use]
pub fn latest_plan_path(store: &dyn ReadStore, agent_id: AgentId) -> Option<String> {
    let entries = scan_tail(store, agent_id, HISTORY_SCAN_LIMIT)
        .inspect_err(|e| warn!(error = %e, "Failed to scan record for plans"))
        .ok()?;
    entries
        .iter()
        .rev()
        .filter(|entry| entry.tx.tx_type == TransactionType::ToolExecution)
        .filter_map(tool_execution)
        .filter(|execution| {
            !execution.is_error
                && execution.decision == ToolGateVerdict::Approved
                && matches!(
                    execution.tool.as_str(),
                    CREATE_PLAN_TOOL_NAME | TODO_WRITE_TOOL_NAME
                )
        })
        .find_map(|execution| {
            execution
                .result
                .as_deref()
                .and_then(plan_path_from_output)
                .map(str::to_string)
        })
}

/// Archive spawns from an earlier run of the app that never got to record
/// anything: the app closed or was killed while a `task` call was still
/// being dispatched, or while a writer waited for the workspace. Nothing
/// was recorded for them, so there is no prompt or turn to resume; left
/// alone they sit in the Agents panel with a chat that stays empty.
///
/// Only agents a `task` call spawned (`parent` is set) are candidates: a
/// user-created project or agent legitimately has an empty record before
/// its first message. Run it before reading [`AgentRegistry::active`] at
/// startup, which skips archived agents, so the app never opens on one it
/// just archived. Returns the agents archived.
pub fn archive_unstarted_spawns(
    registry: &mut AgentRegistry,
    store: &dyn ReadStore,
) -> Vec<AgentId> {
    let candidates: Vec<AgentId> = registry
        .agents()
        .iter()
        .filter(|agent| !agent.archived && agent.parent.is_some())
        .map(crate::agent_registry::RegisteredAgent::agent_id)
        .filter(|&agent_id| matches!(store.get_head_seq(agent_id), Ok(0)))
        .collect();
    let mut archived = Vec::with_capacity(candidates.len());
    for agent_id in candidates {
        // Best-effort: say why before archiving, but archive either way.
        if let Err(e) = registry.set_title(agent_id, "Did not start before the app closed") {
            warn!(%agent_id, error = %e, "Could not note an unstarted spawn");
        }
        match registry.archive(agent_id) {
            Ok(()) => {
                tracing::info!(%agent_id, "Archived a spawn an earlier run never started");
                archived.push(agent_id);
            }
            Err(e) => warn!(%agent_id, error = %e, "Could not archive an unstarted spawn"),
        }
    }
    archived
}

/// Rows of the agents column, the Archive and the projects list, with the
/// store not read yet: built under the registry lock, which
/// [`ListRows::read_activity`] does not need.
pub(crate) struct ListRows {
    pub(crate) projects: Vec<ProjectSummary>,
    pub(crate) listed: Vec<(AgentId, AgentSummary)>,
    pub(crate) archived: Vec<(AgentId, AgentSummary)>,
    /// Rows of channel members, taken out of `listed` by
    /// [`ListRows::split_members`] for the channels list
    pub(crate) members: Vec<(AgentId, AgentSummary)>,
    /// The agent the rows mark active
    pub(crate) active: AgentId,
}

impl ListRows {
    /// The rows of every list, in one walk of each. `running` holds the
    /// agents currently working on a task for their parent.
    pub(crate) fn build(registry: &AgentRegistry, running: &HashSet<AgentId>) -> Self {
        let active = registry.active().agent_id();
        let mut names = ProjectNames::default();
        let listed = rows(registry, registry.tree(), active, running, &mut names);
        let none = HashSet::new();
        let unknown = AgentId::new([0; 32]);
        let archived = rows(
            registry,
            registry.archived_tree(),
            unknown,
            &none,
            &mut names,
        );
        let count = |rows: &[(AgentId, AgentSummary)], project: &str| {
            rows.iter()
                .filter(|(_, row)| row.project_id.as_deref() == Some(project))
                .count()
        };
        let projects = registry
            .listed_projects()
            .into_iter()
            .map(|project| {
                project_summary(
                    project,
                    count(&listed, &project.id),
                    count(&archived, &project.id),
                )
            })
            .collect();
        Self {
            projects,
            listed,
            archived,
            members: Vec::new(),
            active,
        }
    }

    /// Move the agents in `members` (channel members) out of the agents
    /// column into [`ListRows::members`], and leave the projects that only
    /// they were listed in out of the projects list. A project's archived
    /// count is left as it is.
    pub(crate) fn split_members(&mut self, members: &HashSet<AgentId>) {
        if members.is_empty() {
            return;
        }
        let mut removed: HashMap<String, usize> = HashMap::new();
        let (taken, kept) = std::mem::take(&mut self.listed)
            .into_iter()
            .partition(|(agent_id, _)| members.contains(agent_id));
        self.listed = kept;
        for (_, row) in &taken {
            if let Some(project) = &row.project_id {
                *removed.entry(project.clone()).or_default() += 1;
            }
        }
        self.members = taken;
        self.projects.retain_mut(|project| {
            let Some(&gone) = removed.get(&project.id) else {
                return true;
            };
            project.listed_agents = project.listed_agents.saturating_sub(gone);
            project.listed_agents > 0
        });
    }

    /// Fill in each row's record count and last activity from `store`,
    /// reusing what `cache` saw for agents with no new entries.
    pub(crate) fn read_activity(&mut self, store: &dyn ReadStore, cache: &mut LastActive) {
        for (agent_id, row) in self
            .listed
            .iter_mut()
            .chain(self.archived.iter_mut())
            .chain(self.members.iter_mut())
        {
            set_activity(row, cache.of(store, *agent_id));
        }
    }

    /// The commands that send the lists, in the order the UI takes them:
    /// the projects first, so one with every agent archived is known when
    /// the rows arrive.
    pub(crate) fn into_commands(self) -> [UiCommand; 3] {
        let strip = |rows: Vec<(AgentId, AgentSummary)>| -> Vec<AgentSummary> {
            rows.into_iter().map(|(_, row)| row).collect()
        };
        [
            UiCommand::SetProjects(self.projects),
            UiCommand::SetAgents(strip(self.listed)),
            UiCommand::SetArchivedAgents(strip(self.archived)),
        ]
    }
}

fn set_activity(row: &mut AgentSummary, (record_count, last_active_ms): (u64, Option<u64>)) {
    row.record_count = record_count;
    row.last_active_ms = last_active_ms;
    row.last_active = last_active_ms.map_or_else(|| "-".to_string(), format_local_time);
}

/// When each agent was last active, by the head of its record: the latest
/// entry, which is decoded whole, is only read again once the head moves.
#[derive(Debug, Default)]
pub struct LastActive(HashMap<AgentId, (u64, Option<u64>)>);

impl LastActive {
    /// `agent_id`'s record count and the time of its latest entry.
    pub fn of(&mut self, store: &dyn ReadStore, agent_id: AgentId) -> (u64, Option<u64>) {
        let head = store.get_head_seq(agent_id).unwrap_or(0);
        if head == 0 {
            self.0.remove(&agent_id);
            return (0, None);
        }
        if let Some(&(seen, ts)) = self.0.get(&agent_id) {
            if seen == head {
                return (head, ts);
            }
        }
        let ts = store
            .get_record_entry(agent_id, head)
            .ok()
            .map(|entry| entry.tx.ts_ms);
        self.0.insert(agent_id, (head, ts));
        (head, ts)
    }
}

/// Build the agents-column rows for every registered agent in the order of
/// [`AgentRegistry::tree`]. `running` holds the agents currently working
/// on a task for their parent.
#[must_use]
pub fn agent_summaries(
    registry: &AgentRegistry,
    store: &dyn ReadStore,
    active: AgentId,
    running: &HashSet<AgentId>,
) -> Vec<AgentSummary> {
    let mut names = ProjectNames::default();
    let rows = rows(registry, registry.tree(), active, running, &mut names);
    with_activity(rows, store)
}

/// Rows for the Archive. Each archived agent's `project` names the project
/// it was in, so the row can say where it came from.
#[must_use]
pub fn archived_summaries(registry: &AgentRegistry, store: &dyn ReadStore) -> Vec<AgentSummary> {
    let none = HashSet::new();
    let unknown = AgentId::new([0; 32]);
    let mut names = ProjectNames::default();
    let rows = rows(
        registry,
        registry.archived_tree(),
        unknown,
        &none,
        &mut names,
    );
    with_activity(rows, store)
}

fn with_activity(rows: Vec<(AgentId, AgentSummary)>, store: &dyn ReadStore) -> Vec<AgentSummary> {
    let mut cache = LastActive::default();
    rows.into_iter()
        .map(|(agent_id, mut row)| {
            set_activity(&mut row, cache.of(store, agent_id));
            row
        })
        .collect()
}

/// Rows of the listed projects, in list order.
#[must_use]
pub fn project_summaries(registry: &AgentRegistry) -> Vec<ProjectSummary> {
    let counts = registry.project_counts_all();
    registry
        .listed_projects()
        .into_iter()
        .map(|project| {
            let (listed, archived) = counts.get(&project.id).copied().unwrap_or_default();
            project_summary(project, listed, archived)
        })
        .collect()
}

fn project_summary(
    project: &crate::agent_registry::Project,
    listed_agents: usize,
    archived_agents: usize,
) -> ProjectSummary {
    ProjectSummary {
        id: project.id.clone(),
        name: project.name(),
        folder: project.folder.display().to_string(),
        folder_exists: project.folder.is_dir(),
        created: project
            .created_at
            .with_timezone(&chrono::Local)
            .format("%Y-%m-%d %H:%M")
            .to_string(),
        listed_agents,
        archived_agents,
    }
}

/// Names of projects by id, each worked out once per list.
#[derive(Default)]
struct ProjectNames(HashMap<String, Option<String>>);

impl ProjectNames {
    fn of(&mut self, registry: &AgentRegistry, project_id: Option<&str>) -> Option<String> {
        let id = project_id?;
        if let Some(name) = self.0.get(id) {
            return name.clone();
        }
        let name = registry
            .project(id)
            .map(crate::agent_registry::Project::name);
        self.0.insert(id.to_string(), name.clone());
        name
    }
}

/// Rows for `tree`, a walk of [`AgentRegistry::tree`] or
/// [`AgentRegistry::archived_tree`], without the store's fields.
///
/// A row below the top has as parent the nearest row above it one level
/// up, so whether a `/compare` holds it carries down the walk instead of
/// being looked up for each agent; rows at the top look it up.
fn rows(
    registry: &AgentRegistry,
    tree: Vec<(&crate::agent_registry::RegisteredAgent, usize)>,
    active: AgentId,
    running: &HashSet<AgentId>,
    names: &mut ProjectNames,
) -> Vec<(AgentId, AgentSummary)> {
    // Per depth: whether the agents below the row there are under a
    // comparison.
    let mut below_compare: Vec<bool> = Vec::new();
    tree.into_iter()
        .map(|(agent, depth)| {
            let agent_id = agent.agent_id();
            let above = match depth.checked_sub(1) {
                Some(up) => below_compare.get(up).copied().unwrap_or(false),
                None => registry
                    .ancestry(agent_id)
                    .into_iter()
                    .skip(1)
                    .any(|id| registry.find(id).is_some_and(|a| a.is_compare())),
            };
            below_compare.truncate(depth);
            below_compare.push(above || agent.is_compare());
            let row = AgentSummary {
                id: agent_id.to_hex(),
                name: agent.display_name().into_owned(),
                zns_id: agent.zns_id.clone(),
                is_active: agent_id == active,
                record_count: 0,
                last_active: "-".to_string(),
                last_active_ms: None,
                definition: agent.definition.clone(),
                depth,
                running: running.contains(&agent_id),
                bench_run: above && !agent.is_compare_arm(),
                project_id: agent.project_id.clone(),
                project: names.of(registry, agent.project_id.as_deref()),
                // The registry does not know modes; the hub fills this in.
                plan_mode: false,
                code: agent.code.clone(),
                subtitle: agent.subtitle.clone(),
            };
            (agent_id, row)
        })
        .collect()
}

pub(crate) fn scan_tail(
    store: &dyn ReadStore,
    agent_id: AgentId,
    limit: usize,
) -> Result<Vec<RecordEntry>, StoreError> {
    let head_seq = store.get_head_seq(agent_id)?;
    if head_seq == 0 {
        return Ok(Vec::new());
    }
    let window = u64::try_from(limit).unwrap_or(u64::MAX);
    let from_seq = head_seq.saturating_sub(window.saturating_sub(1)).max(1);
    store.scan_record(agent_id, from_seq, limit)
}

fn history_message(tx: &Transaction) -> Option<MessageData> {
    if is_user_proposal(tx) {
        let command = extract_tool_info(tx);
        return (!command.is_empty()).then(|| MessageData {
            role: MessageRole::User,
            content: command.clone(),
            is_streaming: false,
            timestamp_ms: Some(tx.ts_ms),
            command: Some(command),
            activity: None,
            model: None,
            usage: None,
            sources: Vec::new(),
        });
    }
    let (role, content) = match tx.tx_type {
        TransactionType::UserPrompt => (MessageRole::User, prompt_text(tx)),
        TransactionType::AgentMsg => (
            MessageRole::Assistant,
            String::from_utf8_lossy(&tx.payload).into_owned(),
        ),
        TransactionType::ToolProposal => {
            let info = extract_tool_info(tx);
            if info.is_empty() {
                return None;
            }
            (MessageRole::System, format!("$ {info}"))
        }
        TransactionType::PolicyDecision => (
            MessageRole::System,
            serde_json::from_slice::<PolicyDecision>(&tx.payload)
                .ok()
                .and_then(|decision| policy_chat_line(&decision))?,
        ),
        TransactionType::SessionStart => (MessageRole::System, "── new session ──".to_string()),
        _ => return None,
    };
    if content.trim().is_empty() {
        return None;
    }
    Some(MessageData {
        role,
        content,
        is_streaming: false,
        timestamp_ms: Some(tx.ts_ms),
        command: None,
        activity: None,
        model: None,
        usage: None,
        sources: Vec::new(),
    })
}

/// Build the UI summary for a record entry. Shared by the history
/// loader and the live record feed so both render identically.
pub fn build_record_summary(
    seq: u64,
    tx: &Transaction,
    entry: &RecordEntry,
    timestamp: String,
) -> RecordSummary {
    let (tx_kind, mut sender) = tx_type_label(tx.tx_type);
    if is_user_proposal(tx) {
        sender = "USER";
    }

    let message = record_message(&tx.payload);

    let effect_count = entry.effects.len();
    let ok_count = entry
        .effects
        .iter()
        .filter(|e| matches!(e.status, EffectStatus::Committed))
        .count();
    let pending_count = entry
        .effects
        .iter()
        .filter(|e| matches!(e.status, EffectStatus::Pending))
        .count();
    let err_count = effect_count - ok_count - pending_count;

    let effect_status = if effect_count == 0 {
        "-".to_string()
    } else if err_count == 0 {
        format!("{ok_count} ok")
    } else {
        format!("{ok_count} ok, {err_count} err")
    };

    let status = if err_count > 0 {
        RecordStatus::Error
    } else if pending_count > 0 {
        RecordStatus::Pending
    } else {
        RecordStatus::Ok
    };

    let error_details: String = entry
        .effects
        .iter()
        .filter(|e| matches!(e.status, EffectStatus::Failed))
        .filter_map(|e| String::from_utf8(e.payload.to_vec()).ok())
        .collect::<Vec<_>>()
        .join("; ");

    let full_hash = entry.context_hash.as_hex();
    let hash_suffix = full_hash[full_hash.len().saturating_sub(4)..].to_string();
    let columns = record_columns(tx, entry, sender).unwrap_or_default();

    RecordSummary {
        seq,
        timestamp,
        full_hash,
        hash_suffix,
        tx_kind: tx_kind.to_string(),
        sender: sender.to_string(),
        message,
        action_count: entry.actions.len(),
        effect_status,
        status,
        info: summarize_entry(tx, entry),
        what: columns.what,
        group: columns.group,
        detail: truncate_chars(&collapse_whitespace(&columns.detail), MAX_INFO_CHARS),
        error_details,
        tx_id: hex::encode(tx.hash.as_bytes()),
        agent_id: hex::encode(tx.agent_id.as_bytes()),
        ts_ms: tx.ts_ms,
        policy: policy_detail(tx),
    }
}

/// The payload as shown in the record detail view: JSON objects and
/// arrays pretty-printed, anything else truncated.
fn record_message(payload: &[u8]) -> String {
    match serde_json::from_slice::<Value>(payload) {
        Ok(value @ (Value::Object(_) | Value::Array(_))) => truncate_chars(
            &serde_json::to_string_pretty(&value).unwrap_or_default(),
            MAX_JSON_MESSAGE_CHARS,
        ),
        _ => truncate_chars(&String::from_utf8_lossy(payload), MAX_MESSAGE_CHARS),
    }
}

/// One-line chat rendering of a policy decision, e.g.
/// `policy: read_file allowed by user_default.full_access`.
///
/// `None` when the user's own command was allowed: its output already
/// shows that it ran.
#[must_use]
pub fn policy_chat_line(decision: &PolicyDecision) -> Option<String> {
    policy_outcome(decision).map(|outcome| format!("policy: {} {outcome}", decision.tool))
}

/// The verdict and deciding rule of a policy decision, e.g.
/// `allowed by user_default.full_access`; `None` as for
/// [`policy_chat_line`].
#[must_use]
pub fn policy_outcome(decision: &PolicyDecision) -> Option<String> {
    let verdict = match decision.verdict {
        ToolGateVerdict::Approved if is_user_tool_use(&decision.tool_use_id) => return None,
        ToolGateVerdict::Approved => "allowed",
        ToolGateVerdict::Denied => "denied",
        ToolGateVerdict::PendingApproval => "needs approval",
    };
    Some(format!("{verdict} by {}", decision.decided_by))
}

/// Policy provenance for the record detail view; `None` for entries that
/// are not policy decisions.
fn policy_detail(tx: &Transaction) -> Option<PolicyDetail> {
    if tx.tx_type != TransactionType::PolicyDecision {
        return None;
    }
    let decision = serde_json::from_slice::<PolicyDecision>(&tx.payload).ok()?;
    let approval = decision
        .approval
        .as_ref()
        .map_or_else(String::new, |approval| {
            let approver = match &approval.approver {
                PolicyApprover::Hook => "hook".to_string(),
                PolicyApprover::User { user_id: Some(id) } => format!("user {id}"),
                PolicyApprover::User { user_id: None } => "user".to_string(),
            };
            match approval.remember.as_deref() {
                Some(remember) => format!("{approver} ({remember})"),
                None => approver,
            }
        });
    Some(PolicyDetail {
        tool: decision.tool.clone(),
        verdict: verdict_label(decision.verdict).to_string(),
        decided_by: decision.decided_by.to_string(),
        reason: decision.reason.clone().unwrap_or_default(),
        trace: decision
            .trace
            .iter()
            .map(|check| {
                let outcome = match check.outcome {
                    PolicyCheckOutcome::Allow => "allow",
                    PolicyCheckOutcome::Deny => "deny ",
                    PolicyCheckOutcome::Ask => "ask  ",
                };
                format!("{outcome} {}", check.rule)
            })
            .collect(),
        approval,
        mode: decision.mode,
        policy_hash: decision.policy_hash,
        proposal_tx: tx
            .reference_tx_hash
            .map(|hash| hash.to_hex())
            .unwrap_or_default(),
    })
}

const fn verdict_label(verdict: ToolGateVerdict) -> &'static str {
    match verdict {
        ToolGateVerdict::Approved => "approved",
        ToolGateVerdict::Denied => "denied",
        ToolGateVerdict::PendingApproval => "pending_approval",
    }
}

/// Like [`summarize_record`], but tool executions read their output from
/// the entry's effects (the kernel no longer copies it into the payload).
fn summarize_entry(tx: &Transaction, entry: &RecordEntry) -> String {
    if tx.tx_type == TransactionType::ToolExecution {
        if let Some(execution) = tool_execution(entry) {
            return truncate_chars(
                &collapse_whitespace(&summarize_tool_execution(&execution)),
                MAX_INFO_CHARS,
            );
        }
    }
    summarize_record(tx)
}

/// The Record panel's fixed columns for one entry.
#[derive(Debug, Default, PartialEq, Eq)]
struct RecordColumns {
    /// What the entry is about: tool name, LLM stop reason, sender.
    what: String,
    /// Category of `what`: tool group, LLM error class, model family.
    group: String,
    /// The rest of the summary, without what the columns already show.
    detail: String,
}

impl RecordColumns {
    fn new(what: impl Into<String>, group: impl Into<String>, detail: impl Into<String>) -> Self {
        Self {
            what: what.into(),
            group: group.into(),
            detail: detail.into(),
        }
    }
}

/// Split an entry into the Record panel's columns; `None` when the
/// payload doesn't parse, so the panel falls back to the summary.
fn record_columns(tx: &Transaction, entry: &RecordEntry, sender: &str) -> Option<RecordColumns> {
    let columns = match tx.tx_type {
        TransactionType::UserPrompt => RecordColumns::new(sender, "text", prompt_text(tx)),
        TransactionType::AgentMsg | TransactionType::Trigger => {
            RecordColumns::new(sender, "text", payload_text(tx))
        }
        TransactionType::Outcome => {
            let outcome = tx.outcome_payload()?;
            RecordColumns::new(
                outcome.signal.as_str(),
                outcome_verdict(outcome.success),
                outcome_detail(&outcome).join(" · "),
            )
        }
        TransactionType::ToolProposal => {
            let proposal = serde_json::from_slice::<ToolProposal>(&tx.payload).ok()?;
            let detail: Vec<String> = [
                Some(tool_target(&proposal.tool, &proposal.args)),
                proposal.source.as_deref().and_then(proposal_source_label),
            ]
            .into_iter()
            .flatten()
            .filter(|part| !part.is_empty())
            .collect();
            RecordColumns::new(
                proposal.tool.as_str(),
                tool_group(&proposal.tool),
                detail.join(" · "),
            )
        }
        TransactionType::ToolExecution => {
            let execution = tool_execution(entry)?;
            let target = tool_target(&execution.tool, &execution.args);
            let detail: Vec<String> = [
                Some(target).filter(|target| !target.is_empty()),
                Some(tool_outcome(&execution)),
                execution.duration_ms.map(format_duration_ms),
            ]
            .into_iter()
            .flatten()
            .collect();
            let detail = detail.join(" · ");
            RecordColumns::new(execution.tool.as_str(), tool_group(&execution.tool), detail)
        }
        TransactionType::PolicyDecision => {
            let decision = serde_json::from_slice::<PolicyDecision>(&tx.payload).ok()?;
            RecordColumns::new(
                decision.tool.as_str(),
                tool_group(&decision.tool),
                format!(
                    "{} · {}",
                    policy_verdict(decision.verdict),
                    decision.decided_by
                ),
            )
        }
        TransactionType::ActionResult | TransactionType::ProcessComplete => {
            let result = serde_json::from_slice::<ActionResultPayload>(&tx.payload).ok()?;
            RecordColumns::new(
                if result.success { "ok" } else { "failed" },
                result
                    .exit_code
                    .map_or_else(String::new, |code| format!("exit {code}")),
                format_duration_ms(result.duration_ms),
            )
        }
        TransactionType::Reasoning => reasoning_columns(&json_payload(tx)?),
        TransactionType::System => {
            let payload = json_payload(tx)?;
            RecordColumns::new(system_kind(&payload), "", system_detail(&payload).join(" "))
        }
        TransactionType::SessionStart => RecordColumns::new("start", "", ""),
        TransactionType::SubagentSpawn => {
            let payload = json_payload(tx)?;
            let mode = payload
                .get("spawn_mode")
                .and_then(Value::as_str)
                .unwrap_or("spawn");
            RecordColumns::new(mode, "agent", spawn_detail(&payload).join(" · "))
        }
    };
    Some(columns)
}

/// Columns for a model call: stop reason and model family, or the class
/// of error when the call failed.
fn reasoning_columns(payload: &Value) -> RecordColumns {
    if let Some(error) = reasoning_error(payload) {
        return RecordColumns::new("error", llm_error_class(error), error);
    }
    let stop = payload
        .get("stop_reason")
        .and_then(Value::as_str)
        .unwrap_or("");
    let family = payload
        .get("model")
        .and_then(Value::as_str)
        .map_or("", model_family);
    let took = payload
        .get("duration_ms")
        .and_then(Value::as_u64)
        .map(format_duration_ms);
    let detail: Vec<String> = [reasoning_tokens(payload), reasoning_tools(payload), took]
        .into_iter()
        .flatten()
        .collect();
    RecordColumns::new(stop, family, detail.join(" · "))
}

/// The error of a failed model call; `None` when the call succeeded. A
/// stream that broke says `status: failed`; a call that got no answer at
/// all (an error or timeout before any stream) says `stop_reason: Error`,
/// which no answer has.
pub(crate) fn reasoning_error(payload: &Value) -> Option<&str> {
    let text = |key: &str| payload.get(key).and_then(Value::as_str);
    (text("status") == Some("failed") || text("stop_reason") == Some("Error"))
        .then(|| text("error").unwrap_or(""))
}

/// Short class of a model call error, at most 7 characters.
fn llm_error_class(error: &str) -> &'static str {
    let error = error.to_ascii_lowercase();
    let mentions = |needles: &[&str]| needles.iter().any(|needle| error.contains(needle));
    if mentions(&["429", "rate limit", "rate_limit"]) {
        "rate"
    } else if mentions(&["529", "overloaded"]) {
        "busy"
    } else if mentions(&["timeout", "timed out"]) {
        "timeout"
    } else if mentions(&[
        "401",
        "403",
        "unauthorized",
        "forbidden",
        "api key",
        "authentication",
    ]) {
        "auth"
    } else if mentions(&["stream"]) {
        "stream"
    } else {
        "other"
    }
}

/// Model family shown in the group column, e.g. `claude-sonnet-4-5` -> `sonnet`.
fn model_family(model: &str) -> &str {
    const FAMILIES: &[&str] = &["opus", "sonnet", "haiku", "gpt", "gemini", "grok"];
    let lower = model.to_ascii_lowercase();
    if let Some(family) = FAMILIES.iter().find(|family| lower.contains(*family)) {
        return family;
    }
    let name = model.rsplit('/').next().unwrap_or(model);
    name.split(['-', '_', ':', '.']).next().unwrap_or(name)
}

/// Group a tool belongs to, at most 7 characters.
fn tool_group(tool: &str) -> &'static str {
    match tool {
        "read_file" | "write_file" | "edit_file" | "delete_file" | "list_files" | "find_files"
        | "stat_file" => "file",
        "run_command" => "shell",
        "search_code" => "search",
        _ if tool.starts_with("git_") => "git",
        _ if tool.starts_with("web_") || tool.contains("http") || tool.contains("fetch") => "web",
        _ if tool.contains("spec") => "spec",
        _ if tool.contains("task") => "task",
        _ if tool.contains("agent") => "agent",
        _ => "other",
    }
}

pub(crate) fn format_local_time(ts_ms: u64) -> String {
    i64::try_from(ts_ms)
        .ok()
        .and_then(chrono::DateTime::from_timestamp_millis)
        .map(|dt| {
            dt.with_timezone(&chrono::Local)
                .format("%H:%M:%S")
                .to_string()
        })
        .unwrap_or_else(|| "??:??:??".to_string())
}

/// Map a transaction type to its display label and sender name.
///
/// Labels are kept to at most 8 characters so they fit the Record
/// panel's fixed-width kind column.
pub fn tx_type_label(tx_type: TransactionType) -> (&'static str, &'static str) {
    match tx_type {
        TransactionType::UserPrompt => ("Prompt", "USER"),
        TransactionType::ActionResult => ("Action", "SYSTEM"),
        TransactionType::System => ("System", "SYSTEM"),
        TransactionType::AgentMsg => ("Reply", "AURA"),
        TransactionType::Trigger => ("Trigger", "SYSTEM"),
        TransactionType::SessionStart => ("Session", "SYSTEM"),
        TransactionType::ToolProposal => ("ToolCall", "LLM"),
        TransactionType::PolicyDecision => ("Policy", "KERNEL"),
        TransactionType::ToolExecution => ("ToolRes", "KERNEL"),
        TransactionType::ProcessComplete => ("Complete", "SYSTEM"),
        TransactionType::Reasoning => ("LLM", "KERNEL"),
        TransactionType::SubagentSpawn => ("Spawn", "AGENT"),
        TransactionType::Outcome => ("Outcome", "GATE"),
    }
}

/// One-line, human-readable summary of what a transaction contains.
pub fn summarize_record(tx: &Transaction) -> String {
    let summary = match tx.tx_type {
        TransactionType::UserPrompt => prompt_text(tx),
        TransactionType::AgentMsg | TransactionType::Trigger => payload_text(tx),
        TransactionType::Outcome => tx.outcome_payload().map_or_else(String::new, |outcome| {
            std::iter::once(outcome.signal.as_str().to_string())
                .chain(outcome_detail(&outcome))
                .collect::<Vec<_>>()
                .join(" · ")
        }),
        TransactionType::ToolProposal => serde_json::from_slice::<ToolProposal>(&tx.payload)
            .map_or_else(
                |_| payload_text(tx),
                |p| summarize_tool_call(&p.tool, &p.args),
            ),
        TransactionType::ToolExecution => serde_json::from_slice::<ToolExecution>(&tx.payload)
            .map_or_else(|_| payload_text(tx), |e| summarize_tool_execution(&e)),
        TransactionType::PolicyDecision => serde_json::from_slice::<PolicyDecision>(&tx.payload)
            .map_or_else(|_| payload_text(tx), |d| summarize_policy_decision(&d)),
        TransactionType::ActionResult | TransactionType::ProcessComplete => {
            serde_json::from_slice::<ActionResultPayload>(&tx.payload)
                .map_or_else(|_| payload_text(tx), |r| summarize_action_result(&r))
        }
        TransactionType::Reasoning => {
            json_payload(tx).map_or_else(|| payload_text(tx), |v| summarize_reasoning(&v))
        }
        TransactionType::System => {
            json_payload(tx).map_or_else(|| payload_text(tx), |v| summarize_system(&v))
        }
        TransactionType::SessionStart => "new session".to_string(),
        TransactionType::SubagentSpawn => {
            json_payload(tx).map_or_else(String::new, |v| summarize_spawn(&v))
        }
    };
    truncate_chars(&collapse_whitespace(&summary), MAX_INFO_CHARS)
}

pub(crate) fn summarize_tool_call(tool: &str, args: &Value) -> String {
    let target = tool_target(tool, args);
    if target.is_empty() {
        tool.to_string()
    } else {
        format!("{tool} {target}")
    }
}

/// What a tool call acts on: the command line for `run_command`, else
/// its primary argument.
pub(crate) fn tool_target(tool: &str, args: &Value) -> String {
    if tool == "run_command" {
        extract_cmd_run_command(args)
    } else {
        primary_arg(args).unwrap_or_default()
    }
}

fn summarize_tool_execution(execution: &ToolExecution) -> String {
    format!(
        "{} · {}",
        summarize_tool_call(&execution.tool, &execution.args),
        tool_outcome(execution)
    )
}

/// How a tool call ended, e.g. `ok 5B` or `denied: reason`.
pub(crate) fn tool_outcome(execution: &ToolExecution) -> String {
    match execution.decision {
        ToolGateVerdict::Denied => match execution.reason.as_deref() {
            Some(reason) if !reason.is_empty() => format!("denied: {reason}"),
            _ => "denied".to_string(),
        },
        ToolGateVerdict::PendingApproval => "awaiting approval".to_string(),
        ToolGateVerdict::Approved if execution.is_error => {
            let first_line = execution
                .result
                .as_deref()
                .and_then(|r| r.lines().find(|l| !l.trim().is_empty()))
                .unwrap_or("");
            if first_line.is_empty() {
                "err".to_string()
            } else {
                format!("err: {first_line}")
            }
        }
        ToolGateVerdict::Approved => match execution.result.as_deref() {
            Some(result) => format!("ok {}", format_bytes(result.len())),
            None => "ok".to_string(),
        },
    }
}

pub(crate) fn summarize_policy_decision(decision: &PolicyDecision) -> String {
    format!(
        "{} · {} · {}",
        decision.tool,
        policy_verdict(decision.verdict),
        decision.decided_by
    )
}

const fn policy_verdict(verdict: ToolGateVerdict) -> &'static str {
    match verdict {
        ToolGateVerdict::Approved => "allow",
        ToolGateVerdict::Denied => "deny",
        ToolGateVerdict::PendingApproval => "ask",
    }
}

pub(crate) fn summarize_action_result(result: &ActionResultPayload) -> String {
    let outcome = if result.success { "ok" } else { "failed" };
    let duration = format_duration_ms(result.duration_ms);
    match result.exit_code {
        Some(code) => format!("{outcome} · exit {code} · {duration}"),
        None => format!("{outcome} · {duration}"),
    }
}

fn summarize_reasoning(payload: &Value) -> String {
    if let Some(error) = reasoning_error(payload) {
        return format!("failed: {error}");
    }

    let mut parts = Vec::new();
    if let Some(model) = payload.get("model").and_then(Value::as_str) {
        parts.push(model.to_string());
    }
    parts.extend(reasoning_tokens(payload));
    if let Some(stop) = payload.get("stop_reason").and_then(Value::as_str) {
        parts.push(stop.to_string());
    }
    parts.extend(reasoning_tools(payload));
    parts.join(" · ")
}

/// Input/output token counts of a model call, e.g. `1.2k/300 tok`.
pub(crate) fn reasoning_tokens(payload: &Value) -> Option<String> {
    let input = payload.get("input_tokens").and_then(Value::as_u64);
    let output = payload.get("output_tokens").and_then(Value::as_u64);
    (input.is_some() || output.is_some()).then(|| {
        format!(
            "{}/{} tok",
            input.map_or_else(|| "?".to_string(), format_tokens),
            output.map_or_else(|| "?".to_string(), format_tokens),
        )
    })
}

/// Tools a model call asked for, e.g. `tools: read_file, run_command`.
pub(crate) fn reasoning_tools(payload: &Value) -> Option<String> {
    let tools: Vec<&str> = payload
        .get("tool_uses")
        .and_then(Value::as_array)
        .map(|arr| arr.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();
    (!tools.is_empty()).then(|| format!("tools: {}", tools.join(", ")))
}

pub(crate) fn summarize_system(payload: &Value) -> String {
    let kind = system_kind(payload);
    let mode = |key: &str| payload.get(key).and_then(Value::as_str);
    if let (Some(from), Some(to)) = (mode("from"), mode("to")) {
        return format!("{kind} · {from} → {to}");
    }
    let detail = system_detail(payload);
    if detail.is_empty() {
        kind.to_string()
    } else {
        format!("{kind} · {}", detail.join(" "))
    }
}

fn system_kind(payload: &Value) -> &str {
    payload
        .get("system_kind")
        .and_then(Value::as_str)
        .unwrap_or("system")
}

fn system_detail(payload: &Value) -> Vec<&str> {
    ["method", "event", "action", "scope", "phase"]
        .iter()
        .filter_map(|key| payload.get(*key).and_then(Value::as_str))
        .collect()
}

fn summarize_spawn(payload: &Value) -> String {
    let mut parts = Vec::new();
    if let Some(mode) = payload.get("spawn_mode").and_then(Value::as_str) {
        parts.push(mode.to_string());
    }
    parts.extend(spawn_detail(payload));
    parts.join(" · ")
}

/// Child agent, the permanent agent it is an instance of, the todo it
/// does and the override count of a spawn, without the spawn mode.
fn spawn_detail(payload: &Value) -> Vec<String> {
    let mut parts = Vec::new();
    let text = |key| payload.get(key).and_then(Value::as_str);
    if let Some(name) = text("child_name").or_else(|| text("subagent_type")) {
        parts.push(name.to_string());
    }
    if let Some(todo) = text("todo_id") {
        parts.push(format!("todo {todo}"));
    }
    if let Some(child) = payload.get("child_agent_id").and_then(Value::as_str) {
        let short: String = child.chars().take(8).collect();
        parts.push(format!("child {short}"));
    }
    let overrides = payload
        .pointer("/override_manifest/applied")
        .and_then(Value::as_array)
        .map_or(0, Vec::len);
    if overrides > 0 {
        parts.push(format!("{overrides} overrides"));
    }
    parts
}

/// Pick the argument that best identifies what a tool call targets.
fn primary_arg(args: &Value) -> Option<String> {
    const KEYS: &[&str] = &[
        "path",
        "file_path",
        "pattern",
        "query",
        "url",
        "command",
        "name",
    ];
    KEYS.iter()
        .find_map(|key| args.get(*key).and_then(Value::as_str))
        .map(str::to_string)
}

/// Whether `tx` is a tool proposal the user made directly.
fn is_user_proposal(tx: &Transaction) -> bool {
    tx.tx_type == TransactionType::ToolProposal
        && serde_json::from_slice::<ToolProposal>(&tx.payload)
            .is_ok_and(|p| p.source.as_deref() == Some(crate::event_loop::run_command::USER_SOURCE))
}

/// Extract the command string from cmd_run tool arguments.
fn extract_cmd_run_command(args: &Value) -> String {
    if let Some(script) = ["shell_script", "command"]
        .iter()
        .find_map(|key| args.get(*key).and_then(Value::as_str))
    {
        return script.to_string();
    }
    let program = args["program"].as_str().unwrap_or("");
    let cmd_args: Vec<&str> = args["args"]
        .as_array()
        .map(|arr| arr.iter().filter_map(|v| v.as_str()).collect())
        .unwrap_or_default();

    if cmd_args.is_empty() {
        program.to_string()
    } else {
        format!("{program} {}", cmd_args.join(" "))
    }
}

/// Tool name (or the command line for `run_command`) of a tool proposal
/// or execution; empty for other transaction types.
pub fn extract_tool_info(tx: &Transaction) -> String {
    let call = match tx.tx_type {
        TransactionType::ToolProposal => serde_json::from_slice::<ToolProposal>(&tx.payload)
            .ok()
            .map(|p| (p.tool, p.args)),
        TransactionType::ToolExecution => serde_json::from_slice::<ToolExecution>(&tx.payload)
            .ok()
            .map(|e| (e.tool, e.args)),
        _ => None,
    };
    match call {
        Some((tool, args)) if tool == "run_command" => extract_cmd_run_command(&args),
        Some((tool, _)) => tool,
        None => String::new(),
    }
}

pub(crate) fn json_payload(tx: &Transaction) -> Option<Value> {
    serde_json::from_slice::<Value>(&tx.payload)
        .ok()
        .filter(Value::is_object)
}

/// The text of a prompt entry; empty when its payload is not a prompt.
fn prompt_text(tx: &Transaction) -> String {
    tx.prompt_payload()
        .map(|prompt| prompt.text)
        .unwrap_or_default()
}

/// `ok`, `failed`, or `-` when an outcome says neither.
const fn outcome_verdict(success: Option<bool>) -> &'static str {
    match success {
        Some(true) => "ok",
        Some(false) => "failed",
        None => "-",
    }
}

/// Category, model, cost and wall time of an outcome's work.
fn outcome_detail(outcome: &aura_core_types::OutcomePayload) -> Vec<String> {
    vec![
        outcome.category.to_string(),
        outcome.model.clone(),
        format!("${:.4}", outcome.cost_usd),
        format_duration_ms(outcome.latency_ms),
    ]
}

pub(crate) fn payload_text(tx: &Transaction) -> String {
    std::str::from_utf8(&tx.payload)
        .map(str::to_string)
        .unwrap_or_default()
}

pub(crate) fn collapse_whitespace(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

pub(crate) fn truncate_chars(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_string();
    }
    let kept: String = text.chars().take(max_chars.saturating_sub(3)).collect();
    format!("{kept}...")
}

/// `n / unit` rendered with one truncated decimal, e.g. `1234 / 1000` -> `1.2`.
fn one_decimal(n: u64, unit: u64) -> String {
    let tenths = n.saturating_mul(10) / unit;
    format!("{}.{}", tenths / 10, tenths % 10)
}

pub(crate) fn format_tokens(n: u64) -> String {
    if n < 1000 {
        n.to_string()
    } else {
        format!("{}k", one_decimal(n, 1000))
    }
}

fn format_bytes(n: usize) -> String {
    let n = n as u64;
    if n < 1024 {
        format!("{n}B")
    } else if n < 1024 * 1024 {
        format!("{}KB", one_decimal(n, 1024))
    } else {
        format!("{}MB", one_decimal(n, 1024 * 1024))
    }
}

/// Why a recorded call didn't go to the tool executor, for the TOOLCALL
/// row. `None` for calls the panel already marks some other way (the
/// sender column shows user commands).
fn proposal_source_label(source: &str) -> Option<String> {
    let label = match source {
        crate::event_loop::run_command::USER_SOURCE => return None,
        "cache" => "cached result",
        "steering_block" => "blocked by steering",
        "invalid_tool_use" => "invalid call",
        "task_guard" => "blocked by task guard",
        "task_executor" => "handled by task runner",
        other => other,
    };
    Some(label.to_string())
}

pub(crate) fn format_duration_ms(ms: u64) -> String {
    if ms < 1000 {
        format!("{ms}ms")
    } else {
        format!("{}s", one_decimal(ms, 1000))
    }
}

#[cfg(test)]
mod tests;
