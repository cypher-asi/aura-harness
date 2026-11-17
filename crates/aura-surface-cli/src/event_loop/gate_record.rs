//! Gate's record of each turn: the category the classifier gives its
//! prompt, the `Outcome` written when it ends, and the later signals that
//! judge it again — a rewind, the same prompt sent again on a stronger
//! model, the user's `/good` or `/bad`. Each later signal appends another
//! `Outcome` for the same prompt; [`aura_agent_gate::outcome::effective`]
//! says which one counts.

use super::agent_events::AgentUi;
use super::agents::AgentSession;
use super::LoopState;
use aura_agent_gate::{classify, outcome::is_retry_stronger};
use aura_agent_kernel::Kernel;
use aura_core_types::{
    AgentId, Hash, OutcomePayload, OutcomeSignal, PromptCategory, PromptPayload, Transaction,
    TransactionType,
};
use aura_store_db::ReadStore;
use aura_terminal::UiCommand;
use std::time::Duration;
use tracing::{info, warn};

/// How long the classifier may take before the prompt goes unclassified.
const CLASSIFY_TIMEOUT: Duration = Duration::from_secs(20);

/// Entries scanned back from the head of a record to find a turn's
/// outcome.
const SCAN_LIMIT: usize = 2_000;

/// Label `text` with its category by asking `model` through `kernel`, so
/// the call is recorded. Any failure leaves it unclassified.
pub(crate) async fn classify_prompt(
    kernel: &Kernel,
    model: &str,
    text: &str,
    auth_token: Option<String>,
) -> PromptCategory {
    let request = match classify::request(model, text, auth_token) {
        Ok(request) => request,
        Err(e) => {
            warn!(%model, error = %e, "Could not build the classifier request");
            return PromptCategory::Unclassified;
        }
    };
    match tokio::time::timeout(CLASSIFY_TIMEOUT, kernel.reason(request)).await {
        Ok(Ok(result)) => {
            let category = classify::category_of(&result.response);
            info!(%model, %category, "Classified the prompt");
            category
        }
        Ok(Err(e)) => {
            warn!(%model, error = %e, "Classifier call failed");
            PromptCategory::Unclassified
        }
        Err(_) => {
            warn!(%model, "Classifier call timed out");
            PromptCategory::Unclassified
        }
    }
}

/// Write `outcome` to the record of `agent_id` through `kernel`. A failure
/// is logged.
pub(crate) async fn record(kernel: &Kernel, agent_id: AgentId, outcome: &OutcomePayload) -> bool {
    match kernel
        .process_direct(Transaction::outcome(agent_id, outcome))
        .await
    {
        Ok(_) => {
            info!(
                %agent_id,
                signal = %outcome.signal,
                category = %outcome.category,
                model = %outcome.model,
                "Recorded turn outcome"
            );
            true
        }
        Err(e) => {
            warn!(%agent_id, error = %e, "Failed to record turn outcome");
            false
        }
    }
}

/// The latest turn on a record: the outcome written when it ended, and its
/// prompt when the scan still reaches it.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct LastTurn {
    pub outcome: OutcomePayload,
    pub prompt: Option<PromptPayload>,
}

/// The latest turn recorded for `agent_id`, if one ended within the last
/// [`SCAN_LIMIT`] entries.
pub(crate) fn last_turn<S: ReadStore + ?Sized>(store: &S, agent_id: AgentId) -> Option<LastTurn> {
    let entries = newest_first(store, agent_id);
    let outcome = entries
        .iter()
        .filter_map(|entry| entry.tx.outcome_payload())
        .find(|outcome| outcome.signal.is_turn_end())?;
    let prompt = entries
        .iter()
        .find(|entry| {
            entry.tx.tx_type == TransactionType::UserPrompt && entry.tx.hash == outcome.prompt_tx
        })
        .and_then(|entry| entry.tx.prompt_payload());
    Some(LastTurn { outcome, prompt })
}

/// The outcome written when the turn of each prompt in `prompts` ended,
/// for those the scan reaches, in the order of `prompts`.
pub(crate) fn turn_outcomes<S: ReadStore + ?Sized>(
    store: &S,
    agent_id: AgentId,
    prompts: &[Hash],
) -> Vec<OutcomePayload> {
    let ended: Vec<OutcomePayload> = newest_first(store, agent_id)
        .iter()
        .filter_map(|entry| entry.tx.outcome_payload())
        .filter(|outcome| outcome.signal.is_turn_end())
        .collect();
    prompts
        .iter()
        .filter_map(|prompt| ended.iter().find(|o| o.prompt_tx == *prompt).cloned())
        .collect()
}

fn newest_first<S: ReadStore + ?Sized>(
    store: &S,
    agent_id: AgentId,
) -> Vec<aura_core_types::RecordEntry> {
    let head = match store.get_head_seq(agent_id) {
        Ok(head) => head,
        Err(e) => {
            warn!(%agent_id, error = %e, "Failed to read the record head");
            return Vec::new();
        }
    };
    store
        .scan_record_descending(agent_id, head, SCAN_LIMIT)
        .unwrap_or_else(|e| {
            warn!(%agent_id, error = %e, "Failed to scan the record for outcomes");
            Vec::new()
        })
}

/// Before a turn of `session` answers `text` on `model`: when `text`
/// repeats the previous turn's prompt on a stronger model, the previous
/// turn failed.
pub(super) async fn note_retry(session: &AgentSession, text: &str, model: &str) {
    let agent_id = session.agent_id();
    let Some(LastTurn {
        outcome,
        prompt: Some(prompt),
    }) = last_turn(session.kernel.store().as_ref(), agent_id)
    else {
        return;
    };
    if is_retry_stronger(&prompt.text, &outcome.model, text, model) {
        info!(%agent_id, from = %outcome.model, to = %model, "Prompt retried on a stronger model");
        record(
            &session.kernel,
            agent_id,
            &outcome.with_signal(OutcomeSignal::RetryStronger),
        )
        .await;
    }
}

/// After `/rewind` undid the turns whose prompts are `prompts`: each of
/// them failed.
pub(super) async fn note_rewind(session: &AgentSession, prompts: &[Hash]) {
    let agent_id = session.agent_id();
    for outcome in turn_outcomes(session.kernel.store().as_ref(), agent_id, prompts) {
        record(
            &session.kernel,
            agent_id,
            &outcome.with_signal(OutcomeSignal::Rewind),
        )
        .await;
    }
}

/// `/good` or `/bad`: the user's rating of `agent_id`'s last turn, which
/// overrides what the turn's own signals said.
pub(super) async fn handle_rate(state: &LoopState<'_>, agent_id: AgentId, good: bool) {
    let ui = AgentUi::new(state.commands.clone(), agent_id);
    let Some(kernel) = state.kernel_for(agent_id) else {
        ui.send(UiCommand::ShowWarning("No turn to rate yet".to_string()))
            .await;
        return;
    };
    let Some(last) = last_turn(kernel.store().as_ref(), agent_id) else {
        ui.send(UiCommand::ShowWarning(
            "No finished turn to rate yet".to_string(),
        ))
        .await;
        return;
    };
    let signal = if good {
        OutcomeSignal::Good
    } else {
        OutcomeSignal::Bad
    };
    let rated = last.outcome.with_signal(signal);
    if record(&kernel, agent_id, &rated).await {
        ui.send(UiCommand::ShowSuccess(format!(
            "Rated the last turn {signal} ({} on {})",
            rated.category, rated.model
        )))
        .await;
    } else {
        ui.send(UiCommand::ShowWarning(
            "Could not record the rating".to_string(),
        ))
        .await;
    }
}
